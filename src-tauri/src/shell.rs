//! Runs a shell command the user confirmed in Clawd's chat, with a time limit.

use std::{
    io::Read,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::Serialize;

const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_OUTPUT: usize = 4000;
/// Windows PowerShell 5.1 writes redirected output in the console's OEM code page
/// (cp866 for Russian), so Cyrillic and emoji come out garbled; switch it to UTF-8 first.
const UTF8_PRELUDE: &str = "[Console]::OutputEncoding = [Text.Encoding]::UTF8; $OutputEncoding = [Text.Encoding]::UTF8; ";

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CommandResult {
    /// None when the command was killed by the timeout or a signal.
    pub code: Option<i32>,
    pub output: String,
    pub timed_out: bool,
}

fn shell(command: &str) -> Command {
    if cfg!(windows) {
        let mut cmd = Command::new("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", &format!("{UTF8_PRELUDE}{command}")]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd
    } else {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", command]);
        cmd
    }
}

/// Output as text. Old console programs (ping, ipconfig...) still print in the OEM
/// code page whatever PowerShell is set to, so bytes that aren't UTF-8 are read as that.
fn decode(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => from_oem(e.into_bytes()),
    }
}

#[cfg(windows)]
fn from_oem(bytes: Vec<u8>) -> String {
    use windows::Win32::Globalization::{MultiByteToWideChar, CP_OEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS};
    let flags = MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0);
    unsafe {
        let len = MultiByteToWideChar(CP_OEMCP, flags, &bytes, None);
        if len <= 0 {
            return String::from_utf8_lossy(&bytes).into_owned();
        }
        let mut wide = vec![0u16; len as usize];
        MultiByteToWideChar(CP_OEMCP, flags, &bytes, Some(&mut wide));
        String::from_utf16_lossy(&wide)
    }
}

#[cfg(not(windows))]
fn from_oem(bytes: Vec<u8>) -> String {
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Keeps the end of long output: that's where results and errors usually are.
fn tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let skip = text.char_indices().nth(count - max).map_or(0, |(i, _)| i);
    format!("…{}", &text[skip..])
}

pub fn run(command: &str) -> Result<CommandResult, String> {
    let mut cmd = shell(command);
    if let Some(home) = crate::claude::home_dir() {
        cmd.current_dir(home);
    }
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let read_all = |mut pipe: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            decode(buf)
        })
    };
    let stdout = read_all(Box::new(child.stdout.take().expect("stdout is piped")));
    let stderr = read_all(Box::new(child.stderr.take().expect("stderr is piped")));
    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break Some(status);
        }
        if started.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            timed_out = true;
            break None;
        }
        thread::sleep(Duration::from_millis(50));
    };
    let mut output = stdout.join().unwrap_or_default();
    let err = stderr.join().unwrap_or_default();
    if !err.trim().is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&err);
    }
    Ok(CommandResult {
        code: status.and_then(|s| s.code()),
        output: tail(output.trim_end(), MAX_OUTPUT),
        timed_out,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn captures_output_and_exit_code() {
        let ok = run("echo hello; echo oops >&2").unwrap();
        assert_eq!(ok.code, Some(0));
        assert_eq!(ok.output, "hello\noops");
        let failed = run("exit 3").unwrap();
        assert_eq!(failed.code, Some(3));
        assert!(!failed.timed_out);
    }

    #[test]
    fn keeps_the_end_of_long_output() {
        let long = "x".repeat(10) + "END";
        assert_eq!(tail(&long, 5), "…xxEND");
        assert_eq!(tail("short", 5), "short");
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn powershell_output_keeps_cyrillic() {
        let out = run("Write-Output 'Привет, мир'").unwrap();
        assert_eq!(out.code, Some(0));
        assert_eq!(out.output, "Привет, мир");
    }

    #[test]
    fn non_utf8_bytes_fall_back_to_the_oem_code_page() {
        assert_eq!(decode("ok".as_bytes().to_vec()), "ok");
        // Whatever the OEM code page is, invalid UTF-8 must still come out as text.
        assert!(!decode(vec![0x8f, 0xe0, 0xa8]).is_empty());
    }
}

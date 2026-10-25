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
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", command]);
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
            String::from_utf8_lossy(&buf).into_owned()
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

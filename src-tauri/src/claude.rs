//! Asks Claude Haiku for Clawd's lines through the Claude Code CLI in headless
//! mode, so it runs on the user's own Claude Code login and needs no API key.

use std::{
    env, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        RwLock,
    },
    thread,
    time::{Duration, Instant},
};

use crate::settings::Lang;

/// Haiku as the rest of the app sees it: where `claude` lives, which language
/// to speak and whether it's switched on. All of these can change at runtime.
pub struct Haiku {
    pub workdir: PathBuf,
    pub cli: RwLock<Option<PathBuf>>,
    pub lang: RwLock<Lang>,
    pub enabled: AtomicBool,
}

impl Haiku {
    pub fn lang(&self) -> Lang {
        *self.lang.read().unwrap()
    }

    pub fn cli(&self) -> Option<PathBuf> {
        self.cli.read().unwrap().clone()
    }

    /// Runs a prompt written for the current language.
    pub fn run(&self, prompt: impl FnOnce(Lang) -> String) -> Result<String, String> {
        if !self.enabled.load(Ordering::Relaxed) {
            return Err("disabled".into());
        }
        let cli = self.cli().ok_or("claude CLI not found")?;
        let lang = self.lang();
        ask(&cli, &self.workdir, lang, &prompt(lang))
    }
}

const TIMEOUT: Duration = Duration::from_secs(90);
const VERSION_TIMEOUT: Duration = Duration::from_secs(20);

fn persona(lang: Lang) -> String {
    format!(
        "You are Clawd, a tiny orange pixel-art pet and the mascot of Claude Code. You live on the user's \
         desktop: you stroll along the taskbar, read, code, dance to music and cheer the user on. You speak \
         {}, in the first person, very briefly, warmly and with light geeky humour. Reply with the lines \
         only: no quotes, emoji, numbering, markdown or explanations.",
        lang.name()
    )
}

fn exe_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["claude.exe", "claude.cmd"]
    } else {
        &["claude"]
    }
}

/// Subfolders of `dir` (e.g. one per installed Node version), newest name first.
fn subdirs(dir: &Path, suffix: &[&str]) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| suffix.iter().fold(e.path(), |p, s| p.join(s)))
        .collect();
    found.sort();
    found.reverse();
    found
}

/// Where installers put `claude` besides PATH. A desktop launcher often starts
/// apps without the PATH set up in ~/.bashrc or ~/.zshrc (nvm, volta, ...).
fn install_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let Some(home) = home_dir() else {
        return dirs;
    };
    dirs.push(home.join(".local").join("bin"));
    dirs.push(home.join(".claude").join("local"));
    if cfg!(windows) {
        let var = |name: &str| env::var_os(name).map(PathBuf::from);
        if let Some(appdata) = var("APPDATA") {
            dirs.push(appdata.join("npm"));
            dirs.extend(subdirs(&appdata.join("nvm"), &[]));
            // Claude Desktop keeps its own copy of Claude Code.
            dirs.extend(subdirs(&appdata.join("Claude").join("claude-code"), &[]));
        }
        if let Some(local) = var("LOCALAPPDATA") {
            dirs.push(local.join("pnpm"));
            dirs.push(local.join("Volta").join("bin"));
        }
        dirs.push(home.join("scoop").join("shims"));
        if let Some(program_files) = var("ProgramFiles") {
            dirs.push(program_files.join("nodejs"));
        }
    } else {
        for rel in [".npm-global/bin", ".volta/bin", ".bun/bin", ".local/share/pnpm", ".yarn/bin", ".asdf/shims", ".local/share/mise/shims"] {
            dirs.push(home.join(rel));
        }
        dirs.extend(subdirs(&home.join(".nvm/versions/node"), &["bin"]));
        dirs.extend(subdirs(&home.join(".local/share/fnm/node-versions"), &["installation", "bin"]));
        for dir in ["/usr/local/bin", "/opt/homebrew/bin", "/usr/bin", "/snap/bin"] {
            dirs.push(PathBuf::from(dir));
        }
    }
    dirs
}

/// PATH as the user's shell sees it, which may differ from the one Clawd was started with.
fn shell_lookup() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        // The registry has the PATH from user and system settings, even if it changed after login.
        let mut dirs = Vec::new();
        for key in [r"HKCU\Environment", r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment"] {
            let Ok(out) = run_with_timeout(Command::new("reg").args(["query", key, "/v", "Path"]), VERSION_TIMEOUT) else {
                continue;
            };
            for line in out.lines() {
                if let Some((_, value)) = line.split_once("REG_EXPAND_SZ").or_else(|| line.split_once("REG_SZ")) {
                    dirs.extend(value.trim().split(';').map(|d| PathBuf::from(expand_env(d))));
                }
            }
        }
        find_in(&dirs)
    }
    #[cfg(not(windows))]
    {
        let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let out = run_with_timeout(Command::new(shell).args(["-ilc", "command -v claude"]), VERSION_TIMEOUT).ok()?;
        out.lines()
            .map(str::trim)
            .filter(|l| l.starts_with('/'))
            .map(PathBuf::from)
            .find(|p| p.is_file())
    }
}

#[cfg(windows)]
fn expand_env(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        match rest[start + 1..].find('%') {
            Some(len) => {
                let name = &rest[start + 1..start + 1 + len];
                out.push_str(&env::var(name).unwrap_or_else(|_| format!("%{name}%")));
                rest = &rest[start + len + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn find_in(dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter()
        .flat_map(|dir| exe_names().iter().map(move |name| dir.join(name)))
        .find(|path| path.is_file())
}

/// Locates the `claude` executable: a path the user picked, $CLAWD_CLAUDE_PATH,
/// PATH, the usual install folders, and finally the user's login shell.
pub fn find_cli(saved: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = saved.filter(|p| p.is_file()) {
        return Some(path.to_path_buf());
    }
    if let Some(path) = env::var_os("CLAWD_CLAUDE_PATH").map(PathBuf::from) {
        if path.is_file() {
            return Some(path);
        }
    }
    let mut dirs: Vec<PathBuf> = env::var_os("PATH")
        .map(|p| env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend(install_dirs());
    find_in(&dirs).or_else(shell_lookup)
}

pub fn home_dir() -> Option<PathBuf> {
    env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

fn hide_console(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// Runs a short command and returns its stdout, giving up after `timeout`.
fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> Result<String, String> {
    hide_console(cmd);
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let reader = thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        out
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err("timed out".into());
        }
        thread::sleep(Duration::from_millis(50));
    };
    let out = reader.join().unwrap_or_default();
    if status.success() {
        Ok(out)
    } else {
        Err(format!("exited with {status}"))
    }
}

/// Checks that `path` really is Claude Code; returns its version line.
pub fn verify(path: &Path) -> Result<String, String> {
    let out = run_with_timeout(Command::new(path).arg("--version"), VERSION_TIMEOUT)?;
    let line = out.lines().next().unwrap_or_default().trim().to_string();
    if line.contains("Claude Code") || line.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        Ok(line)
    } else {
        Err(format!("unexpected --version output: {line}"))
    }
}

/// Runs one headless Haiku turn with no tools, MCP servers, skills or hooks.
/// `workdir` is a private folder, so no project CLAUDE.md gets loaded.
pub fn ask(cli: &Path, workdir: &Path, lang: Lang, prompt: &str) -> Result<String, String> {
    let persona = persona(lang);
    let mut cmd = Command::new(cli);
    cmd.args([
        "-p",
        "--model",
        "haiku",
        "--output-format",
        "text",
        "--no-session-persistence",
        "--tools",
        "",
        "--strict-mcp-config",
        "--disable-slash-commands",
        "--settings",
        r#"{"disableAllHooks":true}"#,
        "--system-prompt",
        &persona,
    ])
    .current_dir(workdir)
    .env("CLAWD_INTERNAL", "1")
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    hide_console(&mut cmd);

    let mut child = cmd.spawn().map_err(|e| format!("cannot start claude: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(prompt.as_bytes()).map_err(|e| e.to_string())?;
    }
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let reader = thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        out
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if started.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err("claude timed out".into());
        }
        thread::sleep(Duration::from_millis(100));
    };
    let out = reader.join().unwrap_or_default();
    if !status.success() {
        let mut err = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut err);
        }
        let reason = if err.trim().is_empty() { out.trim() } else { err.trim() };
        return Err(format!("claude failed: {reason}"));
    }
    Ok(out)
}

/// Drops a leading "- ", "• " or "1. " / "2) " list marker.
fn strip_list_marker(line: &str) -> &str {
    let bullet = line.trim_start_matches(['-', '*', '•', '–', '—']);
    if bullet.len() != line.len() {
        return bullet.trim_start();
    }
    let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    match line[digits..].chars().next() {
        Some('.' | ')') if digits > 0 => line[digits + 1..].trim_start(),
        _ => line,
    }
}

/// Turns model output into clean one-line phrases.
pub fn phrases(output: &str, max_chars: usize) -> Vec<String> {
    let mut seen = Vec::new();
    for line in output.lines() {
        let line = strip_list_marker(line.trim())
            .trim_matches(|c: char| matches!(c, '"' | '«' | '»' | '“' | '”' | '`' | '\''))
            .trim();
        let len = line.chars().count();
        if len >= 2 && len <= max_chars && !seen.iter().any(|s: &String| s == line) {
            seen.push(line.to_string());
        }
    }
    seen
}

pub fn idle_prompt(lang: Lang, context: &str, count: u32) -> String {
    format!(
        "What is going on right now:\n{context}\n\nWrite {count} different short lines (up to 40 characters) \
         that Clawd could say right now, in {}. Most lines should react to the situation above (time \
         of day, the open window, the music, whether the user is busy or idle). If the user seems away or idle, \
         gently tease it (e.g. hint that you've been standing around doing nothing). Keep a couple of lines \
         generic. Light, warm, geeky humour. One per line.",
        lang.name()
    )
}

pub fn thought_prompt(lang: Lang, task_text: &str) -> String {
    format!(
        "Claude is working on the user's task and writes:\n\n{task_text}\n\nRetell it as one short line \
         (up to 50 characters) in {}: Clawd thinking out loud, as if he were doing the work.",
        lang.name()
    )
}

pub fn done_prompt(lang: Lang, project: &str, last_message: &str) -> String {
    format!(
        "Claude finished a task in the project \"{project}\". Its final message:\n\n{last_message}\n\n\
         In one short line (up to 60 characters) in {}, tell the user what was done and what to check.",
        lang.name()
    )
}

/// The prompt for a reply to something the user typed to Clawd in the chat box.
/// `history` holds earlier turns as (speaker, text), speaker being "user" or "clawd".
pub fn chat_prompt(lang: Lang, context: &str, history: &[[String; 2]], message: &str) -> String {
    let mut dialogue = String::new();
    for [who, text] in history.iter().rev().take(8).rev() {
        let who = if who == "user" { "User" } else { "Clawd" };
        dialogue.push_str(&format!("{who}: {text}\n"));
    }
    format!(
        "What is going on right now:\n{context}\n\nConversation so far:\n{dialogue}User: {message}\n\n\
         Answer the user as Clawd, in {}, in one or two short sentences (under 200 characters), staying \
         in character. If they ask something you can't know, say so playfully.",
        lang.name()
    )
}

/// One clean reply for the speech bubble: no quotes or speaker prefix, whitespace collapsed.
pub fn reply(output: &str, max_chars: usize) -> Option<String> {
    let text = output.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = text.strip_prefix("Clawd:").unwrap_or(&text).trim();
    let text = text.trim_matches(|c: char| matches!(c, '"' | '«' | '»' | '“' | '”' | '`')).trim();
    if text.is_empty() {
        return None;
    }
    Some(match text.char_indices().nth(max_chars) {
        Some((i, _)) => format!("{}…", text[..i].trim_end()),
        None => text.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_up_model_output() {
        let out = "1. «Привет!»\n- Ещё строка\n\n\"Привет!\"\n• Третья\n".to_string()
            + &"x".repeat(80);
        assert_eq!(phrases(&out, 40), vec!["Привет!", "Ещё строка", "Третья"]);
        assert_eq!(phrases("3 бага починил\n2) Готово", 40), vec!["3 бага починил", "Готово"]);
    }

    #[test]
    fn chat_reply_is_one_clean_line() {
        assert_eq!(reply("Clawd: «Привет!\n  Как дела?»", 200).as_deref(), Some("Привет! Как дела?"));
        assert_eq!(reply("   ", 200), None);
        assert_eq!(reply(&"a".repeat(10), 4).as_deref(), Some("aaaa…"));
    }

    #[test]
    fn chat_prompt_keeps_recent_turns() {
        let history: Vec<[String; 2]> = (0..12).map(|i| ["user".into(), format!("m{i}")]).collect();
        let prompt = chat_prompt(Lang::En, "- ctx", &history, "hi");
        assert!(!prompt.contains("User: m3\n") && prompt.contains("User: m4\n") && prompt.contains("User: hi"));
    }
}

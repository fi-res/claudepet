//! Asks Claude Haiku for Clawd's lines through the Claude Code CLI in headless
//! mode, so it runs on the user's own Claude Code login and needs no API key.

use std::{
    env,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(90);

const PERSONA: &str = "Ты — Clawd, маленький оранжевый пиксельный питомец, талисман Claude Code. \
Ты живёшь на рабочем столе пользователя: гуляешь по панели задач, читаешь, программируешь, \
танцуешь под музыку и болеешь за пользователя. Говоришь по-русски, от первого лица, очень коротко, \
дружелюбно и с лёгким гиковским юмором. Отвечай только самими репликами: без кавычек, эмодзи, \
нумерации, markdown и пояснений.";

/// Locates the `claude` executable: $CLAWD_CLAUDE_PATH, then PATH, then the usual install spots.
pub fn find_cli() -> Option<PathBuf> {
    if let Some(path) = env::var_os("CLAWD_CLAUDE_PATH").map(PathBuf::from) {
        return path.is_file().then_some(path);
    }
    let names: &[&str] = if cfg!(windows) {
        &["claude.exe", "claude.cmd"]
    } else {
        &["claude"]
    };
    let mut dirs: Vec<PathBuf> = env::var_os("PATH")
        .map(|p| env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(home) = home_dir() {
        dirs.push(home.join(".local").join("bin"));
        dirs.push(home.join(".claude").join("local"));
        if cfg!(windows) {
            if let Some(appdata) = env::var_os("APPDATA") {
                dirs.push(PathBuf::from(appdata).join("npm"));
            }
        }
    }
    dirs.iter()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|path| path.is_file())
}

pub fn home_dir() -> Option<PathBuf> {
    env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

/// Runs one headless Haiku turn with no tools, MCP servers, skills or hooks.
/// `workdir` is a private folder, so no project CLAUDE.md gets loaded.
pub fn ask(cli: &Path, workdir: &Path, prompt: &str) -> Result<String, String> {
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
        PERSONA,
    ])
    .current_dir(workdir)
    .env("CLAWD_INTERNAL", "1")
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

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

pub fn idle_prompt(context: &str, count: u32) -> String {
    format!(
        "Что сейчас происходит:\n{context}\n\nПридумай {count} разных коротких реплик (до 40 символов), \
         которые Clawd мог бы сказать прямо сейчас. Можно подшутить над контекстом. По одной на строку."
    )
}

pub fn thought_prompt(task_text: &str) -> String {
    format!(
        "Claude сейчас работает над задачей пользователя и пишет:\n\n{task_text}\n\n\
         Перескажи это одной короткой репликой (до 50 символов) — как мысль вслух Clawd, \
         будто это он делает работу."
    )
}

pub fn done_prompt(project: &str, last_message: &str) -> String {
    format!(
        "Claude закончил задачу в проекте «{project}». Его итоговое сообщение:\n\n{last_message}\n\n\
         Одной короткой репликой (до 60 символов) скажи пользователю, что сделано и что стоит проверить."
    )
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
}

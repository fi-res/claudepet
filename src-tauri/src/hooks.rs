//! Claude Code integration: hooks in ~/.claude/settings.json POST each event to
//! a tiny HTTP endpoint on localhost, and Clawd acts out what the session is doing.

use std::{
    collections::HashMap,
    fs,
    io::{Read, Seek, SeekFrom, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::{
    claude::{self, Haiku},
    settings::Lang,
};

pub const PORT: u16 = 47823;
const MARKER: &str = "127.0.0.1:47823/hook/";
const EVENTS: [&str; 5] = ["UserPromptSubmit", "PreToolUse", "Stop", "Notification", "SessionEnd"];
const MAX_BODY: usize = 8 << 20;
const TRANSCRIPT_TAIL: u64 = 256 << 10;
const THOUGHT_EVERY: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ClaudeEvent {
    Start { session: String, project: String },
    Tool { session: String, project: String, tool: String, detail: String },
    Thought { session: String, text: String },
    Attention { session: String, project: String, message: String },
    Stop { session: String, project: String },
    Report { session: String, project: String, text: String },
    End { session: String },
}

struct Parsed {
    event: ClaudeEvent,
    transcript: Option<PathBuf>,
    last_message: Option<String>,
}

fn str_field<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

fn clip(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match text.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &text[..i]),
        None => text,
    }
}

/// Short human description of what a tool call is about.
fn tool_detail(tool: &str, input: &Value) -> String {
    let detail = match tool {
        "Read" | "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => {
            let path = str_field(input, "file_path");
            let path = if path.is_empty() { str_field(input, "notebook_path") } else { path };
            file_name(path)
        }
        "Bash" => str_field(input, "command").to_string(),
        "Grep" | "Glob" => str_field(input, "pattern").to_string(),
        "WebSearch" => str_field(input, "query").to_string(),
        "WebFetch" => {
            let url = str_field(input, "url");
            url.split('/').nth(2).unwrap_or(url).to_string()
        }
        "Task" | "Agent" => str_field(input, "description").to_string(),
        _ => String::new(),
    };
    clip(&detail, 40)
}

fn parse(body: &Value, internal: &Path) -> Option<Parsed> {
    let cwd = str_field(body, "cwd");
    // Clawd's own Haiku calls run in `internal`; never react to those.
    if !cwd.is_empty() && Path::new(cwd).starts_with(internal) {
        return None;
    }
    let session = str_field(body, "session_id").to_string();
    let project = file_name(cwd);
    let event = match str_field(body, "hook_event_name") {
        "UserPromptSubmit" => ClaudeEvent::Start { session, project },
        "PreToolUse" => {
            let tool = str_field(body, "tool_name").to_string();
            let detail = tool_detail(&tool, body.get("tool_input").unwrap_or(&Value::Null));
            ClaudeEvent::Tool { session, project, tool, detail }
        }
        "Notification" => ClaudeEvent::Attention {
            session,
            project,
            message: clip(str_field(body, "message"), 80),
        },
        "Stop" => ClaudeEvent::Stop { session, project },
        "SessionEnd" => ClaudeEvent::End { session },
        _ => return None,
    };
    let transcript = Some(str_field(body, "transcript_path"))
        .filter(|p| !p.is_empty())
        .map(PathBuf::from);
    let last_message = Some(str_field(body, "last_assistant_message"))
        .filter(|m| !m.trim().is_empty())
        .map(str::to_string);
    Some(Parsed { event, transcript, last_message })
}

/// The text of the most recent assistant message in a session transcript (JSONL).
pub fn last_assistant_text(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TRANSCRIPT_TAIL))).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    let tail = String::from_utf8_lossy(&buf);
    tail.lines().rev().find_map(|line| {
        let entry: Value = serde_json::from_str(line).ok()?;
        if str_field(&entry, "type") != "assistant" {
            return None;
        }
        let text: Vec<&str> = entry
            .pointer("/message/content")?
            .as_array()?
            .iter()
            .filter(|block| str_field(block, "type") == "text")
            .map(|block| str_field(block, "text"))
            .filter(|t| !t.trim().is_empty())
            .collect();
        (!text.is_empty()).then(|| text.join("\n"))
    })
}

/// First sentence of a message, cleaned of markdown, for when Haiku isn't available.
fn first_sentence(text: &str, max: usize) -> String {
    let plain: String = text.chars().filter(|c| !matches!(c, '#' | '*' | '`' | '_' | '>')).collect();
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    // A sentence ends at . ! ? followed by a space, so "main.rs" stays whole.
    let end = plain
        .char_indices()
        .enumerate()
        .find(|&(n, (i, c))| {
            n >= 10 && matches!(c, '.' | '!' | '?') && plain[i + c.len_utf8()..].starts_with(' ')
        })
        .map(|(_, (i, c))| i + c.len_utf8())
        .unwrap_or(plain.len());
    clip(&plain[..end], max)
}

#[derive(Default)]
struct Session {
    last_text: String,
    last_thought: Option<Instant>,
    thinking: bool,
}

pub struct Hub {
    haiku: Arc<Haiku>,
    sessions: Mutex<HashMap<String, Session>>,
    emit: Box<dyn Fn(ClaudeEvent) + Send + Sync>,
}

impl Hub {
    pub fn new(haiku: Arc<Haiku>, emit: impl Fn(ClaudeEvent) + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            haiku,
            sessions: Mutex::new(HashMap::new()),
            emit: Box::new(emit),
        })
    }

    /// Runs Haiku if it's available and enabled; None means "fall back to plain text".
    fn haiku(&self, prompt: impl FnOnce(Lang) -> String, max: usize) -> Option<String> {
        match self.haiku.run(prompt) {
            Ok(out) => claude::phrases(&out, max).into_iter().next(),
            Err(e) => {
                eprintln!("haiku: {e}");
                None
            }
        }
    }

    pub fn handle(self: &Arc<Self>, body: &Value) {
        let Some(parsed) = parse(body, &self.haiku.workdir) else {
            return;
        };
        let latest = || {
            parsed
                .last_message
                .clone()
                .or_else(|| parsed.transcript.as_deref().and_then(last_assistant_text))
        };
        match &parsed.event {
            ClaudeEvent::Start { session, .. } => {
                // Remember the previous answer so it isn't mistaken for a new thought.
                let text = parsed.transcript.as_deref().and_then(last_assistant_text).unwrap_or_default();
                self.sessions.lock().unwrap().entry(session.clone()).or_default().last_text = text;
                (self.emit)(parsed.event.clone());
            }
            ClaudeEvent::Tool { session, .. } => {
                (self.emit)(parsed.event.clone());
                let Some(text) = parsed.transcript.as_deref().and_then(last_assistant_text) else {
                    return;
                };
                {
                    let mut sessions = self.sessions.lock().unwrap();
                    let state = sessions.entry(session.clone()).or_default();
                    let due = state.last_thought.is_none_or(|t| t.elapsed() >= THOUGHT_EVERY);
                    if text == state.last_text || state.thinking || !due {
                        return;
                    }
                    state.last_text = text.clone();
                    state.thinking = true;
                    state.last_thought = Some(Instant::now());
                }
                let hub = self.clone();
                let session = session.clone();
                thread::spawn(move || {
                    let thought = hub
                        .haiku(|lang| claude::thought_prompt(lang, &clip(&text, 1500)), 60)
                        .unwrap_or_else(|| first_sentence(&text, 60));
                    if let Some(state) = hub.sessions.lock().unwrap().get_mut(&session) {
                        state.thinking = false;
                    }
                    (hub.emit)(ClaudeEvent::Thought { session, text: thought });
                });
            }
            ClaudeEvent::Stop { session, project } => {
                (self.emit)(parsed.event.clone());
                let Some(text) = latest() else {
                    return;
                };
                let hub = self.clone();
                let (session, project) = (session.clone(), project.clone());
                thread::spawn(move || {
                    let report = hub
                        .haiku(|lang| claude::done_prompt(lang, &project, &clip(&text, 2000)), 80)
                        .unwrap_or_else(|| first_sentence(&text, 70));
                    (hub.emit)(ClaudeEvent::Report { session, project, text: report });
                });
            }
            ClaudeEvent::End { session } => {
                self.sessions.lock().unwrap().remove(session);
                (self.emit)(parsed.event.clone());
            }
            _ => (self.emit)(parsed.event.clone()),
        }
    }
}

/// Reads one HTTP request; returns its body if it is `POST /hook/<token>`.
fn read_request(stream: &mut TcpStream, token: &str) -> Option<Vec<u8>> {
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > 64 << 10 {
            return None;
        }
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.lines();
    let mut request = lines.next()?.split_whitespace();
    let (method, path) = (request.next()?, request.next()?);
    if method != "POST" || path != format!("/hook/{token}") {
        return None;
    }
    let length: usize = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse().ok())?;
    if length > MAX_BODY {
        return None;
    }
    let mut body = buf.split_off(header_end);
    while body.len() < length {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(length);
    Some(body)
}

pub fn serve(token: String, hub: Arc<Hub>) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", PORT))?;
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let body = read_request(&mut stream, &token);
            let status = if body.is_some() { "204 No Content" } else { "404 Not Found" };
            let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            drop(stream);
            if let Some(value) = body.and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
                hub.handle(&value);
            }
        }
    });
    Ok(())
}

/// A per-install secret in the hook URL, so web pages can't make Clawd talk.
pub fn load_token(dir: &Path) -> String {
    let path = dir.join("hook-token");
    if let Ok(token) = fs::read_to_string(&path) {
        let token = token.trim();
        if token.len() >= 16 && token.chars().all(|c| c.is_ascii_alphanumeric()) {
            return token.to_string();
        }
    }
    use std::hash::{BuildHasher, Hasher};
    let mut token = String::new();
    for i in 0..2 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
        h.write_u32(std::process::id() ^ i);
        token.push_str(&format!("{:016x}", h.finish()));
    }
    let _ = fs::create_dir_all(dir);
    let _ = fs::write(&path, &token);
    token
}

pub fn settings_path() -> Option<PathBuf> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| claude::home_dir().map(|h| h.join(".claude")))?;
    Some(dir.join("settings.json"))
}

fn hook_command(token: &str) -> String {
    format!(
        "curl -s -m 2 -o /dev/null -H Content-Type:application/json --data-binary @- http://{MARKER}{token} || true"
    )
}

fn is_clawd_group(group: &Value) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| hooks.iter().any(|h| str_field(h, "command").contains(MARKER)))
}

/// Removes Clawd's hooks from settings and, given a command, adds fresh ones.
fn with_hooks(mut settings: Value, command: Option<&str>) -> Result<Value, String> {
    let root = settings.as_object_mut().ok_or("settings.json is not a JSON object")?;
    let hooks = root.entry("hooks").or_insert_with(|| Value::Object(Map::new()));
    let hooks = hooks.as_object_mut().ok_or("\"hooks\" in settings.json is not an object")?;
    for event in EVENTS {
        let groups = hooks.entry(event).or_insert_with(|| Value::Array(Vec::new()));
        let groups = groups.as_array_mut().ok_or(format!("hooks.{event} is not an array"))?;
        groups.retain(|g| !is_clawd_group(g));
        if let Some(command) = command {
            let hook = json!({ "type": "command", "command": command, "timeout": 5 });
            let group = if event == "PreToolUse" {
                json!({ "matcher": "*", "hooks": [hook] })
            } else {
                json!({ "hooks": [hook] })
            };
            groups.push(group);
        }
        if groups.is_empty() {
            hooks.remove(event);
        }
    }
    if hooks.is_empty() {
        root.remove("hooks");
    }
    Ok(settings)
}

pub fn is_installed(path: &Path) -> bool {
    fs::read_to_string(path).is_ok_and(|s| s.contains(MARKER))
}

/// Adds (with a token) or removes Clawd's hooks, keeping everything else intact.
pub fn set_installed(path: &Path, token: Option<&str>) -> Result<(), String> {
    let original = match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    let settings = match &original {
        Some(text) if !text.trim().is_empty() => {
            serde_json::from_str(text).map_err(|e| format!("settings.json is not valid JSON: {e}"))?
        }
        _ => Value::Object(Map::new()),
    };
    let updated = with_hooks(settings, token.map(hook_command).as_deref())?;
    if let Some(text) = &original {
        let backup = path.with_extension("json.clawd-backup");
        if !backup.exists() {
            fs::write(&backup, text).map_err(|e| e.to_string())?;
        }
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.clawd-tmp");
    let pretty = serde_json::to_string_pretty(&updated).map_err(|e| e.to_string())? + "\n";
    fs::write(&tmp, pretty).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("clawd-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn install_keeps_user_settings_and_uninstall_restores_them() {
        let user = json!({
            "model": "opus",
            "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "notify-send done" }] }] }
        });
        let installed = with_hooks(user.clone(), Some(&hook_command("abc"))).unwrap();
        assert_eq!(installed["model"], "opus");
        assert_eq!(installed["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(installed["hooks"]["PreToolUse"][0]["matcher"], "*");
        // Reinstalling replaces rather than duplicates.
        let again = with_hooks(installed, Some(&hook_command("def"))).unwrap();
        assert_eq!(again["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert!(again.to_string().contains("/hook/def") && !again.to_string().contains("/hook/abc"));
        assert_eq!(with_hooks(again, None).unwrap(), user);
        assert_eq!(with_hooks(json!({}), None).unwrap(), json!({}));
    }

    #[test]
    fn refuses_to_touch_broken_settings() {
        let dir = temp_dir("broken");
        let path = dir.join("settings.json");
        fs::write(&path, "{ not json").unwrap();
        assert!(set_installed(&path, Some("abc")).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json");
        assert!(!dir.join("settings.json.clawd-backup").exists());
    }

    #[test]
    fn install_writes_backup_and_detects_itself() {
        let dir = temp_dir("install");
        let path = dir.join("settings.json");
        fs::write(&path, r#"{"theme":"dark"}"#).unwrap();
        set_installed(&path, Some("tok123")).unwrap();
        assert!(is_installed(&path));
        assert_eq!(fs::read_to_string(dir.join("settings.json.clawd-backup")).unwrap(), r#"{"theme":"dark"}"#);
        set_installed(&path, None).unwrap();
        assert!(!is_installed(&path));
        let restored: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(restored, json!({"theme": "dark"}));
    }

    #[test]
    fn parses_hook_events() {
        let internal = Path::new("/home/u/.local/share/clawd/haiku");
        let tool = json!({
            "hook_event_name": "PreToolUse", "session_id": "s1", "cwd": "/home/u/proj",
            "tool_name": "Edit", "tool_input": { "file_path": "/home/u/proj/src/main.rs" }
        });
        let parsed = parse(&tool, internal).unwrap();
        assert_eq!(
            parsed.event,
            ClaudeEvent::Tool { session: "s1".into(), project: "proj".into(), tool: "Edit".into(), detail: "main.rs".into() }
        );
        let own = json!({ "hook_event_name": "Stop", "cwd": "/home/u/.local/share/clawd/haiku" });
        assert!(parse(&own, internal).is_none());
        let bash = json!({ "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": { "command": "cargo   test\n --all" } });
        assert!(matches!(parse(&bash, internal).unwrap().event, ClaudeEvent::Tool { detail, .. } if detail == "cargo test --all"));
    }

    #[test]
    fn reads_last_assistant_text_from_transcript() {
        let dir = temp_dir("transcript");
        let path = dir.join("t.jsonl");
        let lines = [
            json!({"type": "user", "message": {"role": "user", "content": "fix it"}}),
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Сначала посмотрю тесты."}]}}),
            json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Bash"}]}}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result", "content": "ok"}]}}),
        ];
        let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        fs::write(&path, text.join("\n")).unwrap();
        assert_eq!(last_assistant_text(&path).as_deref(), Some("Сначала посмотрю тесты."));
    }

    #[test]
    fn first_sentence_is_short_and_plain() {
        assert_eq!(first_sentence("**Готово!** Я починил тесты. Ещё...", 60), "Готово! Я починил тесты.");
        assert_eq!(first_sentence("## Итог: всё работает", 60), "Итог: всё работает");
        assert_eq!(first_sentence("Исправил баг в `main.rs`. Проверь.", 60), "Исправил баг в main.rs.");
    }

    #[test]
    fn reads_requests_with_the_right_token_only() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let send = |path: &'static str| {
            thread::spawn(move || {
                let mut s = TcpStream::connect(addr).unwrap();
                let body = r#"{"a":1}"#;
                write!(s, "POST {path} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
            })
        };
        send("/hook/secret");
        let (mut stream, _) = listener.accept().unwrap();
        assert_eq!(read_request(&mut stream, "secret").as_deref(), Some(&br#"{"a":1}"#[..]));
        send("/hook/wrong");
        let (mut stream, _) = listener.accept().unwrap();
        assert!(read_request(&mut stream, "secret").is_none());
    }
}

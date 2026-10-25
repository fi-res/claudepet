#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod claude;
mod context;
mod hooks;
mod settings;
mod tray;

use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, RwLock,
    },
    thread,
    time::Duration,
};

use serde::Serialize;
use tauri::{Emitter, Manager, State, WebviewWindow};

use claude::Haiku;
use settings::{Lang, Settings};

pub const WINDOW: &str = "pet";
const CURSOR_POLL: Duration = Duration::from_millis(16);
const REFIT_EVERY: u32 = 120;

#[derive(Clone, Copy, PartialEq, Serialize)]
struct Cursor {
    x: f64,
    y: f64,
}

pub struct AppState {
    settings_path: PathBuf,
    pub settings: Mutex<Settings>,
    pub haiku: Arc<Haiku>,
    pub listening: Arc<AtomicBool>,
    pub watching: Arc<AtomicBool>,
    pub token: String,
    idle_busy: AtomicBool,
}

impl AppState {
    /// Changes the saved preferences and writes them to disk.
    pub fn update(&self, change: impl FnOnce(&mut Settings)) {
        let mut settings = self.settings.lock().unwrap();
        change(&mut settings);
        settings.save(&self.settings_path);
    }
}

#[tauri::command]
fn set_click_through(window: WebviewWindow, enabled: bool) -> Result<(), String> {
    window
        .set_ignore_cursor_events(enabled)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_language(state: State<'_, Arc<AppState>>) -> Lang {
    state.haiku.lang()
}

/// A batch of idle lines from Haiku for the pet's chatter.
#[tauri::command]
async fn claude_phrases(state: State<'_, Arc<AppState>>, context: String, count: u32) -> Result<Vec<String>, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        if state.idle_busy.swap(true, Ordering::AcqRel) {
            return Err("busy".to_string());
        }
        let result = state.haiku.run(|lang| claude::idle_prompt(lang, &context, count.clamp(1, 10)));
        state.idle_busy.store(false, Ordering::Release);
        result.map(|out| claude::phrases(&out, 50))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn main() {
    #[cfg(target_os = "linux")]
    prepare_linux_env();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![set_click_through, get_language, claude_phrases])
        .setup(|app| {
            let window = app
                .get_webview_window(WINDOW)
                .expect("pet window is declared in tauri.conf.json");
            fit_to_work_area(&window)?;
            // On Linux the native window must be realized (shown) before click-through can be set.
            window.show()?;
            window.set_ignore_cursor_events(true)?;
            spawn_cursor_poller(window.clone());

            let data_dir = app.path().app_data_dir()?;
            let settings_path = app.path().app_config_dir()?.join("settings.json");
            let settings = Settings::load(&settings_path);
            let workdir = data_dir.join("haiku");
            fs::create_dir_all(&workdir)?;
            // Claude Code reports cwd with symlinks resolved.
            #[cfg(unix)]
            let workdir = fs::canonicalize(&workdir).unwrap_or(workdir);

            let state = Arc::new(AppState {
                settings_path,
                haiku: Arc::new(Haiku {
                    workdir,
                    cli: RwLock::new(None),
                    lang: RwLock::new(settings.language),
                    enabled: AtomicBool::new(settings.llm),
                }),
                listening: Arc::new(AtomicBool::new(settings.listen)),
                watching: Arc::new(AtomicBool::new(settings.watch)),
                token: hooks::load_token(&data_dir),
                idle_busy: AtomicBool::new(false),
                settings: Mutex::new(settings),
            });
            app.manage(state.clone());

            // Looking through the login shell can take a few seconds; don't hold up startup.
            let haiku = state.haiku.clone();
            let saved = state.settings.lock().unwrap().claude_path.clone();
            thread::spawn(move || {
                let found = claude::find_cli(saved.as_deref());
                eprintln!("claude: {}", found.as_deref().map_or("not found".into(), |p| p.display().to_string()));
                *haiku.cli.write().unwrap() = found;
            });

            let w = window.clone();
            audio::spawn(state.listening.clone(), move |e| send(&w, "audio", e));
            let w = window.clone();
            context::spawn(state.watching.clone(), move |e| send(&w, "context", e));
            let w = window.clone();
            let hub = hooks::Hub::new(state.haiku.clone(), move |e| send(&w, "claude", e));
            if let Err(e) = hooks::serve(state.token.clone(), hub) {
                eprintln!("Claude Code hook endpoint unavailable: {e}");
            }

            tray::build(app, state)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Clawd");
}

/// Forwards an event to the page; with CLAWD_DEBUG set, also logs it to stderr.
fn send<T: Serialize + Clone + std::fmt::Debug>(window: &WebviewWindow, event: &str, payload: T) {
    static DEBUG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *DEBUG.get_or_init(|| std::env::var_os("CLAWD_DEBUG").is_some()) {
        eprintln!("[{event}] {payload:?}");
    }
    let _ = window.emit_to(WINDOW, event, payload);
}

#[cfg(target_os = "linux")]
fn prepare_linux_env() {
    use std::env;
    // Wayland doesn't let clients position windows or read the global cursor,
    // so run through XWayland unless the user picked a backend explicitly.
    if env::var_os("WAYLAND_DISPLAY").is_some() && env::var_os("GDK_BACKEND").is_none() {
        env::set_var("GDK_BACKEND", "x11");
    }
    // Transparent WebKitGTK windows render black on some GPU drivers with DMA-BUF.
    if env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
}

/// Stretch the overlay over the primary monitor, minus the taskbar/panels,
/// so Clawd walks on top of the taskbar.
fn fit_to_work_area(window: &WebviewWindow) -> tauri::Result<()> {
    let monitor = match window.primary_monitor()? {
        Some(m) => Some(m),
        None => window.current_monitor()?,
    };
    let Some(monitor) = monitor else {
        return Ok(());
    };
    let area = monitor.work_area();
    if window.outer_position()? != area.position {
        window.set_position(area.position)?;
    }
    if window.inner_size()? != area.size {
        window.set_size(area.size)?;
    }
    Ok(())
}

/// The webview gets no mouse events while click-through is on, so feed it
/// the global cursor position (in window-local CSS pixels) instead.
fn spawn_cursor_poller(window: WebviewWindow) {
    thread::spawn(move || {
        let mut last: Option<Cursor> = None;
        let mut ticks = 0u32;
        loop {
            thread::sleep(CURSOR_POLL);
            if !window.is_visible().unwrap_or(false) {
                continue;
            }
            ticks = ticks.wrapping_add(1);
            if ticks.is_multiple_of(REFIT_EVERY) {
                let _ = fit_to_work_area(&window);
            }
            let (Ok(cursor), Ok(origin), Ok(scale)) = (
                window.cursor_position(),
                window.inner_position(),
                window.scale_factor(),
            ) else {
                continue;
            };
            let pos = Cursor {
                x: (cursor.x - f64::from(origin.x)) / scale,
                y: (cursor.y - f64::from(origin.y)) / scale,
            };
            if last != Some(pos) {
                last = Some(pos);
                let _ = window.emit_to(WINDOW, "cursor", pos);
            }
        }
    });
}

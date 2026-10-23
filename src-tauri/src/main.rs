#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod claude;
mod context;
mod hooks;

use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use serde::Serialize;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    App, AppHandle, Emitter, Manager, State, WebviewWindow,
};

const WINDOW: &str = "pet";
const CURSOR_POLL: Duration = Duration::from_millis(16);
const REFIT_EVERY: u32 = 120;

#[derive(Clone, Copy, PartialEq, Serialize)]
struct Cursor {
    x: f64,
    y: f64,
}

#[tauri::command]
fn set_click_through(window: WebviewWindow, enabled: bool) -> Result<(), String> {
    window
        .set_ignore_cursor_events(enabled)
        .map_err(|e| e.to_string())
}

/// Haiku access for the frontend's idle chatter.
struct Brain {
    cli: Option<PathBuf>,
    workdir: PathBuf,
    llm: Arc<AtomicBool>,
    busy: AtomicBool,
}

impl Brain {
    fn phrases(&self, context: &str, count: u32) -> Result<Vec<String>, String> {
        if !self.llm.load(Ordering::Relaxed) {
            return Err("disabled".into());
        }
        let cli = self.cli.as_ref().ok_or("claude CLI not found")?;
        if self.busy.swap(true, Ordering::AcqRel) {
            return Err("busy".into());
        }
        let result = claude::ask(cli, &self.workdir, &claude::idle_prompt(context, count.clamp(1, 10)));
        self.busy.store(false, Ordering::Release);
        result.map(|out| claude::phrases(&out, 50))
    }
}

#[tauri::command]
async fn claude_phrases(brain: State<'_, Arc<Brain>>, context: String, count: u32) -> Result<Vec<String>, String> {
    let brain = brain.inner().clone();
    tauri::async_runtime::spawn_blocking(move || brain.phrases(&context, count))
        .await
        .map_err(|e| e.to_string())?
}

#[derive(Clone, Serialize)]
struct Notice {
    phrase: &'static str,
}

struct Toggles {
    listening: Arc<AtomicBool>,
    watching: Arc<AtomicBool>,
    llm: Arc<AtomicBool>,
    has_cli: bool,
    token: String,
}

fn main() {
    #[cfg(target_os = "linux")]
    prepare_linux_env();

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![set_click_through, claude_phrases])
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
            let workdir = data_dir.join("haiku");
            fs::create_dir_all(&workdir)?;
            // Claude Code reports cwd with symlinks resolved.
            #[cfg(unix)]
            let workdir = fs::canonicalize(&workdir).unwrap_or(workdir);
            let cli = claude::find_cli();
            let toggles = Toggles {
                listening: Arc::new(AtomicBool::new(true)),
                watching: Arc::new(AtomicBool::new(true)),
                llm: Arc::new(AtomicBool::new(cli.is_some())),
                has_cli: cli.is_some(),
                token: hooks::load_token(&data_dir),
            };
            app.manage(Arc::new(Brain {
                cli: cli.clone(),
                workdir: workdir.clone(),
                llm: toggles.llm.clone(),
                busy: AtomicBool::new(false),
            }));

            let w = window.clone();
            audio::spawn(toggles.listening.clone(), move |e| send(&w, "audio", e));
            let w = window.clone();
            context::spawn(toggles.watching.clone(), move |e| send(&w, "context", e));
            let w = window.clone();
            let hub = hooks::Hub::new(cli, workdir, toggles.llm.clone(), move |e| send(&w, "claude", e));
            if let Err(e) = hooks::serve(toggles.token.clone(), hub) {
                eprintln!("Claude Code hook endpoint unavailable: {e}");
            }

            build_tray(app, toggles)?;
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

fn build_tray(app: &App, toggles: Toggles) -> tauri::Result<()> {
    let hooks_installed = hooks::settings_path().is_some_and(|p| hooks::is_installed(&p));
    let recall = MenuItem::with_id(app, "recall", "Позвать Clawd", true, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle", "Спрятать", true, None::<&str>)?;
    let listen = CheckMenuItem::with_id(app, "listen", "Слушать звук", true, true, None::<&str>)?;
    let watch = CheckMenuItem::with_id(app, "watch", "Видеть окна и печать", true, true, None::<&str>)?;
    let llm_label = if toggles.has_cli {
        "Реплики через Claude (Haiku)"
    } else {
        "Реплики через Claude (не найден claude)"
    };
    let llm = CheckMenuItem::with_id(app, "llm", llm_label, toggles.has_cli, toggles.has_cli, None::<&str>)?;
    let link = CheckMenuItem::with_id(app, "hooks", "Связать с Claude Code", true, hooks_installed, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &recall,
            &toggle,
            &PredefinedMenuItem::separator(app)?,
            &listen,
            &watch,
            &llm,
            &link,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let mut tray = TrayIconBuilder::with_id("clawd")
        .tooltip("Clawd")
        .menu(&menu)
        .show_menu_on_left_click(true);
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.on_menu_event(move |app, event| match event.id.as_ref() {
        "recall" => {
            set_visible(app, &toggle, true);
            let _ = app.emit_to(WINDOW, "recall", ());
        }
        "toggle" => {
            let visible = app
                .get_webview_window(WINDOW)
                .and_then(|w| w.is_visible().ok())
                .unwrap_or(false);
            set_visible(app, &toggle, !visible);
        }
        "listen" => {
            let on = listen.is_checked().unwrap_or(true);
            toggles.listening.store(on, Ordering::Relaxed);
        }
        "watch" => {
            let on = watch.is_checked().unwrap_or(true);
            toggles.watching.store(on, Ordering::Relaxed);
            if !on {
                // Make the pet forget what it saw.
                let _ = app.emit_to(WINDOW, "context", context::ContextEvent::Window { app: String::new(), title: String::new() });
                let _ = app.emit_to(WINDOW, "context", context::ContextEvent::Typing { kpm: 0 });
            }
        }
        "llm" => {
            toggles.llm.store(llm.is_checked().unwrap_or(false), Ordering::Relaxed);
        }
        "hooks" => {
            let on = link.is_checked().unwrap_or(false);
            let result = hooks::settings_path()
                .ok_or_else(|| "home directory not found".to_string())
                .and_then(|path| hooks::set_installed(&path, on.then_some(toggles.token.as_str())));
            let phrase = match result {
                Ok(()) if on => "hooks_on",
                Ok(()) => "hooks_off",
                Err(e) => {
                    eprintln!("Claude Code hooks: {e}");
                    let _ = link.set_checked(!on);
                    "hooks_error"
                }
            };
            let _ = app.emit_to(WINDOW, "notice", Notice { phrase });
        }
        "quit" => app.exit(0),
        _ => {}
    })
    .build(app)?;
    Ok(())
}

fn set_visible(app: &AppHandle, toggle: &MenuItem<tauri::Wry>, visible: bool) {
    let Some(window) = app.get_webview_window(WINDOW) else {
        return;
    };
    let _ = if visible { window.show() } else { window.hide() };
    let _ = toggle.set_text(if visible { "Спрятать" } else { "Показать" });
}

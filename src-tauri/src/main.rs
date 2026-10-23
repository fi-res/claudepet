#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{thread, time::Duration};

use serde::Serialize;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    App, AppHandle, Emitter, Manager, WebviewWindow,
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

fn main() {
    #[cfg(target_os = "linux")]
    prepare_linux_env();

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![set_click_through])
        .setup(|app| {
            let window = app
                .get_webview_window(WINDOW)
                .expect("pet window is declared in tauri.conf.json");
            fit_to_work_area(&window)?;
            // On Linux the native window must be realized (shown) before click-through can be set.
            window.show()?;
            window.set_ignore_cursor_events(true)?;
            build_tray(app)?;
            spawn_cursor_poller(window);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Clawd");
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

fn build_tray(app: &App) -> tauri::Result<()> {
    let recall = MenuItem::with_id(app, "recall", "Позвать Clawd", true, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle", "Спрятать", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&recall, &toggle, &separator, &quit])?;

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

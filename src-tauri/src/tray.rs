//! The tray menu: switches, language and finding `claude` for Haiku lines.

use std::{
    path::PathBuf,
    sync::{atomic::Ordering, Arc},
    thread,
};

use serde::Serialize;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::TrayIconBuilder,
    App, AppHandle, Emitter, Manager, Wry,
};
use tauri_plugin_dialog::DialogExt;

use crate::{claude, context::ContextEvent, hooks, settings::Lang, AppState, WINDOW};

#[derive(Clone, Serialize)]
struct Notice {
    phrase: &'static str,
}

fn notice(app: &AppHandle, phrase: &'static str) {
    let _ = app.emit_to(WINDOW, "notice", Notice { phrase });
}

fn text(lang: Lang, key: &str) -> &'static str {
    let ru = lang == Lang::Ru;
    match key {
        "recall" => if ru { "Позвать Clawd" } else { "Call Clawd" },
        "hide" => if ru { "Спрятать" } else { "Hide" },
        "show" => if ru { "Показать" } else { "Show" },
        "listen" => if ru { "Слушать звук" } else { "Listen to sound" },
        "watch" => if ru { "Видеть окна и печать" } else { "See windows and typing" },
        "llm" => if ru { "Реплики через Claude (Haiku)" } else { "Lines from Claude (Haiku)" },
        "pick" => if ru { "Указать путь к claude…" } else { "Locate claude…" },
        "hooks" => if ru { "Связать с Claude Code" } else { "Connect to Claude Code" },
        "quit" => if ru { "Выход" } else { "Quit" },
        "pick_title" => if ru { "Где находится claude?" } else { "Where is claude?" },
        _ => "",
    }
}

#[derive(Clone)]
struct Items {
    recall: MenuItem<Wry>,
    toggle: MenuItem<Wry>,
    listen: CheckMenuItem<Wry>,
    watch: CheckMenuItem<Wry>,
    llm: CheckMenuItem<Wry>,
    pick: MenuItem<Wry>,
    link: CheckMenuItem<Wry>,
    ru: CheckMenuItem<Wry>,
    en: CheckMenuItem<Wry>,
    quit: MenuItem<Wry>,
}

impl Items {
    fn relabel(&self, lang: Lang, visible: bool) {
        let _ = self.recall.set_text(text(lang, "recall"));
        let _ = self.toggle.set_text(text(lang, if visible { "hide" } else { "show" }));
        let _ = self.listen.set_text(text(lang, "listen"));
        let _ = self.watch.set_text(text(lang, "watch"));
        let _ = self.llm.set_text(text(lang, "llm"));
        let _ = self.pick.set_text(text(lang, "pick"));
        let _ = self.link.set_text(text(lang, "hooks"));
        let _ = self.quit.set_text(text(lang, "quit"));
        let _ = self.ru.set_checked(lang == Lang::Ru);
        let _ = self.en.set_checked(lang == Lang::En);
    }
}

fn window_visible(app: &AppHandle) -> bool {
    app.get_webview_window(WINDOW).and_then(|w| w.is_visible().ok()).unwrap_or(false)
}

pub fn build(app: &App, state: Arc<AppState>) -> tauri::Result<()> {
    let settings = state.settings.lock().unwrap().clone();
    let lang = settings.language;
    let hooks_installed = hooks::settings_path().is_some_and(|p| hooks::is_installed(&p));
    let check = |id: &str, on: bool| CheckMenuItem::with_id(app, id, text(lang, id), true, on, None::<&str>);
    let items = Items {
        recall: MenuItem::with_id(app, "recall", text(lang, "recall"), true, None::<&str>)?,
        toggle: MenuItem::with_id(app, "toggle", text(lang, "hide"), true, None::<&str>)?,
        listen: check("listen", settings.listen)?,
        watch: check("watch", settings.watch)?,
        llm: check("llm", settings.llm)?,
        pick: MenuItem::with_id(app, "pick", text(lang, "pick"), true, None::<&str>)?,
        link: CheckMenuItem::with_id(app, "hooks", text(lang, "hooks"), true, hooks_installed, None::<&str>)?,
        ru: CheckMenuItem::with_id(app, "lang_ru", "Русский", true, lang == Lang::Ru, None::<&str>)?,
        en: CheckMenuItem::with_id(app, "lang_en", "English", true, lang == Lang::En, None::<&str>)?,
        quit: MenuItem::with_id(app, "quit", text(lang, "quit"), true, None::<&str>)?,
    };
    let language = Submenu::with_items(app, "Язык / Language", true, &[&items.ru, &items.en])?;
    let menu = Menu::with_items(
        app,
        &[
            &items.recall,
            &items.toggle,
            &PredefinedMenuItem::separator(app)?,
            &items.listen,
            &items.watch,
            &items.llm,
            &items.pick,
            &items.link,
            &language,
            &PredefinedMenuItem::separator(app)?,
            &items.quit,
        ],
    )?;

    let mut tray = TrayIconBuilder::with_id("clawd").tooltip("Clawd").menu(&menu).show_menu_on_left_click(true);
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.on_menu_event(move |app, event| {
        let lang = state.haiku.lang();
        match event.id.as_ref() {
            "recall" => {
                set_visible(app, &items, lang, true);
                let _ = app.emit_to(WINDOW, "recall", ());
            }
            "toggle" => set_visible(app, &items, lang, !window_visible(app)),
            "listen" => {
                let on = items.listen.is_checked().unwrap_or(true);
                state.listening.store(on, Ordering::Relaxed);
                state.update(|s| s.listen = on);
            }
            "watch" => {
                let on = items.watch.is_checked().unwrap_or(true);
                state.watching.store(on, Ordering::Relaxed);
                state.update(|s| s.watch = on);
                if !on {
                    // Make the pet forget what it saw.
                    let _ = app.emit_to(WINDOW, "context", ContextEvent::Window { app: String::new(), title: String::new() });
                    let _ = app.emit_to(WINDOW, "context", ContextEvent::Typing { kpm: 0 });
                }
            }
            "llm" => {
                let on = items.llm.is_checked().unwrap_or(false);
                state.update(|s| s.llm = on);
                if !on {
                    state.haiku.enabled.store(false, Ordering::Relaxed);
                } else if state.haiku.cli().is_some() {
                    state.haiku.enabled.store(true, Ordering::Relaxed);
                } else {
                    find_or_ask(app.clone(), state.clone(), items.clone());
                }
            }
            "pick" => pick_claude(app.clone(), state.clone(), items.clone()),
            "hooks" => {
                let on = items.link.is_checked().unwrap_or(false);
                let result = hooks::settings_path()
                    .ok_or_else(|| "home directory not found".to_string())
                    .and_then(|path| hooks::set_installed(&path, on.then_some(state.token.as_str())));
                let phrase = match result {
                    Ok(()) if on => "hooks_on",
                    Ok(()) => "hooks_off",
                    Err(e) => {
                        eprintln!("Claude Code hooks: {e}");
                        let _ = items.link.set_checked(!on);
                        "hooks_error"
                    }
                };
                notice(app, phrase);
            }
            id @ ("lang_ru" | "lang_en") => {
                let lang = if id == "lang_ru" { Lang::Ru } else { Lang::En };
                *state.haiku.lang.write().unwrap() = lang;
                state.update(|s| s.language = lang);
                items.relabel(lang, window_visible(app));
                let _ = app.emit_to(WINDOW, "language", lang);
            }
            "quit" => app.exit(0),
            _ => {}
        }
    })
    .build(app)?;
    Ok(())
}

fn set_visible(app: &AppHandle, items: &Items, lang: Lang, visible: bool) {
    if let Some(window) = app.get_webview_window(WINDOW) {
        let _ = if visible { window.show() } else { window.hide() };
    }
    let _ = items.toggle.set_text(text(lang, if visible { "hide" } else { "show" }));
}

/// Uses `path` for Haiku if it really is Claude Code.
fn adopt(app: &AppHandle, state: &AppState, items: &Items, path: PathBuf) -> bool {
    match claude::verify(&path) {
        Ok(version) => {
            eprintln!("claude: using {} ({version})", path.display());
            *state.haiku.cli.write().unwrap() = Some(path.clone());
            state.haiku.enabled.store(true, Ordering::Relaxed);
            state.update(|s| {
                s.claude_path = Some(path);
                s.llm = true;
            });
            let _ = items.llm.set_checked(true);
            true
        }
        Err(e) => {
            eprintln!("claude: {} rejected: {e}", path.display());
            notice(app, "claude_invalid");
            false
        }
    }
}

/// Searches for `claude` off the UI thread; if it's nowhere to be found, asks the user.
fn find_or_ask(app: AppHandle, state: Arc<AppState>, items: Items) {
    thread::spawn(move || {
        let saved = state.settings.lock().unwrap().claude_path.clone();
        if let Some(path) = claude::find_cli(saved.as_deref()) {
            if adopt(&app, &state, &items, path) {
                notice(&app, "claude_found");
                return;
            }
        }
        notice(&app, "claude_missing");
        pick_claude(app, state, items);
    });
}

fn pick_claude(app: AppHandle, state: Arc<AppState>, items: Items) {
    let mut dialog = app.dialog().file().set_title(text(state.haiku.lang(), "pick_title"));
    if cfg!(windows) {
        dialog = dialog.add_filter("claude", &["exe", "cmd"]);
    }
    if let Some(home) = claude::home_dir() {
        dialog = dialog.set_directory(home);
    }
    dialog.pick_file(move |file| {
        let picked = file.and_then(|f| f.into_path().ok());
        thread::spawn(move || {
            let adopted = picked.is_some_and(|path| adopt(&app, &state, &items, path));
            if adopted {
                notice(&app, "claude_found");
            } else if state.haiku.cli().is_none() {
                // Still nothing to talk through: leave Haiku off.
                state.haiku.enabled.store(false, Ordering::Relaxed);
                state.update(|s| s.llm = false);
                let _ = items.llm.set_checked(false);
            }
        });
    });
}

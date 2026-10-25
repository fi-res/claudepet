//! Where a dropped file lives. The page only sees a dropped file's name and contents;
//! on Windows it can hand the File objects to us with WebView2's
//! postMessageWithAdditionalObjects, and here they turn into real paths and metadata,
//! so Clawd can offer commands on the file itself (delete it, move it, open its folder).

use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;

/// The message the page posts together with the dropped files.
#[cfg(windows)]
const MESSAGE: &str = "clawd:files";

/// A dropped file's whereabouts; times are unix seconds.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FileMeta {
    pub name: String,
    pub path: String,
    pub folder: bool,
    pub size: u64,
    pub modified: Option<u64>,
    pub created: Option<u64>,
}

fn unix(time: std::io::Result<SystemTime>) -> Option<u64> {
    time.ok()?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

pub fn meta(path: &str) -> Option<FileMeta> {
    let p = Path::new(path);
    let info = fs::metadata(p).ok()?;
    Some(FileMeta {
        name: p.file_name().map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned()),
        path: path.to_string(),
        folder: info.is_dir(),
        size: info.len(),
        modified: unix(info.modified()),
        created: unix(info.created()),
    })
}

/// Calls `on_paths` with the paths of files the page posts with the "clawd:files" message.
#[cfg(windows)]
pub fn listen(window: &tauri::WebviewWindow, on_paths: impl Fn(Vec<String>) + Send + 'static) {
    use webview2_com::{
        take_pwstr, Microsoft::Web::WebView2::Win32::{ICoreWebView2File, ICoreWebView2WebMessageReceivedEventArgs2},
        WebMessageReceivedEventHandler,
    };
    use windows::core::{Interface, PWSTR};

    let result = window.with_webview(move |webview| unsafe {
        let Ok(core) = webview.controller().CoreWebView2() else {
            eprintln!("drop paths: no CoreWebView2");
            return;
        };
        let handler = WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut message = PWSTR::null();
            // Tauri's own IPC messages aren't plain strings or aren't ours: ignore them.
            if args.TryGetWebMessageAsString(&mut message).is_err() || take_pwstr(message) != MESSAGE {
                return Ok(());
            }
            let objects = args.cast::<ICoreWebView2WebMessageReceivedEventArgs2>()?.AdditionalObjects()?;
            let mut count = 0u32;
            objects.Count(&mut count)?;
            let mut paths = Vec::new();
            for i in 0..count {
                let Ok(file) = objects.GetValueAtIndex(i).and_then(|o| o.cast::<ICoreWebView2File>()) else {
                    continue;
                };
                let mut path = PWSTR::null();
                if file.Path(&mut path).is_ok() {
                    paths.push(take_pwstr(path));
                }
            }
            on_paths(paths);
            Ok(())
        }));
        let mut token = 0i64;
        if let Err(e) = core.add_WebMessageReceived(&handler, &mut token) {
            eprintln!("drop paths: {e}");
        }
    });
    if let Err(e) = result {
        eprintln!("drop paths: {e}");
    }
}

/// Elsewhere the page keeps names and contents only.
#[cfg(not(windows))]
pub fn listen(_window: &tauri::WebviewWindow, _on_paths: impl Fn(Vec<String>) + Send + 'static) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_file_metadata() {
        let dir = std::env::temp_dir().join(format!("clawd-meta-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("notes.txt");
        fs::write(&file, "hello").unwrap();
        let m = meta(&file.display().to_string()).unwrap();
        assert_eq!((m.name.as_str(), m.size, m.folder), ("notes.txt", 5, false));
        assert!(m.modified.is_some());
        assert!(meta(&dir.display().to_string()).unwrap().folder);
        assert_eq!(meta("Z:/definitely/not/here.txt"), None);
        let _ = fs::remove_dir_all(dir);
    }
}

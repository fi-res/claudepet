//! What the user is up to: the focused window and how fast they type.
//! Only the number of key presses is counted; which keys were pressed is never kept.

use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use serde::Serialize;

const POLL: Duration = Duration::from_millis(30);
const WINDOW_EVERY: u32 = 33; // ~1 s
const TYPING_EVERY: Duration = Duration::from_secs(2);
const TYPING_WINDOW: Duration = Duration::from_secs(10);
const MAX_TITLE: usize = 120;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ContextEvent {
    Window { app: String, title: String },
    /// Key presses per minute over the last few seconds.
    Typing { kpm: u32 },
}

type KeyBits = [u8; 32];

fn count_new_presses(prev: &KeyBits, now: &KeyBits) -> u32 {
    prev.iter().zip(now).map(|(p, n)| (n & !p).count_ones()).sum()
}

fn shorten(title: &str) -> String {
    let title = title.trim();
    match title.char_indices().nth(MAX_TITLE) {
        Some((i, _)) => format!("{}…", &title[..i]),
        None => title.to_string(),
    }
}

pub fn spawn(enabled: Arc<AtomicBool>, mut emit: impl FnMut(ContextEvent) + Send + 'static) {
    thread::spawn(move || loop {
        if !enabled.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(500));
            continue;
        }
        let Some(probe) = platform::Probe::new() else {
            eprintln!("context: no desktop session to watch");
            thread::sleep(Duration::from_secs(30));
            continue;
        };
        let mut last_window: Option<(String, String)> = None;
        let mut prev_keys: KeyBits = [0; 32];
        let mut presses: VecDeque<(Instant, u32)> = VecDeque::new();
        let mut last_typing = Instant::now();
        let mut last_kpm = 0;
        let mut tick = 0u32;
        while enabled.load(Ordering::Relaxed) {
            thread::sleep(POLL);
            tick = tick.wrapping_add(1);

            let keys = probe.keys();
            let n = count_new_presses(&prev_keys, &keys);
            prev_keys = keys;
            if n > 0 {
                presses.push_back((Instant::now(), n));
            }
            while presses.front().is_some_and(|(t, _)| t.elapsed() > TYPING_WINDOW) {
                presses.pop_front();
            }
            if last_typing.elapsed() >= TYPING_EVERY {
                last_typing = Instant::now();
                let total: u32 = presses.iter().map(|(_, n)| n).sum();
                let kpm = total * 60 / TYPING_WINDOW.as_secs() as u32;
                if kpm != last_kpm {
                    last_kpm = kpm;
                    emit(ContextEvent::Typing { kpm });
                }
            }

            if tick.is_multiple_of(WINDOW_EVERY) {
                if let Some((app, title)) = probe.active_window() {
                    // Clicking Clawd focuses his own overlay; that's not interesting.
                    let current = (app, shorten(&title));
                    if current.1 != "Clawd" && last_window.as_ref() != Some(&current) {
                        last_window = Some(current.clone());
                        emit(ContextEvent::Window {
                            app: current.0,
                            title: current.1,
                        });
                    }
                }
            }
        }
    });
}

#[cfg(target_os = "linux")]
mod platform {
    use super::KeyBits;
    use x11rb::{
        connection::Connection,
        protocol::xproto::{Atom, AtomEnum, ConnectionExt, Window},
        rust_connection::RustConnection,
    };

    pub struct Probe {
        conn: RustConnection,
        root: Window,
        net_active_window: Atom,
        net_wm_name: Atom,
        utf8_string: Atom,
    }

    impl Probe {
        pub fn new() -> Option<Self> {
            let (conn, screen) = x11rb::connect(None).ok()?;
            let root = conn.setup().roots.get(screen)?.root;
            let atom = |name: &[u8]| Some(conn.intern_atom(false, name).ok()?.reply().ok()?.atom);
            let net_active_window = atom(b"_NET_ACTIVE_WINDOW")?;
            let net_wm_name = atom(b"_NET_WM_NAME")?;
            let utf8_string = atom(b"UTF8_STRING")?;
            Some(Self {
                conn,
                root,
                net_active_window,
                net_wm_name,
                utf8_string,
            })
        }

        fn property(&self, window: Window, property: Atom, type_: Atom) -> Option<Vec<u8>> {
            let reply = self
                .conn
                .get_property(false, window, property, type_, 0, 1024)
                .ok()?
                .reply()
                .ok()?;
            Some(reply.value)
        }

        pub fn active_window(&self) -> Option<(String, String)> {
            let raw = self.property(self.root, self.net_active_window, AtomEnum::WINDOW.into())?;
            let window = u32::from_ne_bytes(raw.get(..4)?.try_into().ok()?);
            if window == 0 {
                return None;
            }
            let title = self
                .property(window, self.net_wm_name, self.utf8_string)
                .filter(|t| !t.is_empty())
                .or_else(|| self.property(window, AtomEnum::WM_NAME.into(), AtomEnum::STRING.into()))
                .unwrap_or_default();
            // WM_CLASS is "instance\0Class\0"; the class reads like an app name.
            let class = self
                .property(window, AtomEnum::WM_CLASS.into(), AtomEnum::STRING.into())
                .unwrap_or_default();
            let app = class
                .split(|&b| b == 0)
                .rfind(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .unwrap_or_default();
            Some((app, String::from_utf8_lossy(&title).into_owned()))
        }

        pub fn keys(&self) -> KeyBits {
            self.conn
                .query_keymap()
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|r| r.keys)
                .unwrap_or([0; 32])
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::KeyBits;
    use std::path::Path;
    use windows::{
        core::PWSTR,
        Win32::{
            Foundation::CloseHandle,
            System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION},
            UI::{
                Input::KeyboardAndMouse::GetAsyncKeyState,
                WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId},
            },
        },
    };

    pub struct Probe;

    impl Probe {
        pub fn new() -> Option<Self> {
            Some(Self)
        }

        pub fn active_window(&self) -> Option<(String, String)> {
            unsafe {
                let hwnd = GetForegroundWindow();
                if hwnd.is_invalid() {
                    return None;
                }
                let mut buf = [0u16; 512];
                let len = GetWindowTextW(hwnd, &mut buf).max(0) as usize;
                let title = String::from_utf16_lossy(&buf[..len]);

                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                let mut app = String::new();
                if let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                    let mut path = [0u16; 1024];
                    let mut size = path.len() as u32;
                    if QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(path.as_mut_ptr()), &mut size).is_ok() {
                        let full = String::from_utf16_lossy(&path[..size as usize]);
                        if let Some(stem) = Path::new(&full).file_stem() {
                            app = stem.to_string_lossy().into_owned();
                        }
                    }
                    let _ = CloseHandle(process);
                }
                Some((app, title))
            }
        }

        pub fn keys(&self) -> KeyBits {
            let mut bits = [0u8; 32];
            // 0x08.. skips the mouse buttons.
            for vk in 0x08..=0xFE_i32 {
                if unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000 != 0 {
                    bits[vk as usize / 8] |= 1 << (vk % 8);
                }
            }
            bits
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    use super::KeyBits;

    pub struct Probe;

    impl Probe {
        pub fn new() -> Option<Self> {
            None
        }
        pub fn active_window(&self) -> Option<(String, String)> {
            None
        }
        pub fn keys(&self) -> KeyBits {
            [0; 32]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_only_new_presses() {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        b[1] = 0b0000_0110;
        assert_eq!(count_new_presses(&a, &b), 2);
        a[1] = 0b0000_0010;
        assert_eq!(count_new_presses(&a, &b), 1);
        assert_eq!(count_new_presses(&b, &a), 0);
    }

    #[test]
    fn long_titles_are_shortened() {
        let long = "я".repeat(200);
        let short = shorten(&long);
        assert_eq!(short.chars().count(), MAX_TITLE + 1);
        assert_eq!(shorten("  Firefox  "), "Firefox");
    }
}

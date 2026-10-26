//! Which monitor Clawd lives on. The overlay covers one monitor's work area at a
//! time; when Clawd walks or flies off a side that has another monitor next to it,
//! the whole overlay moves over there and he steps in from the opposite edge.

use serde::Serialize;
use tauri::{Monitor, WebviewWindow};

/// Monitors closer than this (physical pixels) count as touching.
const TOUCH: i32 = 16;

/// The overlay's size in CSS pixels and which sides lead to another monitor.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Layout {
    pub width: f64,
    pub height: f64,
    pub left: bool,
    pub right: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl Rect {
    fn of(m: &Monitor) -> Self {
        let (p, s) = (m.position(), m.size());
        Self { left: p.x, top: p.y, right: p.x + s.width as i32, bottom: p.y + s.height as i32 }
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.left as f64 && x < self.right as f64 && y >= self.top as f64 && y < self.bottom as f64
    }
}

/// A stable name for a monitor, to remember where Clawd was.
pub fn key(m: &Monitor) -> String {
    m.name().cloned().unwrap_or_else(|| format!("{}x{}", m.position().x, m.position().y))
}

/// The monitor called `saved`, or the primary one when it's gone or unset.
pub fn current(window: &WebviewWindow, saved: Option<&str>) -> Option<Monitor> {
    let all = window.available_monitors().ok()?;
    if let Some(found) = saved.and_then(|name| all.iter().find(|m| key(m) == name)) {
        return Some(found.clone());
    }
    window
        .primary_monitor()
        .ok()
        .flatten()
        .or_else(|| window.current_monitor().ok().flatten())
        .or_else(|| all.into_iter().next())
}

/// The monitor under a point in physical screen coordinates.
pub fn at(window: &WebviewWindow, x: f64, y: f64) -> Option<Monitor> {
    window.available_monitors().ok()?.into_iter().find(|m| Rect::of(m).contains(x, y))
}

/// Picks the touching monitor on one side of `from`, preferring the one at height `y`.
fn neighbor_of(from: Rect, others: &[(Rect, usize)], right: bool, y: f64) -> Option<usize> {
    let touching: Vec<&(Rect, usize)> = others
        .iter()
        .filter(|(r, _)| {
            let edge = if right { (r.left - from.right).abs() } else { (r.right - from.left).abs() };
            edge <= TOUCH && r.top < from.bottom && r.bottom > from.top
        })
        .collect();
    touching
        .iter()
        .find(|(r, _)| y >= r.top as f64 && y < r.bottom as f64)
        .or_else(|| touching.first())
        .map(|(_, i)| *i)
}

/// The monitor next to `from` on the left or right, if there is one.
pub fn neighbor(window: &WebviewWindow, from: &Monitor, right: bool, y: f64) -> Option<Monitor> {
    let all = window.available_monitors().ok()?;
    let home = Rect::of(from);
    let others: Vec<(Rect, usize)> =
        all.iter().enumerate().filter(|(_, m)| key(m) != key(from)).map(|(i, m)| (Rect::of(m), i)).collect();
    neighbor_of(home, &others, right, y).map(|i| all[i].clone())
}

/// Stretches the overlay over a monitor's work area (minus the taskbar), so Clawd
/// walks on top of the taskbar.
pub fn fit(window: &WebviewWindow, monitor: &Monitor) -> tauri::Result<()> {
    let area = monitor.work_area();
    if window.outer_position()? != area.position {
        window.set_position(area.position)?;
    }
    if window.inner_size()? != area.size {
        window.set_size(area.size)?;
    }
    Ok(())
}

pub fn layout(window: &WebviewWindow, monitor: &Monitor) -> Layout {
    let area = monitor.work_area();
    let scale = monitor.scale_factor();
    let mid = f64::from(monitor.position().y) + f64::from(monitor.size().height) / 2.0;
    Layout {
        width: f64::from(area.size.width) / scale,
        height: f64::from(area.size.height) / scale,
        left: neighbor(window, monitor, false, mid).is_some(),
        right: neighbor(window, monitor, true, mid).is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect { left, top, right, bottom }
    }

    #[test]
    fn finds_side_by_side_monitors() {
        let main = rect(0, 0, 1920, 1080);
        let others = [(rect(1920, 0, 4480, 1440), 1), (rect(-1280, 200, 0, 1224), 2), (rect(0, -1080, 1920, 0), 3)];
        assert_eq!(neighbor_of(main, &others, true, 500.0), Some(1));
        assert_eq!(neighbor_of(main, &others, false, 500.0), Some(2));
        // The monitor above doesn't count as a side.
        assert_eq!(neighbor_of(main, &others[2..], true, 500.0), None);
        // Monitors that don't overlap vertically aren't neighbours either.
        assert_eq!(neighbor_of(main, &[(rect(1920, 1100, 3840, 2180), 4)], true, 500.0), None);
    }

    #[test]
    fn prefers_the_monitor_at_clawds_height() {
        let main = rect(0, 0, 1920, 2160);
        let others = [(rect(1920, 0, 3840, 1080), 1), (rect(1920, 1080, 3840, 2160), 2)];
        assert_eq!(neighbor_of(main, &others, true, 1500.0), Some(2));
        assert_eq!(neighbor_of(main, &others, true, 100.0), Some(1));
    }
}

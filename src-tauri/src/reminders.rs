//! Reminders the user sets in the chat ("remind me in 25 minutes"). They live in
//! settings.json, so a reminder set before a restart still goes off afterwards.

use std::{
    sync::Arc,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{settings::Reminder, AppState};

const MAX_REMINDERS: usize = 20;

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// Adds a reminder `minutes` from now; returns when it's due (unix seconds).
pub fn add(state: &AppState, minutes: f64, text: &str) -> u64 {
    let at = now() + (minutes.max(0.0) * 60.0).round() as u64;
    state.update(|s| {
        s.reminders.push(Reminder { at, text: text.trim().to_string() });
        s.reminders.sort_by_key(|r| r.at);
        s.reminders.truncate(MAX_REMINDERS);
    });
    at
}

pub fn clear(state: &AppState) -> usize {
    let mut count = 0;
    state.update(|s| {
        count = s.reminders.len();
        s.reminders.clear();
    });
    count
}

pub fn count(state: &AppState) -> usize {
    state.settings.lock().unwrap().reminders.len()
}

/// Takes the reminders that are due at `now` out of `list`.
fn take_due(list: &mut Vec<Reminder>, now: u64) -> Vec<Reminder> {
    let (due, later) = list.drain(..).partition(|r| r.at <= now);
    *list = later;
    due
}

/// Checks once a second and hands due reminders to `fire`.
pub fn spawn(state: Arc<AppState>, fire: impl Fn(&Reminder) + Send + 'static) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(1));
        let pending = state.settings.lock().unwrap().reminders.first().is_some_and(|r| r.at <= now());
        if !pending {
            continue;
        }
        let mut due = Vec::new();
        state.update(|s| due = take_due(&mut s.reminders, now()));
        for reminder in &due {
            fire(reminder);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_only_due_reminders() {
        let r = |at, text: &str| Reminder { at, text: text.into() };
        let mut list = vec![r(10, "a"), r(20, "b"), r(30, "c")];
        assert_eq!(take_due(&mut list, 20), vec![r(10, "a"), r(20, "b")]);
        assert_eq!(list, vec![r(30, "c")]);
        assert!(take_due(&mut list, 5).is_empty());
    }
}

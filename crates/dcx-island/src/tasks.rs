//! Tasks and reminders: the data model, the day roll-over and the due check.
//!
//! # Storage
//!
//! `%LOCALAPPDATA%\dcx\island_tasks.json`, written through
//! `dcx_win32::config_file` (path mutex + atomic swap + poisoned read-only).
//! JSON rather than TOML because the file is a list of records that the UI
//! edits field by field, and `serde_json` round-trips it without the array-of-
//! tables gymnastics TOML needs.
//!
//! **One writer.** The UI thread owns every save. Background threads (clipboard,
//! mail, QQ) never touch this file; they post into the store and ask for a
//! repaint. `FileLock::acquire` blocks up to 3 s, and a sampling thread taking
//! that lock every second would stall a frame — the single-writer rule is what
//! keeps the lock uncontended in practice.
//!
//! # Recurrence
//!
//! A repeating task stores the day of its **next** occurrence. Completing it, or
//! the day passing, advances that day instead of creating a new task. This is
//! why "carry over" must not touch repeating tasks: shifting their day to today
//! would fire a reminder they already had.
//!
//! There are two independent axes and they are deliberately not merged:
//!
//! - [`Task::repeat`] is **which days** the task exists on (once / daily /
//!   weekdays / weekly);
//! - [`Task::every`] is **how often within one day** it reminds, starting at
//!   [`Task::at`] and ending at [`Task::until`].
//!
//! "工作日上午九点到下午六点每半小时提醒" is therefore `repeat = workdays`,
//! `at = 09:00`, `every = 30`, `until = 18:00` — one record, not forty. A day
//! with an interval fires up to `(until - at) / every + 1` times, and the fire
//! key is the **slot**, not the day, so each slot fires exactly once even across
//! restarts.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// How long an interval window lasts when the user only typed a start time.
/// Eight hours from 09:00 is a working day, which is the case this feature was
/// asked for; anything longer turns "每半小时" into an evening of popups.
const DEFAULT_WINDOW: u32 = 8 * 3600;

/// A reminder more than this late is stale: the machine was asleep or the app
/// was closed. Firing it would pop a notification about something the user has
/// already moved past.
const STALE_AFTER: u32 = 1800;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Repeat {
    Once,
    Daily,
    /// Monday–Friday.
    Workdays,
    /// Same weekday as the task's own day.
    Weekly,
}

impl Repeat {
    pub fn label(self) -> &'static str {
        match self {
            Repeat::Once => "不重复",
            Repeat::Daily => "每天",
            Repeat::Workdays => "工作日",
            Repeat::Weekly => "每周",
        }
    }

    /// Next occurrence strictly after `day`, or `None` for a one-off.
    pub fn next_day(self, day: &str) -> Option<String> {
        match self {
            Repeat::Once => None,
            Repeat::Daily => shift_days(day, 1),
            Repeat::Weekly => shift_days(day, 7),
            Repeat::Workdays => {
                let mut d = shift_days(day, 1)?;
                for _ in 0..7 {
                    if weekday_of(&d).map(|w| w < 5).unwrap_or(false) {
                        return Some(d);
                    }
                    d = shift_days(&d, 1)?;
                }
                Some(d)
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub note: String,
    /// Local calendar day this task belongs to, `YYYY-MM-DD`.
    pub day: String,
    /// Reminder time as seconds of day. `None` = no reminder. For an interval
    /// task this is the **first** slot of the day.
    #[serde(default)]
    pub at: Option<u32>,
    /// Minutes between reminders inside the day. `None` = a single reminder at
    /// `at`. Only meaningful together with `at`.
    #[serde(default)]
    pub every: Option<u32>,
    /// Last slot of the day, inclusive. Only meaningful with `every`; a file
    /// written by an older build has neither field and keeps working.
    #[serde(default)]
    pub until: Option<u32>,
    #[serde(default = "once")]
    pub repeat: Repeat,
    #[serde(default)]
    pub done: bool,
    /// The task shown in the capsule. At most one task is current.
    #[serde(default)]
    pub current: bool,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub done_at: Option<u64>,
    /// `YYYY-MM-DD@HH:MM` of the last reminder, so a tick loop cannot fire the
    /// same reminder twice — including after a restart.
    #[serde(default)]
    pub fired_on: Option<String>,
}

fn once() -> Repeat {
    Repeat::Once
}

impl Task {
    pub fn is_repeating(&self) -> bool {
        self.repeat != Repeat::Once
    }

    /// True when the task reminds more than once a day.
    pub fn is_interval(&self) -> bool {
        self.every.is_some_and(|e| e > 0) && self.at.is_some()
    }

    /// Last slot the day will remind at. For a single reminder this is `at`.
    pub fn last_slot(&self) -> Option<u32> {
        let at = self.at?;
        match self.every.filter(|e| *e > 0) {
            None => Some(at),
            Some(_) => Some(self.until.unwrap_or(at).max(at)),
        }
    }

    /// The most recent slot that is not in the future, or `None` before the
    /// first one. This is what the tick loop asks: it is deliberately
    /// self-correcting rather than a queue, so a machine that was asleep for
    /// three hours fires **once** (the slot that just passed) instead of
    /// replaying the backlog.
    pub fn slot_before_or_at(&self, now: u32) -> Option<u32> {
        let at = self.at?;
        if now < at {
            return None;
        }
        let Some(every) = self.every.filter(|e| *e > 0) else {
            return Some(at);
        };
        let step = every as u64 * 60;
        let end = self.until.unwrap_or(at).max(at);
        let now = now as u64;
        let at64 = at as u64;
        let mut slot = at64 + (now - at64) / step * step;
        if slot > end as u64 {
            // Past the window: the last grid point at or before `end`.
            slot = at64 + (end as u64 - at64) / step * step;
        }
        Some(slot as u32)
    }

    /// Per-slot fire key, `YYYY-MM-DD@HH:MM`. One key per reminder, so the same
    /// day can fire many times without a `Vec` of history.
    pub fn slot_key(&self, day: &str, slot: u32) -> String {
        format!("{day}@{:02}:{:02}", (slot / 3600) % 24, (slot % 3600) / 60)
    }

    /// `每 30 分钟` / `每 2 小时` — the interval as the user reads it.
    pub fn every_label(&self) -> Option<String> {
        let every = self.every.filter(|e| *e > 0)?;
        Some(if every % 60 == 0 {
            format!("每 {} 小时", every / 60)
        } else if every > 60 {
            format!("每 {} 小时 {} 分", every / 60, every % 60)
        } else {
            format!("每 {every} 分钟")
        })
    }

    /// `09:00–18:00` for an interval task, `09:00` for a single one.
    pub fn window_label(&self) -> Option<String> {
        let at = self.at?;
        let head = crate::clock::hhmm_of(at);
        match self.every.filter(|e| *e > 0) {
            Some(_) => Some(format!("{head}–{}", crate::clock::hhmm_of(self.last_slot()?))),
            None => Some(head),
        }
    }

    /// True while a reminder for the slot that just passed is worth
    /// highlighting in red. One-off reminders stay marked for half an hour (the
    /// window in which they still deserve attention); an interval task is
    /// marked only for [`Task::FRESH_INTERVAL`], because "within 30 minutes of
    /// the last slot" is *always* true when the interval is 30 minutes and the
    /// row would sit there permanently red.
    pub fn due_now(&self, day: &str, now: u32) -> bool {
        if self.done || self.day != day {
            return false;
        }
        let Some(slot) = self.slot_before_or_at(now) else {
            return false;
        };
        let window = if self.is_interval() {
            Self::FRESH_INTERVAL
        } else {
            STALE_AFTER
        };
        now <= slot + window
    }

    /// How long an interval reminder stays highlighted after it fires.
    const FRESH_INTERVAL: u32 = 300;
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TaskFile {
    #[serde(default)]
    pub next_id: u64,
    #[serde(default)]
    pub tasks: Vec<Task>,
}

pub struct Store {
    pub file: TaskFile,
    pub path: std::path::PathBuf,
    /// Set when a save failed; the panel shows it instead of pretending the
    /// edit was persisted.
    pub error: Option<String>,
    dirty: bool,
}

/// How a new task is described at the point of creation. A struct rather than
/// six positional arguments: `at`/`until`/`every` are three `Option<u32>` of
/// seconds and minutes, and swapping two of them silently produces a task that
/// reminds at 09:30 every fifty minutes.
pub struct NewTask<'a> {
    pub title: &'a str,
    pub day: &'a str,
    pub at: Option<u32>,
    /// End of the interval window; ignored when `every` is `None`.
    pub until: Option<u32>,
    /// Minutes between reminders.
    pub every: Option<u32>,
    pub repeat: Repeat,
}

impl Store {
    pub fn load(path: std::path::PathBuf) -> Store {
        let file = read_file(&path);
        Store {
            file,
            path,
            error: None,
            dirty: false,
        }
    }

    pub fn save_if_dirty(&mut self) {
        if !self.dirty {
            return;
        }
        match write_file(&self.path, &self.file) {
            Ok(()) => {
                self.dirty = false;
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn add(&mut self, new: NewTask<'_>) -> u64 {
        let id = self.file.next_id.max(1);
        self.file.next_id = id + 1;
        // An interval needs a window; a single reminder must not carry one, or
        // the list would print a range that means nothing.
        let every = new.every.filter(|e| *e > 0 && new.at.is_some());
        let until = match (every, new.at) {
            (Some(_), Some(at)) => Some(
                new.until
                    .unwrap_or(at.saturating_add(DEFAULT_WINDOW))
                    .max(at),
            ),
            _ => None,
        };
        self.file.tasks.push(Task {
            id,
            title: new.title.trim().to_string(),
            note: String::new(),
            day: new.day.to_string(),
            at: new.at,
            every,
            until,
            repeat: new.repeat,
            done: false,
            current: false,
            created_at: now_secs(),
            done_at: None,
            fired_on: None,
        });
        self.dirty = true;
        id
    }

    pub fn get(&self, id: u64) -> Option<&Task> {
        self.file.tasks.iter().find(|t| t.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Task> {
        self.file.tasks.iter_mut().find(|t| t.id == id)
    }

    /// Complete or re-open. A repeating task that is completed advances to its
    /// next day and stays open — "done for today" is the useful state for a
    /// daily stand-up, and creating a second task for tomorrow is not.
    pub fn toggle_done(&mut self, id: u64, today: &str) {
        let mut advance_to: Option<String> = None;
        let mut set_current = false;
        if let Some(t) = self.get_mut(id) {
            if t.done {
                t.done = false;
                t.done_at = None;
                if t.is_repeating() {
                    t.day = today.to_string();
                }
            } else {
                t.done = true;
                t.done_at = Some(now_secs());
                t.current = false;
                if t.is_repeating() {
                    advance_to = t.repeat.next_day(&t.day);
                }
            }
            if t.done && t.is_repeating() {
                set_current = true;
            }
        }
        if let (Some(next), true) = (advance_to, set_current) {
            // Keep the record, move it to the next occurrence and re-open it.
            if let Some(t) = self.get_mut(id) {
                t.day = next;
                t.done = false;
                t.done_at = None;
                t.fired_on = None;
            }
        }
        self.dirty = true;
    }

    pub fn set_current(&mut self, id: u64) {
        for t in &mut self.file.tasks {
            t.current = t.id == id && !t.done;
        }
        self.dirty = true;
    }

    pub fn remove(&mut self, id: u64) {
        self.file.tasks.retain(|t| t.id != id);
        self.dirty = true;
    }

    pub fn current(&self) -> Option<&Task> {
        self.file.tasks.iter().find(|t| t.current && !t.done)
    }

    /// Tasks to show for `today`: open ones first, current pinned on top, then
    /// by reminder time. Yesterday's unfinished one-offs are carried over first
    /// (see [`Store::carry_over`]).
    pub fn for_day(&self, today: &str) -> Vec<&Task> {
        let mut out: Vec<&Task> = self
            .file
            .tasks
            .iter()
            .filter(|t| t.day == today)
            .collect();
        out.sort_by(|a, b| {
            b.current
                .cmp(&a.current)
                .then(a.done.cmp(&b.done))
                .then(a.at.unwrap_or(u32::MAX).cmp(&b.at.unwrap_or(u32::MAX)))
                .then(a.id.cmp(&b.id))
        });
        out
    }

    /// Move unfinished one-off tasks from earlier days to `today`. Repeating
    /// tasks are rolled forward instead of shifted, so a daily reminder does not
    /// become "today at the same time" twice.
    pub fn carry_over(&mut self, today: &str) -> usize {
        let mut moved = 0;
        for t in &mut self.file.tasks {
            if t.done || t.day.as_str() >= today {
                continue;
            }
            if t.is_repeating() {
                if let Some(next) = next_on_or_after(t.repeat, &t.day, today) {
                    if next != t.day {
                        t.day = next;
                        t.fired_on = None;
                        moved += 1;
                    }
                }
            } else {
                t.day = today.to_string();
                t.fired_on = None;
                moved += 1;
            }
        }
        if moved > 0 {
            self.dirty = true;
        }
        moved
    }

    /// Reminders whose slot has arrived and that have not fired. Marks them
    /// fired so the caller can be called every tick.
    ///
    /// An interval task is driven by [`Task::slot_before_or_at`] rather than by
    /// a cursor: at most one reminder per task per tick, and only the slot that
    /// just passed. Sleeping through three slots fires once, not three times —
    /// a burst of catch-up notifications at wake-up is worse than a single
    /// "it's time again".
    pub fn take_due(&mut self, today: &str, secs_of_day: u32) -> Vec<Task> {
        let mut due = Vec::new();
        for t in &mut self.file.tasks {
            if t.done || t.day != today {
                continue;
            }
            let Some(at) = t.at else { continue };
            if secs_of_day < at {
                continue;
            }
            let Some(slot) = t.slot_before_or_at(secs_of_day) else {
                continue;
            };
            let key = t.slot_key(today, slot);
            if t.fired_on.as_deref() == Some(key.as_str()) {
                continue;
            }
            // Mark before pushing: the key is what makes this idempotent, and
            // the early returns below must leave it written.
            t.fired_on = Some(key);
            if secs_of_day > slot + STALE_AFTER {
                continue;
            }
            due.push(t.clone());
        }
        if !due.is_empty() {
            self.dirty = true;
        }
        due
    }

    /// Markdown archive for one day, appended if the file already exists.
    pub fn archive(&self, dir: &str, today: &str) -> Result<std::path::PathBuf, String> {
        if dir.trim().is_empty() {
            return Err("未设置归档目录".to_string());
        }
        let path = Path::new(dir).join(format!("{today}.md"));
        let mut text = String::new();
        text.push_str(&format!("# {today}\n\n"));
        let items = self.for_day(today);
        if items.is_empty() {
            text.push_str("（无任务）\n");
        }
        for t in items {
            let mark = if t.done { "x" } else { " " };
            let at = t
                .at
                .map(|s| format!(" {:02}:{:02}", s / 3600, (s % 3600) / 60))
                .unwrap_or_default();
            text.push_str(&format!("- [{mark}] {}{at}", t.title));
            if !t.note.trim().is_empty() {
                text.push_str(&format!(" — {}", t.note.trim()));
            }
            text.push('\n');
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("无法创建归档目录：{e}"))?;
        std::fs::write(&path, text).map_err(|e| format!("写入归档失败：{e}"))?;
        Ok(path)
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Calendar arithmetic without a date crate
// ---------------------------------------------------------------------------

/// Days since 1970-01-01 for a `YYYY-MM-DD` day. Uses Howard Hinnant's
/// `days_from_civil`, which is exact for the whole proleptic Gregorian range.
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as i64;
    let mp = ((m + 9) % 12) as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era as i64 * 146_097 + doe - 719_468
}

pub fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((if m <= 2 { y + 1 } else { y }) as i32, m, d)
}

pub fn parse_day(day: &str) -> Option<(i32, u32, u32)> {
    let mut it = day.split('-');
    let y = it.next()?.parse().ok()?;
    let m = it.next()?.parse().ok()?;
    let d = it.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some((y, m, d))
}

pub fn shift_days(day: &str, delta: i64) -> Option<String> {
    let (y, m, d) = parse_day(day)?;
    let (y2, m2, d2) = civil_from_days(days_from_civil(y, m, d) + delta);
    Some(format!("{y2:04}-{m2:02}-{d2:02}"))
}

/// 0 = Monday … 6 = Sunday.
pub fn weekday_of(day: &str) -> Option<u32> {
    let (y, m, d) = parse_day(day)?;
    // 1970-01-01 was a Thursday (index 3 with Monday = 0).
    let n = days_from_civil(y, m, d) + 3;
    Some((n.rem_euclid(7)) as u32)
}

fn next_on_or_after(repeat: Repeat, from: &str, today: &str) -> Option<String> {
    let mut day = from.to_string();
    for _ in 0..4000 {
        if day.as_str() >= today {
            return Some(day);
        }
        day = match repeat.next_day(&day) {
            Some(d) => d,
            None => return Some(today.to_string()),
        };
    }
    Some(today.to_string())
}

fn read_file(path: &Path) -> TaskFile {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<TaskFile>(&text) {
            Ok(mut f) => {
                dcx_win32::config_file::note_parse_ok(path);
                if f.next_id == 0 {
                    f.next_id = f.tasks.iter().map(|t| t.id).max().unwrap_or(0) + 1;
                }
                f
            }
            Err(e) => {
                // Keep the broken file for the user; start empty and read-only
                // so a later save cannot overwrite it with defaults.
                eprintln!("[dcx-island] tasks parse failed ({e})");
                dcx_win32::config_file::note_parse_error(path);
                TaskFile {
                    next_id: 1,
                    tasks: Vec::new(),
                }
            }
        },
        Err(_) => TaskFile {
            next_id: 1,
            tasks: Vec::new(),
        },
    }
}

fn write_file(path: &Path, file: &TaskFile) -> Result<(), String> {
    let _guard = dcx_win32::config_file::FileLock::acquire(
        path,
        dcx_win32::config_file::LOCK_TIMEOUT,
    )
    .map_err(|e| format!("任务文件被占用：{e}"))?;
    if dcx_win32::config_file::is_poisoned(path) {
        return Err(dcx_win32::config_file::poisoned_error(path).to_string());
    }
    let text = serde_json::to_string_pretty(file).map_err(|e| format!("序列化任务失败：{e}"))?;
    dcx_win32::config_file::write_atomic(path, text.as_bytes())
        .map_err(|e| format!("写入任务失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> Store {
        let dir = std::env::temp_dir().join(format!("dcx-island-tasks-{}-{name}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        Store::load(dir.join("tasks.json"))
    }

    /// One-off / single-reminder task, the shape most tests need.
    fn add(s: &mut Store, title: &str, day: &str, at: Option<u32>, repeat: Repeat) -> u64 {
        s.add(NewTask {
            title,
            day,
            at,
            until: None,
            every: None,
            repeat,
        })
    }

    /// Interval task: `every` minutes from `at` to `until`.
    fn add_every(
        s: &mut Store,
        title: &str,
        day: &str,
        at: u32,
        until: u32,
        every: u32,
        repeat: Repeat,
    ) -> u64 {
        s.add(NewTask {
            title,
            day,
            at: Some(at),
            until: Some(until),
            every: Some(every),
            repeat,
        })
    }

    #[test]
    fn civil_dates_round_trip() {
        for day in ["2026-11-07", "2024-02-29", "1999-12-31", "2100-01-01"] {
            let (y, m, d) = parse_day(day).unwrap();
            let back = civil_from_days(days_from_civil(y, m, d));
            assert_eq!(back, (y, m, d), "{day}");
        }
        assert_eq!(shift_days("2026-11-07", 1).unwrap(), "2026-11-08");
        assert_eq!(shift_days("2026-12-31", 1).unwrap(), "2027-01-01");
        assert_eq!(shift_days("2024-02-28", 1).unwrap(), "2024-02-29");
    }

    #[test]
    fn weekday_of_known_dates() {
        // 2026-11-07 is a Saturday.
        assert_eq!(weekday_of("2026-11-07"), Some(5));
        assert_eq!(weekday_of("2026-11-09"), Some(0));
    }

    #[test]
    fn workday_repeat_skips_the_weekend() {
        // Friday 2026-11-06 → Monday 2026-11-09.
        assert_eq!(
            Repeat::Workdays.next_day("2026-11-06").unwrap(),
            "2026-11-09"
        );
        assert_eq!(Repeat::Daily.next_day("2026-11-06").unwrap(), "2026-11-07");
        assert_eq!(Repeat::Weekly.next_day("2026-11-06").unwrap(), "2026-11-13");
        assert!(Repeat::Once.next_day("2026-11-06").is_none());
    }

    #[test]
    fn carry_over_moves_one_offs_but_rolls_repeats() {
        let mut s = store("carry");
        let a = add(&mut s, "写周报", "2026-11-06", None, Repeat::Once);
        let b = add(&mut s, "站会", "2026-11-06", Some(9 * 3600), Repeat::Workdays);
        let moved = s.carry_over("2026-11-09");
        assert_eq!(moved, 2);
        assert_eq!(s.get(a).unwrap().day, "2026-11-09");
        // Friday's daily task rolls to Monday, not to "today" blindly.
        assert_eq!(s.get(b).unwrap().day, "2026-11-09");
    }

    #[test]
    fn a_reminder_fires_once_per_day() {
        let mut s = store("due");
        let id = add(&mut s, "吃药", "2026-11-07", Some(8 * 3600), Repeat::Daily);
        assert!(s.take_due("2026-11-07", 8 * 3600 - 1).is_empty());
        let due = s.take_due("2026-11-07", 8 * 3600);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, id);
        assert!(s.take_due("2026-11-07", 8 * 3600 + 5).is_empty());
        // Next day fires again.
        assert_eq!(s.take_due("2026-11-08", 8 * 3600).len(), 0);
    }

    #[test]
    fn a_stale_reminder_is_swallowed_instead_of_firing_late() {
        let mut s = store("stale");
        add(&mut s, "错过", "2026-11-07", Some(8 * 3600), Repeat::Once);
        assert!(s.take_due("2026-11-07", 12 * 3600).is_empty());
        assert!(s.take_due("2026-11-07", 13 * 3600).is_empty());
    }

    #[test]
    fn completing_a_daily_task_advances_it() {
        let mut s = store("repeat");
        let id = add(&mut s, "站会", "2026-11-06", Some(9 * 3600), Repeat::Workdays);
        s.toggle_done(id, "2026-11-06");
        let t = s.get(id).unwrap();
        assert!(!t.done, "a repeating task must stay open");
        assert_eq!(t.day, "2026-11-09");
    }

    #[test]
    fn a_broken_file_is_not_overwritten() {
        let dir = std::env::temp_dir().join(format!("dcx-island-tasks-broken-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tasks.json");
        std::fs::write(&path, "{ this is not json").unwrap();
        let mut s = Store::load(path.clone());
        add(&mut s, "x", "2026-11-07", None, Repeat::Once);
        s.save_if_dirty();
        assert!(s.error.is_some());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{ this is not json"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn ordering_puts_the_current_task_first() {
        let mut s = store("order");
        let a = add(&mut s, "B", "2026-11-07", Some(10 * 3600), Repeat::Once);
        let b = add(&mut s, "A", "2026-11-07", Some(9 * 3600), Repeat::Once);
        s.set_current(a);
        let list = s.for_day("2026-11-07");
        assert_eq!(list[0].id, a);
        assert_eq!(list[1].id, b);
    }

    #[test]
    fn interval_fires_once_per_slot() {
        let mut s = store("interval");
        let id = add_every(
            &mut s,
            "喝水",
            "2026-11-07",
            9 * 3600,
            18 * 3600,
            30,
            Repeat::Workdays,
        );
        // The task this whole feature exists for: half-hourly on weekdays.
        assert!(s.take_due("2026-11-07", 8 * 3600 + 59 * 60).is_empty());
        assert_eq!(s.take_due("2026-11-07", 9 * 3600).len(), 1);
        // Same slot, many ticks: one reminder, not one per tick.
        assert!(s.take_due("2026-11-07", 9 * 3600 + 60).is_empty());
        assert!(s.take_due("2026-11-07", 9 * 3600 + 25 * 60).is_empty());
        assert_eq!(s.take_due("2026-11-07", 9 * 3600 + 30 * 60).len(), 1);
        let t = s.get(id).unwrap();
        assert_eq!(t.every, Some(30));
        assert_eq!(t.until, Some(18 * 3600));
    }

    #[test]
    fn interval_does_not_replay_slots_it_slept_through() {
        let mut s = store("interval-sleep");
        add_every(
            &mut s,
            "喝水",
            "2026-11-07",
            9 * 3600,
            18 * 3600,
            30,
            Repeat::Workdays,
        );
        // Woke up at 12:34 after 09:00 — one reminder for 12:30, not seven.
        let due = s.take_due("2026-11-07", 12 * 3600 + 34 * 60);
        assert_eq!(due.len(), 1, "a backlog must not become a notification burst");
        assert!(due[0].fired_on.as_deref().unwrap().ends_with("@12:30"));
        // And the next slot still works.
        assert_eq!(s.take_due("2026-11-07", 13 * 3600).len(), 1);
    }

    #[test]
    fn interval_slots_stop_at_until() {
        let mut s = store("interval-until");
        add_every(
            &mut s,
            "喝水",
            "2026-11-07",
            9 * 3600,
            18 * 3600,
            30,
            Repeat::Workdays,
        );
        // 18:00 is the last slot; 18:20 is late but still worth one popup.
        assert_eq!(s.take_due("2026-11-07", 18 * 3600 + 20 * 60).len(), 1);
        // 18:40 maps back to the 18:00 slot, already fired, and the window is
        // over anyway — nothing more today.
        assert!(s.take_due("2026-11-07", 18 * 3600 + 40 * 60).is_empty());
        assert!(s.take_due("2026-11-07", 23 * 3600).is_empty());
    }

    #[test]
    fn interval_window_defaults_to_a_working_day() {
        let mut s = store("interval-default");
        let id = s.add(NewTask {
            title: "喝水",
            day: "2026-11-07",
            at: Some(9 * 3600),
            until: None,
            every: Some(30),
            repeat: Repeat::Workdays,
        });
        let t = s.get(id).unwrap();
        assert_eq!(t.until, Some(17 * 3600), "09:00 + eight hours");
        assert_eq!(t.window_label().unwrap(), "09:00–17:00");
        assert_eq!(t.every_label().unwrap(), "每 30 分钟");
    }

    #[test]
    fn a_single_reminder_carries_no_window() {
        let mut s = store("no-window");
        // A range typed without an interval must not leave a dangling `until`
        // that the list would print as if it meant something.
        let id = s.add(NewTask {
            title: "站会",
            day: "2026-11-07",
            at: Some(9 * 3600),
            until: Some(18 * 3600),
            every: None,
            repeat: Repeat::Daily,
        });
        let t = s.get(id).unwrap();
        assert_eq!(t.until, None);
        assert!(!t.is_interval());
        assert_eq!(t.window_label().unwrap(), "09:00");
        assert_eq!(t.every_label(), None);
    }

    #[test]
    fn an_interval_repeats_to_the_next_workday() {
        let mut s = store("interval-advance");
        // Friday 2026-11-06.
        let id = add_every(
            &mut s,
            "喝水",
            "2026-11-06",
            9 * 3600,
            18 * 3600,
            30,
            Repeat::Workdays,
        );
        s.take_due("2026-11-06", 9 * 3600);
        s.toggle_done(id, "2026-11-06");
        let t = s.get(id).unwrap();
        assert_eq!(t.day, "2026-11-09", "Monday, not Saturday");
        assert_eq!(t.fired_on, None, "the new day starts with a clean slate");
        assert!(!t.done);
        assert_eq!(t.every, Some(30), "the interval survives the roll");
    }
}

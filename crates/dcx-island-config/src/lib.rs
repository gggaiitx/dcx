//! `%LOCALAPPDATA%\dcx\island.toml` — the one document shared by the island
//! process and the host's settings window.
//!
//! Both processes write it, so it is loaded and saved through
//! [`dcx_win32::config_file`] exactly like `organizer.toml` / `search.toml`:
//! a path-derived mutex around the read-modify-write cycle, an atomic swap for
//! the write, and a poisoned document that is read-only until it parses again.
//! There is **no** `fs::write` fallback — a half-written TOML read by the other
//! process would silently reset every switch to its default.
//!
//! The schema is deliberately flat and `#[serde(default)]` on every struct: a
//! hand-edited file that is missing a table must not fail to parse, because a
//! parse failure marks the document poisoned and freezes both writers.

pub mod credential;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Version of the document layout. Bumped only when a rename would lose data;
/// additions never need a bump because everything is `#[serde(default)]`.
pub const SCHEMA_VERSION: u32 = 1;

pub fn local_dir() -> PathBuf {
    let mut p = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    p.push("dcx");
    let _ = std::fs::create_dir_all(&p);
    p
}

pub fn config_path() -> PathBuf {
    local_dir().join("island.toml")
}

pub fn tasks_path() -> PathBuf {
    local_dir().join("island_tasks.json")
}

pub fn clipboard_path() -> PathBuf {
    local_dir().join("island_clipboard.json")
}

pub fn image_dir() -> PathBuf {
    let p = local_dir().join("island_clipboard");
    let _ = std::fs::create_dir_all(&p);
    p
}

pub fn log_path(name: &str) -> PathBuf {
    local_dir().join(name)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    /// Master switch, mirrored into `apps.toml [apps.island].enabled` by the
    /// settings centre. Read here so the island can stop itself when the user
    /// turned it off while it was running.
    pub enabled: bool,
    pub hotkey_enabled: bool,
    pub hotkey: String,
    pub clock: ClockCfg,
    pub metrics: MetricsCfg,
    pub behavior: BehaviorCfg,
    pub tasks: TaskCfg,
    pub clipboard: ClipboardCfg,
    pub inbox: InboxCfg,
    pub mail: MailCfg,
    pub dispatch: DispatchCfg,
    pub ui: UiCfg,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClockCfg {
    pub show: bool,
    /// `false` = 12-hour clock with AM/PM.
    pub hour24: bool,
    pub show_seconds: bool,
    pub show_weekday: bool,
    /// Show `2026-11-07` next to the weekday in the capsule.
    pub show_date: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MetricsCfg {
    pub show: bool,
    pub cpu: bool,
    pub memory: bool,
    pub disk: bool,
    pub network: bool,
    /// Sampling period of the background thread, 1–10 s.
    pub interval_secs: u64,
    /// "bar" | "ring".
    pub style: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BehaviorCfg {
    /// Collapse the panel automatically after this many idle seconds; 0 = off.
    pub auto_collapse_secs: u64,
    /// Hide the whole island while a full-screen window is foreground (video,
    /// games, slideshows). Without this the capsule sits on top of them.
    pub hide_fullscreen: bool,
    /// Capsule width in points; the panel is wider and anchored to its centre.
    pub capsule_width: f32,
    pub capsule_height: f32,
    /// 0.6–1.0; applied to the capsule background only, text stays opaque.
    pub opacity: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskCfg {
    /// Yesterday's unfinished tasks move to today on the first start of a day.
    pub carry_over: bool,
    /// Beep when a reminder fires.
    pub sound: bool,
    /// Optional Markdown archive directory; empty = no archive.
    pub archive_dir: String,
    /// Push due reminders to the configured outbound channels.
    pub dispatch_due: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipboardCfg {
    pub enabled: bool,
    pub capture_images: bool,
    /// Ring buffer size. Both the list and the image folder are trimmed.
    pub max_items: usize,
    /// Longer text is truncated (keeps the list fast and the file small).
    pub max_text_chars: usize,
    /// Total size budget for the image folder, in MB.
    pub max_image_mb: u64,
    /// Case-insensitive regexes; a match means "do not record".
    pub exclude_patterns: Vec<String>,
    /// Refuse to record the clipboard while a password field is focused.
    pub skip_password_fields: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InboxCfg {
    pub wechat: bool,
    pub wecom: bool,
    pub qq: bool,
    pub mail: bool,
    /// How many items each channel keeps in memory.
    pub keep: usize,
    /// Incoming items also raise the capsule badge.
    pub badge: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MailCfg {
    /// IMAP host, e.g. `imap.qq.com`.
    pub imap_host: String,
    pub imap_port: u16,
    /// SMTP host for outbound reminder mail, e.g. `smtp.qq.com`.
    pub smtp_host: String,
    pub smtp_port: u16,
    pub user: String,
    /// Recipient; empty = send to `user`.
    pub to: String,
    pub poll_minutes: u64,
    /// First sync only records the unread count, no per-message reminders.
    pub skip_backlog: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DispatchCfg {
    pub wecom: bool,
    pub mail: bool,
    /// Minimum spacing between two outbound messages, in seconds. The WeCom
    /// bot is limited to 20 messages/minute; 5 s keeps us far below it.
    pub min_gap_secs: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiCfg {
    pub panel_width: f32,
    pub panel_height: f32,
    /// Last expanded tab: "tasks" | "clip" | "inbox" | "perf".
    pub last_tab: String,
    /// Pin the island to a monitor: empty = primary, otherwise a device name
    /// such as `\\.\DISPLAY2`.
    pub monitor: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: SCHEMA_VERSION,
            // Off by default: a permanent topmost window changes the desktop for
            // someone who never asked for it, and the settings centre is where
            // the feature is discovered. Same policy as the file search app.
            enabled: false,
            hotkey_enabled: true,
            hotkey: "Alt+i".to_string(),
            clock: ClockCfg::default(),
            metrics: MetricsCfg::default(),
            behavior: BehaviorCfg::default(),
            tasks: TaskCfg::default(),
            clipboard: ClipboardCfg::default(),
            inbox: InboxCfg::default(),
            mail: MailCfg::default(),
            dispatch: DispatchCfg::default(),
            ui: UiCfg::default(),
        }
    }
}

impl Default for ClockCfg {
    fn default() -> Self {
        ClockCfg {
            show: true,
            hour24: true,
            show_seconds: false,
            show_weekday: true,
            show_date: false,
        }
    }
}

impl Default for MetricsCfg {
    fn default() -> Self {
        MetricsCfg {
            show: true,
            cpu: true,
            memory: true,
            disk: false,
            network: true,
            interval_secs: 1,
            style: "bar".to_string(),
        }
    }
}

impl Default for BehaviorCfg {
    fn default() -> Self {
        BehaviorCfg {
            auto_collapse_secs: 0,
            hide_fullscreen: true,
            capsule_width: 300.0,
            capsule_height: 34.0,
            opacity: 0.94,
        }
    }
}

impl Default for TaskCfg {
    fn default() -> Self {
        TaskCfg {
            carry_over: true,
            sound: true,
            archive_dir: String::new(),
            dispatch_due: false,
        }
    }
}

impl Default for ClipboardCfg {
    fn default() -> Self {
        ClipboardCfg {
            enabled: true,
            capture_images: true,
            max_items: 200,
            max_text_chars: 20_000,
            max_image_mb: 200,
            // The default is a hard "do not record" net, not a guarantee: a
            // password manager that copies a bare token looks like any other
            // text. The UI says so instead of promising safety.
            exclude_patterns: vec![
                r"(?i)-----BEGIN [A-Z ]*PRIVATE KEY-----".to_string(),
                r"(?i)\b(password|passwd|pwd)\s*[:=]".to_string(),
            ],
            skip_password_fields: true,
        }
    }
}

impl Default for InboxCfg {
    fn default() -> Self {
        InboxCfg {
            wechat: true,
            wecom: true,
            qq: false,
            mail: false,
            keep: 20,
            badge: true,
        }
    }
}

impl Default for MailCfg {
    fn default() -> Self {
        MailCfg {
            imap_host: "imap.qq.com".to_string(),
            imap_port: 993,
            smtp_host: "smtp.qq.com".to_string(),
            smtp_port: 465,
            user: String::new(),
            to: String::new(),
            poll_minutes: 5,
            skip_backlog: true,
        }
    }
}

impl Default for DispatchCfg {
    fn default() -> Self {
        DispatchCfg {
            wecom: false,
            mail: false,
            min_gap_secs: 5,
        }
    }
}

impl Default for UiCfg {
    fn default() -> Self {
        UiCfg {
            panel_width: 388.0,
            panel_height: 436.0,
            last_tab: "tasks".to_string(),
            monitor: String::new(),
        }
    }
}

impl Config {
    /// Clamp values a hand-edited file can get wrong. Called after every load
    /// and before every save, so the renderer never has to defend itself.
    pub fn repair(&mut self) {
        if self.version == 0 {
            self.version = SCHEMA_VERSION;
        }
        self.metrics.interval_secs = self.metrics.interval_secs.clamp(1, 10);
        if !matches!(self.metrics.style.as_str(), "bar" | "ring") {
            self.metrics.style = "bar".to_string();
        }
        self.behavior.capsule_width = self.behavior.capsule_width.clamp(150.0, 420.0);
        self.behavior.capsule_height = self.behavior.capsule_height.clamp(26.0, 64.0);
        self.behavior.opacity = self.behavior.opacity.clamp(0.6, 1.0);
        self.clipboard.max_items = self.clipboard.max_items.clamp(20, 2000);
        self.clipboard.max_text_chars = self.clipboard.max_text_chars.clamp(200, 200_000);
        self.clipboard.max_image_mb = self.clipboard.max_image_mb.clamp(0, 4096);
        self.inbox.keep = self.inbox.keep.clamp(5, 200);
        self.ui.panel_width = self.ui.panel_width.clamp(320.0, 720.0);
        self.ui.panel_height = self.ui.panel_height.clamp(260.0, 900.0);
        self.mail.poll_minutes = self.mail.poll_minutes.clamp(1, 240);
        self.mail.imap_port = if self.mail.imap_port == 0 { 993 } else { self.mail.imap_port };
        self.mail.smtp_port = if self.mail.smtp_port == 0 { 465 } else { self.mail.smtp_port };
        self.dispatch.min_gap_secs = self.dispatch.min_gap_secs.clamp(0, 60);
        if !matches!(
            self.ui.last_tab.as_str(),
            "tasks" | "clip" | "inbox" | "perf"
        ) {
            self.ui.last_tab = "tasks".to_string();
        }
    }
}

pub fn load() -> Config {
    load_from(&config_path())
}

pub fn load_from(path: &Path) -> Config {
    match std::fs::read_to_string(path) {
        Ok(text) => match toml::from_str::<Config>(&text) {
            Ok(mut c) => {
                dcx_win32::config_file::note_parse_ok(path);
                c.repair();
                c
            }
            Err(e) => {
                // Do not persist defaults over a document somebody hand-edited:
                // `poisoned` makes every later save fail with an actionable
                // message instead.
                eprintln!("[dcx-island] parse {path:?} failed ({e}); using defaults (read-only)");
                dcx_win32::config_file::note_parse_error(path);
                let mut c = Config::default();
                c.repair();
                c
            }
        },
        Err(_) => {
            let mut c = Config::default();
            c.repair();
            c
        }
    }
}

pub fn save(cfg: &Config) -> Result<(), String> {
    let mut cfg = cfg.clone();
    cfg.repair();
    save_to(&cfg, &config_path())
}

/// Read-modify-write **only** the `ui` table, under the config lock.
///
/// Two processes own this document: the island owns `ui` (last tab, panel
/// size), the settings centre owns everything else. Writing the island's whole
/// in-memory copy back — what the island used to do when the panel closed —
/// rolled back any switch the user had just changed in the settings centre,
/// because the island only re-reads the file every `CONFIG_POLL`. Re-reading
/// inside the lock keeps the two writers from stepping on each other.
pub fn patch_ui(f: impl FnOnce(&mut UiCfg)) -> Result<(), String> {
    patch_ui_at(&config_path(), f)
}

pub fn patch_ui_at(path: &Path, f: impl FnOnce(&mut UiCfg)) -> Result<(), String> {
    let _guard = dcx_win32::config_file::FileLock::acquire(
        path,
        dcx_win32::config_file::LOCK_TIMEOUT,
    )
    .map_err(|e| format!("配置文件被占用：{e}"))?;
    if dcx_win32::config_file::is_poisoned(path) {
        return Err(dcx_win32::config_file::poisoned_error(path).to_string());
    }
    let mut cfg = load_from(path);
    f(&mut cfg.ui);
    cfg.repair();
    let text = toml::to_string_pretty(&cfg).map_err(|e| format!("序列化失败：{e}"))?;
    dcx_win32::config_file::write_atomic(path, text.as_bytes())
        .map_err(|e| format!("写入失败：{e}"))
}

pub fn save_to(cfg: &Config, path: &Path) -> Result<(), String> {
    let _guard = dcx_win32::config_file::FileLock::acquire(
        path,
        dcx_win32::config_file::LOCK_TIMEOUT,
    )
    .map_err(|e| format!("配置文件被占用：{e}"))?;
    if dcx_win32::config_file::is_poisoned(path) {
        return Err(dcx_win32::config_file::poisoned_error(path).to_string());
    }
    let text = toml::to_string_pretty(cfg).map_err(|e| format!("序列化失败：{e}"))?;
    dcx_win32::config_file::write_atomic(path, text.as_bytes())
        .map_err(|e| format!("写入失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_survive_a_round_trip() {
        let dir = std::env::temp_dir().join(format!("dcx-island-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("island.toml");
        let a = Config::default();
        save_to(&a, &path).unwrap();
        let b = load_from(&path);
        assert_eq!(a, b);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn an_empty_document_means_defaults() {
        let dir = std::env::temp_dir().join(format!("dcx-island-cfg2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("island.toml");
        std::fs::write(&path, "").unwrap();
        let c = load_from(&path);
        assert_eq!(c.metrics.interval_secs, 1);
        assert!(c.clock.hour24);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn repair_clamps_hand_edited_values() {
        let mut c = Config::default();
        c.metrics.interval_secs = 0;
        c.behavior.opacity = 3.0;
        c.ui.last_tab = "nope".to_string();
        c.clipboard.max_items = 1;
        c.repair();
        assert_eq!(c.metrics.interval_secs, 1);
        assert_eq!(c.behavior.opacity, 1.0);
        assert_eq!(c.ui.last_tab, "tasks");
        assert_eq!(c.clipboard.max_items, 20);
    }

    #[test]
    fn patch_ui_keeps_the_tables_the_island_does_not_own() {
        let dir = std::env::temp_dir().join(format!("dcx-island-cfg4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("island.toml");
        let mut a = Config::default();
        a.behavior.capsule_width = 360.0;
        a.tasks.carry_over = false;
        save_to(&a, &path).unwrap();

        // Simulates the settings centre saving, then the island patching `ui`.
        patch_ui_at(&path, |ui| {
            ui.last_tab = "clip".to_string();
            ui.panel_height = 500.0;
        })
        .unwrap();

        let b = load_from(&path);
        assert_eq!(b.ui.last_tab, "clip");
        assert_eq!(b.ui.panel_height, 500.0);
        // Untouched tables must survive: this is what the full save rolled back.
        assert_eq!(b.behavior.capsule_width, 360.0);
        assert!(!b.tasks.carry_over);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn a_garbage_document_does_not_panic() {
        let dir = std::env::temp_dir().join(format!("dcx-island-cfg3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("island.toml");
        std::fs::write(&path, "this is not toml = = =").unwrap();
        let c = load_from(&path);
        assert!(!c.enabled);
        // Poisoned: a save must refuse instead of overwriting the user's file.
        assert!(save_to(&c, &path).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "this is not toml = = ="
        );
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }
}

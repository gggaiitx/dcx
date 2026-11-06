//! `%LOCALAPPDATA%\dcx\search.toml` — shared by the app and the host's hotkey
//! module (host only reads `activation`).
//!
//! Both processes write this file, so every write is read-modify-write with an
//! atomic `*.tmp` + `rename`, exactly like `apps.toml` / `organizer.toml`.
//! A half-written TOML must never be observed as a parse failure that resets
//! somebody else's edit.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub activation: Activation,
    pub scope: Scope,
    pub content: ContentCfg,
    pub ui: UiCfg,
    pub index: IndexCfg,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IndexCfg {
    /// Load the on-disk snapshot at startup; only rebuild it when it is older
    /// than this. Live changes are still picked up by the watcher in real time.
    pub refresh_minutes: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Activation {
    /// `false` = do not start with the platform, no resident index.
    /// The hotkey still works because the host owns it.
    pub enabled: bool,
    pub hotkey_enabled: bool,
    pub hotkey: String,
    /// "release" | "keep_5m" | "resident"
    pub residency: String,
    /// "mouse" | "center" | "last".
    pub show_on: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scope {
    pub roots: Vec<String>,
    pub include_hidden: bool,
    pub follow_links: bool,
    pub exclude: Vec<String>,
    pub custom_roots: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContentCfg {
    pub max_file_size_mb: u32,
    /// "auto" | "utf8" | "gb18030".
    pub encoding: String,
    pub max_results: usize,
    pub time_budget_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiCfg {
    /// "name" | "content".
    pub last_mode: String,
    pub last_scope: String,
    pub last_category: String,
    pub window_w: f32,
    pub window_h: f32,
    /// Physical-pixel top-left, written only while `activation.show_on` is
    /// `last`. `None` (or a position no longer on any monitor) falls back to
    /// the cursor's monitor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_x: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_y: Option<i32>,
    /// Result-table column widths in points. The time column takes whatever is
    /// left, so only the first three are stored.
    #[serde(default = "default_col_name")]
    pub col_name_w: f32,
    #[serde(default = "default_col_path")]
    pub col_path_w: f32,
    #[serde(default = "default_col_size")]
    pub col_size_w: f32,
    /// "" (score order) | "name" | "path" | "size" | "mtime"
    #[serde(default)]
    pub sort_by: String,
    #[serde(default)]
    pub sort_desc: bool,
}

fn default_col_name() -> f32 {
    280.0
}

fn default_col_path() -> f32 {
    320.0
}

fn default_col_size() -> f32 {
    90.0
}

impl Default for Config {
    fn default() -> Self {
        Config {
            activation: Activation::default(),
            scope: Scope::default(),
            content: ContentCfg::default(),
            ui: UiCfg::default(),
            index: IndexCfg::default(),
        }
    }
}

impl Default for IndexCfg {
    fn default() -> Self {
        IndexCfg {
            refresh_minutes: 360,
        }
    }
}

impl Default for Activation {
    fn default() -> Self {
        Activation {
            enabled: false,
            hotkey_enabled: true,
            hotkey: "Alt+s".into(),
            residency: "release".into(),
            show_on: "mouse".into(),
        }
    }
}

impl Default for Scope {
    fn default() -> Self {
        Scope {
            roots: vec!["desktop".into(), "computer".into()],
            include_hidden: false,
            follow_links: false,
            exclude: vec![
                "$Recycle.Bin".into(),
                "System Volume Information".into(),
                "WinSxS".into(),
                "node_modules".into(),
                "AppData\\Local\\Temp".into(),
                "pagefile.sys".into(),
                "hiberfil.sys".into(),
                "swapfile.sys".into(),
            ],
            custom_roots: Vec::new(),
        }
    }
}

impl Default for ContentCfg {
    fn default() -> Self {
        ContentCfg {
            max_file_size_mb: 2,
            encoding: "auto".into(),
            max_results: 2000,
            time_budget_ms: 8000,
        }
    }
}

impl Default for UiCfg {
    fn default() -> Self {
        UiCfg {
            last_mode: "name".into(),
            last_scope: "computer".into(),
            last_category: "all".into(),
            window_w: 880.0,
            window_h: 620.0,
            window_x: None,
            window_y: None,
            col_name_w: default_col_name(),
            col_path_w: default_col_path(),
            col_size_w: default_col_size(),
            sort_by: String::new(),
            sort_desc: false,
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        crate::win::local_dir().join("search.toml")
    }

    /// Read the document. A file that exists but does not parse is reported
    /// through the poisoned-path registry so [`Config::save`] refuses to
    /// replace it with defaults — the settings window and the search app both
    /// write this file, and silently resetting it lost the other side's edits.
    pub fn load() -> Config {
        let p = Self::path();
        match std::fs::read_to_string(&p) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(mut c) => {
                    dcx_win32::config_file::note_parse_ok(&p);
                    c.repair();
                    c
                }
                Err(e) => {
                    eprintln!("[dcx-search] parse {p:?} failed ({e}); using defaults (read-only)");
                    dcx_win32::config_file::note_parse_error(&p);
                    Config::default()
                }
            },
            Err(_) => Config::default(),
        }
    }

    /// Clamp the values that drive allocation.
    ///
    /// They are hand-editable and were passed straight through: a typo in
    /// `max_results` or `max_file_size_mb` turned one query into an out-of-memory
    /// kill, and `time_budget_ms = 0` disabled the only bound on a content scan.
    pub fn repair(&mut self) {
        self.content.max_file_size_mb = self.content.max_file_size_mb.clamp(1, 512);
        self.content.max_results = self.content.max_results.clamp(10, 50_000);
        self.content.time_budget_ms = self.content.time_budget_ms.clamp(200, 120_000);
        self.index.refresh_minutes = self.index.refresh_minutes.clamp(1, 24 * 60);
        if !matches!(
            self.activation.residency.as_str(),
            "release" | "keep_5m" | "resident"
        ) {
            self.activation.residency = "release".to_string();
        }
        if !matches!(self.activation.show_on.as_str(), "mouse" | "center" | "last") {
            self.activation.show_on = "mouse".to_string();
        }
        if self.activation.hotkey.trim().is_empty() {
            self.activation.hotkey = "Alt+s".to_string();
        }
        // Relative or non-existent custom roots are never walked: `WalkBuilder`
        // and `notify` would either fail silently or watch an unintended path.
        self.scope.custom_roots.retain(|r| {
            let p = std::path::Path::new(r);
            p.is_absolute() && p.is_dir()
        });
    }

    /// Atomic, cross-process-safe replace. Callers must `load()` immediately
    /// beforehand so they do not roll back a change the other process just made.
    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        let _guard = dcx_win32::config_file::FileLock::acquire(
            &path,
            dcx_win32::config_file::LOCK_TIMEOUT,
        )
        .map_err(|e| format!("配置文件被占用：{e}"))?;
        if dcx_win32::config_file::is_poisoned(&path) {
            return Err(dcx_win32::config_file::poisoned_error(&path).to_string());
        }
        let text = toml::to_string_pretty(self).map_err(|e| format!("序列化失败：{e}"))?;
        dcx_win32::config_file::write_atomic(&path, text.as_bytes())
            .map_err(|e| format!("保存失败：{e}"))
    }

    /// Cheap `(len, mtime_ms)` token for hot-reload without parsing every frame.
    pub fn stamp() -> (u64, u128) {
        match std::fs::metadata(Self::path()) {
            Ok(m) => {
                let t = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis())
                    .unwrap_or(0);
                (m.len(), t)
            }
            Err(_) => (0, 0),
        }
    }

    pub fn max_file_size(&self) -> u64 {
        (self.content.max_file_size_mb as u64).saturating_mul(1024 * 1024)
    }
}

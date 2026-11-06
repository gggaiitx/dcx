//! DCX 桌面收纳盒 — data layer.
//!
//! # Model
//!
//! Explorer's desktop stays native: the box neither hides its icons nor paints
//! a desktop of its own. What it owns is a physical **collection library**
//! ([`Config::library_dir`], default `Documents\dcx收纳`):
//!
//! * the「桌面」tab lists items still on the real desktop;
//! * collecting an item **moves it into the library**, so Explorer stops
//!   showing it on the desktop; the file itself is not deleted;
//! * releasing an item **moves it back to the desktop root**;
//! * categories inside the library are virtual metadata: extension rules
//!   (`categories[].exts`) file everything unmatched and a manual choice
//!   (`overrides`: normalised path → category key) wins.
//!
//! `excluded` and `hidden` are legacy manifest keys from older builds. They are
//! still parsed and pruned so hand-edited documents keep working, but the
//! collection model clears them as files move. [`Organizer::snapshot`] also
//! unhides files hidden by the old per-file migration once.
//!
//! `%LOCALAPPDATA%\dcx\organizer.toml` is the IPC boundary with the settings
//! centre in `dcx-host.exe`. Every field added since v1 is `#[serde(default)]`
//! so an existing file keeps working and the settings window never drops a
//! value it does not know about.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Key of the catch-all category. Everything unmatched lands here.
pub const CATCH_ALL: &str = "other";
/// Built-in category for desktop folders. Folders are classified by type,
/// not by extension, so this key has no `exts`.
pub const FOLDER: &str = "folder";

/// Consecutive unreachable refreshes before a manifest key is dropped.
pub const PRUNE_MISSES: u8 = 3;

// ---------------------------------------------------------------------------
// View / placement preferences
// ---------------------------------------------------------------------------

/// How the taskbar tile is anchored.
///
/// `Tray` is the default: the tile sits just left of the notification area,
/// which is a *fixed* anchor. `Follow` reproduces the old behaviour — it
/// tracks the right edge of the last app button, so opening or closing a
/// program moves the tile. That is exactly the "图标漂移" the rewrite fixes.
/// `Fixed` pins an absolute physical-pixel x coordinate until the user
/// unlocks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TileMode {
    /// Fixed against the tray's left edge (recommended).
    #[default]
    Tray,
    /// Follow the last app button (legacy, drifts).
    Follow,
    /// Absolute, user-locked position.
    Fixed,
}

/// Column the file list is sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
    #[default]
    Name,
    Size,
    Modified,
    Type,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Name => "名称",
            SortKey::Size => "大小",
            SortKey::Modified => "修改时间",
            SortKey::Type => "类型",
        }
    }
}

/// File list presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewMode {
    #[default]
    List,
    Grid,
}

/// One category: a tab in the panel plus the extensions that feed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    /// Stable id — neither the manifest nor anything on disk depends on the
    /// label, so renaming a tab is free.
    pub key: String,
    /// What the tab and the settings row show.
    pub label: String,
    /// Lowercase extensions without the dot. Empty for the catch-all.
    #[serde(default)]
    pub exts: Vec<String>,
}

impl Category {
    fn new(key: &str, label: &str, exts: &[&str]) -> Self {
        Category {
            key: key.to_string(),
            label: label.to_string(),
            exts: exts.iter().map(|e| e.to_ascii_lowercase()).collect(),
        }
    }
}

/// Parsed `organizer.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Master switch: the taskbar tile and the panel exist only while on.
    pub enabled: bool,
    /// Items without a manual choice are filed by the extension rules.
    pub auto_classify: bool,
    /// Category rules, in tab order. The catch-all is always kept last.
    pub categories: Vec<Category>,
    /// Manual category assignments: normalised path → category key.
    #[serde(default)]
    pub overrides: BTreeMap<String, String>,
    /// How the panel opens: on hover, or only after a click.
    #[serde(default)]
    pub open_mode: OpenMode,
    /// Desktop items this app hid, so they can be put back exactly as they were.
    #[serde(default)]
    pub hidden: BTreeSet<String>,
    /// Legacy: items the old view pulled out of the box. Kept so hand-edited
    /// documents and old installs still parse; the library model ignores it.
    #[serde(default)]
    pub excluded: BTreeSet<String>,
    /// Root of the physical collection. Missing/empty = Documents\dcx收纳.
    #[serde(default)]
    pub library_root: Option<String>,
    /// Global hotkey that opens/toggles the panel. Owned by `dcx-host.exe`.
    #[serde(default = "default_box_hotkey")]
    pub hotkey: String,
    #[serde(default = "yes")]
    pub hotkey_enabled: bool,

    // -- v2: placement -----------------------------------------------------
    /// Where the taskbar tile is anchored.
    #[serde(default)]
    pub tile_mode: TileMode,
    /// Horizontal fine tune, in points. Applied after the anchor is resolved.
    #[serde(default)]
    pub tile_offset: i32,
    /// Absolute physical-pixel x used by [`TileMode::Fixed`].
    #[serde(default)]
    pub tile_fixed_x: Option<i32>,

    // -- v2: view preferences ----------------------------------------------
    #[serde(default)]
    pub sort_by: SortKey,
    #[serde(default)]
    pub sort_desc: bool,
    #[serde(default)]
    pub view_mode: ViewMode,
    /// Panel size in points; both are clamped by the UI to the work area.
    #[serde(default = "default_panel_width")]
    pub panel_width: u32,
    #[serde(default = "default_panel_height")]
    pub panel_height: u32,
    /// Show files that were hidden by someone else (system/other tools).
    #[serde(default)]
    pub show_hidden: bool,
    /// Glob-ish basename/extension patterns never shown in the box.
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Open on a single click instead of a double click.
    #[serde(default)]
    pub single_click_open: bool,
    /// Restore the last active tab on startup.
    #[serde(default = "yes")]
    pub remember_tab: bool,
    #[serde(default)]
    pub last_tab: Option<String>,
    /// Ask before sending files to the Recycle Bin.
    #[serde(default = "yes")]
    pub confirm_delete: bool,

    // -- v3: list display toggles ------------------------------------------
    /// Show the modification time in each row.
    #[serde(default = "yes")]
    pub show_time: bool,
    /// Show the size (or 「文件夹」 for directories) in each row.
    #[serde(default = "yes")]
    pub show_size: bool,
    /// List directories in the panel at all.
    #[serde(default = "yes")]
    pub show_folders: bool,
    /// Drop the extension from displayed file names, like Explorer's
    /// 「隐藏已知文件类型的扩展名」.
    #[serde(default)]
    pub hide_ext: bool,
    /// Collapse the panel after something is opened from it.
    #[serde(default = "yes")]
    pub close_after_open: bool,
    /// Collapse the panel when the pointer clicks outside it.
    #[serde(default = "yes")]
    pub close_on_outside: bool,
    /// Paint the panel over the Windows 11 acrylic system backdrop.
    #[serde(default = "yes")]
    pub acrylic: bool,
    /// Panel tint opacity over the acrylic backdrop, 0-255 (higher = more solid).
    #[serde(default = "default_acrylic_alpha")]
    pub acrylic_alpha: u16,
}

fn yes() -> bool {
    true
}
fn default_box_hotkey() -> String {
    "Alt+d".to_string()
}
fn default_acrylic_alpha() -> u16 {
    170
}
fn default_panel_width() -> u32 {
    420
}
fn default_panel_height() -> u32 {
    520
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OpenMode {
    /// Open after a short rest on the tile (the default).
    #[default]
    Hover,
    /// Only open on a click.
    Click,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            enabled: true,
            auto_classify: true,
            categories: default_categories(),
            overrides: BTreeMap::new(),
            open_mode: OpenMode::Hover,
            hidden: BTreeSet::new(),
            excluded: BTreeSet::new(),
            library_root: None,
            hotkey: default_box_hotkey(),
            hotkey_enabled: true,
            tile_mode: TileMode::Tray,
            tile_offset: 0,
            tile_fixed_x: None,
            sort_by: SortKey::Name,
            sort_desc: false,
            view_mode: ViewMode::List,
            panel_width: default_panel_width(),
            panel_height: default_panel_height(),
            show_hidden: false,
            ignore: default_ignore(),
            single_click_open: false,
            remember_tab: true,
            last_tab: None,
            confirm_delete: true,
            show_time: true,
            show_size: true,
            show_folders: true,
            hide_ext: false,
            close_after_open: true,
            close_on_outside: true,
            acrylic: true,
            acrylic_alpha: 170,
        }
    }
}

pub fn default_ignore() -> Vec<String> {
    vec![
        "desktop.ini".into(),
        "thumbs.db".into(),
        "~$*".into(),
        "*.tmp".into(),
        "*.crdownload".into(),
        "*.part".into(),
    ]
}

/// Fresh install: five real categories plus the catch-all.
pub fn default_categories() -> Vec<Category> {
    vec![
        Category::new(FOLDER, "文件夹", &[]),
        Category::new(
            "doc",
            "文档",
            &[
                "doc", "docx", "xls", "xlsx", "ppt", "pptx", "pdf", "txt", "md", "rtf", "csv",
                "wps", "et", "dps", "odt", "ods", "odp", "epub", "caj", "tex", "json", "xml",
                "yaml", "yml", "log", "ini",
            ],
        ),
        Category::new(
            "image",
            "图片",
            &[
                "png", "jpg", "jpeg", "gif", "bmp", "webp", "svg", "ico", "tif", "tiff", "heic",
                "raw", "psd", "cr2", "nef", "ai", "eps", "avif",
            ],
        ),
        Category::new(
            "archive",
            "压缩",
            &[
                "zip", "rar", "7z", "tar", "gz", "bz2", "xz", "zst", "iso", "cab", "001",
            ],
        ),
        Category::new(
            "media",
            "音视频",
            &[
                "mp3", "wav", "flac", "aac", "m4a", "ogg", "wma", "mp4", "mkv", "avi", "mov",
                "wmv", "flv", "webm", "m4v", "ts", "m4p", "mid", "aiff",
            ],
        ),
        Category::new(
            "app",
            "程序",
            &[
                "exe", "msi", "msix", "appx", "bat", "cmd", "ps1", "lnk", "url", "reg", "apk",
                "deb", "jar",
            ],
        ),
        Category::new(CATCH_ALL, "其它", &[]),
    ]
}

impl Config {
    /// The physical collection root. Files here are "in the box"; they are no
    /// longer on the desktop, so Explorer does not show them there.
    pub fn library_dir(&self) -> PathBuf {
        if let Some(root) = self
            .library_root
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return PathBuf::from(root);
        }
        let docs = dcx_win32::reg_read_string(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Shell Folders",
            "Personal",
        )
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("Documents"))
        })
        .unwrap_or_else(|| PathBuf::from("."));
        docs.join("dcx收纳")
    }

    /// `%LOCALAPPDATA%\dcx\organizer.toml`.
    pub fn path() -> PathBuf {
        let mut p = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        p.push("dcx");
        p.push("organizer.toml");
        p
    }

    /// Load, repairing anything a hand edit could have broken.
    ///
    /// An unparseable document falls back to defaults **in memory only**: the
    /// path is marked read-only so the next save cannot replace the user's
    /// categories, overrides and custom library path with those defaults.
    pub fn load() -> Config {
        let p = Self::path();
        let mut cfg = match std::fs::read_to_string(&p) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(c) => {
                    dcx_win32::config_file::note_parse_ok(&p);
                    c
                }
                Err(e) => {
                    eprintln!("[dcx-organizer] parse {p:?} failed ({e}); using defaults (read-only)");
                    dcx_win32::config_file::note_parse_error(&p);
                    Config::default()
                }
            },
            Err(_) => Config::default(),
        };
        cfg.repair();
        cfg
    }

    /// Normalise a document after a hand edit or a programmatic change:
    /// unique keys, lowercase extensions, catch-all last, valid ranges.
    pub fn repair(&mut self) {
        if self.categories.is_empty() {
            self.categories = default_categories();
        }
        for c in &mut self.categories {
            if c.label.trim().is_empty() {
                c.label = c.key.clone();
            }
            if c.key.trim().is_empty() {
                c.key = "custom".to_string();
            }
            c.key
                .retain(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-');
            if c.key.is_empty() {
                c.key = "custom".to_string();
            }
            c.exts = c
                .exts
                .iter()
                .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
                .filter(|e| !e.is_empty())
                .collect();
        }
        let mut seen = std::collections::HashSet::new();
        for c in &mut self.categories {
            let mut k = c.key.clone();
            let mut n = 2;
            while !seen.insert(k.clone()) {
                k = format!("{}_{n}", c.key);
                n += 1;
            }
            c.key = k;
        }
        // The folder category is built in. Older documents predate it; insert
        // it exactly once so existing installs get the tab without a migration
        // step.
        match self.categories.iter().position(|c| c.key == FOLDER) {
            Some(i) => self.categories[i].exts.clear(),
            None => {
                // First real category: the tab strip scrolls horizontally, and
                // a built-in tab appended after six custom ones was invisible
                // until the user found the arrow.
                self.categories
                    .insert(0, Category::new(FOLDER, "文件夹", &[]));
            }
        }
        match self.categories.iter().position(|c| c.key == CATCH_ALL) {
            Some(i) => {
                let mut c = self.categories.remove(i);
                c.exts.clear();
                self.categories.push(c);
            }
            None => self.categories.push(Category::new(CATCH_ALL, "其它", &[])),
        }
        self.overrides
            .retain(|_, key| self.categories.iter().any(|c| c.key == *key));
        if let Some(last) = self.last_tab.clone() {
            if !self.categories.iter().any(|c| c.key == last) {
                self.last_tab = None;
            }
        }

        self.tile_offset = self.tile_offset.clamp(-400, 400);
        self.acrylic_alpha = self.acrylic_alpha.clamp(120, 255);
        self.panel_width = self.panel_width.clamp(320, 900);
        self.panel_height = self.panel_height.clamp(300, 1000);
        self.ignore = self
            .ignore
            .iter()
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        self.library_root = self
            .library_root
            .take()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        self.hotkey = self.hotkey.trim().to_string();
        if self.hotkey.is_empty() {
            self.hotkey = default_box_hotkey();
        }
    }

    /// Persist the document under the cross-process write lock.
    ///
    /// `organizer.toml` is written by both `dcx-host.exe --settings` and
    /// `dcx-box.exe`; without the lock two read-modify-write cycles interleave
    /// and the later one restores a stale snapshot of the other's keys.
    pub fn save(&self) -> io::Result<()> {
        let _guard = dcx_win32::config_file::FileLock::acquire(
            &Self::path(),
            dcx_win32::config_file::LOCK_TIMEOUT,
        )?;
        self.save_unlocked()
    }

    /// Save while the caller already holds the document lock.
    fn save_unlocked(&self) -> io::Result<()> {
        let p = Self::path();
        if dcx_win32::config_file::is_poisoned(&p) {
            return Err(dcx_win32::config_file::poisoned_error(&p));
        }
        let text = toml::to_string_pretty(self).map_err(io::Error::other)?;
        // No truncating fallback: a failed swap leaves the old document intact.
        dcx_win32::config_file::write_atomic(&p, text.as_bytes())
    }

    /// Category that owns `ext`, or `None` when no rule matches.
    pub fn category_of_ext(&self, ext: &str) -> Option<&Category> {
        let ext = ext.trim_start_matches('.').to_ascii_lowercase();
        self.categories
            .iter()
            .find(|c| c.key != CATCH_ALL && c.exts.iter().any(|e| *e == ext))
    }

    pub fn category(&self, key: &str) -> Option<&Category> {
        self.categories.iter().find(|c| c.key == key)
    }

    /// Move `ext` to `key`'s rule list and strip it from every other category.
    pub fn assign_ext(&mut self, key: &str, ext: &str) {
        let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
        if ext.is_empty() {
            return;
        }
        for c in &mut self.categories {
            if c.key != key {
                c.exts.retain(|e| *e != ext);
            }
        }
        if let Some(c) = self.categories.iter_mut().find(|c| c.key == key) {
            if c.key != CATCH_ALL && !c.exts.contains(&ext) {
                c.exts.push(ext);
            }
        }
    }

    /// Persist a UI-only preference without disturbing anything else. The box
    /// process owns these keys; the settings centre writes the same document,
    /// so every write must be read-modify-write **and** serialised.
    pub fn patch<F: FnOnce(&mut Config)>(f: F) -> io::Result<()> {
        let _guard = dcx_win32::config_file::FileLock::acquire(
            &Self::path(),
            dcx_win32::config_file::LOCK_TIMEOUT,
        )?;
        let mut cfg = Config::load();
        f(&mut cfg);
        cfg.save_unlocked()
    }
}

/// The user's desktop, via the shell's own registry value so a redirected or
/// OneDrive-backed desktop works. This **is** the box root.
pub fn desktop_dir() -> PathBuf {
    dcx_win32::reg_read_string(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Shell Folders",
        "Desktop",
    )
    .filter(|s| !s.trim().is_empty())
    .map(PathBuf::from)
    .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("Desktop")))
    .unwrap_or_else(|| PathBuf::from("."))
}

/// What a panel row needs to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    /// Lowercase extension, empty for folders.
    pub ext: String,
    pub size: u64,
    /// Unix seconds; 0 when unavailable.
    pub modified: u64,
    /// Unix seconds; 0 when unavailable.
    pub created: u64,
    /// `FILE_ATTRIBUTE_HIDDEN` was set on this item.
    pub attrs_hidden: bool,
    /// `FILE_ATTRIBUTE_SYSTEM` was set on this item.
    pub attrs_system: bool,
    /// Category assigned right now (filled by [`Organizer::snapshot`]).
    pub category: String,
    /// True when the category came from a manual override.
    pub manual: bool,
}

impl Entry {
    fn from_path(path: PathBuf) -> Option<Entry> {
        let name = path.file_name()?.to_string_lossy().to_string();
        if name.starts_with('.') {
            return None;
        }
        let meta = std::fs::symlink_metadata(&path).ok()?;
        let is_dir = meta.is_dir();
        let ext = if is_dir {
            String::new()
        } else {
            path.extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default()
        };
        let unix = |t: std::io::Result<SystemTime>| {
            t.ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0)
        };
        let attrs = file_attributes(&path);
        Some(Entry {
            path,
            name,
            is_dir,
            ext,
            size: if is_dir { 0 } else { meta.len() },
            modified: unix(meta.modified()),
            created: unix(meta.created()),
            attrs_hidden: attrs & dcx_win32::FILE_ATTRIBUTE_HIDDEN != 0,
            attrs_system: attrs & dcx_win32::FILE_ATTRIBUTE_SYSTEM != 0,
            category: String::new(),
            manual: false,
        })
    }
}

fn file_attributes(path: &Path) -> u32 {
    let w = dcx_win32::wide_path(path);
    let a = unsafe { dcx_win32::GetFileAttributesW(w.as_ptr()) };
    if a == dcx_win32::INVALID_FILE_ATTRIBUTES {
        0
    } else {
        a
    }
}

/// Per-tab summary for the tab strip and the footer.
#[derive(Debug, Clone, Default)]
pub struct CategoryCount {
    pub key: String,
    pub label: String,
    pub count: usize,
    pub bytes: u64,
}

/// Everything one panel frame needs, from a **single** desktop scan.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Every item the view is allowed to show, already classified.
    pub items: Vec<Entry>,
    /// One row per configured category (catch-all last).
    pub counts: Vec<CategoryCount>,
}

/// Box handle: the physical collection root plus the real desktop.
#[derive(Debug, Clone)]
pub struct Organizer {
    pub cfg: Config,
    root: PathBuf,
    desktop: PathBuf,
    /// Consecutive refreshes in which a manifest key was unreachable. A single
    /// miss is not enough to drop it: a network drive, a OneDrive placeholder or
    /// a drag that has just landed can be briefly invisible to `exists()`.
    missing: BTreeMap<String, u8>,
}

impl Organizer {
    pub fn new(cfg: Config) -> Organizer {
        let desktop = desktop_dir();
        let mut cfg = cfg;
        let mut root = cfg.library_dir();
        // A collection inside the desktop would itself become a desktop icon
        // and be scanned recursively; fall back to the app data directory.
        if is_same_or_under(&root, &desktop) {
            eprintln!(
                "[dcx-organizer] library {:?} is inside the desktop; using LOCALAPPDATA",
                root
            );
            root = std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join("dcx")
                .join("library");
            // Report the root that is actually in use. Leaving `library_root`
            // at the rejected value made the settings page show one path while
            // the box scanned another.
            cfg.library_root = Some(root.to_string_lossy().to_string());
        }
        if let Err(e) = std::fs::create_dir_all(&root) {
            eprintln!("[dcx-organizer] cannot create library {:?}: {e}", root);
        }
        Organizer {
            cfg,
            root,
            desktop,
            missing: BTreeMap::new(),
        }
    }

    /// Explicit roots, for tests. Production uses [`Self::new`].
    pub fn at(cfg: Config, root: PathBuf) -> Organizer {
        let _ = std::fs::create_dir_all(&root);
        Organizer {
            cfg,
            desktop: root.clone(),
            root,
            missing: BTreeMap::new(),
        }
    }

    pub fn with_roots(cfg: Config, root: PathBuf, desktop: PathBuf) -> Organizer {
        let _ = std::fs::create_dir_all(&root);
        Organizer {
            cfg,
            root,
            desktop,
            missing: BTreeMap::new(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn desktop(&self) -> &Path {
        &self.desktop
    }

    fn scan_top(&self, dir: &Path) -> Vec<Entry> {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut out: Vec<Entry> = rd
            .filter_map(|e| e.ok())
            .filter_map(|e| Entry::from_path(e.path()))
            .collect();
        out.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        out
    }

    /// Every item in the collection (no view filtering).
    pub fn items(&self) -> Vec<Entry> {
        self.scan_top(&self.root)
    }

    /// Top-level items still on the real desktop, i.e. not collected yet.
    /// These are what the「桌面」tab shows.
    pub fn desktop_items(&self) -> Vec<Entry> {
        let mut out: Vec<Entry> = self
            .scan_top(&self.desktop)
            .into_iter()
            .filter(|e| self.ui_visible(e))
            .collect();
        for e in &mut out {
            e.category = self.classify(e);
            e.manual = self.is_manual(e);
        }
        out
    }

    /// Direct children of `dir`, already filtered by the view policy, sorted
    /// like the panel and capped at `limit`. The second value is the total
    /// before the cap, so the UI can say when it is showing a subset.
    pub fn folder_children(&self, dir: &Path, limit: usize) -> (Vec<Entry>, usize) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return (Vec::new(), 0);
        };
        // Statting a directory goes through `symlink_metadata` and
        // `GetFileAttributesW` per entry. Cap the scan so an expanded folder
        // with 100k files cannot turn the 1.2 s desktop rescan into a CPU
        // burner; the UI only paints the first `limit` anyway.
        const SCAN_CAP: usize = 4_000;
        let mut out = Vec::new();
        let mut capped = false;
        for e in rd.filter_map(|e| e.ok()) {
            if out.len() >= SCAN_CAP {
                capped = true;
                break;
            }
            if let Some(e) = Entry::from_path(e.path()) {
                if self.ui_visible(&e) {
                    out.push(e);
                }
            }
        }
        let total = if capped { out.len() + 1 } else { out.len() };
        sort_entries(&mut out, self.cfg.sort_by, self.cfg.sort_desc);
        out.truncate(limit.max(1));
        (out, total)
    }

    /// Should this item appear in the panel at all?
    ///
    /// Files hidden by *us* are the box's content and must stay visible here;
    /// files hidden by the system or another tool are normally not user files.
    pub fn ui_visible(&self, e: &Entry) -> bool {
        let key = path_key(&e.path);
        if self.matches_ignore(e) {
            return false;
        }
        if self.cfg.show_hidden {
            return true;
        }
        let ours = self.cfg.hidden.contains(&key);
        if ours {
            return true;
        }
        !e.attrs_hidden && !e.attrs_system
    }

    /// Glob-ish match against basename and extension. `*` and `?` are the only
    /// metacharacters; a leading dot means "match the extension".
    pub fn matches_ignore(&self, e: &Entry) -> bool {
        let name = e.name.to_ascii_lowercase();
        let with_ext = if e.ext.is_empty() {
            name.clone()
        } else {
            format!("{name}.{}", e.ext)
        };
        self.cfg.ignore.iter().any(|pat| {
            let p = pat.trim().to_ascii_lowercase();
            if p.is_empty() {
                return false;
            }
            if let Some(ext) = p.strip_prefix('.') {
                return !e.ext.is_empty() && e.ext == ext;
            }
            glob_match(&p, &name) || glob_match(&p, &with_ext)
        })
    }

    /// Which tab an item belongs to: manual choice, extension rule, catch-all.
    pub fn classify(&self, entry: &Entry) -> String {
        if let Some(key) = self.cfg.overrides.get(&path_key(&entry.path)) {
            if self.cfg.category(key).is_some() {
                return key.clone();
            }
        }
        if entry.is_dir {
            return if self.cfg.category(FOLDER).is_some() {
                FOLDER.to_string()
            } else {
                CATCH_ALL.to_string()
            };
        }
        if !self.cfg.auto_classify || entry.ext.is_empty() {
            return CATCH_ALL.to_string();
        }
        self.cfg
            .category_of_ext(&entry.ext)
            .map(|c| c.key.clone())
            .unwrap_or_else(|| CATCH_ALL.to_string())
    }

    pub fn is_manual(&self, entry: &Entry) -> bool {
        self.cfg.overrides.contains_key(&path_key(&entry.path))
    }

    /// Items currently filed under one category (legacy helper).
    pub fn entries(&self, key: &str) -> Vec<Entry> {
        self.items()
            .into_iter()
            .filter(|e| self.ui_visible(e) && self.classify(e) == key)
            .collect()
    }

    /// `(key, count, bytes)` per category — kept for the settings page.
    pub fn counts(&self) -> Vec<(String, usize, u64)> {
        let items: Vec<Entry> = self
            .items()
            .into_iter()
            .filter(|e| self.ui_visible(e))
            .collect();
        self.cfg
            .categories
            .iter()
            .map(|c| {
                let mine: Vec<&Entry> =
                    items.iter().filter(|e| self.classify(e) == c.key).collect();
                let bytes = mine.iter().filter(|e| !e.is_dir).map(|e| e.size).sum();
                (c.key.clone(), mine.len(), bytes)
            })
            .collect()
    }

    /// One scan of the collection: classification, rows and tab counts.
    /// Older builds hid individual files with `FILE_ATTRIBUTE_HIDDEN`; clear
    /// those once so Windows file dialogs see everything again.
    pub fn snapshot(&mut self) -> Snapshot {
        self.restore_hidden();
        let all = self.items();

        let mut items: Vec<Entry> = all.iter().filter(|e| self.ui_visible(e)).cloned().collect();
        for e in &mut items {
            e.category = self.classify(e);
            e.manual = self.is_manual(e);
        }

        let counts = self
            .cfg
            .categories
            .iter()
            .map(|c| {
                let mine: Vec<&Entry> = items.iter().filter(|e| e.category == c.key).collect();
                CategoryCount {
                    key: c.key.clone(),
                    label: c.label.clone(),
                    count: mine.len(),
                    bytes: mine.iter().filter(|e| !e.is_dir).map(|e| e.size).sum(),
                }
            })
            .collect();
        Snapshot { items, counts }
    }

    /// Legacy single-tab view, kept for callers outside the box UI.
    pub fn view(&mut self, key: &str) -> (Vec<Entry>, Vec<(String, usize, u64)>) {
        let snap = self.snapshot();
        let entries = snap
            .items
            .into_iter()
            .filter(|e| e.category == key)
            .collect();
        let counts = snap
            .counts
            .into_iter()
            .map(|c| (c.key, c.count, c.bytes))
            .collect();
        (entries, counts)
    }

    /// Tag an item with a manual category. Pure metadata.
    pub fn set_override(&mut self, path: &Path, key: &str) {
        self.cfg.excluded.remove(&path_key(path));
        if self.cfg.category(key).is_none() {
            return;
        }
        self.cfg.overrides.insert(path_key(path), key.to_string());
    }

    /// Batch version of [`Self::set_override`].
    pub fn set_overrides(&mut self, paths: &[PathBuf], key: &str) -> usize {
        if self.cfg.category(key).is_none() {
            return 0;
        }
        for p in paths {
            self.cfg.excluded.remove(&path_key(p));
            self.cfg.overrides.insert(path_key(p), key.to_string());
        }
        paths.len()
    }

    /// Forget a manual choice; the item falls back to the extension rules.
    pub fn clear_override(&mut self, path: &Path) -> bool {
        self.cfg.overrides.remove(&path_key(path)).is_some()
    }

    pub fn clear_overrides_of(&mut self, paths: &[PathBuf]) -> usize {
        let mut n = 0;
        for p in paths {
            if self.clear_override(p) {
                n += 1;
            }
        }
        n
    }

    /// Drop every manual choice (the 「重新分类」 action).
    pub fn clear_overrides(&mut self) -> usize {
        let n = self.cfg.overrides.len();
        self.cfg.overrides.clear();
        n
    }

    pub fn override_count(&self) -> usize {
        self.cfg.overrides.len()
    }

    /// Handle a drop *into* the panel. Returns `(tagged, moved)`.
    ///
    /// Every path crosses [`drop_refusal`] first. Without it a dragged desktop
    /// folder reached `move_into_dir` unchecked and `rename(desktop,
    /// root\Desktop)` moved the whole desktop into the library; `absorb_auto`
    /// had the ancestor check, this entry point did not.
    pub fn absorb(&mut self, paths: &[PathBuf], key: &str) -> (usize, usize) {
        let (mut tagged, mut moved) = (0, 0);
        for p in paths {
            let p = norm(p);
            if let Some(why) = drop_refusal(&p, &self.root, &self.desktop) {
                eprintln!("[dcx-organizer] 拒绝收纳 {p:?}: {why}");
                continue;
            }
            if !is_same_or_under(&p, &self.root) {
                match move_into_dir(&p, &self.root) {
                    Ok(new_path) => {
                        moved += 1;
                        self.set_override(&new_path, key);
                        tagged += 1;
                        if is_same_or_under(&p, &self.desktop) {
                            notify_desktop(&self.desktop, false);
                        }
                        continue;
                    }
                    Err(e) => {
                        eprintln!("[dcx-organizer] 收纳 {p:?} 失败: {e}");
                        continue;
                    }
                }
            }
            self.set_override(&p, key);
            tagged += 1;
        }
        (tagged, moved)
    }

    /// Drop items into the panel without naming a category: adopt anything
    /// from outside the desktop, then let the extension rules classify it.
    /// Manual choices and "pulled out of the box" marks are cleared, otherwise
    /// a dragged-back item could stay invisible or land in a stale category.
    /// Returns `(adopted, moved_onto_desktop)`.
    pub fn absorb_auto(&mut self, paths: &[PathBuf]) -> (usize, usize) {
        let (mut adopted, mut moved) = (0, 0);
        for p in paths {
            let p = norm(p);
            if let Some(why) = drop_refusal(&p, &self.root, &self.desktop) {
                eprintln!("[dcx-organizer] 拒绝收纳 {p:?}: {why}");
                continue;
            }
            let target = if is_same_or_under(&p, &self.root) {
                p
            } else {
                match move_into_dir(&p, &self.root) {
                    Ok(new_path) => {
                        moved += 1;
                        if is_same_or_under(&p, &self.desktop) {
                            notify_desktop(&self.desktop, false);
                        }
                        new_path
                    }
                    Err(e) => {
                        eprintln!("[dcx-organizer] 收纳 {p:?} 失败: {e}");
                        continue;
                    }
                }
            };
            let key = path_key(&target);
            self.cfg.overrides.remove(&key);
            self.cfg.excluded.remove(&key);
            self.cfg.hidden.remove(&key);
            adopted += 1;
        }
        (adopted, moved)
    }

    /// Physically move items into a folder. The destination can be a folder in
    /// the collection or any folder on disk; moving a desktop item here collects
    /// it. Manifest keys for the old paths are cleaned up.
    pub fn move_to_folder(&mut self, paths: &[PathBuf], dir: &Path) -> (usize, usize) {
        if !dir.is_dir() {
            return (0, paths.len());
        }
        let mut moved = 0;
        let mut failed = 0;
        let mut touched_desktop = false;
        for p in paths {
            let p = norm(p);
            if let Some(why) = drop_refusal(&p, &self.root, &self.desktop) {
                eprintln!("[dcx-organizer] 拒绝移动 {p:?}: {why}");
                failed += 1;
                continue;
            }
            // Never move a folder into itself or into one of its descendants.
            if is_same_or_under(dir, &p) {
                continue;
            }
            if p.parent().map(|par| same_path(par, dir)).unwrap_or(false) {
                continue;
            }
            let key = path_key(&p);
            let hidden_by_us = self.cfg.hidden.contains(&key);
            let from_desktop = is_same_or_under(&p, &self.desktop);
            match move_into_dir(&p, dir) {
                Ok(dst) => {
                    moved += 1;
                    if hidden_by_us {
                        set_hidden(&dst, false);
                    }
                    if from_desktop {
                        touched_desktop = true;
                    }
                    self.cfg.hidden.remove(&key);
                    self.cfg.overrides.remove(&key);
                    self.cfg.excluded.remove(&key);
                }
                Err(e) => {
                    failed += 1;
                    eprintln!("[dcx-organizer] 移入 {p:?} 失败: {e}");
                }
            }
        }
        if touched_desktop {
            notify_desktop(&self.desktop, false);
        }
        (moved, failed)
    }

    /// Move items from the collection back to the desktop root, where Explorer
    /// shows them again. Returns the new desktop paths, in input order where
    /// successful. Top-level items already on the desktop are returned as-is.
    pub fn move_to_desktop(&mut self, paths: &[PathBuf]) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for p in paths {
            let p = norm(p);
            if let Some(why) = drop_refusal(&p, &self.root, &self.desktop) {
                eprintln!("[dcx-organizer] 拒绝放回桌面 {p:?}: {why}");
                continue;
            }
            let already_top = p
                .parent()
                .map(|par| same_path(par, &self.desktop))
                .unwrap_or(false);
            let dst = if already_top {
                p.clone()
            } else {
                match move_into_dir(&p, &self.desktop) {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("[dcx-organizer] 移回桌面 {p:?} 失败: {e}");
                        continue;
                    }
                }
            };
            set_hidden(&dst, false);
            let old_key = path_key(&p);
            let key = path_key(&dst);
            self.cfg.overrides.remove(&old_key);
            self.cfg.overrides.remove(&key);
            self.cfg.hidden.remove(&old_key);
            self.cfg.hidden.remove(&key);
            // Legacy manifest: the library model does not use exclusions.
            self.cfg.excluded.remove(&old_key);
            self.cfg.excluded.remove(&key);
            out.push(dst);
        }
        if !out.is_empty() {
            notify_desktop(&self.desktop, false);
        }
        out
    }

    /// Create a folder under `parent` (a desktop folder, or the desktop
    /// itself). Names collide like Explorer's: `新建文件夹 (2)`.
    pub fn create_folder(&mut self, parent: &Path, name: &str) -> io::Result<PathBuf> {
        let name = name.trim();
        if name.is_empty()
            || name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
            || name == "."
            || name == ".."
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "文件夹名无效"));
        }
        if !parent.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "目标不是文件夹",
            ));
        }
        let dst = unique_path(parent, name)?;
        std::fs::create_dir(&dst)?;
        if is_same_or_under(parent, &self.desktop) {
            notify_desktop(&self.desktop, false);
        }
        Ok(dst)
    }

    /// Persist the manifest, pruning choices for files that no longer exist.
    pub fn save(&mut self) -> io::Result<()> {
        self.save_keep_gone()
    }

    /// Save without pruning. Used by the UI while an external drag is still
    /// settling (the destination may not be visible to `stat` yet).
    ///
    /// Writes **only** the three manifest keys this type owns. The box UI edits
    /// `categories` in the same document and the settings centre rewrites the
    /// whole thing on its own debounce, so a blind `cfg.save()` here restored
    /// whatever order this copy happened to be loaded with — a drag-reorder
    /// vanished on the next refresh.
    pub fn save_keep_gone(&mut self) -> io::Result<()> {
        let hidden = self.cfg.hidden.clone();
        let excluded = self.cfg.excluded.clone();
        let overrides = self.cfg.overrides.clone();
        Config::patch(|d| {
            d.hidden = hidden;
            d.excluded = excluded;
            d.overrides = overrides;
        })?;
        // Adopt the document that is now on disk, so this copy can never write
        // another writer's keys back over theirs.
        self.cfg = Config::load();
        Ok(())
    }

    /// Prune manifest entries for paths that stayed unreachable.
    ///
    /// A single `exists() == false` used to be enough, and the result was
    /// written on the next save: a sleeping network drive, a OneDrive
    /// placeholder or a drag that had not landed yet permanently dropped the
    /// user's manual category. Three consecutive misses (~3.6 s at the panel's
    /// refresh cadence) are required now.
    pub fn prune(&mut self) {
        let keys: Vec<String> = self
            .cfg
            .overrides
            .keys()
            .chain(self.cfg.excluded.iter())
            .chain(self.cfg.hidden.iter())
            .cloned()
            .collect();
        for k in keys {
            if Path::new(&k).exists() {
                self.missing.remove(&k);
                continue;
            }
            let misses = {
                let slot = self.missing.entry(k.clone()).or_insert(0);
                *slot = slot.saturating_add(1);
                *slot
            };
            if misses < PRUNE_MISSES {
                continue;
            }
            self.cfg.overrides.remove(&k);
            self.cfg.excluded.remove(&k);
            self.cfg.hidden.remove(&k);
            self.missing.remove(&k);
        }
        let keep: BTreeSet<String> = self
            .cfg
            .overrides
            .keys()
            .cloned()
            .chain(self.cfg.excluded.iter().cloned())
            .chain(self.cfg.hidden.iter().cloned())
            .collect();
        self.missing.retain(|k, _| keep.contains(k));
    }

    pub fn contains(&self, path: &Path) -> bool {
        is_same_or_under(path, &self.root)
    }

    /// Rename a desktop item, keeping every manifest key in sync. This is an
    /// explicit user action, not part of classification.
    pub fn rename(&mut self, path: &Path, new_name: &str) -> io::Result<PathBuf> {
        let new_name = new_name.trim();
        if new_name.is_empty()
            || new_name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
            || new_name == ".".to_string()
            || new_name == "..".to_string()
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "文件名无效"));
        }
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "没有父目录"))?;
        let dst = parent.join(new_name);
        if dst == path {
            return Ok(dst);
        }
        if dst.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "同名文件已存在",
            ));
        }
        let from = path_key(path);
        let to = path_key(&dst);
        std::fs::rename(path, &dst)?;
        if let Some(v) = self.cfg.overrides.remove(&from) {
            self.cfg.overrides.insert(to.clone(), v);
        }
        if self.cfg.excluded.remove(&from) {
            self.cfg.excluded.insert(to.clone());
        }
        if self.cfg.hidden.remove(&from) {
            self.cfg.hidden.insert(to);
        }
        let _ = self.save_keep_gone();
        notify_desktop(&self.desktop, false);
        Ok(dst)
    }

    /// Forget a path completely after it was deleted.
    pub fn forget(&mut self, path: &Path) {
        let k = path_key(path);
        self.cfg.overrides.remove(&k);
        self.cfg.excluded.remove(&k);
        self.cfg.hidden.remove(&k);
        let _ = self.save_keep_gone();
    }

    /// Bulk removal of manifest state for deleted items.
    pub fn forget_many(&mut self, paths: &[PathBuf]) {
        for p in paths {
            let k = path_key(p);
            self.cfg.overrides.remove(&k);
            self.cfg.excluded.remove(&k);
            self.cfg.hidden.remove(&k);
        }
        let _ = self.save_keep_gone();
    }

    /// Migration away from per-file hiding. Older builds wrote
    /// `FILE_ATTRIBUTE_HIDDEN` on desktop items; that made folders invisible in
    /// Windows file dialogs and painted a grey desktop when Explorer showed
    /// hidden items. Unhide every path this app ever marked, once, then let
    /// the icon-layer toggle do the hiding. Returns how many entries were
    /// cleared (0 on every later scan, so this is effectively free).
    pub fn restore_hidden(&mut self) -> usize {
        if self.cfg.hidden.is_empty() {
            return 0;
        }
        let keys: Vec<String> = self.cfg.hidden.iter().cloned().collect();
        let mut touched = false;
        for key in &keys {
            if set_hidden(&PathBuf::from(key), false) {
                touched = true;
            }
        }
        self.cfg.hidden.clear();
        if touched {
            notify_desktop(&self.desktop, false);
        }
        let n = keys.len();
        let _ = self.save_keep_gone();
        n
    }
}

/// Force one desktop re-enumeration after a batch of visibility changes.
fn notify_desktop(dir: &Path, flush: bool) {
    use dcx_win32 as w;
    let wide = w::wide_path(dir);
    let flags = w::SHCNF_PATHW
        | if flush {
            w::SHCNF_FLUSH
        } else {
            w::SHCNF_FLUSHNOWAIT
        };
    unsafe {
        w::SHChangeNotify(
            w::SHCNE_UPDATEDIR,
            flags,
            wide.as_ptr() as *const core::ffi::c_void,
            core::ptr::null(),
        );
    }
}

fn set_hidden(path: &Path, hidden: bool) -> bool {
    use dcx_win32 as w;
    let wide = w::wide_path(path);
    let attrs = unsafe { w::GetFileAttributesW(wide.as_ptr()) };
    if attrs == w::INVALID_FILE_ATTRIBUTES {
        return false;
    }
    let has = attrs & w::FILE_ATTRIBUTE_HIDDEN != 0;
    if has == hidden {
        return false;
    }
    let next = if hidden {
        attrs | w::FILE_ATTRIBUTE_HIDDEN
    } else {
        attrs & !w::FILE_ATTRIBUTE_HIDDEN
    };
    unsafe { w::SetFileAttributesW(wide.as_ptr(), next) != 0 }
}

/// Manifest key for a path: normalised and lowercased.
fn path_key(p: &Path) -> String {
    norm(p).to_string_lossy().to_ascii_lowercase()
}

/// Case-insensitive path equality, the way Windows compares paths.
pub fn same_path(a: &Path, b: &Path) -> bool {
    path_key(a) == path_key(b)
}

/// Component-wise, case-insensitive comparison of one path component pair.
fn component_eq(a: Component<'_>, b: Component<'_>) -> bool {
    a.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
}

/// `path` is `ancestor` or lives below it, compared **component by component**
/// after `..` folding — so `C:\Desktop2` is not under `C:\Desktop`, and
/// `C:\Desktop\..\Windows` is not either.
pub fn is_same_or_under(path: &Path, ancestor: &Path) -> bool {
    let p = norm(path);
    let a = norm(ancestor);
    let mut pc = p.components();
    for ac in a.components() {
        match pc.next() {
            Some(c) if component_eq(c, ac) => {}
            _ => return false,
        }
    }
    true
}

/// Move `src` into `dir`, never overwriting (`name`, `name (1)`, …).
///
/// A failed move removes whatever it had already copied: the caller counts a
/// success, and a "copied but the source is still there" outcome used to be
/// reported as collected — a silent duplicate on the desktop.
pub fn move_into_dir(src: &Path, dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "未命名".to_string());
    let dst = unique_path(dir, &name)?;
    if same_path(src, &dst) {
        return Ok(dst);
    }
    move_no_replace(src, &dst)?;
    Ok(dst)
}

#[link(name = "kernel32")]
extern "system" {
    fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
}

const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

/// One move that must not overwrite and must not leave a duplicate behind.
fn move_no_replace(src: &Path, dst: &Path) -> io::Result<()> {
    let s = dcx_win32::config_file::wide_path(src);
    let d = dcx_win32::config_file::wide_path(dst);
    if unsafe { MoveFileExW(s.as_ptr(), d.as_ptr(), MOVEFILE_WRITE_THROUGH) } != 0 {
        return Ok(());
    }
    let rename_err = io::Error::last_os_error();
    // Same-volume rename refused (an open handle, or a different volume): copy,
    // verify, then delete the source, and roll the copy back if that fails.
    // `std::fs::rename` was not usable here — on Windows it passes
    // MOVEFILE_REPLACE_EXISTING and can silently overwrite a same-named file.
    copy_then_remove(src, dst).map_err(|e| io::Error::new(e.kind(), format!("{e}（重命名错误：{rename_err}）")))
}

fn copy_then_remove(src: &Path, dst: &Path) -> io::Result<()> {
    if let Err(e) = copy_recursive(src, dst) {
        let _ = remove_recursive(dst);
        return Err(e);
    }
    match remove_recursive(src) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Honest failure: the destination is removed again so the file
            // exists exactly once, where it started.
            match remove_recursive(dst) {
                Ok(()) => Err(io::Error::new(
                    e.kind(),
                    format!("已复制但无法删除源文件（{e}），操作已回滚"),
                )),
                Err(rollback) => Err(io::Error::new(
                    e.kind(),
                    format!(
                        "已复制但无法删除源文件（{e}），回滚也失败（{rollback}）；源与目标同时存在，请手动核对"
                    ),
                )),
            }
        }
    }
}

pub fn unique_path(dir: &Path, name: &str) -> io::Result<PathBuf> {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return Ok(candidate);
    }
    let p = Path::new(name);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_string());
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for n in 1..10_000 {
        let c = dir.join(format!("{stem} ({n}){ext}"));
        if !c.exists() {
            return Ok(c);
        }
    }
    // Never hand back a name that is already taken — the caller would then move
    // onto an existing file. A PID-tagged suffix plus the OS's own refusal is
    // the last resort.
    let pid = std::process::id();
    for n in 0..1_000 {
        let c = dir.join(format!("{stem} ({pid}-{n}){ext}"));
        if !c.exists() {
            return Ok(c);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("无法为 {name} 生成不重复的文件名"),
    ))
}

#[link(name = "kernel32")]
extern "system" {
    fn CopyFileW(existing: *const u16, new: *const u16, fail_if_exists: i32) -> i32;
}

/// Copy one file with its timestamps and attributes.
///
/// `std::fs::copy` resets the modified time, which a cross-volume library move
/// would turn into "every file changed today" in the box and in Explorer. The
/// Win32 copy keeps the metadata the way Explorer's own move does.
fn copy_file(src: &Path, dst: &Path) -> io::Result<()> {
    let src = dcx_win32::config_file::wide_path(src);
    let dst = dcx_win32::config_file::wide_path(dst);
    if unsafe { CopyFileW(src.as_ptr(), dst.as_ptr(), 1) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn copy_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(src)?;
    // `metadata()` followed junctions: dropping one copied the whole external
    // tree into the library and then deleted only the link, leaving the copy
    // behind. A reparse point is refused instead.
    if is_reparse_point(src) {
        return Err(io::Error::other(format!(
            "拒绝跟随链接或重解析点：{}",
            src.display()
        )));
    }
    if meta.is_dir() {
        // `create_dir`, not `create_dir_all`: an existing destination must fail
        // instead of merging two same-named folders.
        std::fs::create_dir(dst)?;
        let rd = std::fs::read_dir(src)?;
        for e in rd {
            let e = e?; // enumeration errors are failures, not skips
            copy_recursive(&e.path(), &dst.join(e.file_name()))?;
        }
        Ok(())
    } else if meta.is_file() {
        copy_file(src, dst)
    } else {
        Err(io::Error::other("不支持的条目类型"))
    }
}

fn remove_recursive(p: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(p)?;
    if meta.is_dir() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    }
}

/// What a [`migrate_library`] run actually did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrateReport {
    /// Top-level items now living under the new root.
    pub moved: usize,
    /// Items that landed under a ` (n)` name because the destination already
    /// had an entry with the same name.
    pub renamed: usize,
    /// Items copied to the new root but still (at least partly) present under
    /// the old one. Always zero since moves became all-or-nothing: a delete
    /// failure rolls the copy back and aborts the migration, and the error
    /// names the file that could not be released.
    pub leftovers: usize,
    /// `(old, new)` for every top-level item that now exists in the new root.
    /// The caller rewrites manifest paths (`overrides`) with this, or manual
    /// categories would silently reset after a move.
    pub pairs: Vec<(PathBuf, PathBuf)>,
}

/// Move every top-level item of one collection root into another.
///
/// Used when the user changes the library folder in Settings. Nothing is
/// overwritten: a name collision gets a ` (n)` suffix. If one item cannot be
/// moved, everything already moved is put back so the library never ends up
/// split across two roots.
pub fn migrate_library(from: &Path, to: &Path) -> io::Result<MigrateReport> {
    let from = norm(from);
    let to = norm(to);
    if same_path(&from, &to) {
        return Ok(MigrateReport::default());
    }
    if is_same_or_under(&to, &from) || is_same_or_under(&from, &to) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "新旧收纳库目录不能互相包含",
        ));
    }
    std::fs::create_dir_all(&to)?;
    if !from.is_dir() {
        return Ok(MigrateReport::default());
    }
    let mut entries: Vec<PathBuf> = Vec::new();
    // `filter_map(|e| e.ok())` skipped unreadable children, so a migration
    // could "succeed" while leaving part of the library behind.
    for e in std::fs::read_dir(&from)? {
        entries.push(e?.path());
    }
    let mut report = MigrateReport::default();
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    for src in entries {
        let collision = src
            .file_name()
            .map(|n| to.join(n).exists())
            .unwrap_or(false);
        match move_into_dir(&src, &to) {
            Ok(dst) => {
                report.moved += 1;
                if collision {
                    report.renamed += 1;
                }
                // `move_into_dir` copies, deletes and rolls the copy back when
                // the delete fails, so a success here means the item really
                // left the old root.
                moved.push((src.clone(), dst.clone()));
                report.pairs.push((src, dst));
            }
            Err(e) => {
                // Put the already moved items back under their old names, so a
                // failed change leaves the library exactly where it was.
                let mut rollback_failures = 0usize;
                for (orig, placed) in moved.iter().rev() {
                    if std::fs::rename(placed, orig).is_err() && move_into_dir(placed, &from).is_err()
                    {
                        rollback_failures += 1;
                    }
                }
                let detail = if rollback_failures == 0 {
                    String::new()
                } else {
                    format!("；{rollback_failures} 项回滚失败，请手动核对新旧目录")
                };
                return Err(io::Error::new(
                    e.kind(),
                    format!("迁移 {} 失败：{e}{detail}", src.display()),
                ));
            }
        }
    }
    Ok(report)
}

/// Rewrite one normalised manifest key after top-level items moved to a new
/// root. Descendants of a moved folder follow its prefix, so manual category
/// choices and legacy manifests survive a library change.
pub fn remap_moved_key(key: &str, pairs: &[(PathBuf, PathBuf)]) -> String {
    for (old, new) in pairs {
        let old = path_key(old);
        let new = path_key(new);
        if key == old {
            return new;
        }
        if let Some(rest) = key.strip_prefix(&old) {
            if rest.starts_with('\\') || rest.starts_with('/') {
                return format!("{new}{rest}");
            }
        }
    }
    key.to_string()
}

/// Lexical normalisation: drop `.` and fold `..` against the preceding
/// component.
///
/// Folding matters for security, not cosmetics: `is_same_or_under` and the
/// "library must not contain the desktop" checks were plain string prefix
/// tests, so `C:\Users\x\Desktop\..\Windows` used to pass them and let a drop
/// move a directory far outside the box.
pub fn norm(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                // `..` above a root or a drive prefix resolves to the root
                // itself, exactly like `GetFullPathNameW` does.
                Some(Component::RootDir) | Some(Component::Prefix(_)) => {}
                _ => out.push(".."),
            },
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `FILE_ATTRIBUTE_REPARSE_POINT` — junctions, symlinks, cloud placeholders.
fn is_reparse_point(p: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    match std::fs::symlink_metadata(p) {
        Ok(m) => m.file_attributes() & 0x400 != 0,
        Err(_) => false,
    }
}

/// Directories a drop must never move: losing one is unrecoverable, and none
/// of them is ever a legitimate box item.
fn is_protected_dir(p: &Path) -> bool {
    let p = norm(p);
    // The drive root itself (`C:\`) has no parent. `%SystemDrive%` must not be
    // used as a prefix root: `C:` would mark every file on the system drive as
    // protected.
    if p.parent().is_none() {
        return true;
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    for var in [
        "SystemRoot",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramData",
    ] {
        if let Some(v) = std::env::var_os(var) {
            let v = PathBuf::from(v);
            if !v.as_os_str().is_empty() {
                roots.push(v);
            }
        }
    }
    roots.iter().any(|r| is_same_or_under(&p, r))
}

/// `\\server\share` — a drop from a network location is refused: the move
/// would silently turn into a long copy with no rollback guarantee.
fn is_network_path(p: &Path) -> bool {
    let s = p.to_string_lossy();
    s.starts_with("\\\\") || s.starts_with("//")
}

/// Why a path may not enter (or leave through) the box.
///
/// Used by every entry point that accepts paths from outside: a UI drop, a
/// clipboard paste, a folder move. The checks are deliberately conservative;
/// a refusal is reported to the user, never silently ignored.
pub fn drop_refusal(path: &Path, root: &Path, desktop: &Path) -> Option<String> {
    let p = norm(path);
    if p.as_os_str().is_empty() {
        return Some("路径为空".to_string());
    }
    if is_network_path(&p) {
        return Some("不支持从网络位置移动文件".to_string());
    }
    if p.components().next().is_none() {
        return Some("路径无效".to_string());
    }
    if same_path(&p, root) || is_same_or_under(root, &p) {
        return Some("不能把收纳库本身移入收纳库".to_string());
    }
    if same_path(&p, desktop) || is_same_or_under(desktop, &p) {
        return Some("不能移动桌面目录本身".to_string());
    }
    if is_protected_dir(&p) {
        return Some("不能移动系统目录".to_string());
    }
    if is_reparse_point(&p) {
        return Some("不支持移动链接或重解析点".to_string());
    }
    None
}

/// Minimal `*` / `?` glob match (case-insensitive input expected).
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            pi += 1;
            mark = ti;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Filter by a free-text query. Whitespace separates AND terms; every term
/// must appear in the name, the extension, or (when present) the path.
pub fn filter_entries(entries: &[Entry], query: &str) -> Vec<Entry> {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    if terms.is_empty() {
        return entries.to_vec();
    }
    entries
        .iter()
        .filter(|e| {
            let hay = format!(
                "{} {} {}",
                e.name.to_ascii_lowercase(),
                e.ext,
                e.path.to_string_lossy().to_ascii_lowercase()
            );
            terms.iter().all(|t| hay.contains(t))
        })
        .cloned()
        .collect()
}

/// Sort in place; folders always come first, then the chosen key.
pub fn sort_entries(entries: &mut [Entry], by: SortKey, desc: bool) {
    entries.sort_by(|a, b| {
        let folder = b.is_dir.cmp(&a.is_dir);
        if folder != std::cmp::Ordering::Equal {
            return folder;
        }
        let ord = match by {
            SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortKey::Size => a.size.cmp(&b.size),
            SortKey::Modified => a.modified.cmp(&b.modified),
            SortKey::Type => a
                .ext
                .cmp(&b.ext)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
        };
        let ord = ord.then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        if desc {
            ord.reverse()
        } else {
            ord
        }
    });
}

/// `12.3 MB` / `840 KB` / `512 B`.
pub fn fmt_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.0} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.2} GB", b / (KB * KB * KB))
    }
}

/// `MM-DD HH:MM` local time from unix seconds; empty when unknown.
pub fn fmt_time(secs: u64) -> String {
    if secs == 0 {
        return String::new();
    }
    dcx_win32::local_time_from_unix(secs)
}

/// Milliseconds since the epoch, for cache invalidation in the UI.
pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, ext: &str, is_dir: bool) -> Entry {
        Entry {
            path: PathBuf::from(path),
            name: Path::new(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string(),
            is_dir,
            ext: ext.into(),
            size: 1,
            modified: 0,
            created: 0,
            attrs_hidden: false,
            attrs_system: false,
            category: String::new(),
            manual: false,
        }
    }

    #[test]
    fn classification_follows_rules() {
        let o = Organizer::new(Config::default());
        assert_eq!(o.classify(&entry(r"C:\d\a.docx", "docx", false)), "doc");
        assert_eq!(o.classify(&entry(r"C:\d\b.ZIP", "zip", false)), "archive");
        assert_eq!(o.classify(&entry(r"C:\d\c", "", false)), CATCH_ALL);
        assert_eq!(o.classify(&entry(r"C:\d\Doc", "", true)), FOLDER);
    }

    #[test]
    fn folder_category_is_built_in_for_old_documents() {
        let mut cfg = Config::default();
        cfg.categories.retain(|c| c.key != FOLDER);
        cfg.repair();
        assert!(cfg.category(FOLDER).is_some());
        assert_eq!(cfg.categories.last().unwrap().key, CATCH_ALL);
        let o = Organizer::new(cfg);
        let docs = entry(r"C:\d\Docs", "", true);
        assert_eq!(o.classify(&docs), FOLDER);
        // A manual choice still wins over the built-in rule.
        let mut cfg = Config::default();
        cfg.overrides
            .insert(r"c:\d\docs".to_string(), "doc".to_string());
        let o = Organizer::new(cfg);
        assert_eq!(o.classify(&docs), "doc");
    }


    #[test]
    fn path_containment_is_component_wise() {
        assert!(is_same_or_under(
            Path::new(r"C:\Users\A\Desktop\Docs"),
            Path::new(r"C:\Users\A\Desktop")
        ));
        assert!(is_same_or_under(
            Path::new(r"c:\users\a\desktop"),
            Path::new(r"C:\Users\A\Desktop")
        ));
        assert!(!is_same_or_under(
            Path::new(r"C:\Users\A\Desktop2"),
            Path::new(r"C:\Users\A\Desktop")
        ));
        assert!(!is_same_or_under(
            Path::new(r"C:\Users\A\Desktop"),
            Path::new(r"C:\Users\A\Desktop\Docs")
        ));
    }

    #[test]
    fn path_normalisation_folds_parent_segments() {
        assert_eq!(norm(Path::new(r"C:\a\b\..\c")), PathBuf::from(r"C:\a\c"));
        // `..` above a drive root resolves to the root, like GetFullPathNameW.
        assert_eq!(norm(Path::new(r"C:\..\..\a")), PathBuf::from(r"C:\a"));
        assert_eq!(norm(Path::new(r"a\..\b")), PathBuf::from("b"));
        assert_eq!(norm(Path::new(r".\a\.\b")), PathBuf::from(r"a\b"));
    }

    #[test]
    fn containment_cannot_be_escaped_with_dotdot() {
        let desktop = Path::new(r"C:\Users\A\Desktop");
        // This used to pass a plain string-prefix test.
        assert!(!is_same_or_under(
            Path::new(r"C:\Users\A\Desktop\..\..\Windows"),
            desktop
        ));
        assert!(!is_same_or_under(Path::new(r"C:\Users\A\Desktop\..\B"), desktop));
        // …while a genuine child still matches.
        assert!(is_same_or_under(Path::new(r"C:\Users\A\Desktop\x"), desktop));
    }

    #[test]
    fn absorb_refuses_the_desktop_directory_itself() {
        let base = std::env::temp_dir().join(format!("dcx-guard-{}", now_ms()));
        let library = base.join("library");
        let desktop = base.join("desktop");
        std::fs::create_dir_all(&library).unwrap();
        std::fs::create_dir_all(&desktop).unwrap();
        std::fs::write(desktop.join("a.txt"), b"x").unwrap();
        let mut o = Organizer::with_roots(Config::default(), library.clone(), desktop.clone());

        // Dragging the desktop folder onto a category tab used to run
        // `rename(desktop, library\desktop)` and take the whole desktop away.
        assert_eq!(o.absorb(&[desktop.clone()], "doc"), (0, 0));
        assert!(desktop.is_dir());
        assert!(desktop.join("a.txt").exists());
        // A path that reaches the desktop through `..` is refused too.
        assert_eq!(
            o.absorb(&[desktop.join("..").join("desktop")], "doc"),
            (0, 0)
        );
        assert!(desktop.join("a.txt").exists());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn move_into_dir_never_overwrites() {
        let base = std::env::temp_dir().join(format!("dcx-move-{}", now_ms()));
        let src = base.join("src");
        let dst = base.join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("a.txt"), b"new").unwrap();
        std::fs::write(dst.join("a.txt"), b"old").unwrap();

        let moved = move_into_dir(&src.join("a.txt"), &dst).unwrap();
        assert_eq!(moved, dst.join("a (1).txt"));
        assert_eq!(std::fs::read(dst.join("a.txt")).unwrap(), b"old");
        assert_eq!(std::fs::read(&moved).unwrap(), b"new");
        assert!(!src.join("a.txt").exists());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn prune_waits_for_consecutive_misses() {
        let base = std::env::temp_dir().join(format!("dcx-prune-{}", now_ms()));
        let library = base.join("library");
        let desktop = base.join("desktop");
        std::fs::create_dir_all(&library).unwrap();
        std::fs::create_dir_all(&desktop).unwrap();
        let mut cfg = Config::default();
        cfg.overrides.insert(
            format!(r"{}\gone.txt", desktop.display()).to_ascii_lowercase(),
            "doc".to_string(),
        );
        let mut o = Organizer::with_roots(cfg, library, desktop);
        for _ in 0..PRUNE_MISSES - 1 {
            o.prune();
            assert_eq!(o.cfg.overrides.len(), 1, "pruned too early");
        }
        o.prune();
        assert!(o.cfg.overrides.is_empty());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn folder_move_is_physical_and_children_are_listed() {
        let root = std::env::temp_dir().join(format!("dcx-org-{}", now_ms()));
        let dest = root.join("收件");
        std::fs::create_dir_all(&dest).unwrap();
        let file = root.join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let mut o = Organizer::at(Config::default(), root.clone());
        assert_eq!(o.move_to_folder(&[file.clone()], &dest), (1, 0));
        assert!(!file.exists());
        assert!(dest.join("a.txt").exists());
        let (kids, total) = o.folder_children(&dest, 10);
        assert_eq!(total, 1);
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].name, "a.txt");
        // A folder cannot be moved into itself.
        assert_eq!(o.move_to_folder(&[dest.clone()], &dest), (0, 0));
        let created = o.create_folder(&root, "新建文件夹").unwrap();
        assert!(created.is_dir());
        let second = o.create_folder(&root, "新建文件夹").unwrap();
        assert_ne!(created, second);
        // A nested row dragged out lands at the collection root.
        let back = o.move_to_desktop(&[dest.join("a.txt")]);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0], root.join("a.txt"));
        assert!(root.join("a.txt").exists());
        assert!(o.contains(&root.join("a.txt")));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn collection_is_separate_from_the_real_desktop() {
        let base = std::env::temp_dir().join(format!("dcx-lib-{}", now_ms()));
        let library = base.join("library");
        let desktop = base.join("desktop");
        std::fs::create_dir_all(&library).unwrap();
        std::fs::create_dir_all(&desktop).unwrap();
        let file = desktop.join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let mut o = Organizer::with_roots(Config::default(), library.clone(), desktop.clone());
        assert_eq!(o.desktop_items().len(), 1);
        assert_eq!(o.items().len(), 0);

        // Collecting moves it out of the desktop, so Explorer no longer shows it.
        assert_eq!(o.absorb_auto(&[file.clone()]), (1, 1));
        assert!(!file.exists());
        assert!(library.join("a.txt").exists());
        assert_eq!(o.desktop_items().len(), 0);
        assert_eq!(o.items().len(), 1);

        // Releasing puts it back on the desktop.
        let back = o.move_to_desktop(&[library.join("a.txt")]);
        assert_eq!(back, vec![desktop.join("a.txt")]);
        assert!(file.exists());
        assert!(o.items().is_empty());
        assert!(!o.cfg.excluded.contains(&path_key(&file)));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn quick_absorb_reclassifies_and_readmits_pulled_items() {
        let root = std::env::temp_dir().join(format!("dcx-auto-{}", now_ms()));
        let desktop = root.join("desktop");
        std::fs::create_dir_all(&desktop).unwrap();
        let mut o = Organizer::at(Config::default(), desktop.clone());

        // From outside the desktop: physically adopted, rule-classified, and
        // no manual override is written.
        let photo = root.join("photo.JPG");
        std::fs::write(&photo, b"x").unwrap();
        assert_eq!(o.absorb_auto(&[photo.clone()]), (1, 1));
        assert!(!photo.exists());
        assert!(desktop.join("photo.JPG").exists());

        // A pulled-out desktop item comes back into the view, and a stale
        // manual choice no longer wins over the extension rule.
        let note = desktop.join("note.txt");
        std::fs::write(&note, b"x").unwrap();
        o.cfg.excluded.insert(path_key(&note));
        o.cfg.overrides.insert(path_key(&note), "image".to_string());
        assert_eq!(o.absorb_auto(&[note.clone()]), (1, 0));
        let snap = o.snapshot();
        let photo = snap
            .items
            .iter()
            .find(|e| e.name == "photo.JPG")
            .expect("photo adopted");
        assert_eq!(photo.category, "image");
        assert!(!photo.manual);
        let note = snap.items.iter().find(|e| e.name == "note.txt").unwrap();
        assert_eq!(note.category, "doc");
        assert!(!note.manual);
        // The desktop itself can never be filed into itself.
        assert_eq!(o.absorb_auto(&[desktop.clone()]), (0, 0));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn manual_choice_wins_and_survives_case_changes() {
        let mut o = Organizer::new(Config::default());
        let pdf = entry(r"C:\d\报告.PDF", "pdf", false);
        o.set_override(&pdf.path, "image");
        assert_eq!(o.classify(&pdf), "image");
        assert_eq!(o.classify(&entry(r"c:\d\报告.pdf", "pdf", false)), "image");
        assert!(o.clear_override(&pdf.path));
        assert_eq!(o.classify(&pdf), "doc");
    }

    #[test]
    fn unknown_override_keys_are_ignored() {
        let mut cfg = Config::default();
        cfg.overrides
            .insert(r"c:\d\a.png".to_string(), "gone".to_string());
        let o = Organizer::new(cfg);
        assert_eq!(o.classify(&entry(r"C:\D\A.png", "png", false)), "image");
    }

    #[test]
    fn extension_is_unique_across_categories() {
        let mut cfg = Config::default();
        cfg.assign_ext("image", "pdf");
        assert_eq!(cfg.category_of_ext("pdf").unwrap().key, "image");
        assert!(!cfg
            .category("doc")
            .unwrap()
            .exts
            .contains(&"pdf".to_string()));
    }

    #[test]
    fn repair_keeps_a_catch_all_last() {
        let mut cfg = Config::default();
        cfg.categories.retain(|c| c.key != CATCH_ALL);
        cfg.repair();
        assert_eq!(cfg.categories.last().unwrap().key, CATCH_ALL);
    }

    #[test]
    fn glob_and_query() {
        assert!(glob_match("*.tmp", "cache.tmp"));
        assert!(glob_match("~$*", "~$report.docx"));
        assert!(!glob_match("*.tmp", "cache.tmpx"));
        let items = vec![
            entry(r"C:\d\季度报告.pdf", "pdf", false),
            entry(r"C:\d\照片.png", "png", false),
        ];
        assert_eq!(filter_entries(&items, "报告").len(), 1);
        assert_eq!(filter_entries(&items, "pdf").len(), 1);
        assert_eq!(filter_entries(&items, "").len(), 2);
    }

    #[test]
    fn sorting_keeps_folders_first() {
        let mut items = vec![
            entry(r"C:\d\a.txt", "txt", false),
            entry(r"C:\d\Z", "", true),
            entry(r"C:\d\b.txt", "txt", false),
        ];
        sort_entries(&mut items, SortKey::Name, false);
        assert!(items[0].is_dir);
    }

    #[test]
    fn library_migration_moves_everything_without_overwriting() {
        let base =
            std::env::temp_dir().join(format!("dcx-migrate-{}-{}", now_ms(), std::process::id()));
        let old = base.join("old");
        let new = base.join("new");
        std::fs::create_dir_all(old.join("sub")).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(old.join("a.txt"), b"old").unwrap();
        std::fs::write(old.join("sub").join("b.txt"), b"b").unwrap();
        std::fs::write(new.join("a.txt"), b"keep").unwrap();

        let report = migrate_library(&old, &new).unwrap();
        assert_eq!(report.moved, 2);
        assert_eq!(report.renamed, 1);
        assert_eq!(report.leftovers, 0);
        assert_eq!(std::fs::read_dir(&old).unwrap().count(), 0);
        assert_eq!(std::fs::read(new.join("a.txt")).unwrap(), b"keep");
        assert_eq!(std::fs::read(new.join("a (1).txt")).unwrap(), b"old");
        assert_eq!(std::fs::read(new.join("sub/b.txt")).unwrap(), b"b");
        assert_eq!(report.pairs.len(), 2);
        let nested = path_key(&old.join("sub/b.txt"));
        assert_eq!(
            remap_moved_key(&nested, &report.pairs),
            path_key(&new.join("sub/b.txt"))
        );
        assert_eq!(
            remap_moved_key("c:\\elsewhere\\x.txt", &report.pairs),
            "c:\\elsewhere\\x.txt"
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn library_migration_rejects_nested_roots() {
        let base = std::env::temp_dir().join(format!(
            "dcx-migrate-nested-{}-{}",
            now_ms(),
            std::process::id()
        ));
        let old = base.join("old");
        std::fs::create_dir_all(&old).unwrap();
        assert!(migrate_library(&old, &old.join("inner")).is_err());
        assert!(migrate_library(&old, &base).is_err());
        let empty = base.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(
            migrate_library(&empty, &base.join("fresh")).unwrap(),
            MigrateReport::default()
        );
        std::fs::remove_dir_all(&base).unwrap();
    }
}

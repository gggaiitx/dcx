//! Shared data types and the channel wire format between search workers and UI.
//!
//! PRD §7.4 specifies `path_id: u32` into a compact string table; this first
//! draft keeps a `String` per hit for clarity (results are capped at 2000),
//! while the *index* itself uses the compact layout. Promoting hits to ids is
//! a pure optimisation and does not change the protocol shape.

use std::time::{SystemTime, UNIX_EPOCH};

pub type Gen = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Name,
    Content,
}

impl Mode {
    pub fn key(self) -> &'static str {
        match self {
            Mode::Name => "name",
            Mode::Content => "content",
        }
    }

    pub fn from_key(s: &str) -> Mode {
        if s.eq_ignore_ascii_case("content") {
            Mode::Content
        } else {
            Mode::Name
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Name => "文件名",
            Mode::Content => "内容",
        }
    }

    pub fn placeholder(self) -> &'static str {
        match self {
            Mode::Name => "输入关键词快速查找本地文件，支持系统通配符 ? 和 *，例如：我的工作计划",
            Mode::Content => "输入要查找的文件内容，支持字面量或正则；大文件与二进制默认跳过",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopeSel {
    Desktop,
    Computer,
    /// Root path, e.g. `D:\` or a custom folder.
    Path(String),
}

impl ScopeSel {
    pub fn label(&self) -> String {
        match self {
            ScopeSel::Desktop => "桌面".into(),
            ScopeSel::Computer => "此电脑".into(),
            ScopeSel::Path(p) => {
                let trimmed = p.trim_end_matches(['\\', '/']);
                trimmed.rsplit(['\\', '/']).next().unwrap_or(p).to_string()
            }
        }
    }
}

/// One of the seven category chips. Extension sets come from
/// `dcx_organizer::default_categories()` so the search app and the desktop
/// box agree on what counts as an image / archive / program.
/// The box's single `media` bucket is split into video + audio here.
#[derive(Clone, Debug)]
pub struct Cat {
    pub key: &'static str,
    pub label: &'static str,
    pub exts: Vec<String>,
}

pub fn categories() -> Vec<Cat> {
    let base = dcx_organizer::default_categories();
    let take = |key: &str| -> Vec<String> {
        base.iter()
            .find(|c| c.key == key)
            .map(|c| c.exts.clone())
            .unwrap_or_default()
    };
    let media = take("media");
    let video: Vec<String> = [
        "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts", "m4p",
    ]
    .iter()
    .map(|s| s.to_string())
    .filter(|e| media.contains(e))
    .collect();
    let audio: Vec<String> = media
        .iter()
        .filter(|e| !video.contains(e))
        .cloned()
        .collect();

    vec![
        Cat { key: "all", label: "文件", exts: Vec::new() },
        Cat { key: "image", label: "图片", exts: take("image") },
        Cat { key: "archive", label: "压缩包", exts: take("archive") },
        Cat { key: "app", label: "应用程序", exts: take("app") },
        Cat { key: "video", label: "视频", exts: video },
        Cat { key: "audio", label: "音频", exts: audio },
        Cat { key: "other", label: "其他", exts: Vec::new() },
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitKind {
    Name,
    Content,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub kind: HitKind,
    pub path: String,
    pub line: Option<u64>,
    pub col: Option<u64>,
    pub preview: String,
    /// Byte range inside `preview` to highlight, if known.
    pub highlight: Option<(usize, usize)>,
    pub score: i32,
    /// Filled from `metadata` when the hit is emitted (at most `max_results`).
    pub size: Option<u64>,
    /// Last write time, seconds since the Unix epoch.
    pub mtime: Option<u64>,
    /// Lowercase sort keys, computed once when the hit is created so a column
    /// sort never allocates inside the comparator.
    pub name_lower: String,
    pub dir_lower: String,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub scanned: u64,
    pub matched: u64,
    pub errors: u64,
    pub elapsed_ms: u64,
}

/// Events pushed from workers to the egui frame loop through a bounded
/// channel. `gen` is checked by the receiver: results from a superseded query
/// are dropped instead of flashing stale rows.
#[derive(Clone, Debug)]
pub enum Ev {
    Batch { gen: Gen, hits: Vec<Hit> },
    /// `epoch` identifies the index generation. A rebuild swaps in a fresh
    /// index; progress from the previous walk must not reset the new one's UI.
    Progress {
        gen: Gen,
        epoch: u64,
        scanned: u64,
        indexing: bool,
    },
    Done { gen: Gen, partial: bool, stats: Stats },
    IndexReady {
        epoch: u64,
        entries: usize,
        errors: u64,
        elapsed_ms: u64,
        /// True when the rows came from the on-disk snapshot, not a fresh walk.
        cached: bool,
    },
    /// The watcher's buffer overflowed (events were dropped). The UI asks the
    /// engine for a fresh walk instead of silently serving a stale index.
    Rebuild { epoch: u64, reason: String },
}

#[derive(Clone, Debug)]
pub struct Query {
    pub gen: Gen,
    pub mode: Mode,
    pub raw: String,
    pub scope: ScopeSel,
    pub category: String,
    pub case_sensitive: bool,
    pub max_results: usize,
    pub max_file_size: u64,
    pub encoding: String,
    pub time_budget_ms: u64,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Batch threshold from fd's `BatchSender`: send when 256 hits accumulate or
/// 32 ms elapsed, whichever comes first.
pub const BATCH_SIZE: usize = 256;
pub const BATCH_MS: u64 = 32;

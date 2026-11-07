//! UI modules: colours, primitives, the capsule and the panel.

pub mod compact;
pub mod panel;
pub mod theme;
pub mod widgets;

pub use widgets::Glyph;

/// Which panel tab is showing. Persisted in `island.toml [ui].last_tab`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Tasks,
    Clip,
    Inbox,
    Perf,
    /// Cards from the local push API.
    Live,
}

impl Tab {
    pub fn all() -> [Tab; 5] {
        [Tab::Tasks, Tab::Clip, Tab::Inbox, Tab::Perf, Tab::Live]
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::Tasks => "任务",
            Tab::Clip => "剪贴板",
            Tab::Inbox => "消息",
            Tab::Perf => "性能",
            Tab::Live => "动态",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Tab::Tasks => "tasks",
            Tab::Clip => "clip",
            Tab::Inbox => "inbox",
            Tab::Perf => "perf",
            Tab::Live => "live",
        }
    }

    pub fn from_key(key: &str) -> Tab {
        match key {
            "clip" => Tab::Clip,
            "inbox" => Tab::Inbox,
            "perf" => Tab::Perf,
            "live" => Tab::Live,
            _ => Tab::Tasks,
        }
    }

    pub fn glyph(self) -> Glyph {
        match self {
            Tab::Tasks => Glyph::Check,
            Tab::Clip => Glyph::Clipboard,
            Tab::Inbox => Glyph::Inbox,
            Tab::Perf => Glyph::Gauge,
            Tab::Live => Glyph::Pulse,
        }
    }
}

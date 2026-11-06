//! DCX settings model + TOML persistence.
//!
//! This crate is the single source of truth for user-tunable options. Both the
//! resident host (which edits the file through its UI) and the injected engine
//! (which reads it on attach and on change) depend on it. The on-disk file is
//! the IPC boundary between the two processes.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Base taskbar max width used by the theme when `Settings::max_width` is
/// `None`. Mirrors the `TaskbarFrameMaxWidth` style constant in
/// `dcx-theme`, so the settings UI can show the effective value without
/// depending on the theme crate.
pub const DEFAULT_MAX_WIDTH: u32 = 1895;

/// Tallest *visible* dock we can draw, in DIPs.
///
/// `Shell_TrayWnd` pins its own geometry: `SetWindowPos` on it is vetoed inside
/// the shell (verified with SWP_NOSENDCHANGING / SWP_ASYNCWINDOWPOS /
/// SetWindowPlacement — the rect is unchanged 20 ms later), so the XAML can
/// never be taller than the window. The window is 48 DIP at 100 %, the theme's
/// margins take 4, so 44 is the ceiling until the shell itself is hooked.
pub const MAX_TASKBAR_HEIGHT: u32 = 44;

/// Shortest supported dock, in DIPs.
pub const MIN_TASKBAR_HEIGHT: u32 = 30;

/// The dock height plus the bottom gap must fit the 48 DIP shell window minus
/// the 2 DIP top margin and the 2 DIP baseline bottom margin.
pub const TASKBAR_BUDGET: u32 = 44;

/// Win11 taskbar height in DIPs at 100 % scaling. Reference value for the
/// schematic preview only; the real height belongs to `Shell_TrayWnd`.
pub const NOMINAL_TASKBAR_HEIGHT: u32 = 48;

/// Taskbar overall shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskbarShape {
    /// Rounded ends, no visible border — the floating "pill" look.
    Capsule,
    /// Uniform rounded corners with a visible border — a "card" look.
    Card,
}

impl std::fmt::Display for TaskbarShape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskbarShape::Capsule => write!(f, "胶囊 Capsule"),
            TaskbarShape::Card => write!(f, "卡片 Card"),
        }
    }
}

/// Smallest supported icon scale, in percent of the shell default.
pub const MIN_ICON_SCALE: u32 = 50;

/// Largest supported icon scale, in percent. 125 % is where Win11's own
/// "large icons" tops out; past that the buttons start colliding with the tray.
pub const MAX_ICON_SCALE: u32 = 125;

/// Icon scale that leaves the shell's icons untouched.
pub const DEFAULT_ICON_SCALE: u32 = 100;

/// Gap between taskbar icons, in px.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spacing {
    /// Horizontal margin added to each task list button. Negative values pull
    /// the buttons together, which is what small icon scales need.
    pub between: i32,
}

/// Range the horizontal spacing slider offers, in px.
pub const MIN_ICON_SPACING: i32 = -6;
/// See [`MIN_ICON_SPACING`].
pub const MAX_ICON_SPACING: i32 = 6;

/// Full settings document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Capsule vs card taskbar shape.
    pub taskbar_shape: TaskbarShape,
    /// Icon render scale in percent (`MIN_ICON_SCALE..=MAX_ICON_SCALE`).
    ///
    /// This is the *only* thing the value controls: it becomes a
    /// `ScaleTransform` on each task button. It deliberately does not imply a
    /// taskbar height — the shell window owns that, and a taller `Height` in the
    /// XAML root only clips the bar.
    pub icon_scale: u32,
    /// Legacy preset name (`Small`/`Medium`/`Large`) written by earlier
    /// versions. Migrated into `icon_scale` on load, never written back.
    #[serde(rename = "icon_size", default, skip_serializing_if = "Option::is_none")]
    pub legacy_icon_size: Option<String>,
    /// Icon spacing.
    #[serde(default)]
    pub icon_spacing: Spacing,
    /// Total taskbar height in DIPs, dock and tray together. `None` = keep the
    /// shell default (48). The engine resizes `Shell_TrayWnd` to this value, so
    /// the XAML segments grow/shrink with it instead of being clipped.
    #[serde(default)]
    pub taskbar_height: Option<u32>,
    /// Optional explicit taskbar max width override (px). `None` = base value.
    #[serde(default)]
    pub max_width: Option<u32>,
    /// Taskbar hover behaviour (see [`HoverEffect`]).
    #[serde(default)]
    pub hover_effect: HoverEffect,
    /// Appearance of the settings window itself. Ignored by the engine.
    #[serde(default)]
    pub ui_theme: UiTheme,
}

/// What the taskbar does when the pointer is over it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HoverEffect {
    /// Stock behaviour: icons keep their size, the button's hover background is
    /// the only feedback.
    Background,
    /// Dock-style magnification: icons scale by distance to the cursor, push
    /// their neighbours aside, and the button background is hidden so only the
    /// glyph appears to lift out.
    #[default]
    Dock,
}

impl std::fmt::Display for HoverEffect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HoverEffect::Background => write!(f, "背景"),
            HoverEffect::Dock => write!(f, "放大"),
        }
    }
}

/// Appearance of the settings window (not the taskbar).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum UiTheme {
    /// Light surfaces, dark text.
    #[default]
    Light,
    /// Dark surfaces, light text.
    Dark,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            taskbar_shape: TaskbarShape::Capsule,
            icon_scale: DEFAULT_ICON_SCALE,
            legacy_icon_size: None,
            icon_spacing: Spacing::default(),
            taskbar_height: None,
            max_width: None,
            hover_effect: HoverEffect::default(),
            ui_theme: UiTheme::default(),
        }
    }
}

impl Settings {
    /// Icon scale clamped to the supported range, in percent.
    pub fn effective_icon_scale(&self) -> u32 {
        self.icon_scale.clamp(MIN_ICON_SCALE, MAX_ICON_SCALE)
    }

    /// Icon scale as a multiplier for the theme (0.5 ..= 1.25).
    pub fn icon_scale_ratio(&self) -> f32 {
        self.effective_icon_scale() as f32 / 100.0
    }

    /// Fold legacy keys into their current counterparts.
    fn migrated(mut self) -> Settings {
        if let Some(preset) = self.legacy_icon_size.take() {
            // Only honour the legacy preset while the new key still sits at its
            // default, so a file carrying both keeps the explicit percentage.
            if self.icon_scale == DEFAULT_ICON_SCALE {
                self.icon_scale = match preset.as_str() {
                    "Small" => 85,
                    "Large" => 125,
                    _ => DEFAULT_ICON_SCALE,
                };
            }
        }
        self.icon_scale = self.effective_icon_scale();
        self.icon_spacing.between = self
            .icon_spacing
            .between
            .clamp(MIN_ICON_SPACING, MAX_ICON_SPACING);
        self
    }

    /// Height the engine will apply, in DIPs.
    pub fn effective_height(&self) -> u32 {
        self.taskbar_height
            .unwrap_or(TASKBAR_BUDGET)
            .clamp(MIN_TASKBAR_HEIGHT, MAX_TASKBAR_HEIGHT)
    }

    /// Whether the height is the shell default (used by the UI to flag "custom").
    pub fn height_is_default(&self) -> bool {
        self.taskbar_height.is_none()
    }

    /// Max width the engine will actually use: explicit override, else the
    /// theme's base value.
    pub fn effective_max_width(&self) -> u32 {
        self.max_width.unwrap_or(DEFAULT_MAX_WIDTH)
    }

    /// Whether this document equals [`Settings::default`] (used by the UI to
    /// enable/disable the "reset" affordance).
    pub fn is_default(&self) -> bool {
        *self == Settings::default()
    }
    /// Default file location: `%LOCALAPPDATA%\dcx\settings.toml`.
    pub fn path() -> PathBuf {
        let mut p = local_app_data();
        p.push("dcx");
        p.push("settings.toml");
        p
    }

    /// Load settings, falling back to defaults on any error (missing file,
    /// unreadable, or unparseable). Errors are logged to stderr so the host/engine
    /// both stay alive.
    ///
    /// A document that exists but does not parse is also marked read-only, so a
    /// later save cannot replace it with these defaults.
    pub fn load() -> Settings {
        let p = Self::path();
        match std::fs::read_to_string(&p) {
            Ok(text) => match toml::from_str::<Settings>(&text) {
                Ok(s) => {
                    dcx_win32::config_file::note_parse_ok(&p);
                    s.migrated()
                }
                Err(e) => {
                    eprintln!("[dcx-settings] parse {p:?} failed ({e}); using defaults (read-only)");
                    dcx_win32::config_file::note_parse_error(&p);
                    Settings::default()
                }
            },
            Err(_) => Settings::default(),
        }
    }

    /// Persist settings, creating `%LOCALAPPDATA%\dcx` if needed.
    ///
    /// Held under the cross-process lock and installed with an atomic swap:
    /// the engine polls `settings.toml` every second, and both a half-written
    /// file and a stale read-modify-write make it fall back to defaults and
    /// visibly reset the whole taskbar. There is deliberately **no** direct
    /// write fallback — the old one re-created exactly the half-written window
    /// its own doc comment warned about, and a failed swap leaves the previous
    /// revision intact instead.
    pub fn save(&self) -> std::io::Result<()> {
        let p = Self::path();
        let _guard = dcx_win32::config_file::FileLock::acquire(
            &p,
            dcx_win32::config_file::LOCK_TIMEOUT,
        )?;
        if dcx_win32::config_file::is_poisoned(&p) {
            return Err(dcx_win32::config_file::poisoned_error(&p));
        }
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        dcx_win32::config_file::write_atomic(&p, text.as_bytes())
    }
}

fn local_app_data() -> PathBuf {
    if let Ok(v) = std::env::var("LOCALAPPDATA") {
        PathBuf::from(v)
    } else {
        // Fallback: current directory. Never fatal — just keep it simple.
        PathBuf::from(".")
    }
}

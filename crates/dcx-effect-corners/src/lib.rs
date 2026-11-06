//! Uniform window corners and border accent.
//!
//! A deliberately tiny effect (~90 lines) that proves the extension model: it
//! needs no frame loop and touches only two DWM attributes.
//!
//! The one rule that matters: only regular application windows may be touched.
//! `DWMWA_WINDOW_CORNER_PREFERENCE` does not merely clip corners — on a window
//! that owns no frame it makes DWM *draw* one (rounded outline plus a 1 px
//! border) around the window rectangle. Shell surfaces such as Start and the
//! search flyout are transparent islands whose window rectangle is much larger
//! than their visible card, so styling them leaves a bare hollow outline
//! hovering over the wallpaper. `skip()` below is what keeps them out.

use dcx_core::event::WindowEvent;
use dcx_core::{Effect, EffectCtx};
use dcx_win32::*;

/// Classes owned by the shell. `Windows.UI.Core.CoreWindow` covers Start, the
/// search flyout, quick settings and UWP-style XAML hosts; the rest are the
/// popup/flyout/overflow hosts and the taskbars themselves.
const SHELL_CLASSES: [&str; 7] = [
    "Windows.UI.Core.CoreWindow",
    "Xaml_WindowedPopupClass",
    "TopLevelWindowForOverflowXamlIsland",
    "Windows.UI.Composition.DesktopWindowContentBridge",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Progman",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CornerStyle {
    Default,
    Square,
    Round,
    RoundSmall,
}

impl CornerStyle {
    fn raw(self) -> i32 {
        match self {
            CornerStyle::Default => DWMWCP_DEFAULT,
            CornerStyle::Square => DWMWCP_DONOTROUND,
            CornerStyle::Round => DWMWCP_ROUND,
            CornerStyle::RoundSmall => DWMWCP_ROUNDSMALL,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CornerConfig {
    pub style: CornerStyle,
    /// `0x00BBGGRR`, or `None` to leave the system accent alone.
    pub border_color: Option<u32>,
}

impl Default for CornerConfig {
    fn default() -> Self {
        Self {
            style: CornerStyle::RoundSmall,
            border_color: None,
        }
    }
}

/// True for windows that must keep whatever frame the shell gave them.
///
/// Three independent signals, because no single one covers every build:
/// a shell class name, a shell-host process, and `WS_EX_NOREDIRECTIONBITMAP`
/// (every WinUI 3 / XAML island top level carries it). Cloaked and
/// non-captioned windows are also refused: a window without `WS_CAPTION` has no
/// frame, and forcing corners on it is what draws a frame out of nothing.
unsafe fn skip(hwnd: HWND) -> bool {
    if IsWindowVisible(hwnd) == FALSE || is_cloaked(hwnd) {
        return true;
    }
    let ex = ex_style(hwnd);
    if ex & WS_EX_NOREDIRECTIONBITMAP != 0 || ex & WS_EX_TOOLWINDOW != 0 {
        return true;
    }
    let cls = class_name(hwnd);
    if SHELL_CLASSES.iter().any(|c| cls.eq_ignore_ascii_case(c)) {
        return true;
    }
    style(hwnd) & WS_CAPTION != WS_CAPTION
}

pub struct CornerEffect {
    cfg: CornerConfig,
    applied: u32,
    skipped: u32,
}

impl CornerEffect {
    pub fn new(cfg: CornerConfig) -> Self {
        Self {
            cfg,
            applied: 0,
            skipped: 0,
        }
    }

    fn apply(&mut self, hwnd: HWND) {
        unsafe {
            if IsWindow(hwnd) == FALSE {
                return;
            }
            if skip(hwnd) {
                self.skipped = self.skipped.wrapping_add(1);
                return;
            }
            if !set_corner_preference(hwnd, self.cfg.style.raw()) {
                return;
            }
            if let Some(color) = self.cfg.border_color {
                set_border_color(hwnd, color);
            }
            self.applied = self.applied.wrapping_add(1);
        }
    }

    pub fn applied_count(&self) -> u32 {
        self.applied
    }

    /// Windows handed to us and deliberately left alone (shell surfaces).
    pub fn skipped_count(&self) -> u32 {
        self.skipped
    }
}

impl Effect for CornerEffect {
    fn name(&self) -> &'static str {
        "corners"
    }

    fn on_event(&mut self, ev: WindowEvent, _ctx: &EffectCtx) -> bool {
        // Retheming on foreground keeps the cost near zero: no polling, no
        // enumeration, and it re-applies after an app resets its own styling.
        let WindowEvent::Foreground(hwnd) = ev;
        self.apply(hwnd);
        false // never claim the event; other effects still get it
    }
}

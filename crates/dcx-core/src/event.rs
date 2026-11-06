//! Window events delivered by the platform.

use dcx_win32::HWND;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowEvent {
    /// A top-level window became foreground.
    Foreground(HWND),
}

impl WindowEvent {
    pub fn hwnd(&self) -> HWND {
        match self {
            WindowEvent::Foreground(h) => *h,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            WindowEvent::Foreground(_) => "foreground",
        }
    }
}

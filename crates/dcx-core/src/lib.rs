//! DCX host platform: window-event plumbing, effect registry, and the frame loop.
//!
//! The design goal is that adding a new desktop effect means writing one new
//! crate that implements [`Effect`] and registering it in `dcx-host`. Nothing
//! in this crate knows about any concrete effect.

pub mod effect;
pub mod event;
pub mod geom;
pub mod platform;

pub use effect::{Effect, EffectCtx};
pub use event::WindowEvent;
pub use platform::Platform;

use core::sync::atomic::{AtomicBool, Ordering};

/// Global kill switch, honoured by long-running effect code.
pub static RUNNING: AtomicBool = AtomicBool::new(true);

pub fn stop() {
    RUNNING.store(false, Ordering::SeqCst);
}

pub fn is_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

/// Minimal timestamped logger. Replace with `tracing` when the product grows.
#[macro_export]
macro_rules! dcx_log {
    ($($arg:tt)*) => {{
        let ms = $crate::platform::now_ms();
        println!("[{:>8} ms] {}", ms, format_args!($($arg)*));
    }};
}

//! Text recognition for a captured region.
//!
//! The default engine is `Windows.Media.Ocr` (local, no model download). The
//! user may instead configure an OpenAI-compatible vision endpoint or a custom
//! HTTP service in the settings centre; `providers` owns that dispatch.

mod card;
mod win;

pub use card::{is_active, request_close};
pub(crate) use win::recognize as windows_recognize;

/// Recognize the selection through the configured provider. Returns the text
/// and a badge for the result card ("离线 OCR" / "在线 OCR").
pub fn recognize(bgra: Vec<u8>, w: u32, h: u32) -> Result<(String, String), String> {
    crate::providers::ocr(bgra, w, h)
}

/// Show the OCR result card and block until it is closed.
pub fn run(
    text: String,
    badge: String,
    anchor: (i32, i32),
    monitor: (i32, i32, i32, i32),
    scale: f32,
    translate_now: bool,
) -> Result<(), String> {
    card::run(card::Ctx {
        text,
        badge,
        anchor,
        monitor,
        scale,
        translate_now,
    })
}

/// Offscreen card render for `--demo-synth`.
pub fn demo(out: &std::path::Path) -> Result<(), String> {
    card::demo(out)
}

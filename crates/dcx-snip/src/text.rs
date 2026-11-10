//! Text runs measured and rasterised by GDI.
//!
//! Two reasons not to ship a font rasteriser: the overlay must accept **IME
//! input** (the popup edit control does that for free) and it must render CJK
//! with the user's own hinting. GDI already does both.
//!
//! The glyphs are turned into an **alpha coverage mask** rather than being
//! painted straight onto the frame, so a text object can sit anywhere in the
//! vector z-order and still composite correctly.
//!
//! Every cache key carries the [`TextStyle`]: the same string in bold, italic
//! or another family has a different width and a different mask, and reusing
//! the plain-family entry would make the selection box disagree with the ink.

use crate::ffi::*;
use crate::model::TextStyle;
use dcx_win32::{HGDIOBJ, SIZE};
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// Font handles are reused for the life of the process: creating one per
    /// frame was visible as a stutter while dragging a selection over text.
    /// The key is (height, face, bold, italic) — a handful of faces times the
    /// sizes the user actually picked.
    static FONTS: RefCell<HashMap<(i32, String, bool, bool), HGDIOBJ>> =
        RefCell::new(HashMap::new());
    static MEASURE: RefCell<HashMap<(String, i32, String, bool, bool), (f32, f32)>> =
        RefCell::new(HashMap::new());
    /// Chrome labels are re-drawn every frame; rasterising them through GDI
    /// each time cost more than the rest of the frame together.
    static COVERAGE:
        RefCell<HashMap<(String, i32, String, bool, bool, bool), std::rc::Rc<Coverage>>> =
        RefCell::new(HashMap::new());
}

const FALLBACK: &str = "Segoe UI";

fn font(height: i32, style: &TextStyle) -> HGDIOBJ {
    let key = (height, style.face.clone(), style.bold, style.italic);
    FONTS.with(|f| {
        let mut map = f.borrow_mut();
        if let Some(h) = map.get(&key) {
            return *h;
        }
        let weight = if style.bold { FW_BOLD } else { FW_NORMAL };
        let italic = if style.italic { 1 } else { 0 };
        let mut handle = unsafe { make_font(height, weight, italic, &style.face) };
        if handle == 0 {
            handle = unsafe { make_font(height, weight, italic, FALLBACK) };
        }
        map.insert(key, handle);
        handle
    })
}

unsafe fn make_font(height: i32, weight: i32, italic: u32, face: &str) -> HGDIOBJ {
    let face = dcx_win32::wide(face);
    unsafe {
        CreateFontW(
            -height,
            0,
            0,
            0,
            weight,
            italic,
            0,
            0,
            DEFAULT_CHARSET,
            0,
            0,
            ANTIALIASED,
            0,
            face.as_ptr(),
        )
    }
}

const ANTIALIASED: u32 = 4;

fn scratch_dc() -> dcx_win32::HDC {
    unsafe {
        let screen = dcx_win32::GetDC(0);
        let dc = dcx_win32::CreateCompatibleDC(screen);
        dcx_win32::ReleaseDC(0, screen);
        dc
    }
}

/// Logical size in physical pixels → the pixel height used for the font.
fn px_height(size: f32) -> i32 {
    (size.max(6.0)).round() as i32
}

/// Width and line height of `text` in the overlay's own chrome font.
pub fn measure(text: &str, size: f32) -> (f32, f32) {
    measure_styled(text, size, &TextStyle::ui())
}

/// Width and line height of `text`, in physical pixels, with `style` applied.
pub fn measure_styled(text: &str, size: f32, style: &TextStyle) -> (f32, f32) {
    let h = px_height(size);
    let key = (
        text.to_string(),
        h,
        style.face.clone(),
        style.bold,
        style.italic,
    );
    if let Some(v) = MEASURE.with(|m| m.borrow().get(&key).copied()) {
        return v;
    }
    let wide: Vec<u16> = text.encode_utf16().collect();
    let dc = scratch_dc();
    if dc == 0 {
        return (text.chars().count() as f32 * size * 0.55, size * 1.3);
    }
    let old = unsafe { dcx_win32::SelectObject(dc, font(h, style)) };
    let mut sz = SIZE::default();
    let ok = unsafe { GetTextExtentPoint32W(dc, wide.as_ptr(), wide.len() as i32, &mut sz) } != 0;
    unsafe {
        dcx_win32::SelectObject(dc, old);
        dcx_win32::DeleteDC(dc);
    }
    let v = if ok {
        (sz.cx as f32, sz.cy as f32)
    } else {
        (text.chars().count() as f32 * size * 0.55, size * 1.3)
    };
    MEASURE.with(|m| {
        let mut map = m.borrow_mut();
        if map.len() > 512 {
            map.clear();
        }
        map.insert(key, v);
    });
    v
}

/// Alpha coverage of `text` at `size`, plus the pixel offset of the mask's
/// top-left corner relative to the text anchor.
pub struct Coverage {
    pub w: u32,
    pub h: u32,
    pub alpha: Vec<u8>,
    /// How far the mask extends past the anchor on every side. Italic overhang
    /// and a drop shadow both need more room than the plain 3 px bleed.
    pub bleed: f32,
}

pub fn coverage_styled(text: &str, size: f32, style: &TextStyle) -> Option<std::rc::Rc<Coverage>> {
    if text.is_empty() {
        return None;
    }
    let key = (
        text.to_string(),
        px_height(size),
        style.face.clone(),
        style.bold,
        style.italic,
        style.shadow,
    );
    if let Some(hit) = COVERAGE.with(|c| c.borrow().get(&key).cloned()) {
        return Some(hit);
    }
    let made = coverage_raw(text, size, style)?;
    let rc = std::rc::Rc::new(made);
    // The drag size readout produces a new string every frame; a 48-entry
    // cache used to clear itself mid-drag and re-rasterise every label. 384
    // entries is a few hundred KB and outlives any one session.
    COVERAGE.with(|c| {
        let mut map = c.borrow_mut();
        if map.len() > 384 {
            map.clear();
        }
        map.insert(key, rc.clone());
    });
    Some(rc)
}

/// Hard-shadow offset for a text size, in physical pixels. Tied to the glyph
/// height so a 12 px caption and a 96 px headline both look right, and capped
/// so the mask stays small.
pub fn shadow_offset(size: f32) -> f32 {
    (size * 0.07).clamp(1.0, 6.0).round()
}

fn coverage_raw(text: &str, size: f32, style: &TextStyle) -> Option<Coverage> {
    let (tw, th) = measure_styled(text, size, style);
    // The mask must contain the shadow, otherwise the offset copy is clipped
    // at the mask border and the shadow turns into a hard-edged ghost.
    let bleed = BLEED
        + if style.shadow {
            shadow_offset(size)
        } else {
            0.0
        };
    let pad = bleed as i32;
    let w = (tw.ceil() as i32 + pad * 2).max(1);
    let h = (th.ceil() as i32 + pad * 2).max(1);

    unsafe {
        let dc = scratch_dc();
        if dc == 0 {
            return None;
        }
        let mut info = dcx_win32::BITMAPINFO::default();
        info.bmiHeader = dcx_win32::BITMAPINFOHEADER {
            biSize: core::mem::size_of::<dcx_win32::BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: dcx_win32::BI_RGB,
            biSizeImage: (w as u32) * (h as u32) * 4,
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
        let dib = dcx_win32::CreateDIBSection(dc, &info, 0, &mut bits, 0, 0);
        if dib == 0 || bits.is_null() {
            dcx_win32::DeleteDC(dc);
            return None;
        }
        let old_bmp = dcx_win32::SelectObject(dc, dib);
        let old_font = dcx_win32::SelectObject(dc, font(px_height(size), style));

        // Black background, white glyphs: the red channel becomes the mask.
        let black = CreateSolidBrush(0);
        let full = dcx_win32::RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        FillRect(dc, &full, black);
        dcx_win32::DeleteObject(black);

        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, 0x00FF_FFFF);
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut rc = dcx_win32::RECT {
            left: pad,
            top: pad,
            right: w - pad,
            bottom: h - pad,
        };
        DrawTextW(
            dc,
            wide.as_ptr(),
            wide.len() as i32,
            &mut rc,
            DT_LEFT | DT_TOP | DT_SINGLELINE | DT_NOPREFIX,
        );
        GdiFlush();

        let n = (w as usize) * (h as usize);
        let src = core::slice::from_raw_parts(bits as *const u8, n * 4);
        let mut alpha = vec![0u8; n];
        for i in 0..n {
            alpha[i] = src[i * 4]; // red channel = coverage
        }

        dcx_win32::SelectObject(dc, old_font);
        dcx_win32::SelectObject(dc, old_bmp);
        dcx_win32::DeleteObject(dib);
        dcx_win32::DeleteDC(dc);

        Some(Coverage {
            w: w as u32,
            h: h as u32,
            alpha,
            bleed,
        })
    }
}

/// Truncate `body` to `max_w` physical pixels with an ellipsis. Counting
/// characters is wrong for mixed CJK/Latin (a CJK glyph is ~2× a Latin one), so
/// the cut is measured per glyph.
pub fn fit_px(body: &str, size: f32, style: &TextStyle, max_w: f32) -> String {
    if measure_styled(body, size, style).0 <= max_w {
        return body.to_string();
    }
    let mut out = String::new();
    for ch in body.chars() {
        let mut probe = out.clone();
        probe.push(ch);
        probe.push('…');
        if measure_styled(&probe, size, style).0 > max_w {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

/// Where the mask's top-left sits relative to the text anchor for a run with
/// no shadow (the mask has a 3 px bleed).
pub const BLEED: f32 = 3.0;

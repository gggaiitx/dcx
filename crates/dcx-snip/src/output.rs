//! Export: clipboard and files.
//!
//! The clipboard carries **two** formats at once: `CF_DIB` (what Word, WeChat
//! and every Win32 app reads) and a registered `PNG` stream (what Chromium,
//! Slack and most modern editors prefer). Publishing both costs one extra copy
//! and removes a whole class of "pastes as a black rectangle" reports.
//!
//! `CF_DIB` is bottom-up by definition: `biHeight` must be positive and the
//! rows are stored in reverse order. Getting this wrong produces a vertically
//! mirrored image that looks fine in Paint and upside-down everywhere else.

use crate::ffi::*;
use crate::surface::Rect;
use std::path::{Path, PathBuf};

/// Crop a top-down BGRA frame, keeping the channel order. Returns the exact
/// pixel size, which is **not** `r.width()`: the rect is float and is snapped
/// to whole pixels, so callers that re-derive the size end up one pixel off and
/// the PNG encoder rejects the buffer.
pub fn crop(bgra: &[u8], stride: usize, w: u32, h: u32, r: &Rect) -> Option<(Vec<u8>, u32, u32)> {
    let (x0, y0, x1, y1) = r.to_px(w, h)?;
    let cw = (x1 - x0) as usize;
    let ch = (y1 - y0) as usize;
    let mut out = vec![0u8; cw * ch * 4];
    for y in 0..ch {
        let src = (y0 as usize + y) * stride + x0 as usize * 4;
        let dst = y * cw * 4;
        if src + cw * 4 <= bgra.len() {
            out[dst..dst + cw * 4].copy_from_slice(&bgra[src..src + cw * 4]);
        }
    }
    Some((out, cw as u32, ch as u32))
}

fn png_bytes(bgra: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    let mut rgba = Vec::with_capacity(bgra.len());
    for px in bgra.chunks_exact(4) {
        rgba.push(px[2]);
        rgba.push(px[1]);
        rgba.push(px[0]);
        // Corner rounding writes alpha 0; a hard-coded 255 here silently
        // squared every rounded export.
        rgba.push(px[3]);
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| format!("PNG 头写入失败：{e}"))?;
        writer
            .write_image_data(&rgba)
            .map_err(|e| format!("PNG 数据写入失败：{e}"))?;
    }
    Ok(out)
}

/// Round the exported corners. Fully outside pixels become white with alpha 0
/// (alpha-aware targets show transparency; `CF_DIB` consumers that ignore the
/// alpha byte at least get clean white corners instead of black).
pub fn round_alpha(bgra: &mut [u8], w: u32, h: u32, radius: f32) {
    if radius <= 0.5 || w < 2 || h < 2 {
        return;
    }
    let r = radius.min(w as f32 * 0.5).min(h as f32 * 0.5);
    let ri = r.ceil() as i32;
    if ri < 1 {
        return;
    }
    let stride = (w as usize) * 4;
    let mut set = |px: usize, py: usize, cov: f32| {
        let off = py * stride + px * 4;
        if off + 3 >= bgra.len() {
            return;
        }
        if cov <= 0.001 {
            bgra[off] = 255;
            bgra[off + 1] = 255;
            bgra[off + 2] = 255;
            bgra[off + 3] = 0;
        } else if cov < 0.999 {
            bgra[off + 3] = (cov * 255.0).round() as u8;
        }
    };
    for yy in 0..ri as usize {
        for xx in 0..ri as usize {
            let dx = (r - (xx as f32 + 0.5)).abs();
            let dy = (r - (yy as f32 + 0.5)).abs();
            let dist = (dx * dx + dy * dy).sqrt();
            let cov = (r - dist + 0.5).clamp(0.0, 1.0);
            set(xx, yy, cov);
            set(w as usize - 1 - xx, yy, cov);
            set(xx, h as usize - 1 - yy, cov);
            set(w as usize - 1 - xx, h as usize - 1 - yy, cov);
        }
    }
}

fn dib_bytes(bgra: &[u8], w: u32, h: u32) -> Vec<u8> {
    let header = 40usize;
    let image = (w as usize) * (h as usize) * 4;
    let mut out = vec![0u8; header + image];
    // BITMAPINFOHEADER
    out[0..4].copy_from_slice(&40u32.to_le_bytes());
    out[4..8].copy_from_slice(&(w as i32).to_le_bytes());
    // Positive height = bottom-up storage.
    out[8..12].copy_from_slice(&(h as i32).to_le_bytes());
    out[12..14].copy_from_slice(&1u16.to_le_bytes());
    out[14..16].copy_from_slice(&32u16.to_le_bytes());
    out[16..20].copy_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out[20..24].copy_from_slice(&(image as u32).to_le_bytes());

    let row = (w as usize) * 4;
    for y in 0..h as usize {
        let src = y * row;
        let dst = header + (h as usize - 1 - y) * row;
        out[dst..dst + row].copy_from_slice(&bgra[src..src + row]);
    }
    out
}

fn set_clipboard(bytes: &[u8], format: u32) -> Result<(), String> {
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
        if mem == 0 {
            return Err("GlobalAlloc 失败".to_string());
        }
        let ptr = GlobalLock(mem) as *mut u8;
        if ptr.is_null() {
            GlobalFree(mem);
            return Err("GlobalLock 失败".to_string());
        }
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        GlobalUnlock(mem);
        if SetClipboardData(format, mem) == 0 {
            GlobalFree(mem);
            return Err("SetClipboardData 失败".to_string());
        }
        // Ownership moved to the clipboard; must NOT be freed here.
    }
    Ok(())
}

/// Put the cropped frame on the clipboard as `CF_DIB` + `PNG`.
pub fn to_clipboard(hwnd: dcx_win32::HWND, bgra: &[u8], w: u32, h: u32) -> Result<(), String> {
    if w == 0 || h == 0 {
        return Err("选区为空".to_string());
    }
    let dib = dib_bytes(bgra, w, h);
    let png = png_bytes(bgra, w, h)?;

    unsafe {
        // Another process can hold the clipboard for a few hundred ms while it
        // is pasting; retry instead of silently losing the copy.
        let mut opened = false;
        for _ in 0..12 {
            if OpenClipboard(hwnd) != 0 {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if !opened {
            return Err("剪贴板被其他程序占用".to_string());
        }
        EmptyClipboard();
        let r1 = set_clipboard(&dib, CF_DIB);
        let name = dcx_win32::wide("PNG");
        let png_format = RegisterClipboardFormatW(name.as_ptr());
        let r2 = if png_format != 0 {
            set_clipboard(&png, png_format)
        } else {
            Ok(())
        };
        CloseClipboard();
        r1?;
        r2?;
    }
    Ok(())
}

/// `DCX截图-20261110-113045.png`
pub fn file_name(jpeg: bool) -> String {
    let stamp = now_stamp();
    let ext = if jpeg { "jpg" } else { "png" };
    format!("DCX截图-{stamp}.{ext}")
}

fn now_stamp() -> String {
    let mut t = dcx_win32::SYSTEMTIME::default();
    unsafe {
        GetLocalTime(&mut t);
    }
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

/// Write a raw BGRA frame as PNG (no clipboard, no naming template).
pub fn write_png(path: &Path, bgra: &[u8], w: u32, h: u32) -> Result<(), String> {
    let bytes = png_bytes(bgra, w, h)?;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, bytes).map_err(|e| format!("无法写入：{e}"))
}

pub fn save(
    dir: &Path,
    bgra: &[u8],
    w: u32,
    h: u32,
    jpeg: bool,
    quality: u8,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("无法创建目录 {}：{e}", dir.display()))?;
    let mut path = dir.join(file_name(jpeg));

    // Never overwrite: two shots in the same second are common.
    let mut n = 2;
    while path.exists() && n < 1000 {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ext = if jpeg { "jpg" } else { "png" };
        path = dir.join(format!("{stem}-{n}.{ext}"));
        n += 1;
    }

    if jpeg {
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for px in bgra.chunks_exact(4) {
            // JPEG has no alpha; flattened rounded corners go white.
            let a = px[3] as u32;
            let inv = 255 - a;
            let mix = |c: u8| ((c as u32 * a + 255 * inv) / 255) as u8;
            rgb.push(mix(px[2]));
            rgb.push(mix(px[1]));
            rgb.push(mix(px[0]));
        }
        let file = std::fs::File::create(&path).map_err(|e| format!("无法写入：{e}"))?;
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(
            std::io::BufWriter::new(file),
            quality.clamp(40, 100),
        );
        enc.encode(&rgb, w, h, image::ExtendedColorType::Rgb8)
            .map_err(|e| format!("JPEG 编码失败：{e}"))?;
    } else {
        let bytes = png_bytes(bgra, w, h)?;
        std::fs::write(&path, bytes).map_err(|e| format!("无法写入：{e}"))?;
    }
    Ok(path)
}

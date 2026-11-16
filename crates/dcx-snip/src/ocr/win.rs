//! `Windows.Media.Ocr` wrapper.
//!
//! The recognizer ships with Windows; there is no model to download and no
//! network access. A language pack must be installed (this machine has
//! zh-Hans-CN, which also reads Latin text). `RoUninitialize` is deliberately
//! never called: windows-rs caches activation factories, and tearing the
//! apartment down under them has crashed a previous tool in this workspace.

use windows::core::{Interface, HSTRING};
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::Buffer;
use windows::Win32::System::WinRT::{IBufferByteAccess, RoInitialize, RO_INIT_MULTITHREADED};

/// Recognize `bgra` (top-down, BGRA) and return reading-ordered text.
/// Dark screenshots are inverted first: the Windows engine is much better on
/// dark-on-light than on light-on-dark, and terminals/dark pages are common.
pub fn recognize(mut bgra: Vec<u8>, mut w: u32, mut h: u32) -> Result<String, String> {
    if w < 2 || h < 2 || bgra.len() != (w as usize) * (h as usize) * 4 {
        return Err("OCR 图像尺寸无效".to_string());
    }
    // A crop this large would be copied three times on the way into WinRT.
    if (w as u64) * (h as u64) > 36_000_000 {
        return Err("选区太大（超过约 3600 万像素），请缩小后重试".to_string());
    }
    // Small UI text is where the engine drops spaces and misreads o/0. A 2×
    // nearest-neighbour pass costs little and fixes most of it; large captures
    // are left alone so OCR stays fast.
    if (w as u64) * (h as u64) <= 2_500_000 {
        bgra = upscale2(&bgra, w, h);
        w *= 2;
        h *= 2;
    }
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }

    if mean_luma(&bgra) < 96 {
        for px in bgra.chunks_exact_mut(4) {
            px[0] = 255 - px[0];
            px[1] = 255 - px[1];
            px[2] = 255 - px[2];
            px[3] = 255;
        }
    }

    let len = bgra.len() as u32;
    let buffer = Buffer::Create(len).map_err(|e| format!("创建 OCR 缓冲区失败：{e}"))?;
    buffer
        .SetLength(len)
        .map_err(|e| format!("设置 OCR 缓冲区失败：{e}"))?;
    let access: IBufferByteAccess = buffer
        .cast()
        .map_err(|e| format!("访问 OCR 缓冲区失败：{e}"))?;
    unsafe {
        let dst = access
            .Buffer()
            .map_err(|e| format!("锁定 OCR 缓冲区失败：{e}"))?;
        core::ptr::copy_nonoverlapping(bgra.as_ptr(), dst, bgra.len());
    }
    let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        w as i32,
        h as i32,
        BitmapAlphaMode::Premultiplied,
    )
    .map_err(|e| format!("创建 OCR 位图失败：{e}"))?;

    let engine = create_engine()?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(|e| format!("启动 OCR 失败：{e}"))?
        .join()
        .map_err(|e| format!("OCR 识别失败：{e}"))?;

    let lines = result
        .Lines()
        .map_err(|e| format!("读取 OCR 行失败：{e}"))?;
    let mut items: Vec<(f32, f32, usize, String)> = Vec::new();
    for (index, line) in lines.into_iter().enumerate() {
        let text = line.Text().map(|t| t.to_string()).unwrap_or_default();
        if text.trim().is_empty() {
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        if let Ok(words) = line.Words() {
            for word in words {
                if let Ok(r) = word.BoundingRect() {
                    x0 = x0.min(r.X);
                    y0 = y0.min(r.Y);
                    x1 = x1.max(r.X + r.Width);
                    y1 = y1.max(r.Y + r.Height);
                }
            }
        }
        if x0 == f32::MAX {
            // No word boxes (rare): keep the engine order.
            x0 = 0.0;
            y0 = index as f32 * 100.0;
            y1 = y0;
        }
        items.push(((y0 + y1) * 0.5, x0, index, text));
    }
    if items.is_empty() {
        return Ok(result.Text().map(|t| t.to_string()).unwrap_or_default());
    }
    // `Lines()` is not guaranteed to be in reading order; sort by visual row,
    // then by horizontal position. Lines sharing a row (columns) join with a
    // space instead of a newline.
    items.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let mut out = String::new();
    let mut row_y = f32::NAN;
    for (cy, _, _, text) in items {
        if out.is_empty() {
            out.push_str(text.trim());
        } else if (cy - row_y).abs() <= 12.0 {
            out.push(' ');
            out.push_str(text.trim());
        } else {
            out.push('\n');
            out.push_str(text.trim());
        }
        row_y = cy;
    }
    Ok(out)
}

fn upscale2(src: &[u8], w: u32, h: u32) -> Vec<u8> {
    let sw = w as usize;
    let sh = h as usize;
    let nw = sw * 2;
    let mut out = vec![0u8; src.len() * 4];
    for y in 0..sh {
        for x in 0..sw {
            let o = (y * sw + x) * 4;
            let px = [src[o], src[o + 1], src[o + 2], src[o + 3]];
            let d = (y * 2 * nw + x * 2) * 4;
            out[d..d + 4].copy_from_slice(&px);
            out[d + 4..d + 8].copy_from_slice(&px);
            let d2 = d + nw * 4;
            out[d2..d2 + 4].copy_from_slice(&px);
            out[d2 + 4..d2 + 8].copy_from_slice(&px);
        }
    }
    out
}

fn mean_luma(bgra: &[u8]) -> u32 {
    let mut sum = 0u64;
    let mut count = 0u64;
    for px in bgra.chunks_exact(16) {
        // Sample one pixel per 16 bytes (four pixels) to keep the scan cheap.
        let b = px[0] as u32;
        let g = px[1] as u32;
        let r = px[2] as u32;
        sum += ((77 * r + 150 * g + 29 * b) >> 8) as u64;
        count += 1;
    }
    if count == 0 {
        255
    } else {
        (sum / count) as u32
    }
}

fn create_engine() -> Result<OcrEngine, String> {
    if let Ok(engine) = OcrEngine::TryCreateFromUserProfileLanguages() {
        return Ok(engine);
    }
    for tag in ["zh-Hans-CN", "en-US"] {
        if let Ok(language) = Language::CreateLanguage(&HSTRING::from(tag)) {
            if let Ok(engine) = OcrEngine::TryCreateFromLanguage(&language) {
                return Ok(engine);
            }
        }
    }
    Err("系统未安装 OCR 语言包（设置 → 时间和语言 → 语言 → 光学字符识别）".to_string())
}

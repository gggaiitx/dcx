//! Shell icons for the file-box rows.
//!
//! `SHGetFileInfoW` hands back an `HICON` for a path; that is the only way to get
//! the *real* icon — the Office "W", the shortcut arrow, the folder — instead of a
//! coloured badge with the extension in it. The icon is converted to RGBA here and
//! uploaded as an egui texture.
//!
//! Caching is by *extension* for ordinary files (a hundred `.docx` share one
//! texture) and by full path for the types whose icon is per-file: shortcuts and
//! executables carry their target's artwork, so keying them by extension would
//! paint every `.lnk` with the same generic glyph.

use std::collections::HashMap;
use std::path::Path;

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

use dcx_win32 as w;

/// Icons are asked for at 32 px and drawn at ~19 pt; the extra resolution keeps
/// them sharp at 125 % / 150 %.
const ICON_PX: i32 = 32;

/// Per-file icons (shortcuts, executables) are keyed by path, so a long-lived
/// box would keep one texture per file it has ever shown. Above this many
/// entries the cache is dropped wholesale; reloading a visible row costs one
/// `SHGetFileInfoW` call.
const MAX_ENTRIES: usize = 192;

pub struct IconCache {
    map: HashMap<String, Option<TextureHandle>>,
}

impl IconCache {
    pub fn new() -> Self {
        IconCache {
            map: HashMap::new(),
        }
    }

    /// Texture for one row, loading it on first use.
    pub fn get(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
        ext: &str,
        is_dir: bool,
    ) -> Option<TextureHandle> {
        let key = cache_key(path, ext, is_dir);
        if !self.map.contains_key(&key) {
            if self.map.len() >= MAX_ENTRIES {
                self.map.clear();
            }
            let tex = load(ctx, path, ext, is_dir, &key);
            self.map.insert(key.clone(), tex);
        }
        self.map.get(&key).and_then(|t| t.clone())
    }

    /// Drop everything — used when the theme flips, so the textures are rebuilt
    /// against the new background if the shell ever returns dark-mode icons.
    pub fn clear(&mut self) {
        self.map.clear();
    }
}

fn cache_key(path: &Path, ext: &str, is_dir: bool) -> String {
    if is_dir {
        return "<dir>".to_string();
    }
    match ext {
        // Per-file artwork: the icon belongs to the target, not to the extension.
        "lnk" | "exe" | "url" | "ico" | "msi" => format!("p:{}", path.display()),
        "" => "<none>".to_string(),
        e => e.to_string(),
    }
}

fn load(
    ctx: &egui::Context,
    path: &Path,
    ext: &str,
    is_dir: bool,
    key: &str,
) -> Option<TextureHandle> {
    let image = unsafe { image_for(path, ext, is_dir) }?;
    Some(ctx.load_texture(format!("dcx-icon:{key}"), image, TextureOptions::LINEAR))
}

unsafe fn image_for(path: &Path, ext: &str, is_dir: bool) -> Option<ColorImage> {
    // A shortcut's icon lives on its target, so the shell has to look at the
    // actual file; asking with `USEFILEATTRIBUTES` would return the generic link
    // glyph for every one of them.
    let per_file = matches!(ext, "lnk" | "exe" | "url" | "ico" | "msi");
    let attrs = if is_dir {
        0x0000_0010 // FILE_ATTRIBUTE_DIRECTORY
    } else {
        w::FILE_ATTRIBUTE_NORMAL
    };
    let mut flags = w::SHGFI_ICON | w::SHGFI_LARGEICON;
    if !per_file {
        // Extension-only lookup: no disk I/O, works for files that are gone.
        flags |= w::SHGFI_USEFILEATTRIBUTES;
    }

    let wide = dcx_win32::wide(&path.to_string_lossy());
    let mut info = w::SHFILEINFOW::default();
    let ok = w::SHGetFileInfoW(
        wide.as_ptr(),
        attrs,
        &mut info,
        core::mem::size_of::<w::SHFILEINFOW>() as u32,
        flags,
    );
    if ok == 0 || info.hIcon == 0 {
        return None;
    }
    let image = rgba_of(info.hIcon);
    w::DestroyIcon(info.hIcon);
    image
}

/// `HICON` → RGBA pixels. The colour bitmap carries its own alpha for modern
/// icons; older ones are 24 bpp and only describe transparency through the
/// monochrome mask, which is used as a fallback.
unsafe fn rgba_of(icon: isize) -> Option<ColorImage> {
    let mut ii = w::ICONINFO::default();
    if w::GetIconInfo(icon, &mut ii) == 0 {
        return None;
    }
    let out = rgba_of_bitmaps(&ii);
    if ii.hbmColor != 0 {
        w::DeleteObject(ii.hbmColor);
    }
    if ii.hbmMask != 0 {
        w::DeleteObject(ii.hbmMask);
    }
    out
}

unsafe fn rgba_of_bitmaps(ii: &w::ICONINFO) -> Option<ColorImage> {
    let (wpx, hpx) = bitmap_size(ii.hbmColor)?;
    if !(8..=256).contains(&wpx) || !(8..=256).contains(&hpx) {
        return None;
    }
    let mut px = dib_bits(ii.hbmColor, wpx, hpx)?;

    // A 32 bpp icon comes with alpha. If every alpha byte is zero the icon is
    // really 24 bpp (XOR colour + mask), and the mask decides transparency.
    if px.chunks_exact(4).all(|p| p[3] == 0) {
        let mask = dib_bits(ii.hbmMask, wpx, hpx)?;
        for (p, m) in px.chunks_exact_mut(4).zip(mask.chunks_exact(4)) {
            // Mask is white where the pixel is transparent.
            p[3] = if m[0] >= 0x80 { 0 } else { 255 };
        }
    }

    // The buffer is BGRA; egui wants RGBA with straight alpha.
    let mut rgba = Vec::with_capacity((wpx * hpx * 4) as usize);
    for p in px.chunks_exact(4) {
        rgba.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
    }
    Some(ColorImage::from_rgba_unmultiplied(
        [wpx as usize, hpx as usize],
        &rgba,
    ))
}

unsafe fn bitmap_size(bmp: isize) -> Option<(i32, i32)> {
    if bmp == 0 {
        return None;
    }
    let mut bm = w::BITMAP::default();
    let got = w::GetObjectW(
        bmp,
        core::mem::size_of::<w::BITMAP>() as i32,
        &mut bm as *mut _ as *mut core::ffi::c_void,
    );
    if got == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
        return None;
    }
    Some((bm.bmWidth, bm.bmHeight))
}

/// Copy a DDB into a top-down 32 bpp buffer (BGRA, Windows byte order).
unsafe fn dib_bits(bmp: isize, wpx: i32, hpx: i32) -> Option<Vec<u8>> {
    if bmp == 0 {
        return None;
    }
    let dc = w::CreateCompatibleDC(0);
    if dc == 0 {
        return None;
    }
    let old = w::SelectObject(dc, bmp);
    let mut bmi = w::BITMAPINFO::default();
    bmi.bmiHeader.biSize = core::mem::size_of::<w::BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = wpx;
    bmi.bmiHeader.biHeight = -hpx; // top-down
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = 0; // BI_RGB
    let mut buf = vec![0u8; (wpx * hpx * 4) as usize];
    let lines = w::GetDIBits(
        dc,
        bmp,
        0,
        hpx as u32,
        buf.as_mut_ptr() as *mut core::ffi::c_void,
        &mut bmi,
        w::DIB_RGB_COLORS,
    );
    w::SelectObject(dc, old);
    w::DeleteDC(dc);
    if lines == 0 {
        return None;
    }
    Some(buf)
}

/// Is the icon list still meaningful at this size? Kept for the caller that wants
/// to fall back to the lettered badge.
pub fn nominal_px() -> i32 {
    ICON_PX
}

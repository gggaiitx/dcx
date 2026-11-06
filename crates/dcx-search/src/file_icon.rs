//! Shell file-type icons for result rows.
//!
//! The result table asks for the real icon the shell would show in Explorer
//! (Word's W, a PDF, a PNG thumbnail glyph) instead of a generic sheet. The
//! extraction path is the same one the desktop box uses: `SHGetFileInfoW` with
//! `SHGFI_USEFILEATTRIBUTES` for ordinary files (extension-only, no disk I/O)
//! and the real path for types whose artwork belongs to the file itself.

use std::collections::HashMap;
use std::path::Path;

use dcx_win32 as w;
use egui::{ColorImage, TextureHandle, TextureOptions};

pub struct IconCache {
    map: HashMap<String, Option<TextureHandle>>,
}

impl IconCache {
    pub fn new() -> Self {
        IconCache {
            map: HashMap::new(),
        }
    }

    pub fn get(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
        ext: &str,
        is_dir: bool,
    ) -> Option<TextureHandle> {
        let key = cache_key(path, ext, is_dir);
        if !self.map.contains_key(&key) {
            let tex = load(ctx, path, ext, is_dir, &key);
            self.map.insert(key.clone(), tex);
        }
        self.map.get(&key).and_then(|t| t.clone())
    }
}

fn cache_key(path: &Path, ext: &str, is_dir: bool) -> String {
    if is_dir {
        return "<dir>".to_string();
    }
    match ext {
        // Per-file artwork: the icon belongs to the target, not the extension.
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
    Some(ctx.load_texture(format!("dcx-search-icon:{key}"), image, TextureOptions::LINEAR))
}

unsafe fn image_for(path: &Path, ext: &str, is_dir: bool) -> Option<ColorImage> {
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
            p[3] = if m[0] >= 0x80 { 0 } else { 255 };
        }
    }

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

//! The drawing surface and every primitive the overlay composes.
//!
//! ## Why the channel order looks "wrong"
//!
//! The surface *is* the DIB section handed to the window, and a 32-bpp DIB is
//! **BGRA** in memory while `tiny-skia` always writes **RGBA**. Rather than
//! swapping 8 MB twice per frame we let tiny-skia draw into the DIB bits and
//! swap red and blue **in the colour constants** ([`sk_color`]). Source-over
//! blending is per-channel and a channel permutation commutes with it, so the
//! result is identical and costs nothing — but only as long as no operation
//! mixes channels (no luminance, no gradients in another space). Keep every
//! paint a solid colour.

use tiny_skia::{FillRule, Paint, Path, PathBuilder, PixmapMut, Stroke, Transform};
use dcx_win32::{HGDIOBJ, HDC, HBITMAP};

/// Straight (non-premultiplied) RGBA in the *screen's* sense.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Rgba { r, g, b, a }
    }
    pub const fn opaque(r: u8, g: u8, b: u8) -> Self {
        Rgba { r, g, b, a: 255 }
    }
    pub fn from_rgb(rgb: [u8; 3]) -> Self {
        Rgba::opaque(rgb[0], rgb[1], rgb[2])
    }
    pub fn alpha(self, a: u8) -> Self {
        Rgba { a, ..self }
    }
}

/// See the module note: r/b are swapped on the way into tiny-skia.
pub fn sk_color(c: Rgba) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(c.b, c.g, c.r, c.a)
}

/// Arbitrary rectangle in physical pixels, normalised so `x0 <= x1`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Rect {
            x0: x0.min(x1),
            y0: y0.min(y1),
            x1: x0.max(x1),
            y1: y0.max(y1),
        }
    }
    pub fn from_xywh(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect::new(x, y, x + w, y + h)
    }
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }
    pub fn center(&self) -> (f32, f32) {
        ((self.x0 + self.x1) * 0.5, (self.y0 + self.y1) * 0.5)
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }
    pub fn inflate(&self, d: f32) -> Self {
        Rect {
            x0: self.x0 - d,
            y0: self.y0 - d,
            x1: self.x1 + d,
            y1: self.y1 + d,
        }
    }
    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let r = Rect {
            x0: self.x0.max(o.x0),
            y0: self.y0.max(o.y0),
            x1: self.x1.min(o.x1),
            y1: self.y1.min(o.y1),
        };
        if r.x1 > r.x0 && r.y1 > r.y0 {
            Some(r)
        } else {
            None
        }
    }
    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }
    /// Integer, clipped to `[0,w] × [0,h]`, ready for row copies.
    pub fn to_px(&self, w: u32, h: u32) -> Option<(i32, i32, i32, i32)> {
        let x0 = self.x0.floor().max(0.0) as i32;
        let y0 = self.y0.floor().max(0.0) as i32;
        let x1 = self.x1.ceil().min(w as f32) as i32;
        let y1 = self.y1.ceil().min(h as f32) as i32;
        if x1 > x0 && y1 > y0 {
            Some((x0, y0, x1, y1))
        } else {
            None
        }
    }
}

/// Offscreen DIB used as both the tiny-skia target and the `BitBlt` source.
pub struct Surface {
    dc: HDC,
    dib: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    pub w: u32,
    pub h: u32,
    pub stride: usize,
}

impl Surface {
    pub fn new(w: u32, h: u32) -> Option<Surface> {
        if w == 0 || h == 0 {
            return None;
        }
        unsafe {
            let screen = dcx_win32::GetDC(0);
            if screen == 0 {
                return None;
            }
            let dc = dcx_win32::CreateCompatibleDC(screen);
            dcx_win32::ReleaseDC(0, screen);
            if dc == 0 {
                return None;
            }
            let mut info = dcx_win32::BITMAPINFO::default();
            info.bmiHeader = dcx_win32::BITMAPINFOHEADER {
                biSize: core::mem::size_of::<dcx_win32::BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: dcx_win32::BI_RGB,
                biSizeImage: w * h * 4,
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
            let dib = dcx_win32::CreateDIBSection(
                dc,
                &info,
                crate::ffi::DIB_RGB_COLORS,
                &mut bits,
                0,
                0,
            );
            if dib == 0 || bits.is_null() {
                dcx_win32::DeleteDC(dc);
                return None;
            }
            let old = dcx_win32::SelectObject(dc, dib);
            Some(Surface {
                dc,
                dib,
                old,
                bits: bits as *mut u8,
                w,
                h,
                stride: (w as usize) * 4,
            })
        }
    }

    pub fn bytes(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.bits, self.stride * self.h as usize) }
    }

    pub fn bytes_mut(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.bits, self.stride * self.h as usize) }
    }

    /// Source DC for `UpdateLayeredWindow` (the DIB is selected into it).
    pub fn dc(&self) -> HDC {
        self.dc
    }

    /// tiny-skia over the live DIB bits. No copy: shapes land straight in the
    /// memory the window blits from.
    pub fn canvas<R>(&mut self, f: impl FnOnce(&mut Sk) -> R) -> R {
        let w = self.w;
        let h = self.h;
        let mut sk = Sk::new(self.bytes_mut(), w, h);
        f(&mut sk)
    }

    /// Copy a rectangle of this surface to `dst` at the same coordinates.
    pub fn blit(&self, dst: HDC, rect: &Rect) {
        let Some((x0, y0, x1, y1)) = rect.to_px(self.w, self.h) else {
            return;
        };
        unsafe {
            dcx_win32::BitBlt(
                dst,
                x0,
                y0,
                x1 - x0,
                y1 - y0,
                self.dc,
                x0,
                y0,
                crate::ffi::SRCCOPY,
            );
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            dcx_win32::SelectObject(self.dc, self.old);
            dcx_win32::DeleteObject(self.dib);
            dcx_win32::DeleteDC(self.dc);
        }
    }
}

/// thin wrapper so call sites read like painting code.
pub struct Sk<'a> {
    pm: PixmapMut<'a>,
}

impl<'a> Sk<'a> {
    pub fn new(bytes: &'a mut [u8], w: u32, h: u32) -> Self {
        let pm = PixmapMut::from_bytes(bytes, w, h)
            .expect("surface length must match width * height * 4");
        Sk { pm }
    }

    fn paint(c: Rgba) -> Paint<'static> {
        let mut p = Paint::default();
        p.set_color(sk_color(c));
        p.anti_alias = true;
        p
    }

    pub fn fill_rect(&mut self, r: &Rect, c: Rgba) {
        let Some(rect) = sk_rect(r) else { return };
        self.pm.fill_rect(rect, &Sk::paint(c), Transform::identity(), None);
    }

    pub fn fill_round(&mut self, r: &Rect, radius: f32, c: Rgba) {
        let Some(path) = path_round_rect(r, radius) else { return };
        self.pm
            .fill_path(&path, &Sk::paint(c), FillRule::Winding, Transform::identity(), None);
    }

    pub fn fill_path(&mut self, path: &Path, c: Rgba) {
        self.pm
            .fill_path(path, &Sk::paint(c), FillRule::Winding, Transform::identity(), None);
    }

    pub fn stroke_rect(&mut self, r: &Rect, width: f32, c: Rgba) {
        let Some(path) = path_rect(r) else { return };
        self.stroke_path(&path, width, c, false);
    }

    pub fn stroke_round(&mut self, r: &Rect, radius: f32, width: f32, c: Rgba) {
        let Some(path) = path_round_rect(r, radius) else { return };
        self.stroke_path(&path, width, c, false);
    }

    pub fn stroke_path(&mut self, path: &Path, width: f32, c: Rgba, dash: bool) {
        let mut stroke = Stroke::default();
        stroke.width = width.max(0.2);
        stroke.line_cap = tiny_skia::LineCap::Round;
        stroke.line_join = tiny_skia::LineJoin::Round;
        if dash {
            stroke.dash = tiny_skia::StrokeDash::new(vec![width * 2.0, width * 1.6], 0.0);
        }
        self.pm.stroke_path(
            path,
            &Sk::paint(c),
            &stroke,
            Transform::identity(),
            None,
        );
    }

    pub fn line(&mut self, a: (f32, f32), b: (f32, f32), width: f32, c: Rgba) {
        let mut pb = PathBuilder::new();
        pb.move_to(a.0, a.1);
        pb.line_to(b.0, b.1);
        if let Some(path) = pb.finish() {
            self.stroke_path(&path, width, c, false);
        }
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, c: Rgba) {
        let Some(path) = path_circle(cx, cy, r) else { return };
        self.fill_path(&path, c);
    }

    pub fn ring(&mut self, cx: f32, cy: f32, r: f32, width: f32, c: Rgba) {
        let Some(path) = path_circle(cx, cy, r) else { return };
        self.stroke_path(&path, width, c, false);
    }

    pub fn draw_coverage(&mut self, x: i32, y: i32, w: u32, h: u32, cov: &[u8], c: Rgba) {
        if x < 0 || y < 0 {
            return;
        }
        let stride = (self.pm.width() as usize) * 4;
        let surf_w = self.pm.width() as i32;
        let surf_h = self.pm.height() as i32;
        let data = self.pm.data_mut();
        for row in 0..h as i32 {
            let sy = y + row;
            if sy >= surf_h {
                break;
            }
            let src = (row as usize) * (w as usize);
            for col in 0..w as i32 {
                let sx = x + col;
                // Without this a label that runs off the right edge wraps its
                // remaining pixels into the start of the next scanline.
                if sx >= surf_w {
                    break;
                }
                let a = cov[src + col as usize] as u32;
                if a == 0 {
                    continue;
                }
                let off = sy as usize * stride + sx as usize * 4;
                if off + 3 >= data.len() {
                    continue;
                }
                let inv = 255 - a;
                data[off] = ((c.b as u32 * a + data[off] as u32 * inv) / 255) as u8;
                data[off + 1] = ((c.g as u32 * a + data[off + 1] as u32 * inv) / 255) as u8;
                data[off + 2] = ((c.r as u32 * a + data[off + 2] as u32 * inv) / 255) as u8;
                data[off + 3] = 255;
            }
        }
    }
}

pub fn sk_rect(r: &Rect) -> Option<tiny_skia::Rect> {
    tiny_skia::Rect::from_ltrb(r.x0, r.y0, r.x1, r.y1)
}

pub fn path_rect(r: &Rect) -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.push_rect(sk_rect(r)?);
    pb.finish()
}

/// Rounded rectangle from four cubic arcs (kappa = 4/3·(√2−1)).
pub fn path_round_rect(r: &Rect, radius: f32) -> Option<Path> {
    let max = r.width().min(r.height()) * 0.5;
    let rad = radius.clamp(0.0, max.max(0.0));
    if rad <= 0.01 {
        return path_rect(r);
    }
    let k = rad * 0.552_284_75;
    let (x0, y0, x1, y1) = (r.x0, r.y0, r.x1, r.y1);
    let mut pb = PathBuilder::new();
    pb.move_to(x0 + rad, y0);
    pb.line_to(x1 - rad, y0);
    pb.cubic_to(x1 - rad + k, y0, x1, y0 + rad - k, x1, y0 + rad);
    pb.line_to(x1, y1 - rad);
    pb.cubic_to(x1, y1 - rad + k, x1 - rad + k, y1, x1 - rad, y1);
    pb.line_to(x0 + rad, y1);
    pb.cubic_to(x0 + rad - k, y1, x0, y1 - rad + k, x0, y1 - rad);
    pb.line_to(x0, y0 + rad);
    pb.cubic_to(x0, y0 + rad - k, x0 + rad - k, y0, x0 + rad, y0);
    pb.close();
    pb.finish()
}

pub fn path_circle(cx: f32, cy: f32, r: f32) -> Option<Path> {
    if r <= 0.0 {
        return None;
    }
    let k = r * 0.552_284_75;
    let mut pb = PathBuilder::new();
    pb.move_to(cx, cy - r);
    pb.cubic_to(cx + k, cy - r, cx + r, cy - k, cx + r, cy);
    pb.cubic_to(cx + r, cy + k, cx + k, cy + r, cx, cy + r);
    pb.cubic_to(cx - k, cy + r, cx - r, cy + k, cx - r, cy);
    pb.cubic_to(cx - r, cy - k, cx - k, cy - r, cx, cy - r);
    pb.close();
    pb.finish()
}

// ---------------------------------------------------------------------------
// Raw pixel operations (no anti-aliasing, no tiny-skia)
// ---------------------------------------------------------------------------

pub fn copy_rect(
    dst: &mut [u8],
    dst_stride: usize,
    src: &[u8],
    src_stride: usize,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
) {
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let row = ((x1 - x0) as usize) * 4;
    for y in y0..y1 {
        let off_d = y as usize * dst_stride + x0 as usize * 4;
        let off_s = y as usize * src_stride + x0 as usize * 4;
        if off_d + row <= dst.len() && off_s + row <= src.len() {
            dst[off_d..off_d + row].copy_from_slice(&src[off_s..off_s + row]);
        }
    }
}

/// Blend `rgb` into a rectangle with weight `k/255` (integer, no branch per
/// channel beyond the min/max clamp).
pub fn tint_rect(
    buf: &mut [u8],
    stride: usize,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    rgb: [u8; 3],
    k: u32,
) {
    if x1 <= x0 || y1 <= y0 || k == 0 {
        return;
    }
    let inv = 255 - k.min(255);
    let (tr, tg, tb) = (rgb[0] as u32 * k, rgb[1] as u32 * k, rgb[2] as u32 * k);
    for y in y0..y1 {
        let base = y as usize * stride;
        let row_start = base + x0 as usize * 4;
        let row_end = base + x1 as usize * 4;
        if row_end > buf.len() {
            continue;
        }
        let row = &mut buf[row_start..row_end];
        for px in row.chunks_exact_mut(4) {
            px[0] = ((px[0] as u32 * inv + tb) / 255) as u8;
            px[1] = ((px[1] as u32 * inv + tg) / 255) as u8;
            px[2] = ((px[2] as u32 * inv + tr) / 255) as u8;
        }
    }
}

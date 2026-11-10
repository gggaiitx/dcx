//! Live thumbnail card beside the viewport.
//!
//! This is the "see the long image growing" surface: a fixed card in the
//! screen space left or right of the selection, bottom-aligned with it, where
//! the stitch is downsampled and drawn bottom-anchored. As the image grows it
//! gets taller and narrower, so the whole capture stays visible without the
//! card ever growing. The window is click- and wheel-through.

use super::{present_layered, px, BoxRect};
use crate::layout::{BORDER, DISABLED, MUTED, PANEL, TEXT};
use crate::stitch::Stitcher;
use crate::surface::{Rect, Surface};
use crate::text;
use crate::{paint, wide};
use dcx_win32::HWND;
use crate::ffi::*;

const MAX_W_BASE: f32 = 250.0;
const MIN_W_BASE: f32 = 150.0;
const MAX_H_BASE: f32 = 600.0;
const MIN_H_BASE: f32 = 140.0;
const MARGIN_BASE: f32 = 12.0;
const PAD_BASE: f32 = 10.0;
const HEADER_BASE: f32 = 32.0;
const RADIUS_BASE: f32 = 11.0;
const FONT_TITLE: f32 = 12.0;
const FONT_META: f32 = 10.5;

/// Right side when it has more room, else left; `None` when neither side can
/// hold a readable card.
pub(super) fn layout(vp: BoxRect, screen: (i32, i32, i32, i32), scale: f32) -> Option<BoxRect> {
    let (sx, sy, sw, sh) = screen;
    let margin = px(MARGIN_BASE, scale);
    let max_w = px(MAX_W_BASE, scale);
    let min_w = px(MIN_W_BASE, scale);
    let min_h = px(MIN_H_BASE, scale);
    let max_h = px(MAX_H_BASE, scale).max(min_h);

    let right = sx + sw - vp.right() - margin * 2;
    let left = vp.x - sx - margin * 2;
    let w = max_w.min(right.max(left));
    if w < min_w {
        return None;
    }
    let x = if right >= left {
        vp.right() + margin
    } else {
        vp.x - margin - w
    };
    let h = (vp.h as i32)
        .clamp(min_h, max_h)
        .min((sh - margin * 2).max(1));
    if h < min_h {
        return None;
    }
    let y = (vp.bottom() - h)
        .min(sy + sh - margin - h)
        .max(sy + margin);
    Some(BoxRect {
        x,
        y,
        w: w as u32,
        h: h as u32,
    })
}

pub(super) struct Preview {
    hwnd: HWND,
    surface: Surface,
    scale: f32,
    /// Last stitched height drawn; `None` means the placeholder is showing.
    last_h: Option<u32>,
}

impl Preview {
    pub fn spawn(vp: BoxRect, screen: (i32, i32, i32, i32), scale: f32) -> Option<Preview> {
        let rect = layout(vp, screen, scale)?;
        let surface = Surface::new(rect.w, rect.h)?;
        unsafe {
            let class = wide("DCX.Snip.LongPreview");
            let cls = dcx_win32::WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(preview_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: GetModuleHandleW(core::ptr::null()),
                hIcon: 0,
                hCursor: 0,
                hbrBackground: 0,
                lpszMenuName: core::ptr::null(),
                lpszClassName: class.as_ptr(),
            };
            let _ = dcx_win32::RegisterClassW(&cls);
            let title = wide("DCX 长截图预览");
            let hwnd = dcx_win32::CreateWindowExW(
                // TRANSPARENT: wheel and clicks belong to the page underneath.
                WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                rect.x,
                rect.y,
                rect.w as i32,
                rect.h as i32,
                0,
                0,
                GetModuleHandleW(core::ptr::null()),
                core::ptr::null_mut(),
            );
            if hwnd == 0 {
                return None;
            }
            let mut preview = Preview {
                hwnd,
                surface,
                scale,
                last_h: None,
            };
            preview.draw(None);
            dcx_win32::SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                rect.x,
                rect.y,
                rect.w as i32,
                rect.h as i32,
                SWP_SHOWWINDOW | SWP_NOACTIVATE,
            );
            Some(preview)
        }
    }

    /// Re-draw only when the stitch height changed. `None` draws the
    /// "scroll to begin" placeholder; the first call always draws.
    pub fn update(&mut self, stitcher: Option<&Stitcher>) {
        let h = stitcher
            .filter(|s| s.height() > 0)
            .map(|s| s.height());
        if self.last_h == h {
            return;
        }
        self.draw(stitcher);
    }

    fn draw(&mut self, stitcher: Option<&Stitcher>) {
        // A buffered seed has height 0 until the next frame arrives: keep the
        // placeholder rather than flashing an empty card.
        let stitcher = stitcher.filter(|s| s.height() > 0);
        self.last_h = stitcher.map(|s| s.height());
        render(&mut self.surface, stitcher, self.scale);
        unsafe {
            present_layered(self.hwnd, &self.surface);
        }
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        unsafe {
            if self.hwnd != 0 {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

pub(super) fn render(surface: &mut Surface, stitcher: Option<&Stitcher>, scale: f32) {
    let (w, h) = (surface.w as f32, surface.h as f32);
    surface.bytes_mut().fill(0);
    let radius = px(RADIUS_BASE, scale) as f32;
    let pad = px(PAD_BASE, scale) as f32;
    let header = px(HEADER_BASE, scale) as f32;

    surface.canvas(|sk| {
        let full = Rect::from_xywh(0.5, 0.5, w - 1.0, h - 1.0);
        sk.fill_round(&full, radius, PANEL);
        sk.stroke_round(&full, radius, 1.0, BORDER);
        paint::label(sk, "长截图预览", pad, px(9.0, scale) as f32, FONT_TITLE * scale, TEXT);
        if let Some(st) = stitcher {
            let meta = format!("{} px", st.height());
            let tw = text::measure(&meta, FONT_META * scale).0;
            paint::label(
                sk,
                &meta,
                w - pad - tw,
                px(10.0, scale) as f32,
                FONT_META * scale,
                MUTED,
            );
        }
        sk.line((pad, header), (w - pad, header), 1.0, BORDER);
    });

    let Some(st) = stitcher else {
        let msg = "滚动页面开始拼接";
        let tw = text::measure(msg, 11.0 * scale).0;
        surface.canvas(|sk| {
            paint::label(
                sk,
                msg,
                (w - tw) * 0.5,
                h * 0.5 - 4.0 * scale,
                11.0 * scale,
                DISABLED,
            );
        });
        return;
    };

    let area = Rect::from_xywh(pad, header + pad, w - pad * 2.0, h - header - pad * 2.0);
    if area.is_empty() {
        return;
    }
    let Some((thumb, tw, th)) = st.thumbnail(area.width() as u32, area.height() as u32) else {
        return;
    };
    let dx = area.x0 + (area.width() - tw as f32) * 0.5;
    // Bottom-anchored: the newest rows always touch the card's bottom edge, so
    // growth is visible without the image sliding around.
    let dy = area.y1 - th as f32;
    draw_bgra(surface, &thumb, tw, th, dx.round() as i32, dy.round() as i32);
    surface.canvas(|sk| {
        sk.stroke_rect(
            &Rect::from_xywh(dx.round() - 0.5, dy.round() - 0.5, tw as f32 + 1.0, th as f32 + 1.0),
            1.0,
            BORDER,
        );
    });
}

fn draw_bgra(surface: &mut Surface, src: &[u8], tw: u32, th: u32, x: i32, y: i32) {
    let dst_stride = surface.stride;
    let src_stride = tw as usize * 4;
    let (sw, sh) = (surface.w as i32, surface.h as i32);
    let buf = surface.bytes_mut();
    for row in 0..th as i32 {
        let dy = y + row;
        if dy < 0 || dy >= sh {
            continue;
        }
        let skip = (-x).max(0) as usize;
        let n = (tw as usize).saturating_sub(skip);
        let dx = x + skip as i32;
        if n == 0 || dx < 0 || dx >= sw {
            continue;
        }
        let n = n.min((sw - dx) as usize);
        let d = dy as usize * dst_stride + dx as usize * 4;
        let s = row as usize * src_stride + skip * 4;
        buf[d..d + n * 4].copy_from_slice(&src[s..s + n * 4]);
    }
}

unsafe extern "system" fn preview_proc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_ERASEBKGND => 1,
        _ => unsafe { dcx_win32::DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> (i32, i32, i32, i32) {
        (0, 0, 1920, 1080)
    }

    fn vp(x: i32, w: u32) -> BoxRect {
        BoxRect {
            x,
            y: 100,
            w,
            h: 600,
        }
    }

    #[test]
    fn prefers_the_wider_side() {
        let right = layout(vp(300, 600), screen(), 1.0).expect("right room");
        assert!(right.x >= 900, "right side");
        assert_eq!(right.w, px(MAX_W_BASE, 1.0) as u32);
        let left = layout(vp(1300, 550), screen(), 1.0).expect("left room");
        assert!(left.right() <= 1300, "left side");
    }

    #[test]
    fn hides_when_both_sides_are_narrow() {
        assert!(layout(vp(20, 1880), screen(), 1.0).is_none());
    }

    #[test]
    fn never_covers_the_viewport() {
        for x in [0, 50, 300, 1000, 1400] {
            for w in [200u32, 500, 900] {
                if let Some(r) = layout(vp(x, w), screen(), 1.0) {
                    assert!(
                        r.right() <= x || r.x >= x + w as i32,
                        "preview overlaps the captured region: {r:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn stays_inside_the_screen() {
        let tall = BoxRect {
            x: 400,
            y: 900,
            w: 700,
            h: 170,
        };
        let r = layout(tall, screen(), 1.0).expect("room");
        assert!(r.y >= 0 && r.bottom() <= 1080 - px(MARGIN_BASE, 1.0), "{r:?}");
        assert_eq!(r.bottom(), 1068, "clamped to the screen margin");
    }

    #[test]
    fn scales_with_dpi() {
        let base = layout(vp(300, 600), screen(), 1.0).unwrap();
        let hi = layout(
            BoxRect {
                x: 600,
                y: 200,
                w: 1200,
                h: 600,
            },
            (0, 0, 3840, 2160),
            2.0,
        )
        .unwrap();
        assert_eq!(hi.w, base.w * 2);
        assert_eq!(hi.h, base.h, "height follows the viewport, not the DPI");
    }
}

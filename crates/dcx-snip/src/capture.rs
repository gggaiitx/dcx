//! One `BitBlt` of the whole virtual screen into a top-down 32-bpp DIB.
//!
//! Deliberately SDR only: a DXGI Desktop Duplication path (HDR, FP16 +
//! tone-mapping) is a separate project. What matters here is that the frozen
//! frame is captured **before** the overlay window exists — otherwise the
//! overlay would photograph itself.

use dcx_win32::{
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CreateCompatibleDC, CreateDIBSection, DeleteDC,
    DeleteObject, GetDC, GetSystemMetrics, ReleaseDC, SelectObject, BitBlt,
};
use crate::ffi::{CAPTUREBLT, DIB_RGB_COLORS, SRCCOPY};

/// Frozen screen content, top-down BGRA with alpha forced to 255.
pub struct Shot {
    /// Virtual-screen origin in physical pixels (may be negative).
    pub origin: (i32, i32),
    pub w: u32,
    pub h: u32,
    pub bgra: Vec<u8>,
}

pub fn virtual_screen() -> (i32, i32, i32, i32) {
    unsafe {
        (
            GetSystemMetrics(crate::ffi::SM_XVIRTUALSCREEN),
            GetSystemMetrics(crate::ffi::SM_YVIRTUALSCREEN),
            GetSystemMetrics(crate::ffi::SM_CXVIRTUALSCREEN),
            GetSystemMetrics(crate::ffi::SM_CYVIRTUALSCREEN),
        )
    }
}

pub fn grab() -> Result<Shot, String> {
    let (vx, vy, vw, vh) = virtual_screen();
    if vw <= 0 || vh <= 0 {
        return Err("取不到虚拟屏幕尺寸".to_string());
    }
    grab_region(vx, vy, vw as u32, vh as u32)
}

/// Capture one rectangle in virtual-screen coordinates. Used by the scrolling
/// stitcher, which hides the overlay and re-shoots the same region.
pub fn grab_region(sx: i32, sy: i32, w: u32, h: u32) -> Result<Shot, String> {
    if w == 0 || h == 0 {
        return Err("抓取区域为空".to_string());
    }
    unsafe {
        let screen = GetDC(0);
        if screen == 0 {
            return Err("GetDC 失败".to_string());
        }
        let mem = CreateCompatibleDC(screen);
        if mem == 0 {
            ReleaseDC(0, screen);
            return Err("CreateCompatibleDC 失败".to_string());
        }

        let mut info = BITMAPINFO::default();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: core::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            // Negative height: top-down rows, matching every other buffer here.
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: w * h * 4,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };

        let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
        let dib = CreateDIBSection(mem, &info, DIB_RGB_COLORS, &mut bits, 0, 0);
        if dib == 0 || bits.is_null() {
            DeleteDC(mem);
            ReleaseDC(0, screen);
            return Err("CreateDIBSection 失败".to_string());
        }
        let old = SelectObject(mem, dib);

        // CAPTUREBLT is what pulls in layered windows; without it every
        // topmost tooltip and flyout vanishes from the shot.
        let ok = BitBlt(mem, 0, 0, w as i32, h as i32, screen, sx, sy, SRCCOPY | CAPTUREBLT) != 0;

        let mut bgra = vec![0u8; (w as usize) * (h as usize) * 4];
        if ok {
            core::ptr::copy_nonoverlapping(bits as *const u8, bgra.as_mut_ptr(), bgra.len());
        }

        SelectObject(mem, old);
        DeleteObject(dib);
        DeleteDC(mem);
        ReleaseDC(0, screen);

        if !ok {
            return Err("BitBlt 抓屏失败".to_string());
        }

        // GDI leaves the alpha byte at 0. Everything downstream is premultiplied
        // source-over, so an opaque frame is not optional: with alpha 0 the
        // clipboard image and the saved PNG come out fully transparent.
        for px in bgra.chunks_exact_mut(4) {
            px[3] = 255;
        }

        Ok(Shot {
            origin: (sx, sy),
            w,
            h,
            bgra,
        })
    }
}

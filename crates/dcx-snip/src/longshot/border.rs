//! The fixed 2 px viewport frame shown while scrolling.
//!
//! It is a layered window with a colour-key interior, not an alpha surface: a
//! full-viewport DIB would cost several megabytes for four thin strips. The
//! window is `WS_EX_TRANSPARENT`, so every click and wheel event lands on the
//! page underneath, and the interior is keyed out anyway.

use std::sync::atomic::{AtomicI32, Ordering};

use super::{px, BoxRect};
use crate::ffi::*;
use dcx_win32::{HWND, RECT};
use crate::wide;

/// Core border colour (DCX teal `#1E9C8B`) as a Win32 `COLORREF` (0x00BBGGRR).
const BORDER: u32 = 0x008B_9C1E;
/// Interior key: an almost-black colour no real UI uses.
const KEY: u32 = 0x0003_0201;
/// Thickness of the current frame, set by `spawn` (one capture at a time).
static EDGE: AtomicI32 = AtomicI32::new(2);

pub(super) fn spawn(vp: BoxRect, scale: f32) -> Option<HWND> {
    let edge = px(2.0, scale).max(1);
    EDGE.store(edge, Ordering::Relaxed);
    let x = vp.x - edge;
    let y = vp.y - edge;
    let w = vp.w as i32 + edge * 2;
    let h = vp.h as i32 + edge * 2;
    unsafe {
        let class = wide("DCX.Snip.LongBorder");
        let cls = dcx_win32::WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wndproc),
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
        let title = wide("DCX 长截图边框");
        let hwnd = dcx_win32::CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            x,
            y,
            w,
            h,
            0,
            0,
            GetModuleHandleW(core::ptr::null()),
            core::ptr::null_mut(),
        );
        if hwnd == 0 {
            return None;
        }
        let _ = SetLayeredWindowAttributes(hwnd, KEY, 255, LWA_COLORKEY);
        // Paint once while hidden: showing first can expose an unpainted black
        // rectangle for a frame.
        let _ = UpdateWindow(hwnd);
        dcx_win32::SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            x,
            y,
            w,
            h,
            SWP_SHOWWINDOW | SWP_NOACTIVATE,
        );
        Some(hwnd)
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            unsafe {
                BeginPaint(hwnd, &mut ps);
                draw(hwnd, ps.hdc);
                EndPaint(hwnd, &ps);
            }
            0
        }
        WM_ERASEBKGND => 1,
        _ => unsafe { dcx_win32::DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn draw(hwnd: HWND, dc: dcx_win32::HDC) {
    unsafe {
        let mut rc = RECT::default();
        if dcx_win32::GetClientRect(hwnd, &mut rc) == 0 {
            return;
        }
        let key = CreateSolidBrush(KEY);
        FillRect(dc, &rc, key);
        let _ = dcx_win32::DeleteObject(key);
        let brush = CreateSolidBrush(BORDER);
        let (w, h) = (rc.right, rc.bottom);
        let edge = EDGE.load(Ordering::Relaxed);
        let strips = [
            RECT { left: 0, top: 0, right: w, bottom: edge },
            RECT { left: 0, top: h - edge, right: w, bottom: h },
            RECT { left: 0, top: 0, right: edge, bottom: h },
            RECT { left: w - edge, top: 0, right: w, bottom: h },
        ];
        for strip in strips {
            FillRect(dc, &strip, brush);
        }
        let _ = dcx_win32::DeleteObject(brush);
    }
}

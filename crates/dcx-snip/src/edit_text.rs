//! A native single-line edit control for text annotations.
//!
//! The overlay draws its own UI, but it does **not** draw its own text input:
//! an EDIT control brings the system IME, the candidate window, clipboard
//! shortcuts and accessibility for free, and Chinese input is the whole point
//! of the tool on this desktop. The control is a child of the overlay so it
//! moves and clips with it.
//!
//! The first version hosted a stock white EDIT box, which looked pasted-on and
//! made white annotation colours invisible. The host now paints the same dark
//! panel the toolbar uses and returns a dark brush, so typing always shows.

use crate::ffi::*;
use crate::model::{Pt, TextStyle};
use crate::surface::Rect;
use dcx_win32::{HGDIOBJ, HWND};
use std::cell::Cell;

const WM_SETFONT: u32 = 0x0030;
const WM_CTLCOLOREDIT: u32 = 0x0133;
const EM_SETMARGINS: u32 = 0x00D3;
const EM_SETSEL: u32 = 0x00B1;
const EC_LEFTMARGIN: usize = 0x0001;
const EC_RIGHTMARGIN: usize = 0x0002;
const ES_NOHIDESEL: u32 = 0x0100;

const PANEL_R: u32 = 0x16;
const PANEL_G: u32 = 0x1B;
const PANEL_B: u32 = 0x21;
const ACCENT_R: u32 = 0x1E;
const ACCENT_G: u32 = 0x9C;
const ACCENT_B: u32 = 0x8B;

fn colorref(r: u32, g: u32, b: u32) -> u32 {
    (b << 16) | (g << 8) | r
}

thread_local! {
    static OLD_PROC: Cell<isize> = const { Cell::new(0) };
    static PARENT: Cell<HWND> = const { Cell::new(0) };
    static HOST_CLASS: Cell<u16> = const { Cell::new(0) };
    static EDIT_BRUSH: Cell<HGDIOBJ> = const { Cell::new(0) };
    static BORDER_BRUSH: Cell<HGDIOBJ> = const { Cell::new(0) };
}

pub struct Editor {
    host: HWND,
    edit: HWND,
    font: HGDIOBJ,
    brush: HGDIOBJ,
    border: HGDIOBJ,
}

impl Editor {
    pub fn text(&self) -> String {
        unsafe {
            let len = GetWindowTextLengthW(self.edit);
            if len <= 0 {
                return String::new();
            }
            let mut buf = vec![0u16; len as usize + 1];
            let n = GetWindowTextW(self.edit, buf.as_mut_ptr(), buf.len() as i32);
            String::from_utf16_lossy(&buf[..n.max(0) as usize])
        }
    }

    pub fn close(self) {
        unsafe {
            DestroyWindow(self.host);
            if self.font != 0 {
                dcx_win32::DeleteObject(self.font);
            }
            if self.brush != 0 {
                dcx_win32::DeleteObject(self.brush);
            }
            if self.border != 0 {
                dcx_win32::DeleteObject(self.border);
            }
        }
        OLD_PROC.with(|c| c.set(0));
        PARENT.with(|c| c.set(0));
        EDIT_BRUSH.with(|c| c.set(0));
        BORDER_BRUSH.with(|c| c.set(0));
    }
}

/// `at` is the top-left the text object will be drawn at. `size` is the font
/// height in physical pixels (already scaled by the session). `style` is the
/// style the new object will carry, so typing is WYSIWYG — a bold 宋体 edit box
/// with YaHei regular text in it would be a lie about the result.
pub fn open(
    parent: HWND,
    at: Pt,
    size: f32,
    monitor: Rect,
    initial: &str,
    style: &TextStyle,
) -> Option<Editor> {
    let font_h = size.round().max(10.0) as i32;
    let h = font_h + 12;
    let mut w = 320.max(font_h * 12);
    let mut x = at.x.round() as i32 - 5;
    let mut y = at.y.round() as i32 - 5;
    let mon_x0 = monitor.x0.round() as i32;
    let mon_y0 = monitor.y0.round() as i32;
    let mon_x1 = monitor.x1.round() as i32;
    let mon_y1 = monitor.y1.round() as i32;
    w = w.min((mon_x1 - mon_x0 - 16).max(160));
    if x + w + 4 > mon_x1 {
        x = (mon_x1 - w - 4).max(mon_x0 + 4);
    }
    if x < mon_x0 + 4 {
        x = mon_x0 + 4;
    }
    if y + h + 4 > mon_y1 {
        y = (mon_y1 - h - 4).max(mon_y0 + 4);
    }
    if y < mon_y0 + 4 {
        y = mon_y0 + 4;
    }

    unsafe {
        let class = crate::wide("DCX.Snip.TextHost");
        if HOST_CLASS.with(|c| c.get()) == 0 {
            let cls = dcx_win32::WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(host_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: GetModuleHandleW(core::ptr::null()),
                hIcon: 0,
                hCursor: 0,
                hbrBackground: 0,
                lpszMenuName: core::ptr::null(),
                lpszClassName: class.as_ptr(),
            };
            if dcx_win32::RegisterClassW(&cls) == 0 {
                return None;
            }
            HOST_CLASS.with(|c| c.set(1));
        }

        let edit_brush = CreateSolidBrush(colorref(PANEL_R, PANEL_G, PANEL_B));
        let border_brush = CreateSolidBrush(colorref(ACCENT_R, ACCENT_G, ACCENT_B));
        EDIT_BRUSH.with(|c| c.set(edit_brush));
        BORDER_BRUSH.with(|c| c.set(border_brush));

        let host = dcx_win32::CreateWindowExW(
            0,
            class.as_ptr(),
            crate::wide("").as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN,
            x,
            y,
            w,
            h,
            parent,
            0,
            GetModuleHandleW(core::ptr::null()),
            core::ptr::null_mut(),
        );
        if host == 0 {
            return None;
        }

        let edit_class = crate::wide("EDIT");
        let edit = dcx_win32::CreateWindowExW(
            0,
            edit_class.as_ptr(),
            crate::wide("").as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_LEFT | ES_AUTOHSCROLL | ES_NOHIDESEL,
            2,
            1,
            w - 4,
            h - 2,
            host,
            0,
            GetModuleHandleW(core::ptr::null()),
            core::ptr::null_mut(),
        );
        if edit == 0 {
            DestroyWindow(host);
            return None;
        }

        let face_name = {
            let raw = style.face.trim();
            if raw.is_empty() {
                dcx_win32::font_catalog::DEFAULT_FACE
            } else {
                raw
            }
        };
        let face = crate::wide(face_name);
        let font = CreateFontW(
            -font_h,
            0,
            0,
            0,
            if style.bold { FW_BOLD } else { FW_NORMAL },
            if style.italic { 1 } else { 0 },
            0,
            0,
            DEFAULT_CHARSET,
            0,
            0,
            CLEARTYPE_QUALITY,
            0,
            face.as_ptr(),
        );
        if font != 0 {
            SendMessageW(edit, WM_SETFONT, font as usize, 1);
        }
        SendMessageW(
            edit,
            EM_SETMARGINS,
            EC_LEFTMARGIN | EC_RIGHTMARGIN,
            ((6i32) << 16 | 6i32) as isize,
        );

        if !initial.is_empty() {
            SetWindowTextW(edit, crate::wide(initial).as_ptr());
            let len = initial.encode_utf16().count() as isize;
            SendMessageW(edit, EM_SETSEL, 0, len);
        } else {
            SendMessageW(edit, EM_SETSEL, 0, 0);
        }

        let old = dcx_win32::SetWindowLongPtrW(edit, GWLP_WNDPROC, edit_proc as *const () as isize);
        OLD_PROC.with(|c| c.set(old));
        PARENT.with(|c| c.set(parent));

        SetFocus(edit);
        Some(Editor {
            host,
            edit,
            font,
            brush: edit_brush,
            border: border_brush,
        })
    }
}

unsafe extern "system" fn host_proc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_CTLCOLOREDIT => {
            let dc = wp as isize;
            unsafe {
                SetBkMode(dc, TRANSPARENT);
                SetBkColor(dc, colorref(PANEL_R, PANEL_G, PANEL_B));
                SetTextColor(dc, colorref(0xED, 0xF2, 0xF5));
            }
            EDIT_BRUSH.with(|c| c.get()) as isize
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            unsafe {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                if dc != 0 {
                    let mut rc = dcx_win32::RECT::default();
                    dcx_win32::GetClientRect(hwnd, &mut rc);
                    let brush = EDIT_BRUSH.with(|c| c.get());
                    FillRect(dc, &rc, brush);
                    let border = BORDER_BRUSH.with(|c| c.get());
                    if border != 0 {
                        FrameRect(dc, &rc, border);
                    }
                    EndPaint(hwnd, &ps);
                }
            }
            0
        }
        _ => unsafe { dcx_win32::DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

unsafe extern "system" fn edit_proc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    if msg == WM_KEYDOWN {
        match wp as i32 {
            VK_RETURN => {
                let parent = PARENT.with(|c| c.get());
                if parent != 0 {
                    unsafe { PostMessageW(parent, crate::session::WM_APP_TEXT_COMMIT, 0, 0) };
                }
                return 0;
            }
            VK_ESCAPE => {
                let parent = PARENT.with(|c| c.get());
                if parent != 0 {
                    unsafe { PostMessageW(parent, crate::session::WM_APP_TEXT_CANCEL, 0, 0) };
                }
                return 0;
            }
            _ => {}
        }
    }
    if msg == WM_CHAR && wp as i32 == VK_RETURN {
        return 0;
    }
    let old = OLD_PROC.with(|c| c.get());
    if old == 0 {
        return unsafe { dcx_win32::DefWindowProcW(hwnd, msg, wp, lp) };
    }
    unsafe { CallWindowProcW(old, hwnd, msg, wp, lp) }
}

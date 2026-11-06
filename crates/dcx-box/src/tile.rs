//! The taskbar button, as its own tiny Win32 window.
//!
//! # Why it is not part of the eframe window
//!
//! The button lives on the taskbar; the panel lives wherever the user dragged
//! it. Painting both into one window forces that window to be their bounding
//! box, and every panel gesture then has to move or resize that union:
//!
//! * dragging the panel dragged the button along (the button is painted in the
//!   same surface) or made it blink out while the window was re-anchored;
//! * the transparent parts of the union covered the desktop, the notification
//!   area and the settings window, and `WM_NCHITTEST`/`HTTRANSPARENT` does not
//!   forward clicks to other processes.
//!
//! So the button is a separate `WS_EX_LAYERED | WS_EX_NOACTIVATE` window that
//! paints itself with `UpdateLayeredWindow`. Moving the panel never touches it,
//! and it can be clickable without stealing focus from the foreground app.
//!
//! Communication with the egui loop is deliberately tiny: two one-shot flags
//! (left click, right click) plus a hover bit. A click also asks egui for a
//! repaint so the reaction is immediate instead of waiting for the idle frame.

use dcx_win32 as w;
use egui::Context;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;

const WM_DESTROY: u32 = 0x0002;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_MOUSEACTIVATE: u32 = 0x0021;
const WM_NCHITTEST: u32 = 0x0084;
const WM_ERASEBKGND: u32 = 0x0014;
const MA_NOACTIVATE: isize = 3;
const HTCLIENT: isize = 1;
const TME_LEAVE: u32 = 0x0000_0002;

const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
const WS_EX_TOPMOST: isize = 0x0000_0008;
const WS_EX_NOACTIVATE: isize = 0x0800_0000;
const WS_EX_LAYERED: isize = 0x0008_0000;

const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_NOSIZE: u32 = 0x0001;
const HWND_TOPMOST: isize = -1;

const ULW_ALPHA: u32 = 0x0000_0002;
const AC_SRC_OVER: u8 = 0x00;
const AC_SRC_ALPHA: u8 = 0x01;
const DIB_RGB_COLORS: u32 = 0;

static CTX: Mutex<Option<Context>> = Mutex::new(None);
static CLICK: AtomicBool = AtomicBool::new(false);
static SECONDARY: AtomicBool = AtomicBool::new(false);
static HOVER: AtomicBool = AtomicBool::new(false);
static OPEN: AtomicBool = AtomicBool::new(false);
/// HWND the z-order guard watches, and whether the button is on screen at all.
static GUARD_HWND: AtomicIsize = AtomicIsize::new(0);
/// Current owner (Shell_TrayWnd), 0 while the bar is sliding. Owning the tile
/// keeps it above the taskbar in the Start menu's z-band, but makes
/// `SetWindowPos` lag a frame behind the bar during auto-hide — so ownership is
/// released for the duration of a slide and re-attached once it settles.
static OWNER: AtomicIsize = AtomicIsize::new(0);
/// The bar watch is following a moving bar; do not re-own until it stops.
static BAR_MOVING: AtomicBool = AtomicBool::new(false);
static GUARD_ON: AtomicBool = AtomicBool::new(false);
static GUARD_STARTED: AtomicBool = AtomicBool::new(false);
/// Auto-hide is on and the box is enabled; the bar slide watch runs only then.
static AUTO_HIDE: AtomicBool = AtomicBool::new(false);
static WATCH_STARTED: AtomicBool = AtomicBool::new(false);

#[repr(C)]
struct TrackMouseEvent {
    cb_size: u32,
    flags: u32,
    hwnd: isize,
    hover_time: u32,
}

#[repr(C)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_pels: i32,
    y_pels: i32,
    clr_used: u32,
    clr_important: u32,
}

#[repr(C)]
struct BitmapInfo {
    header: BitmapInfoHeader,
    colors: [u32; 1],
}

#[repr(C)]
struct BlendFunction {
    blend_op: u8,
    blend_flags: u8,
    source_constant_alpha: u8,
    alpha_format: u8,
}

#[link(name = "user32")]
extern "system" {
    fn RegisterClassW(class: *const w::WNDCLASSW) -> u16;
    fn CreateWindowExW(
        ex_style: isize,
        class: *const u16,
        name: *const u16,
        style: isize,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: isize,
        menu: isize,
        instance: isize,
        param: *mut core::ffi::c_void,
    ) -> isize;
    fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
    fn UpdateLayeredWindow(
        hwnd: isize,
        dst_dc: isize,
        dst: *const w::POINT,
        size: *const w::SIZE,
        src_dc: isize,
        src: *const w::POINT,
        color_key: u32,
        blend: *const BlendFunction,
        flags: u32,
    ) -> i32;
    fn TrackMouseEvent(event: *mut TrackMouseEvent) -> i32;
    fn LoadCursorW(instance: isize, name: *const u16) -> isize;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateCompatibleDC(dc: isize) -> isize;
    fn CreateDIBSection(
        dc: isize,
        info: *const BitmapInfo,
        usage: u32,
        bits: *mut *mut core::ffi::c_void,
        section: isize,
        offset: u32,
    ) -> isize;
    fn SelectObject(dc: isize, obj: isize) -> isize;
    fn DeleteObject(obj: isize) -> i32;
    fn DeleteDC(dc: isize) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> isize;
}

/// `IDC_HAND` as a `MAKEINTRESOURCE` pointer (32649).
const IDC_HAND_PTR: *const u16 = 32649 as *const u16;

/// Icon pixels (RGBA, 64×64) — the same bitmap the window icon uses.
const ICON: &[u8] = include_bytes!("../../../brand/embed/box-64.rgba");
const ICON_PX: u32 = 64;

pub struct Tile {
    hwnd: isize,
    /// Physical size of the surface.
    size: (i32, i32),
    /// Last painted state, so a repaint only happens on a real change.
    painted: Option<(bool, bool, (i32, i32))>,
    placed: Option<[i32; 4]>,
    visible: bool,
}

impl Tile {
    pub fn new() -> Self {
        let mut tile = Tile {
            hwnd: 0,
            size: (0, 0),
            painted: None,
            placed: None,
            visible: false,
        };
        tile.create();
        start_bar_watch();
        tile
    }

    fn create(&mut self) {
        unsafe {
            let instance = GetModuleHandleW(core::ptr::null());
            let class_name = w::wide("DCX.BoxTile");
            let wc = w::WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(proc_),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: instance,
                hIcon: 0,
                hCursor: LoadCursorW(0, IDC_HAND_PTR),
                hbrBackground: 0,
                lpszMenuName: core::ptr::null(),
                lpszClassName: class_name.as_ptr(),
            };
            RegisterClassW(&wc);
            let title = w::wide("DCX 收纳盒按钮");
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                class_name.as_ptr(),
                title.as_ptr(),
                0x8000_0000, // WS_POPUP
                0,
                0,
                10,
                10,
                0,
                0,
                instance,
                core::ptr::null_mut(),
            );
            if hwnd == 0 {
                return;
            }
            self.hwnd = hwnd;
            // A recreated window must publish/repaint from scratch.
            self.size = (0, 0);
            self.painted = None;
            self.placed = None;
            self.visible = false;
            GUARD_HWND.store(hwnd, Ordering::SeqCst);
            OWNER.store(0, Ordering::Relaxed);
            bind_owner(hwnd);
            start_guard();
        }
    }

    /// Explorer restarts destroy Shell_TrayWnd, and with it an owned window.
    /// Recreate the tile and re-own it before the next placement.
    fn ensure_alive(&mut self) {
        let alive = self.hwnd != 0 && unsafe { w::IsWindow(self.hwnd) } != 0;
        if !alive {
            crate::win::trace("tile window gone; recreating");
            self.create();
            return;
        }
        let owner = OWNER.load(Ordering::Relaxed);
        if owner != 0 && unsafe { w::IsWindow(owner) } == 0 {
            OWNER.store(0, Ordering::Relaxed);
        }
        if OWNER.load(Ordering::Relaxed) == 0 && !BAR_MOVING.load(Ordering::Relaxed) {
            bind_owner(self.hwnd);
        }
    }

    pub fn is_ready(&self) -> bool {
        self.hwnd != 0
    }

    /// Place the button at `rect` (screen points) and publish its state.
    pub fn sync(&mut self, rect: egui::Rect, scale: f32, open: bool, visible: bool) {
        self.ensure_alive();
        if self.hwnd == 0 {
            return;
        }
        let px = [
            (rect.min.x * scale).round() as i32,
            (rect.min.y * scale).round() as i32,
            (rect.width() * scale).round().max(1.0) as i32,
            (rect.height() * scale).round().max(1.0) as i32,
        ];
        let size = (px[2], px[3]);
        OPEN.store(open, Ordering::Relaxed);
        GUARD_ON.store(visible, Ordering::Relaxed);
        if visible != self.visible {
            self.visible = visible;
            // 4 = SW_SHOWNOACTIVATE: a taskbar button must not take focus.
            unsafe { ShowWindow(self.hwnd, if visible { 4 } else { 0 }) };
        }
        if !visible {
            return;
        }
        if self.placed != Some(px) {
            self.placed = Some(px);
            unsafe {
                w::SetWindowPos(
                    self.hwnd,
                    HWND_TOPMOST,
                    px[0],
                    px[1],
                    px[2],
                    px[3],
                    SWP_NOACTIVATE,
                );
            }
        }
        let hover = HOVER.load(Ordering::Relaxed);
        if self.painted != Some((open, hover, size)) {
            self.paint(size, open, hover, scale);
            self.painted = Some((open, hover, size));
        }
    }

    /// Hide the button when there is no taskbar rectangle to place it on
    /// (Explorer restarting, or auto-hide parking the bar off-screen).
    pub fn hide(&mut self) {
        if self.hwnd == 0 {
            return;
        }
        OPEN.store(false, Ordering::Relaxed);
        GUARD_ON.store(false, Ordering::Relaxed);
        if self.visible {
            self.visible = false;
            unsafe { ShowWindow(self.hwnd, 0) };
        }
    }

    /// Repaint after a hover change without waiting for the next layout pass.
    pub fn repaint_if_needed(&mut self, scale: f32) {
        self.ensure_alive();
        if self.hwnd == 0 || !self.visible {
            return;
        }
        let state = (
            OPEN.load(Ordering::Relaxed),
            HOVER.load(Ordering::Relaxed),
            self.size,
        );
        if self.painted != Some(state) {
            self.paint(self.size, state.0, state.1, scale);
            self.painted = Some(state);
        }
    }

    fn paint(&mut self, size: (i32, i32), open: bool, hover: bool, scale: f32) {
        if size.0 <= 0 || size.1 <= 0 {
            return;
        }
        let pixels = compose(size, open, hover, scale);
        unsafe {
            let screen = w::GetDC(0);
            if screen == 0 {
                return;
            }
            let mem = CreateCompatibleDC(screen);
            if mem == 0 {
                w::ReleaseDC(0, screen);
                return;
            }
            let info = BitmapInfo {
                header: BitmapInfoHeader {
                    size: core::mem::size_of::<BitmapInfoHeader>() as u32,
                    width: size.0,
                    height: -size.1, // top-down
                    planes: 1,
                    bit_count: 32,
                    compression: 0,
                    size_image: 0,
                    x_pels: 0,
                    y_pels: 0,
                    clr_used: 0,
                    clr_important: 0,
                },
                colors: [0],
            };
            let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
            let bmp = CreateDIBSection(mem, &info, DIB_RGB_COLORS, &mut bits, 0, 0);
            if bmp != 0 && !bits.is_null() {
                core::ptr::copy_nonoverlapping(
                    pixels.as_ptr(),
                    bits as *mut u8,
                    pixels.len().min((size.0 * size.1 * 4) as usize),
                );
                let old = SelectObject(mem, bmp);
                let src = w::POINT { x: 0, y: 0 };
                let sz = w::SIZE {
                    cx: size.0,
                    cy: size.1,
                };
                let blend = BlendFunction {
                    blend_op: AC_SRC_OVER,
                    blend_flags: 0,
                    source_constant_alpha: 255,
                    alpha_format: AC_SRC_ALPHA,
                };
                let mut origin = w::POINT { x: 0, y: 0 };
                let mut rc = w::RECT::default();
                w::GetWindowRect(self.hwnd, &mut rc);
                origin.x = rc.left;
                origin.y = rc.top;
                UpdateLayeredWindow(
                    self.hwnd, screen, &origin, &sz, mem, &src, 0, &blend, ULW_ALPHA,
                );
                SelectObject(mem, old);
                DeleteObject(bmp);
            }
            DeleteDC(mem);
            w::ReleaseDC(0, screen);
        }
        self.size = size;
    }
}

/// One frame of the button: rounded plate, icon, running indicator.
///
/// Premultiplied BGRA, top-down — what `UpdateLayeredWindow` expects.
fn compose(size: (i32, i32), open: bool, hover: bool, scale: f32) -> Vec<u8> {
    let (width, height) = (size.0 as usize, size.1 as usize);
    let mut buf = vec![0u8; width * height * 4];
    let s = crate::view::Skin::system();

    // Plate: inset 1 pt from the button cell on every side. The cell now
    // follows the dock row (30–44 DIP); a fixed 40 pt plate overflowed the
    // taskbar when it was short and touched the edges when it was tall.
    let pad = (1.0 * scale).round().max(1.0);
    let plate = (width as f32 - pad * 2.0)
        .min(height as f32 - pad * 2.0)
        .max(1.0);
    let (cx, cy) = (width as f32 / 2.0, height as f32 / 2.0);
    let (x0, y0) = (cx - plate / 2.0, cy - plate / 2.0);
    let radius = (8.0 * scale).min(plate * 0.28);
    // Even at rest the plate is filled with a 1/255 white: a layered window is
    // hit-tested against its **alpha**, so a fully transparent cell would make
    // only the 24 pt icon clickable and every click near the edge fall through
    // to the taskbar.
    let fill = if open {
        s.accent_soft
    } else if hover {
        s.tile_hover
    } else {
        egui::Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 1)
    };

    if fill.a() > 0 {
        for y in 0..height {
            for x in 0..width {
                let cover = round_rect_coverage(
                    x as f32 + 0.5,
                    y as f32 + 0.5,
                    x0,
                    y0,
                    plate,
                    plate,
                    radius,
                );
                if cover <= 0.0 {
                    continue;
                }
                let a = fill.a() as f32 / 255.0 * cover;
                let px = &mut buf[(y * width + x) * 4..(y * width + x) * 4 + 4];
                px[0] = (fill.b() as f32 * a) as u8;
                px[1] = (fill.g() as f32 * a) as u8;
                px[2] = (fill.r() as f32 * a) as u8;
                px[3] = (255.0 * a) as u8;
            }
        }
    }

    // Icon: 24 pt, bilinear from the 64 px source.
    let icon_px = (24.0 * scale).round().max(1.0);
    let ix = (cx - icon_px / 2.0).round() as i32;
    let iy = (cy - icon_px / 2.0).round() as i32;
    blit_icon(&mut buf, width, height, ix, iy, icon_px as i32);

    // Running indicator under the plate when the panel is open.
    if open {
        let bar_w = (10.0 * scale).round().max(4.0);
        let bar_h = (2.5 * scale).round().max(2.0);
        let bx = cx - bar_w / 2.0;
        let by = y0 + plate - bar_h;
        let accent = s.accent;
        for y in 0..height {
            for x in 0..width {
                let px = &mut buf[(y * width + x) * 4..(y * width + x) * 4 + 4];
                let cover = round_rect_coverage(
                    x as f32 + 0.5,
                    y as f32 + 0.5,
                    bx,
                    by,
                    bar_w,
                    bar_h,
                    bar_h / 2.0,
                );
                if cover > 0.0 {
                    let a = cover;
                    let src = [accent.b(), accent.g(), accent.r()];
                    for c in 0..3 {
                        px[c] = ((px[c] as f32) * (1.0 - a) + src[c] as f32 * a) as u8;
                    }
                    px[3] = (px[3] as f32 * (1.0 - a) + 255.0 * a) as u8;
                }
            }
        }
    }
    buf
}

fn round_rect_coverage(x: f32, y: f32, rx: f32, ry: f32, rw: f32, rh: f32, radius: f32) -> f32 {
    if x < rx || y < ry || x > rx + rw || y > ry + rh {
        return 0.0;
    }
    let r = radius.min(rw / 2.0).min(rh / 2.0);
    let dx = if x < rx + r {
        rx + r - x
    } else if x > rx + rw - r {
        x - (rx + rw - r)
    } else {
        0.0
    };
    let dy = if y < ry + r {
        ry + r - y
    } else if y > ry + rh - r {
        y - (ry + rh - r)
    } else {
        0.0
    };
    if dx == 0.0 || dy == 0.0 {
        return 1.0;
    }
    let d = (dx * dx + dy * dy).sqrt();
    (r - d + 0.5).clamp(0.0, 1.0)
}

fn blit_icon(buf: &mut [u8], width: usize, height: usize, ox: i32, oy: i32, dst: i32) {
    if dst <= 0 {
        return;
    }
    for y in 0..dst {
        for x in 0..dst {
            let px = ox + x;
            let py = oy + y;
            if px < 0 || py < 0 || px as usize >= width || py as usize >= height {
                continue;
            }
            // Bilinear sample of the 64×64 source.
            let fx = (x as f32 + 0.5) * ICON_PX as f32 / dst as f32 - 0.5;
            let fy = (y as f32 + 0.5) * ICON_PX as f32 / dst as f32 - 0.5;
            let x0 = fx.floor().max(0.0) as usize;
            let y0 = fy.floor().max(0.0) as usize;
            let x1 = (x0 + 1).min(ICON_PX as usize - 1);
            let y1 = (y0 + 1).min(ICON_PX as usize - 1);
            let tx = (fx - x0 as f32).clamp(0.0, 1.0);
            let ty = (fy - y0 as f32).clamp(0.0, 1.0);
            let mut rgba = [0f32; 4];
            for (dx, dy, wgt) in [
                (x0, y0, (1.0 - tx) * (1.0 - ty)),
                (x1, y0, tx * (1.0 - ty)),
                (x0, y1, (1.0 - tx) * ty),
                (x1, y1, tx * ty),
            ] {
                let i = (dy * ICON_PX as usize + dx) * 4;
                for c in 0..4 {
                    rgba[c] += ICON[i + c] as f32 * wgt;
                }
            }
            let a = rgba[3] / 255.0;
            if a <= 0.0 {
                continue;
            }
            let o = (py as usize * width + px as usize) * 4;
            // Source is RGBA, the layed surface is premultiplied BGRA: the red
            // and blue channels have to swap or the brand mark renders crimson.
            for c in 0..3 {
                let src = rgba[2 - c] * a;
                buf[o + c] = (buf[o + c] as f32 * (1.0 - a) + src).min(255.0) as u8;
            }
            buf[o + 3] = (buf[o + 3] as f32 * (1.0 - a) + rgba[3]).min(255.0) as u8;
        }
    }
}

unsafe extern "system" fn proc_(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_LBUTTONUP => {
            CLICK.store(true, Ordering::SeqCst);
            wake();
            0
        }
        WM_RBUTTONUP => {
            SECONDARY.store(true, Ordering::SeqCst);
            wake();
            0
        }
        WM_MOUSEMOVE => {
            let mut ev = TrackMouseEvent {
                cb_size: core::mem::size_of::<TrackMouseEvent>() as u32,
                flags: TME_LEAVE,
                hwnd,
                hover_time: 0,
            };
            TrackMouseEvent(&mut ev);
            if !HOVER.swap(true, Ordering::SeqCst) {
                wake();
            }
            0
        }
        WM_MOUSELEAVE => {
            HOVER.store(false, Ordering::SeqCst);
            wake();
            0
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE,
        WM_NCHITTEST => HTCLIENT,
        WM_ERASEBKGND => 1,
        WM_DESTROY => {
            CLICK.store(false, Ordering::SeqCst);
            0
        }
        _ => w::DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn find_bar() -> isize {
    let cls = w::wide("Shell_TrayWnd");
    unsafe { w::FindWindowW(cls.as_ptr(), core::ptr::null()) }
}

fn bar_parked(bar: isize) -> bool {
    if bar == 0 {
        return true;
    }
    let mut rc = w::RECT::default();
    if unsafe { w::GetWindowRect(bar, &mut rc) } == 0 {
        return true;
    }
    (unsafe { w::GetSystemMetrics(1) - rc.top }) <= 3
}

/// Follow the taskbar's z-band. The owner is a different process, which
/// Windows allows for a top-level popup. Skipped while the bar is parked: an
/// owner set on the hidden bar would only be released again on the next slide.
fn bind_owner(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    let bar = find_bar();
    if bar == 0 || bar_parked(bar) || OWNER.load(Ordering::Relaxed) == bar {
        return;
    }
    let prev = unsafe { w::SetWindowLongPtrW(hwnd, w::GWLP_HWNDPARENT, bar) };
    OWNER.store(bar, Ordering::Relaxed);
    crate::win::trace(&format!("tile owner -> {bar} prev={prev}"));
}

/// Drop the owner while the bar moves: an owned window's position updates are
/// synchronized with the owner's animation and lag behind the dock.
fn clear_owner(hwnd: isize) {
    if hwnd == 0 || OWNER.swap(0, Ordering::Relaxed) == 0 {
        return;
    }
    unsafe { w::SetWindowLongPtrW(hwnd, w::GWLP_HWNDPARENT, 0) };
    crate::win::trace("tile owner cleared (bar moving)");
}

/// Start the z-order guard once: a timer-rate poll instead of a frame-rate one.
///
/// The tile is owned by `Shell_TrayWnd`, so the Start menu's higher z-band no
/// longer buries it (see [`Tile::bind_owner`]). This guard remains a fallback
/// for other topmost windows the shell raises when the tray or clock is used;
/// because `sync` only raises when the *position* changes, the button used to
/// stay buried for the rest of the session. Watching this from the egui loop
/// would mean running that loop at 60 Hz whenever the pointer is on the bar
/// (~47 % of a core even with nothing drawn), so the check lives on its own
/// thread: three cheap calls per tick.
fn start_guard() {
    if GUARD_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_millis(8));
        if !GUARD_ON.load(Ordering::Relaxed) {
            continue;
        }
        let hwnd = GUARD_HWND.load(Ordering::Relaxed);
        if hwnd != 0 {
            raise_if_buried(hwnd);
        }
    });
}

/// Wake the frame loop while an auto-hidden taskbar is sliding.
///
/// Windows animates the bar by moving `Shell_TrayWnd` down off the screen over
/// roughly 300-450 ms. The box idles at one frame per 200 ms, so sampling that
/// from the frame loop left the button frozen while the bar moved and only
/// snapped it into place afterwards. A timer-rate loop would fix the follow but
/// costs ~half a core (measured), so the bar's rectangle is watched here and
/// the idle loop is woken only when it actually changes.
pub fn set_auto_hide(on: bool) {
    AUTO_HIDE.store(on, Ordering::Relaxed);
}

fn start_bar_watch() {
    if WATCH_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        let cls = w::wide("Shell_TrayWnd");
        let mut last: Option<(i32, i32)> = None;
        let mut last_change = std::time::Instant::now();
        let mut last_wake = std::time::Instant::now() - std::time::Duration::from_millis(20);
        loop {
            let auto = AUTO_HIDE.load(Ordering::Relaxed);
            // Nothing to watch with auto-hide off; do not spin at timer rate.
            std::thread::sleep(std::time::Duration::from_millis(if auto { 8 } else { 200 }));
            if !auto {
                last = None;
                BAR_MOVING.store(false, Ordering::Relaxed);
                continue;
            }
            let bar = unsafe { w::FindWindowW(cls.as_ptr(), core::ptr::null()) };
            if bar == 0 {
                last = None;
                BAR_MOVING.store(false, Ordering::Relaxed);
                continue;
            }
            let mut rc = w::RECT::default();
            if unsafe { w::GetWindowRect(bar, &mut rc) } == 0 {
                continue;
            }
            let now_rect = (rc.top, rc.bottom);
            if last == Some(now_rect) {
                // The slide is over: re-attach the owner so the Start menu's
                // z-band cannot cover the button. Do not re-own while parked.
                if BAR_MOVING.load(Ordering::Relaxed)
                    && last_change.elapsed() >= std::time::Duration::from_millis(120)
                {
                    BAR_MOVING.store(false, Ordering::Relaxed);
                    bind_owner(GUARD_HWND.load(Ordering::Relaxed));
                }
                continue;
            }
            last = Some(now_rect);
            last_change = std::time::Instant::now();
            let hwnd = GUARD_HWND.load(Ordering::Relaxed);
            // Fully out (sliver ≈ bar height) means this is a Start-menu raise
            // or pixel jitter, not an auto-hide slide: keep/fetch the owner so
            // the Start z-band cannot bury the button. Only release ownership
            // while the dock is actually sliding.
            let fully_out =
                (unsafe { w::GetSystemMetrics(1) } - rc.top) >= rc.height() - 2;
            if fully_out {
                BAR_MOVING.store(false, Ordering::Relaxed);
                bind_owner(hwnd);
            } else {
                BAR_MOVING.store(true, Ordering::Relaxed);
                clear_owner(hwnd);
            }
            // Move the layered window from this thread. The egui loop is
            // throttled while its panel window is parked off-screen, so
            // waiting for a frame made the tile jump in ~60–100 ms steps and
            // stay a beat behind the bar. This is two cheap calls per bar
            // change and independent of the repaint cadence. Ownership is
            // dropped for real slides or the same calls would lag behind.
            follow_bar(&rc);
            // Wake the app at most every ~16 ms for the rest of its state
            // (slot/anchor, visibility); the tile itself already moved.
            if last_wake.elapsed() < std::time::Duration::from_millis(16) {
                continue;
            }
            last_wake = std::time::Instant::now();
            wake();
        }
    });
}

/// Keep the button vertically centred while `Shell_TrayWnd` slides. The x and
/// the size come from the last frame (the anchor only changes when the task
/// list changes), so this cannot fight the layout code.
fn follow_bar(bar: &w::RECT) {
    let hwnd = GUARD_HWND.load(Ordering::Relaxed);
    if hwnd == 0 {
        return;
    }
    let mut tr = w::RECT::default();
    if unsafe { w::GetWindowRect(hwnd, &mut tr) } == 0 {
        return;
    }
    let target = bar.top + (bar.height() - tr.height()) / 2;
    if (tr.top - target).abs() <= 1 {
        return;
    }
    unsafe {
        w::SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            tr.left,
            target,
            tr.width(),
            tr.height(),
            SWP_NOACTIVATE,
        );
    }
}

/// Re-assert the topmost slot when another process owns the button's pixels.
/// Raising only on a real conflict keeps this from fighting the shell's own
/// animations the way a blind raise every tick would.
fn raise_if_buried(hwnd: isize) {
    let mut rc = w::RECT::default();
    if unsafe { w::GetWindowRect(hwnd, &mut rc) } == 0 {
        return;
    }
    let pt = w::POINT {
        x: (rc.left + rc.right) / 2,
        y: (rc.top + rc.bottom) / 2,
    };
    let hit = unsafe { w::WindowFromPoint(pt) };
    if hit == 0 {
        return;
    }
    let root = unsafe { w::GetAncestor(hit, w::GA_ROOT) };
    if root == hwnd {
        return;
    }
    // Our own windows (the panel, the drag ghost) may legitimately sit on top
    // of the button; only somebody else covering it is worth a raise.
    let mut pid: u32 = 0;
    unsafe { w::GetWindowThreadProcessId(root, &mut pid) };
    if pid == unsafe { w::GetCurrentProcessId() } {
        return;
    }
    unsafe {
        w::SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// Ask egui for the next frame. `request_repaint` is thread-safe and posts a
/// wake-up to the winit loop, which is what turns a click into an immediate
/// reaction instead of waiting for the 200 ms idle tick.
fn wake() {
    if let Ok(g) = CTX.lock() {
        if let Some(ctx) = g.as_ref() {
            ctx.request_repaint();
        }
    }
}

pub fn bind_ctx(ctx: &Context) {
    if let Ok(mut g) = CTX.lock() {
        *g = Some(ctx.clone());
    }
}

pub fn take_click() -> bool {
    CLICK.swap(false, Ordering::SeqCst)
}

pub fn take_secondary() -> bool {
    SECONDARY.swap(false, Ordering::SeqCst)
}

pub fn hovered() -> bool {
    HOVER.load(Ordering::SeqCst)
}

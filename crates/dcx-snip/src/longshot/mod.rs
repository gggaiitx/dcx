//! Scrolling long capture, AirSnip-style.
//!
//! The overlay **closes** when the capture starts. What stays on screen is:
//!
//! * a 2 px viewport border (click-through, so the page behind keeps working),
//! * a small floating action bar (no-activate: it never steals focus),
//! * a thumbnail card beside the viewport that grows as the stitch grows.
//!
//! The user scrolls the target app with the wheel. Every 60 ms the same screen
//! rectangle is grabbed and row-matched into the stitched image. There is no
//! "next segment" step and no automatic scrolling: the page is the UI.
//! Enter copies, Ctrl+S saves, Esc closes; the bar buttons do the same.
//!
//! The bar and preview are layered windows because they must draw rounded,
//! semi-transparent chrome straight over someone else's window. The border
//! uses a colour key instead: a viewport-sized alpha surface would be 8 MB of
//! pixels for a 2 px frame.

mod bar;
mod border;
mod preview;

use std::cell::Cell;
use std::time::Duration;

use dcx_win32::{HWND, MSG, POINT, RECT, SIZE};
use crate::ffi::*;
use crate::stitch::{Step, Stitcher};
use crate::surface::Surface;

/// Capture cadence. A fast wheel flick moves a lot between grabs; 60 ms keeps
/// the overlap inside the matcher's range where 90 ms sometimes lost it.
const TICK_MS: u32 = 60;
/// Let DWM finish removing the overlay (and the page settle after regaining
/// focus) before the first frame is grabbed; otherwise the seed frame is the
/// dimmed screenshot itself or a mid-activation repaint.
const SEED_DELAY_MS: u64 = 200;
const HINT_MS: u64 = 2200;
const DEFAULT_HINT: &str = "滚轮滚动页面，自动拼接";
const TIMER_ID: usize = 1;

const HK_ENTER: i32 = 601;
const HK_ESC: i32 = 602;
const HK_UNDO: i32 = 603;
const HK_SAVE: i32 = 604;

thread_local! {
    static CURRENT: Cell<*mut Long> = const { Cell::new(core::ptr::null_mut()) };
}

/// Integer window rectangle in virtual-screen coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoxRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl BoxRect {
    pub fn right(&self) -> i32 {
        self.x + self.w as i32
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h as i32
    }
    pub fn intersects(&self, o: &BoxRect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
}

/// Everything the capture needs from the overlay that started it.
pub struct Ctx {
    /// Selected rectangle in virtual-screen coordinates.
    pub region: (i32, i32, u32, u32),
    pub scale: f32,
    /// Output corner radius, physical pixels.
    pub radius: f32,
    pub out_dir: std::path::PathBuf,
    pub jpeg: bool,
    pub quality: u8,
}

fn now_ms() -> u64 {
    unsafe { GetTickCount64() }
}

pub(super) fn px(v: f32, scale: f32) -> i32 {
    (v * scale).round() as i32
}

fn point_of(lp: isize) -> (f32, f32) {
    let x = (lp & 0xFFFF) as u16 as i16;
    let y = ((lp >> 16) & 0xFFFF) as u16 as i16;
    (x as f32, y as f32)
}

/// True while a long capture owns the screen.
pub fn is_active() -> bool {
    CURRENT.with(|c| !c.get().is_null())
}

/// Host stop / hotkey while capturing: close the capture, not the process.
pub fn request_close() {
    let p = CURRENT.with(|c| c.get());
    if !p.is_null() {
        unsafe {
            PostMessageW((*p).bar, WM_CLOSE, 0, 0);
        }
    }
}

/// Run the long capture until copy/save/close. Returns `false` when the
/// floating windows could not be created (the caller restores the overlay).
pub fn run(ctx: Ctx) -> bool {
    let mut long = match Long::spawn(ctx) {
        Ok(long) => long,
        Err(e) => {
            crate::app::log(&format!("longshot: {e}"));
            return false;
        }
    };
    let ptr = &mut long as *mut Long;
    unsafe {
        dcx_win32::SetWindowLongPtrW(long.bar, GWLP_USERDATA, ptr as isize);
    }
    CURRENT.with(|c| c.set(ptr));
    long.show();

    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, 0, 0, 0) };
        if r <= 0 {
            break;
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if long.done {
            break;
        }
    }

    CURRENT.with(|c| c.set(core::ptr::null_mut()));
    unsafe {
        dcx_win32::SetWindowLongPtrW(long.bar, GWLP_USERDATA, 0);
    }
    true
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum End {
    Copy,
    Save,
}

struct Long {
    region: BoxRect,
    scale: f32,
    radius: f32,
    out_dir: std::path::PathBuf,
    jpeg: bool,
    quality: u8,
    bar: HWND,
    bar_geom: bar::Bar,
    bar_surface: Surface,
    border: Option<HWND>,
    preview: Option<preview::Preview>,
    stitcher: Option<Stitcher>,
    /// Transient message: (text, is_error, hidden_after).
    hint: Option<(String, bool, u64)>,
    hover: Option<bar::Btn>,
    pressed: Option<bar::Btn>,
    started: u64,
    /// Height currently drawn in the bar/preview.
    shown_h: u32,
    /// Consecutive failed scrolls; one failure is usually a repaint in flight.
    fail_run: u32,
    /// Last time a failure hint was shown (rate limit).
    fail_at: u64,
    /// Height cap reached: keep the bar for save/close, stop grabbing.
    limit: bool,
    done: bool,
}

impl Long {
    fn spawn(ctx: Ctx) -> Result<Long, String> {
        let (sx, sy, w, h) = ctx.region;
        if w == 0 || h == 0 {
            return Err("选区为空".into());
        }
        let region = BoxRect { x: sx, y: sy, w, h };
        let screen = crate::capture::virtual_screen();
        let bar = bar::layout(region, screen, ctx.scale);
        let surface =
            Surface::new(bar.rect.w, bar.rect.h).ok_or_else(|| "无法创建状态条表面".to_string())?;
        let bar_hwnd = create_bar(bar.rect)?;

        let mut long = Long {
            region,
            scale: ctx.scale,
            radius: ctx.radius,
            out_dir: ctx.out_dir,
            jpeg: ctx.jpeg,
            quality: ctx.quality,
            bar: bar_hwnd,
            bar_geom: bar,
            bar_surface: surface,
            border: None,
            preview: None,
            stitcher: None,
            hint: None,
            hover: None,
            pressed: None,
            started: now_ms(),
            shown_h: 0,
            fail_run: 0,
            fail_at: 0,
            limit: false,
            done: false,
        };
        // Created after the bar: they never overlap it, and creation order then
        // puts the interactive bar on top of the topmost band.
        long.border = border::spawn(region, ctx.scale);
        long.preview = preview::Preview::spawn(region, screen, ctx.scale);
        long.render();
        Ok(long)
    }

    fn show(&mut self) {
        self.started = now_ms();
        unsafe {
            ShowWindow(self.bar, SW_SHOWNA);
            dcx_win32::SetWindowPos(
                self.bar,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            register_hotkeys(self.bar);
            SetTimer(self.bar, TIMER_ID, TICK_MS, core::ptr::null());
        }
        self.render();
    }

    fn state(&self) -> (String, bool) {
        let now = now_ms();
        match &self.hint {
            Some((text, error, until)) if now < *until => (text.clone(), *error),
            _ => (DEFAULT_HINT.to_string(), false),
        }
    }

    fn render(&mut self) {
        let (hint, error) = self.state();
        let height = self.stitcher.as_ref().map(|s| s.height()).unwrap_or(0);
        let can_undo = self.stitcher.as_ref().is_some_and(|s| s.can_undo());
        let state = bar::State {
            can_undo,
            hint: &hint,
            error,
            height,
            hover: self.hover,
            pressed: self.pressed,
        };
        // Preview first: it is the surface the user watches while scrolling.
        if let Some(preview) = self.preview.as_mut() {
            preview.update(self.stitcher.as_ref());
        }
        bar::render(&mut self.bar_surface, &self.bar_geom, &state, self.scale);
        unsafe {
            present_layered(self.bar, &self.bar_surface);
        }
        self.shown_h = height;
    }

    fn set_hint(&mut self, text: impl Into<String>, error: bool) {
        self.hint = Some((text.into(), error, now_ms() + HINT_MS));
        self.render();
    }

    fn tick(&mut self) {
        if self.done || self.limit {
            return;
        }
        let now = now_ms();
        if now < self.started + SEED_DELAY_MS {
            return;
        }
        if matches!(&self.hint, Some((_, _, until)) if now >= *until) {
            self.hint = None;
            self.render();
        }
        self.capture();
    }

    /// Grab the viewport once. Identical frames are the normal state between
    /// wheel notches, so they cost one compare and nothing else.
    fn capture(&mut self) {
        let region = self.region;
        // A bar wedged over the viewport (tiny selection near a screen edge)
        // must not be photographed into every frame.
        let overlaps = self.bar_geom.rect.intersects(&region);
        unsafe {
            if overlaps {
                ShowWindow(self.bar, SW_HIDE);
            }
        }
        if overlaps {
            std::thread::sleep(Duration::from_millis(70));
        }
        let shot = crate::capture::grab_region(region.x, region.y, region.w, region.h);
        unsafe {
            if overlaps {
                ShowWindow(self.bar, SW_SHOWNA);
            }
        }
        let shot = match shot {
            Ok(shot) => shot,
            Err(e) => {
                self.set_hint(format!("抓屏失败：{e}"), true);
                return;
            }
        };

        if self.stitcher.is_none() {
            match Stitcher::new(&shot.bgra, region.w, region.h) {
                Some(st) => {
                    crate::app::log(&format!("longshot: seed {}x{}", region.w, region.h));
                    self.stitcher = Some(st);
                    self.hint = None;
                    self.render();
                }
                None => self.set_hint("无法开始长截图", true),
            }
            return;
        }

        let result = {
            let st = self.stitcher.as_mut().expect("checked above");
            if st.unchanged(&shot.bgra) {
                self.fail_run = 0;
                return;
            }
            st.push(&shot.bgra, region.w, region.h)
        };
        match result {
            Step::Appended(info) => {
                crate::app::log(&format!(
                    "longshot: +{} rows (offset {}, total {})",
                    info.added, info.offset, info.total_h
                ));
                self.fail_run = 0;
                self.hint = None;
                self.render();
            }
            Step::Still => {
                self.fail_run = 0;
                // The first Still is also the moment the seed frame becomes
                // the canvas: show it instead of waiting for a scroll.
                let height = self.stitcher.as_ref().map(|s| s.height()).unwrap_or(0);
                if height != self.shown_h {
                    self.render();
                }
            }
            Step::Failed => self.note_fail("未接上，请放慢滚动速度", true),
            Step::Limit => {
                self.limit = true;
                self.set_hint("已达长度上限，请保存或关闭", true);
            }
        }
    }

    /// One failed grab is usually a repaint still in flight; only nag after a
    /// second consecutive failure, and at most every few seconds.
    fn note_fail(&mut self, text: impl Into<String>, error: bool) {
        self.fail_run += 1;
        let now = now_ms();
        if self.fail_run >= 2 && now.saturating_sub(self.fail_at) > 2500 {
            self.fail_at = now;
            self.set_hint(text, error);
        }
    }

    fn act(&mut self, button: bar::Btn) {
        if self.done {
            return;
        }
        match button {
            bar::Btn::Undo => {
                if let Some(st) = self.stitcher.as_mut() {
                    if st.undo_last() {
                        self.limit = false;
                        self.hint = None;
                        self.render();
                    }
                }
            }
            bar::Btn::Copy => self.finish(End::Copy),
            bar::Btn::Save => self.finish(End::Save),
            bar::Btn::Close => self.done = true,
        }
    }

    fn finish(&mut self, end: End) {
        // One last grab: the user may press Enter in the same instant they
        // stop scrolling, before the 90 ms timer sees the new viewport.
        self.capture();
        // Enter right after starting: the seed is still buffered.
        if let Some(st) = self.stitcher.as_mut() {
            st.settle();
        }
        let Some(st) = self.stitcher.as_ref() else {
            return;
        };
        let (mut bytes, w, h) = st.image();
        if h == 0 || w == 0 {
            self.set_hint("还没有拼接内容", true);
            return;
        }
        if self.radius > 0.5 {
            crate::output::round_alpha(&mut bytes, w, h, self.radius);
        }
        let result = match end {
            End::Copy => crate::output::to_clipboard(self.bar, &bytes, w, h),
            End::Save => crate::output::save(&self.out_dir, &bytes, w, h, self.jpeg, self.quality)
                .map(|_| ()),
        };
        match result {
            Ok(()) => self.done = true,
            Err(e) => self.set_hint(e, true),
        }
    }

    fn on_message(&mut self, hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
        match msg {
            WM_MOUSEACTIVATE => MA_NOACTIVATE,
            WM_LBUTTONDOWN => {
                let (x, y) = point_of(lp);
                let hit = self.bar_geom.hit(x, y);
                unsafe {
                    SetCapture(hwnd);
                }
                self.pressed = hit;
                self.hover = hit;
                self.render();
                0
            }
            WM_MOUSEMOVE => {
                let (x, y) = point_of(lp);
                let hit = self.bar_geom.hit(x, y);
                if hit != self.hover {
                    self.hover = hit;
                    self.render();
                }
                0
            }
            WM_LBUTTONUP => {
                // Read the pressed button *before* releasing capture:
                // ReleaseCapture synchronously sends WM_CAPTURECHANGED, whose
                // handler clears `pressed`, so taking it afterwards always
                // yielded None and every button click was swallowed.
                let pressed = self.pressed.take();
                unsafe {
                    ReleaseCapture();
                }
                let (x, y) = point_of(lp);
                let hit = self.bar_geom.hit(x, y);
                self.hover = hit;
                if pressed == hit {
                    if let Some(button) = hit {
                        self.act(button);
                    }
                }
                self.render();
                0
            }
            WM_CAPTURECHANGED => {
                self.pressed = None;
                self.render();
                0
            }
            WM_TIMER if wp == TIMER_ID => {
                self.tick();
                0
            }
            WM_HOTKEY => {
                match wp as i32 {
                    HK_ENTER => self.act(bar::Btn::Copy),
                    HK_ESC => self.act(bar::Btn::Close),
                    HK_UNDO => self.act(bar::Btn::Undo),
                    HK_SAVE => self.act(bar::Btn::Save),
                    _ => {}
                }
                0
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                unsafe {
                    BeginPaint(hwnd, &mut ps);
                    EndPaint(hwnd, &ps);
                }
                0
            }
            WM_ERASEBKGND => 1,
            WM_SETCURSOR => {
                let arrow = unsafe { LoadCursorW(0, IDC_ARROW as *const u16) };
                unsafe { SetCursor(arrow) };
                1
            }
            WM_CLOSE => {
                self.done = true;
                0
            }
            _ => unsafe { dcx_win32::DefWindowProcW(hwnd, msg, wp, lp) },
        }
    }
}

impl Drop for Long {
    fn drop(&mut self) {
        unsafe {
            KillTimer(self.bar, TIMER_ID);
            unregister_hotkeys(self.bar);
            DestroyWindow(self.bar);
            if let Some(border) = self.border.take() {
                DestroyWindow(border);
            }
        }
    }
}

fn create_bar(rect: BoxRect) -> Result<HWND, String> {
    unsafe {
        let class = crate::wide("DCX.Snip.LongBar");
        let cls = dcx_win32::WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(bar_proc),
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
        let title = crate::wide("DCX 长截图");
        let hwnd = dcx_win32::CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
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
            Err("无法创建长截图状态条".into())
        } else {
            Ok(hwnd)
        }
    }
}

unsafe extern "system" fn bar_proc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    let ptr = unsafe { dcx_win32::GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Long;
    if ptr.is_null() {
        return unsafe { dcx_win32::DefWindowProcW(hwnd, msg, wp, lp) };
    }
    unsafe { (*ptr).on_message(hwnd, msg, wp, lp) }
}

fn register_hotkeys(hwnd: HWND) {
    unsafe {
        let entries = [
            (HK_ENTER, MOD_NOREPEAT, VK_RETURN as u32, "Enter"),
            (HK_ESC, MOD_NOREPEAT, VK_ESCAPE as u32, "Esc"),
            (HK_UNDO, MOD_CONTROL | MOD_NOREPEAT, b'Z' as u32, "Ctrl+Z"),
            (HK_SAVE, MOD_CONTROL | MOD_NOREPEAT, b'S' as u32, "Ctrl+S"),
        ];
        for (id, modifiers, key, name) in entries {
            if RegisterHotKey(hwnd, id, modifiers, key) == 0 {
                crate::app::log(&format!("longshot: hotkey {name} unavailable"));
            }
        }
    }
}

fn unregister_hotkeys(hwnd: HWND) {
    unsafe {
        for id in [HK_ENTER, HK_ESC, HK_UNDO, HK_SAVE] {
            let _ = UnregisterHotKey(hwnd, id);
        }
    }
}

/// `UpdateLayeredWindow` with per-pixel alpha. The caller renders into a
/// `Surface`, so the DIB is already the premultiplied BGRA the API wants.
pub(super) unsafe fn present_layered(hwnd: HWND, surface: &Surface) {
    let mut rect = RECT::default();
    if unsafe { dcx_win32::GetWindowRect(hwnd, &mut rect) } == 0 {
        return;
    }
    let dst = POINT {
        x: rect.left,
        y: rect.top,
    };
    let size = SIZE {
        cx: surface.w as i32,
        cy: surface.h as i32,
    };
    let src = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA,
    };
    unsafe {
        let _ = UpdateLayeredWindow(
            hwnd,
            0,
            &dst,
            &size,
            surface.dc(),
            &src,
            0,
            &blend,
            ULW_ALPHA,
        );
    }
}

/// Offscreen render of the bar and preview for `--demo-synth`. No windows, no
/// desktop capture: the only way to eyeball the chrome without driving a real
/// scroll.
pub fn demo(out: &std::path::Path) -> Result<(), String> {
    let (w, h) = (420u32, 260u32);
    let mut page = vec![0u8; (w as usize) * 900 * 4];
    for y in 0..900u32 {
        for x in 0..w {
            let o = ((y * w + x) * 4) as usize;
            page[o] = (x % 251) as u8;
            page[o + 1] = (y % 251) as u8;
            page[o + 2] = ((x + y) % 241) as u8;
            page[o + 3] = 255;
        }
    }
    let frame = |y: u32| -> Vec<u8> {
        let stride = (w as usize) * 4;
        page[(y as usize) * stride..(y as usize + h as usize) * stride].to_vec()
    };
    let mut st = Stitcher::new(&frame(0), w, h).ok_or("demo stitcher")?;
    for y in [100u32, 200, 300] {
        let step = st.push(&frame(y), w, h);
        if !matches!(step, Step::Appended(_)) {
            return Err("demo stitch failed".into());
        }
    }

    let vp = BoxRect {
        x: 300,
        y: 120,
        w: 700,
        h: 420,
    };
    let screen = (0, 0, 1600, 900);
    let scale = 1.0f32;

    let geom = bar::layout(vp, screen, scale);
    let mut surface = Surface::new(geom.rect.w, geom.rect.h).ok_or("demo bar surface")?;
    let hint = "滚轮滚动页面，自动拼接";
    bar::render(
        &mut surface,
        &geom,
        &bar::State {
            can_undo: true,
            hint,
            error: false,
            height: st.height(),
            hover: Some(bar::Btn::Copy),
            pressed: None,
        },
        scale,
    );
    crate::output::write_png(
        &out.with_file_name(format!(
            "{}-longbar.png",
            out.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default()
        )),
        surface.bytes(),
        surface.w,
        surface.h,
    )?;

    if let Some(rect) = preview::layout(vp, screen, scale) {
        let mut surface = Surface::new(rect.w, rect.h).ok_or("demo preview surface")?;
        preview::render(&mut surface, Some(&st), scale);
        crate::output::write_png(
            &out.with_file_name(format!(
                "{}-longpreview.png",
                out.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default()
            )),
            surface.bytes(),
            surface.w,
            surface.h,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_rect_intersection() {
        let a = BoxRect { x: 0, y: 0, w: 100, h: 100 };
        let b = BoxRect { x: 90, y: 90, w: 50, h: 50 };
        let c = BoxRect { x: 100, y: 0, w: 10, h: 10 };
        assert!(a.intersects(&b));
        assert!(!a.intersects(&c), "edges touching is not an overlap");
        assert!(!c.intersects(&a));
    }
}

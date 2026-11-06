//! macOS-style dock magnification for the Windows 11 taskbar.
//!
//! # What the effect actually is
//!
//! Not a transition: every frame is a pure function of the cursor position.
//! For each task button the distance to the cursor is turned into a scale, and
//! the scaled widths are re-packed left to right so neighbours get pushed aside.
//! The row is then anchored on the widest icon, which is what makes the
//! expansion spread outward from the pointer instead of shoving the whole row.
//!
//! # Why it runs on `RenderTransform`
//!
//! Two other routes were considered and rejected:
//!
//! * Rebuilding a `<CompositeTransform/>` through the XAML parser every frame —
//!   `XamlReader.Load` costs milliseconds; a frame has 8.
//! * Driving the composition `Visual` directly — cheap, but composition offsets
//!   are invisible to XAML hit testing, so once the icons spread apart a click
//!   would land on whatever *layout* cell is under the pointer, i.e. the wrong
//!   app. A dock that launches the wrong app is worse than no dock.
//!
//! Mutating one cached `CompositeTransform` keeps the geometry honest:
//! `RenderTransform` participates in hit testing and invalidates only the render
//! pass, never layout. A frame costs four `put_` calls per moving icon, all
//! in-process vtable calls — this DLL lives inside explorer.
//!
//! # Ownership
//!
//! Every entry in [`State::buttons`] owns a reference to both the element and its
//! transform. `ItemsRepeater` recycles task buttons, so the table is rebuilt
//! whenever the visual tree reports a change, and the cached pointers die with it.

#![allow(non_snake_case)]

use core::ffi::c_void;
use std::sync::Mutex;
use std::time::Instant;

use dcx_xaml::com::{self as xcom, ComPtr as XamlPtr};
use dcx_settings::HoverEffect;
use dcx_xaml::ids::IID_IUIELEMENT10;
use dcx_xaml::xaml;

use crate::xaml::ComPtr;

/// Scale of the icon directly under the cursor. 1.42 on a 24 px icon lands at
/// ~34 px: visibly "picked up", still inside the 48 DIP bar, so nothing has to be
/// clipped or the window resized.
const PEAK: f64 = 1.55;
/// Influence radius, in icon pitches. macOS uses roughly ±3 icons.
const RADIUS_ICONS: f64 = 2.8;
/// Below this a value counts as unchanged and is not written.
const EPS: f64 = 0.002;
/// Frame interval while the cursor is on the dock; with `timeBeginPeriod(1)` the
/// scheduler honours ~6 ms, so the effect tracks at up to ~166 fps.
const FRAME_MS_TRACK: u32 = 6;
/// Frame interval at rest: just enough to notice the cursor arriving.
const FRAME_MS_IDLE: u32 = 40;
const TIMER_ID: usize = 0x0DC0_0001;

/// Spring for the enter/exit envelope. Under-damped on purpose: it reproduces the
/// small overshoot macOS has when the dock wakes up.
const SPRING_K: f64 = 420.0;
const SPRING_DAMPING: f64 = 0.82;

/// Height of the transparent band the dock grows into while hovered, in DIP.
///
/// A 48 DIP taskbar cannot show an icon *leaving* the dock: the window edge is
/// the clip, and the pill already fills it. So the window itself gets taller by
/// this much while the pointer is on the dock, and the pill (`TaskbarFrame` and
/// `SystemTrayFrame`) is pinned back to its normal height at the bottom. The band
/// is empty XAML, therefore transparent — icons scale into it and visibly rise
/// out of the dock, exactly like the reference effect. Nothing else on screen
/// moves: the work area is computed from the pill's top, which does not change.
const POPUP_DIP: f64 = 0.0;

struct Btn {
    /// Owned `IInspectable` of the `Taskbar.TaskListButton`.
    element: ComPtr,
    /// Owned `ICompositeTransform` installed as its `RenderTransform`.
    xf: ComPtr,
    /// Layout left edge and width, in DIP.
    x: f64,
    w: f64,
    /// Height of the icon glyph itself (not the button), in DIP.
    icon_h: f64,
    /// Last written `(scale, tx, ty)`, so a settled frame writes nothing.
    last: (f64, f64, f64),
}

struct State {
    buttons: Vec<Btn>,
    built: bool,
    dirty: bool,
    /// Icon scale from settings; the hover factor multiplies it.
    base_scale: f64,
    /// Envelope 0..1 (with overshoot), and its velocity.
    amp: f64,
    amp_v: f64,
    tracking: bool,
    last_tick: Option<Instant>,
    tray: isize,
    dpi: f64,
    /// Right edge (DIP) the magnified row may not cross: the system tray's left
    /// edge, minus a gap.
    limit_x: f64,
    /// Bar height and the tallest icon glyph, both DIP: together they cap the
    /// magnification so a centred icon never gets clipped by the bar's edges.
    bar_dip: f64,
    icon_h: f64,
    frame_ms: u32,
    frames: u64,
    /// Frames slower than 12 ms, for the diagnostic line.
    slow: u64,
    wrote: u64,
    /// The window is currently grown for the pop-up band.
    grown: bool,
    /// Growth was abandoned for this session (the bar did not survive it).
    growth_off: bool,
    /// The bar has been pinned to the window bottom for the pop-up band.
    pinned: bool,
    /// Which hover behaviour the settings currently ask for.
    mode: HoverEffect,
    /// Window rect captured just before growing, restored on the way out.
    base_rect: (i32, i32, i32, i32),
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static STARTED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

#[link(name = "winmm")]
extern "system" {
    fn timeBeginPeriod(period: u32) -> u32;
    fn timeEndPeriod(period: u32) -> u32;
}

extern "system" {
    fn FindWindowExW(parent: isize, after: isize, class: *const u16, name: *const u16) -> isize;
}

/// Class of the child window that actually hosts the taskbar's XAML island.
///
/// This is the thing that clips: `Shell_TrayWnd` can be made taller, but until
/// this child is made taller too the XAML content keeps its old 60 px surface and
/// every magnified icon is cut off exactly at the old bar top.
const CONTENT_BRIDGE_CLASS: &str = "Windows.UI.Composition.DesktopWindowContentBridge";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Start the frame timer. Must be called from the thread that hosts the taskbar
/// XAML — the timer callback runs there, and XAML may only be written there.
pub fn ensure_started() {
    if STARTED.swap(true, core::sync::atomic::Ordering::SeqCst) {
        return;
    }
    {
        let mut g = match STATE.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if g.is_none() {
            *g = Some(State {
                buttons: Vec::new(),
                built: false,
                dirty: true,
                base_scale: base_scale_from_settings(),
                amp: 0.0,
                amp_v: 0.0,
                tracking: false,
                last_tick: None,
                tray: 0,
                dpi: 1.0,
                limit_x: f64::INFINITY,
                bar_dip: 48.0,
                icon_h: 0.0,
                frame_ms: FRAME_MS_IDLE,
                frames: 0,
                slow: 0,
                wrote: 0,
                grown: false,
                growth_off: false,
                pinned: false,
                mode: HoverEffect::Dock,
                base_rect: (0, 0, 0, 0),
            });
        }
    }
    unsafe {
        dcx_win32::SetTimer(0, TIMER_ID, FRAME_MS_IDLE, timer_proc as *const () as usize);
    }
    crate::log("hover: timer started");
}

fn base_scale_from_settings() -> f64 {
    let r = dcx_settings::Settings::load().icon_scale_ratio() as f64;
    if r > 0.2 {
        r
    } else {
        1.0
    }
}

/// Settings were reloaded: pick the new mode (and geometry) up on the next frame.
pub fn on_settings_changed() {
    mark_dirty();
}

/// A task button was (re)created: refresh geometry on the next idle frame.
pub fn mark_dirty() {
    ensure_started();
    if let Ok(mut g) = STATE.lock() {
        if let Some(st) = g.as_mut() {
            st.dirty = true;
        }
    }
}

unsafe extern "system" fn timer_proc(_hwnd: isize, _msg: u32, _id: usize, _time: u32) {
    tick();
    // `slot` arms a thread timer of its own (`hWnd = 0`), and such a timer dies
    // with the dispatcher thread that created it — XAML recycles those. Measured
    // on 10.0.26200: the slot sampler ran ~8 minutes, then went silent for good
    // while the shell kept running and this very timer kept ticking. Re-arming
    // from here is cheap (a no-op unless the file has gone quiet) and puts the
    // slot timer back on the thread that is demonstrably alive.
    crate::slot::keep_alive();
}

// ---------------------------------------------------------------------------
// One frame
// ---------------------------------------------------------------------------

fn tick() {
    let start = Instant::now();
    let mut guard = match STATE.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let Some(st) = guard.as_mut() else { return };

    // `Background` mode leaves the icons alone entirely: nothing to animate, so
    // do not even read the cursor.
    if st.mode == HoverEffect::Background && !st.dirty {
        if st.tracking {
            st.tracking = false;
            retune(st, FRAME_MS_IDLE);
            unsafe { timeEndPeriod(1) };
        }
        return;
    }

    let dt = match st.last_tick {
        Some(prev) => {
            st.last_tick = Some(start);
            start.duration_since(prev).as_secs_f64().clamp(0.001, 0.05)
        }
        None => {
            st.last_tick = Some(start);
            FRAME_MS_IDLE as f64 / 1000.0
        }
    };

    unsafe {
        if st.tray == 0 || dcx_win32::IsWindow(st.tray) == 0 {
            st.tray = dcx_win32::FindWindowW(wide("Shell_TrayWnd").as_ptr(), core::ptr::null());
            st.built = false;
            st.dirty = true;
            st.amp = 0.0;
            st.amp_v = 0.0;
            if st.tray == 0 {
                return;
            }
            let d = dcx_win32::GetDpiForWindow(st.tray);
            st.dpi = if d == 0 { 1.0 } else { d as f64 / 96.0 };
            crate::log(&format!("hover: tray=0x{:X} dpi={:.2}", st.tray, st.dpi));
        }

        let mut rect = dcx_win32::RECT::default();
        if dcx_win32::GetWindowRect(st.tray, &mut rect) == 0 {
            return;
        }
        let mut pt = dcx_win32::POINT::default();
        if dcx_win32::GetCursorPos(&mut pt) == 0 {
            return;
        }

        // Inside the bar's window rect. Re-read every frame: with auto-hide on,
        // the window slides to `bottom > screen height` while hidden, and growing
        // it in that state would move it further off screen instead of opening a
        // band above the dock.
        let screen_h = dcx_win32::GetSystemMetrics(1);
        let shown = rect.bottom <= screen_h + 2;
        let inside = shown
            && pt.x >= rect.left
            && pt.x < rect.right
            && pt.y >= rect.top
            && pt.y < rect.bottom;

        if inside && !st.grown && !st.growth_off {
            grow(st, &rect);
        }

        if st.dirty && st.amp < 0.01 {
            rebuild(st, &rect);
        }
        if !st.built {
            return;
        }

        // Enter/exit envelope as a damped spring.
        let target = if inside { 1.0 } else { 0.0 };
        let accel =
            SPRING_K * (target - st.amp) - 2.0 * SPRING_DAMPING * SPRING_K.sqrt() * st.amp_v;
        st.amp_v += accel * dt;
        st.amp += st.amp_v * dt;
        if st.amp < 0.0 && st.amp_v < 0.0 {
            st.amp = 0.0;
            st.amp_v = 0.0;
        }
        if st.amp > 1.06 {
            st.amp = 1.06;
        }

        let idle = st.amp == 0.0 && target == 0.0;
        if idle {
            if st.grown {
                shrink(st);
            }
            if st.tracking {
                st.tracking = false;
                pill(st, false);
                retune(st, FRAME_MS_IDLE);
                timeEndPeriod(1);
            }
            st.frames += 1;
            return;
        }

        if !st.tracking {
            st.tracking = true;
            retune(st, FRAME_MS_TRACK);
            timeBeginPeriod(1);
            pill(st, true);
        }

        let mx = (pt.x - rect.left) as f64 / st.dpi;
        solve_and_write(st, mx);
    }

    st.frames += 1;
    if start.elapsed().as_secs_f64() * 1000.0 > 12.0 {
        st.slow += 1;
    }
    if st.frames % 300 == 0 {
        crate::log(&format!(
            "hover: frames={} slow={} writes={} icons={} amp={:.2}",
            st.frames,
            st.slow,
            st.wrote,
            st.buttons.len(),
            st.amp
        ));
    }
}

fn retune(st: &mut State, ms: u32) {
    if st.frame_ms == ms {
        return;
    }
    st.frame_ms = ms;
    unsafe {
        dcx_win32::SetTimer(0, TIMER_ID, ms, timer_proc as *const () as usize);
    }
}

unsafe fn solve_and_write(st: &mut State, mx: f64) {
    let n = st.buttons.len();
    if n == 0 {
        return;
    }
    let base = st.base_scale;

    let mut pitch = 48.0;
    if n > 1 {
        let p = (st.buttons[n - 1].x - st.buttons[0].x) / (n - 1) as f64;
        if p > 4.0 {
            pitch = p;
        }
    }
    let radius = pitch * RADIUS_ICONS;

    // Centred scaling grows in both directions, so the ceiling is the bar itself:
    // `base * peak * icon_h <= bar`. Hitting it lowers the whole curve instead of
    // clipping individual icons, which keeps the wave's shape intact.
    let peak = if st.icon_h > 1.0 && base > 0.0 {
        (st.bar_dip / (st.icon_h * base)).clamp(1.0, PEAK)
    } else {
        PEAK
    };

    // Scale per button: cosine window over the distance to the cursor.
    let mut ss = vec![1.0f64; n];
    for i in 0..n {
        let c = st.buttons[i].x + st.buttons[i].w * base / 2.0;
        let t = ((mx - c).abs() / radius).min(1.0);
        let f = 0.5 * (1.0 + (core::f64::consts::PI * t).cos());
        ss[i] = 1.0 + (peak - 1.0) * st.amp * f;
    }

    // Pack the scaled widths, and pack the rest widths: the difference is a pure
    // displacement. At rest the two agree, so every translation is zero — which
    // is what keeps the dock from jumping when a hover ends.
    let cur = pack(&st.buttons, &ss, base);
    let ones = vec![1.0f64; n];
    let rest = pack(&st.buttons, &ones, base);
    let mut tx: Vec<f64> = (0..n).map(|i| cur[i] - rest[i]).collect();

    // Anchor so the point under the cursor stays put. This has to be a
    // *continuous* function of the cursor: anchoring on the nearest icon made the
    // reference jump by a whole icon every time the pointer crossed a boundary,
    // which read as the entire row shivering.
    let fix = interpolate_anchor(&rest, &tx, mx);
    for t in tx.iter_mut() {
        *t -= fix;
    }

    // Do not spill into the system tray — and, while the file-box tile is parked
    // in the gap after the row, do not spill into that either: the tile is a
    // top-level window of its own and cannot move, so a magnified button would
    // simply be covered by it. Clamping the row's right edge to its *resting*
    // right edge (`rest[last]` is a centre, so add half a button) keeps the
    // magnifier inside the row; the wave loses a little spread near the tray, and
    // everything else behaves as before.
    let last = n - 1;
    let mut limit = st.limit_x;
    if crate::slot::tile_reserved() {
        limit = limit.min(rest[last] + st.buttons[last].w * base / 2.0);
    }
    let right = rest[last] + tx[last] + st.buttons[last].w * base * ss[last] / 2.0;
    if right > limit {
        let room = (limit - rest[last]).max(0.0);
        let want = right - rest[last];
        if want > 0.01 {
            let k = (room / want).clamp(0.0, 1.0);
            for t in tx.iter_mut() {
                *t *= k;
            }
        }
    }

    for i in 0..n {
        let s = base * ss[i];
        st.wrote += xaml::composite_transform_frame(
            XamlPtr(st.buttons[i].xf.0),
            s,
            tx[i],
            0.0,
            &mut st.buttons[i].last,
            EPS,
        ) as u64;
    }
}

/// Centre of every button after scaling its width by `ss[i]` and closing the
/// gaps. `ss == 1` reproduces the laid-out centres exactly.
fn pack(buttons: &[Btn], ss: &[f64], base: f64) -> Vec<f64> {
    let n = buttons.len();
    let mut out = Vec::with_capacity(n);
    let mut cursor = buttons[0].x;
    for i in 0..n {
        let w = buttons[i].w * base * ss[i];
        out.push(cursor + w / 2.0);
        let gap = if i + 1 < n {
            buttons[i + 1].x - (buttons[i].x + buttons[i].w * base)
        } else {
            0.0
        };
        cursor += w + gap;
    }
    out
}

/// The translation that keeps the dock fixed under the cursor, interpolated
/// between the two icons bracketing it.
///
/// Picking the nearest icon instead would make the anchor a step function of the
/// cursor position: a half-pixel move across an icon boundary would shift every
/// icon in the row by a full icon width, once per frame, in both directions.
fn interpolate_anchor(rest: &[f64], tx: &[f64], mx: f64) -> f64 {
    let n = rest.len();
    if n == 0 {
        return 0.0;
    }
    if n == 1 || mx <= rest[0] {
        return tx[0];
    }
    if mx >= rest[n - 1] {
        return tx[n - 1];
    }
    for i in 0..n - 1 {
        if mx >= rest[i] && mx <= rest[i + 1] {
            let span = rest[i + 1] - rest[i];
            let t = if span > 0.001 {
                (mx - rest[i]) / span
            } else {
                0.0
            };
            return tx[i] + (tx[i + 1] - tx[i]) * t;
        }
    }
    tx[n - 1]
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

unsafe fn rebuild(st: &mut State, tray_rect: &dcx_win32::RECT) {
    for b in st.buttons.drain(..) {
        b.xf.release();
        b.element.release();
    }
    st.built = false;
    st.dirty = false;
    st.base_scale = base_scale_from_settings();
    st.mode = dcx_settings::Settings::load().hover_effect;

    let site = crate::tap::SITE.load(std::sync::atomic::Ordering::SeqCst);
    if site.is_null() {
        return;
    }

    let snapshot = crate::elements_snapshot();
    let mut found: Vec<Btn> = Vec::new();
    let mut skipped_off = 0usize;

    for record in snapshot
        .iter()
        .filter(|r| r.name == "TaskListButton" && r.ty.starts_with("Taskbar."))
    {
        let Some(element) = ComPtr(site).get_inspectable(record.handle) else {
            continue;
        };
        // Absolute position: `ActualOffset` is *parent*-relative, so the whole
        // ancestor chain has to be summed before it can be compared to the
        // cursor. The button's own offset alone points into the repeater, which
        // on a centred taskbar starts hundreds of DIP from the screen edge.
        let Some((abs_x, _abs_y)) = absolute_offset(&snapshot, site, record.handle) else {
            element.release();
            continue;
        };
        // `ItemsRepeater` parks recycled buttons far off-screen (measured: x around
        // -9700 DIP). They keep a valid width, so without this filter they enter the
        // row, sort to the front, and drag every centre thousands of pixels away from
        // the cursor — the falloff then evaluates to zero everywhere and the effect
        // silently does nothing.
        let bar_w_dip = (tray_rect.right - tray_rect.left) as f64 / st.dpi;
        if abs_x < -1.0 || abs_x > bar_w_dip {
            skipped_off += 1;
            element.release();
            continue;
        }
        let w = xaml::framework_element_actual_width(XamlPtr(element.0));
        if w < 8.0 {
            element.release();
            continue;
        }

        // One mutable transform per button, created once and poked in place for
        // the rest of the session.
        let Some(xf) = xaml::composite_transform_new() else {
            element.release();
            continue;
        };
        let h = xaml::framework_element_actual_height(XamlPtr(element.0));
        // Scale about the button's *centre*: an icon that grows from its bottom
        // edge lifts off the row's centre line and reads as "crooked" rather than
        // "bigger". Centred scaling keeps every icon on the same optical line.
        xaml::composite_transform_center(xf, w / 2.0, if h > 1.0 { h / 2.0 } else { 20.0 });

        let hr = xaml::ui_element_put_render_transform(XamlPtr(element.0), xf);
        if found.is_empty() {
            crate::log(&format!(
                "hover: install hr=0x{:08X} scale={:.3} w={:.1} h={:.1}",
                hr as u32,
                xaml::composite_transform_scale_x(xf),
                w,
                h
            ));
        }

        let icon_h = icon_height(&snapshot, site, record.handle).unwrap_or(h);

        found.push(Btn {
            element,
            xf: ComPtr(xf.0),
            x: abs_x,
            w,
            icon_h,
            last: (-1.0, f64::INFINITY, f64::INFINITY),
        });
        // The caller owns `xf`; it is released on rebuild/shutdown.
    }

    found.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(core::cmp::Ordering::Equal));

    st.limit_x = f64::INFINITY;
    for record in snapshot
        .iter()
        .filter(|r| r.ty.ends_with("SystemTrayFrame"))
    {
        if let Some((x, _)) = absolute_offset(&snapshot, site, record.handle) {
            st.limit_x = x - 6.0;
            break;
        }
    }

    crate::log(&format!(
        "hover: geometry n={} off_screen={} base={:.3} limit_x={:.1} tray_w={}",
        found.len(),
        skipped_off,
        st.base_scale,
        st.limit_x,
        tray_rect.right - tray_rect.left
    ));
    st.bar_dip = if tray_rect.bottom > tray_rect.top {
        (tray_rect.bottom - tray_rect.top) as f64 / st.dpi
    } else {
        48.0
    };
    st.icon_h = found.iter().fold(0.0f64, |m, b| m.max(b.icon_h));
    if let Some(first) = found.first() {
        crate::log(&format!(
            "hover: first button x={:.1} w={:.1} icon_h={:.1} bar={:.1}",
            first.x, first.w, st.icon_h, st.bar_dip
        ));
    }
    st.buttons = found;
    st.built = !st.buttons.is_empty();

    // Dock mode hides the button's hover plate so the magnified glyph stands on
    // its own; background mode puts it back.
    set_button_background(!matches!(st.mode, HoverEffect::Dock));

    if matches!(st.mode, HoverEffect::Background) {
        // Icons stay at their configured size — but the scale still has to be
        // written, because dock mode may have left a hover value behind.
        for b in st.buttons.iter_mut() {
            let base = st.base_scale;
            xaml::composite_transform_frame(XamlPtr(b.xf.0), base, 0.0, 0.0, &mut b.last, 0.0);
        }
    }
    if false && st.built && !st.pinned {
        // Pin once, here — not on every hover. `VerticalAlignment=Bottom` on the
        // two halves of the bar is what lets the window grow upward without the
        // pill stretching into the band; writing it per hover would force a full
        // taskbar re-layout (and a visible hitch) on every pointer entry.
        let h = pin_bar(st, true);
        if h >= 20.0 {
            st.pinned = true;
        } else {
            crate::log(&format!("hover: pin collapsed the bar (h={h:.1}) — growth disabled"));
            pin_bar(st, false);
            st.growth_off = true;
        }
    }
}


/// Show (`true`) or hide (`false`) the plate every task button draws behind its
/// icon.
///
/// In dock mode the glyph is scaled up on its own; keeping the plate would put a
/// big rounded rectangle behind it, which is exactly the "it still looks like a
/// Windows button" reading. Hiding it leaves just the icon, like the reference
/// dock. The plate is `Border#BackgroundElement` inside each
/// `TaskListLabeledButtonPanel` — matched by ancestry, because the tray uses an
/// element of the same name.
unsafe fn set_button_background(show: bool) {
    let site = crate::tap::SITE.load(std::sync::atomic::Ordering::SeqCst);
    if site.is_null() {
        return;
    }
    let value = if show { "1" } else { "0" };
    let snapshot = crate::elements_snapshot();
    let mut touched = 0usize;
    for record in snapshot.iter().filter(|r| r.name == "BackgroundElement") {
        if !under_task_button(&snapshot, record.handle) {
            continue;
        }
        let Some(element) = ComPtr(site).get_inspectable(record.handle) else {
            continue;
        };
        let rules = [crate::apply::Rule {
            property: "Opacity",
            value,
            is_xaml: false,
        }];
        let stats = crate::apply::apply_rules(element.0, &record.ty, &rules);
        element.release();
        touched += stats.applied;
    }
    crate::log(&format!(
        "hover: button plate visible={show} writes={touched}"
    ));
}

/// Height of the `Image#Icon` glyph inside one task button, in DIP.
///
/// The button is taller than the glyph it draws, so the button's own height is the
/// wrong yardstick for "how big can this get before the bar clips it".
unsafe fn icon_height(
    snapshot: &[crate::ElementRecord],
    site: *mut c_void,
    button: u64,
) -> Option<f64> {
    let direct = snapshot.iter().find(|r| {
        r.name == "Icon" && r.ty.ends_with("Image") && under_task_button(snapshot, r.handle) && {
            let mut cur = r.parent;
            for _ in 0..3 {
                if cur == button {
                    return true;
                }
                match snapshot.iter().find(|p| p.handle == cur) {
                    Some(p) if p.parent != 0 => cur = p.parent,
                    _ => return false,
                }
            }
            false
        }
    })?;
    let element = ComPtr(site).get_inspectable(direct.handle)?;
    let h = xaml::framework_element_actual_height(XamlPtr(element.0));
    element.release();
    if h > 1.0 {
        Some(h)
    } else {
        None
    }
}

/// Is `handle` inside a task button's subtree?
fn under_task_button(snapshot: &[crate::ElementRecord], handle: u64) -> bool {
    let mut cur = handle;
    for _ in 0..8 {
        let Some(rec) = snapshot.iter().find(|r| r.handle == cur) else {
            return false;
        };
        if rec.name == "TaskListButton" {
            return true;
        }
        if rec.parent == 0 {
            return false;
        }
        cur = rec.parent;
    }
    false
}

/// Grow the taskbar window upward so magnified icons have somewhere to go, and
/// pin the visible bar back to its normal height at the bottom.
unsafe fn grow(st: &mut State, rect: &dcx_win32::RECT) {
    let px = (POPUP_DIP * st.dpi).round() as i32;
    if px < 6 {
        return;
    }
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;
    if w <= 0 || h <= 0 {
        return;
    }
    st.base_rect = (rect.left, rect.top, w, h);
    let flags = dcx_win32::SWP_NOZORDER
        | dcx_win32::SWP_NOACTIVATE
        | dcx_win32::SWP_NOOWNERZORDER
        | dcx_win32::SWP_NOSENDCHANGING;
    let ok = dcx_win32::SetWindowPos(st.tray, 0, rect.left, rect.top - px, w, h + px, flags);
    if ok == 0 {
        crate::log("hover: grow failed");
        st.growth_off = true;
        return;
    }
    st.grown = true;
    resize_bridge(st.tray, rect.left, rect.top - px, w, h + px);
    crate::log(&format!("hover: grown +{px}px (bar {h}px)"));
}


/// Height the dock background keeps while hovered, in DIP.
///
/// The taskbar window is the hard clip, and the capsule already fills it, so an
/// icon can never leave the dock while the background occupies the whole window.
/// Rather than resizing the window (which blanks the XAML host on this build),
/// the background itself is pulled in by this much and pinned to the bottom: the
/// icon then stands *outside* the capsule, fully visible, and nothing outside the
/// taskbar moves.
const PILL_SHRINK_DIP: f64 = 0.0;

/// Shrink / restore the dock and tray backgrounds.
unsafe fn pill(st: &mut State, shrunk: bool) {
    if PILL_SHRINK_DIP <= 0.0 {
        return;
    }
    let site = crate::tap::SITE.load(std::sync::atomic::Ordering::SeqCst);
    if site.is_null() {
        return;
    }
    let (height, align) = if shrunk {
        let bar_dip = if st.base_rect.3 > 0 {
            st.base_rect.3 as f64 / st.dpi
        } else {
            48.0
        };
        (
            format!("{:.1}", (bar_dip - PILL_SHRINK_DIP).max(24.0)),
            "Bottom",
        )
    } else {
        ("Auto".to_string(), "Stretch")
    };
    let snapshot = crate::elements_snapshot();
    for record in snapshot
        .iter()
        .filter(|r| r.name == "RootGrid" || r.ty.ends_with("SystemTrayFrame"))
    {
        let Some(element) = ComPtr(site).get_inspectable(record.handle) else {
            continue;
        };
        let rules = [
            crate::apply::Rule {
                property: "Height",
                value: &height,
                is_xaml: false,
            },
            crate::apply::Rule {
                property: "VerticalAlignment",
                value: align,
                is_xaml: false,
            },
        ];
        let stats = crate::apply::apply_rules(element.0, &record.ty, &rules);
        let h = xaml::framework_element_actual_height(XamlPtr(element.0));
        element.release();
        crate::log(&format!(
            "hover: pill {}#{} shrunk={} ok={} fail={} h={:.1}",
            record.ty, record.name, shrunk, stats.applied, stats.failed, h
        ));
    }
}

/// Give the XAML island the same rectangle as the bar.
///
/// `Shell_TrayWnd` is only the frame; the taskbar's visuals live in this child
/// window, and it keeps whatever size the shell last gave it. Growing only the
/// frame produces exactly the "icon gets cut in half" look that the frame resize
/// was supposed to avoid.
unsafe fn resize_bridge(tray: isize, x: i32, y: i32, w: i32, h: i32) {
    let class = wide(CONTENT_BRIDGE_CLASS);
    let bridge = FindWindowExW(tray, 0, class.as_ptr(), core::ptr::null());
    if bridge == 0 {
        crate::log("hover: no content bridge");
        return;
    }
    let flags = dcx_win32::SWP_NOZORDER
        | dcx_win32::SWP_NOACTIVATE
        | dcx_win32::SWP_NOOWNERZORDER
        | dcx_win32::SWP_NOSENDCHANGING;
    dcx_win32::SetWindowPos(bridge, 0, x, y, w, h, flags);
}

/// Restore the window and let the bar fill it again.
unsafe fn shrink(st: &mut State) {
    let (x, y, w, h) = st.base_rect;
    if w > 0 && h > 0 {
        let flags =
            dcx_win32::SWP_NOZORDER | dcx_win32::SWP_NOACTIVATE | dcx_win32::SWP_NOOWNERZORDER;
        dcx_win32::SetWindowPos(st.tray, 0, x, y, w, h, flags);
        resize_bridge(st.tray, x, y, w, h);
    }
    st.grown = false;
    crate::log("hover: shrunk");
}

/// Pin (`pinned`) or release the two halves of the bar to the bottom of the
/// window.
///
/// Only the alignment is written — **never** an explicit `Height`: a numeric
/// `Height` pushed through the `<Style>` setter path collapses `TaskbarFrame` to
/// zero and the whole taskbar disappears (the pill is that element's content).
/// `VerticalAlignment=Bottom` with the element's natural height already does what
/// is needed: the bar keeps its 48 DIP look and sits at the window's bottom edge,
/// leaving the pop-up band empty and transparent.
unsafe fn pin_bar(st: &mut State, pinned: bool) -> f64 {
    let site = crate::tap::SITE.load(std::sync::atomic::Ordering::SeqCst);
    if site.is_null() {
        return 0.0;
    }
    let align = if pinned { "Bottom" } else { "Stretch" };
    let mut bar_h = f64::INFINITY;

    let snapshot = crate::elements_snapshot();
    for record in snapshot
        .iter()
        .filter(|r| r.ty.ends_with("TaskbarFrame") || r.ty.ends_with("SystemTrayFrame"))
    {
        let Some(element) = ComPtr(site).get_inspectable(record.handle) else {
            continue;
        };
        let rules = [crate::apply::Rule {
            property: "VerticalAlignment",
            value: align,
            is_xaml: false,
        }];
        let stats = crate::apply::apply_rules(element.0, &record.ty, &rules);
        let h = xaml::framework_element_actual_height(XamlPtr(element.0));
        element.release();
        crate::log(&format!(
            "hover: pin {}#{} pinned={} ok={} fail={} h={:.1}",
            record.ty, record.name, pinned, stats.applied, stats.failed, h
        ));
        if h > 0.0 && h < bar_h {
            bar_h = h;
        }
    }
    let _ = st;
    bar_h
}

/// Sum `ActualOffset` up the ancestor chain: XAML reports each element's offset
/// relative to its parent, and only the accumulated value is comparable with a
/// screen coordinate.
pub(crate) unsafe fn absolute_offset(
    snapshot: &[crate::ElementRecord],
    site: *mut c_void,
    handle: u64,
) -> Option<(f64, f64)> {
    let mut x = 0.0f64;
    let mut y = 0.0f64;
    let mut cur = handle;
    let mut guard = 0;
    let mut seen = false;
    loop {
        guard += 1;
        if guard > 64 {
            break;
        }
        let Some(rec) = snapshot.iter().find(|r| r.handle == cur) else {
            break;
        };
        if let Some(el) = ComPtr(site).get_inspectable(cur) {
            if let Some(off) = actual_offset(el) {
                x += off.x as f64;
                y += off.y as f64;
                seen = true;
            }
            el.release();
        }
        if rec.parent == 0 {
            break;
        }
        // The XAML island host is not a visual parent: stop below it.
        if let Some(parent) = snapshot.iter().find(|r| r.handle == rec.parent) {
            if parent.ty.ends_with("DesktopWindowXamlSource") {
                break;
            }
        }
        cur = rec.parent;
    }
    if seen {
        Some((x, y))
    } else {
        None
    }
}

unsafe fn actual_offset(element: ComPtr) -> Option<xaml::Vector3> {
    // Query through the `dcx_xaml` pointer type: its `Guid` is the one the
    // exported IID constants belong to.
    let el = XamlPtr(element.0).query_interface(&IID_IUIELEMENT10)?;
    let result = xcom::guarded(|| {
        type Fn = unsafe extern "system" fn(*mut c_void, *mut xaml::Vector3) -> i32;
        let f: Fn = core::mem::transmute(el.slot(6));
        let mut out = xaml::Vector3::default();
        let hr = f(el.0, &mut out);
        (hr, out)
    });
    el.release();
    match result {
        Ok((0, v)) => Some(v),
        _ => None,
    }
}

/// Drop the effect: restore idle transforms and stop the timer.
pub fn shutdown() {
    unsafe {
        dcx_win32::KillTimer(0, TIMER_ID);
        timeEndPeriod(1);
    }
    if let Ok(mut g) = STATE.lock() {
        if let Some(st) = g.as_mut() {
            for b in st.buttons.drain(..) {
                unsafe {
                    b.xf.release();
                    b.element.release();
                }
            }
            st.built = false;
        }
    }
}

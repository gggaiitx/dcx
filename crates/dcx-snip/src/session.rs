//! The overlay window: frozen frame, selection, annotations, toolbar.
//!
//! One window, one message pump (the process's), one crash domain. Every frame
//! is composed into a DIB section and blitted; nothing is layered, so hit
//! testing, z-order and capture all stay predictable.
//!
//! Composition order is fixed:
//!
//! ```text
//! dimmed frame  →  selection hole (original pixels)  →  effects (blur/spot)
//!   →  vector objects (document order)  →  clip back to the selection
//!   →  chrome (frame, handles, toolbar, hints)
//! ```
//!
//! The content layer (everything up to the clip) is cached between frames:
//! hovering the toolbar or showing a tooltip only costs one copy + the chrome,
//! not a full re-composite.
//!
//! Interaction model: **Hand** is the default tool. It moves resizes and picks
//! up annotations. Picking a drawing tool arms it, and a drawing tool never
//! lets an old annotation steal a drag — that was the single most confusing
//! behaviour of the first version.

use std::cell::Cell;

use crate::capture::Shot;
use crate::config::Config;
use crate::ffi::*;
use crate::font_menu::{FontMenu, MenuHit};
use crate::layout::{self, Act, Layout};
use crate::model::{drag_handle, handle_point, Doc, Kind, Obj, Pt, TextStyle, Tool};
use crate::paint::{self, Chrome};
use crate::picker::{Picker, PickerHit};
use crate::radius::{RadiusHit, RadiusPicker};
use crate::surface::{copy_rect, Rect, Rgba, Surface};
use dcx_win32::HWND;

pub const WM_APP_TEXT_COMMIT: u32 = 0x8000 + 10;
pub const WM_APP_TEXT_CANCEL: u32 = 0x8000 + 11;

const TIMER_FRAME: usize = 1;
const FRAME_MS: u64 = 10;
const HANDLE_HIT: f32 = 7.0;
/// How far the pointer must travel before a stroke counts as one. Below this a
/// press is a click, not a drag: a 3 px arrow under an 11 px head reads as a
/// stray dot, and a click is also how the previous arrow gets inspected.
const DRAG_MIN: f32 = 6.0;
const BORDER_BAND: f32 = 7.0;
const TIP_DELAY_MS: u64 = 320;
const HUD_MS: u64 = 1000;
const HINT_MS: u64 = 2800;
const HINT_FADE_MS: u64 = 650;
/// Default bow when an arrow is first curved from the toolbar.
const DEFAULT_BOW: f32 = 0.22;

// Virtual-key constants: `b'C'` is a u8 and does not match an i32 pattern.
const A_KEY: i32 = b'A' as i32;
const B_KEY: i32 = b'B' as i32;
const C_KEY: i32 = b'C' as i32;
const I_KEY: i32 = b'I' as i32;
const L_KEY: i32 = b'L' as i32;
const S_KEY: i32 = b'S' as i32;
const V_KEY: i32 = b'V' as i32;
const Y_KEY: i32 = b'Y' as i32;
const Z_KEY: i32 = b'Z' as i32;
const K1: i32 = b'1' as i32;
const K8: i32 = b'8' as i32;

thread_local! {
    static CURRENT: Cell<*mut Session> = const { Cell::new(core::ptr::null_mut()) };
    static REAP: Cell<*mut Session> = const { Cell::new(core::ptr::null_mut()) };
    static CLASS_ATOM: Cell<u16> = const { Cell::new(0) };
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Idle,
    Selecting,
    Ready,
    Drawing,
    MoveSel,
    ResizeSel(usize),
    MoveObj,
    ResizeObj(usize),
    /// Drag cubic control 0 (tail side) or 1 (head side) of the selected arrow.
    BendObj(usize),
    /// A drawing tool pressed on an existing annotation of its own kind: the
    /// press may become a move drag or, if it stays put, a plain click that
    /// only selects (text additionally opens its editor on release).
    MaybeGrab,
}

pub struct Session {
    hwnd: HWND,
    w: u32,
    h: u32,
    origin: (i32, i32),
    surface: Surface,
    base: Vec<u8>,
    dim: Vec<u8>,
    /// Composed content without chrome, reused on hover-only frames.
    content: Vec<u8>,
    content_valid: bool,
    cfg: Config,
    scale: f32,
    /// Monitor the toolbar/hints are clamped into, in overlay coordinates.
    monitor: Rect,

    phase: Phase,
    anchor: Pt,
    sel: Option<Rect>,
    tool: Tool,
    doc: Doc,
    draft: Option<Obj>,
    drag_from: Rect,
    drag_start: Pt,
    drag_snapshot: Obj,
    /// Set once the current object drag actually moved something; the undo
    /// snapshot is taken lazily so a plain click never fills the history.
    gesture_dirty: bool,

    color: Rgba,
    width: f32,
    width_index: Option<usize>,
    font_size: f32,
    /// Family / weight / shadow used for the next text object. A selected text
    /// object overrides this for display and edits (see `effective_text_style`).
    text_style: TextStyle,
    font_menu: FontMenu,
    /// Dragging the font list's scrollbar. The pointer is captured, so this
    /// flag has to be cleared on release and on every close path.
    font_scroll_drag: bool,
    fill: bool,
    curve: bool,
    /// Selection/output corner radius in physical pixels.
    sel_radius: f32,
    /// Last custom colour chosen from the picker, and whether it was ever set.
    custom: Rgba,
    custom_set: bool,
    picker: Picker,
    picker_drag: Option<PickerHit>,
    /// One undo entry per picker session even if the pointer sweeps colours.
    picker_snapshot: bool,
    radius_picker: RadiusPicker,
    radius_drag: bool,
    /// A long capture is running (or is being started) on this stack. Closing
    /// the session now would free it under the nested message loop.
    long_active: bool,

    cursor: Pt,
    layout: Layout,
    hover: Option<Act>,
    hover_since: u64,
    tip_active: bool,
    pressed: Option<Act>,
    pending: bool,
    last_frame: u64,
    toast: Option<(String, u64, bool)>,
    hud: Option<(String, u64)>,
    editor: Option<crate::edit_text::Editor>,
    editing: Option<usize>,
    text_at: Pt,
    hint_until: u64,
    hint_last: u64,
}

fn now_ms() -> u64 {
    unsafe { GetTickCount64() }
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

pub fn is_open() -> bool {
    !CURRENT.with(|c| c.get()).is_null()
}

/// Drop a session whose window is already gone. Called from the app timer:
/// freeing it inside `WM_DESTROY` would pull the rug out from under the method
/// that is running at that moment.
pub fn reap() {
    let p = REAP.with(|c| c.replace(core::ptr::null_mut()));
    if !p.is_null() {
        unsafe {
            drop(Box::from_raw(p));
        }
    }
}

/// Ask the running session to close (host shutdown, app stop).
pub fn cancel() {
    let p = CURRENT.with(|c| c.get());
    if !p.is_null() {
        unsafe {
            PostMessageW((*p).hwnd, WM_CLOSE, 0, 0);
        }
    }
}

/// Hotkey pressed while a session is up. In long-capture mode this restores
/// the overlay instead of tearing the whole session down.
pub fn hotkey_toggle() {
    let p = CURRENT.with(|c| c.get());
    if p.is_null() {
        return;
    }
    if crate::longshot::is_active() {
        crate::longshot::request_close();
    } else {
        unsafe { PostMessageW((*p).hwnd, WM_CLOSE, 0, 0) };
    }
}

pub fn open(shot: Shot, cfg: Config) -> Result<(), String> {
    if is_open() {
        return Ok(());
    }
    let origin = shot.origin;
    let mut session = build(shot, cfg)?;
    let hwnd = create_window(&mut session, origin)?;
    session.hwnd = hwnd;
    unsafe {
        let mut r = dcx_win32::RECT::default();
        dcx_win32::GetWindowRect(hwnd, &mut r);
        crate::app::log(&format!(
            "open: want={}x{}@({},{}) got={}x{}@({},{}) dpi={} scale={} monitor={:?}",
            session.w,
            session.h,
            origin.0,
            origin.1,
            r.right - r.left,
            r.bottom - r.top,
            r.left,
            r.top,
            dcx_win32::GetDpiForWindow(hwnd),
            session.scale,
            session.monitor
        ));
    }

    let ptr = Box::into_raw(session);
    CURRENT.with(|c| c.set(ptr));
    unsafe {
        // Compose before the window is visible: showing first leaves a frame of
        // undefined pixels on screen (the "white flash" report).
        (*ptr).compose(true);
        (*ptr).present();
        ShowWindow(hwnd, SW_SHOW);
        force_foreground(hwnd);
        SetFocus(hwnd);
        SetTimer(hwnd, TIMER_FRAME, FRAME_MS as u32, core::ptr::null());
    }
    Ok(())
}

fn build(shot: Shot, cfg: Config) -> Result<Box<Session>, String> {
    let (w, h) = (shot.w, shot.h);
    let scale = ui_scale(&cfg, w, h);
    let surface = Surface::new(w, h).ok_or_else(|| "无法创建绘图表面".to_string())?;
    let dim = build_dim(&shot.bgra);
    let color = Rgba::from_rgb(cfg.rgb());
    let width = cfg.draw.width.clamp(1.0, 40.0);
    let width_index = layout::WIDTHS.iter().position(|v| (v - width).abs() < 0.01);
    let font_size = cfg.draw.font_size.clamp(8.0, 96.0) * scale;
    let text_style = cfg.text_style();
    let fallback = Rect::from_xywh(0.0, 0.0, w as f32, h as f32);
    // `GetCursorPos` speaks virtual-screen coordinates; everything inside the
    // overlay is relative to the frozen frame's origin. Subtract it once here,
    // or a monitor left of / above the primary gets a double offset.
    let cursor_virtual = cursor_pos().unwrap_or(Pt::new(0.0, 0.0));
    let cursor = Pt::new(
        cursor_virtual.x - shot.origin.0 as f32,
        cursor_virtual.y - shot.origin.1 as f32,
    );
    let monitor = monitor_local_for_point(shot.origin, cursor, fallback);
    let now = now_ms();
    let content = vec![0u8; (w as usize) * (h as usize) * 4];

    Ok(Box::new(Session {
        hwnd: 0,
        w,
        h,
        origin: shot.origin,
        surface,
        base: shot.bgra,
        dim,
        content,
        content_valid: false,
        color,
        width,
        width_index,
        font_size,
        text_style,
        font_menu: FontMenu::default(),
        font_scroll_drag: false,
        fill: false,
        curve: false,
        sel_radius: cfg.output.corner_radius.min(128) as f32,
        custom: color,
        custom_set: false,
        picker: Picker::default(),
        picker_drag: None,
        picker_snapshot: false,
        radius_picker: RadiusPicker::default(),
        radius_drag: false,
        long_active: false,
        tool: Tool::Hand,
        doc: Doc::default(),
        draft: None,
        drag_from: Rect::default(),
        drag_start: Pt::default(),
        drag_snapshot: Obj::new(Kind::Spot { r: Rect::default() }, color, width),
        gesture_dirty: false,
        phase: Phase::Idle,
        anchor: Pt::default(),
        sel: None,
        cursor,
        layout: Layout::build(None, monitor, scale, None),
        hover: None,
        hover_since: 0,
        tip_active: false,
        pressed: None,
        pending: false,
        last_frame: 0,
        toast: None,
        hud: None,
        editor: None,
        editing: None,
        text_at: Pt::default(),
        hint_until: now + HINT_MS,
        hint_last: 0,
        cfg,
        scale,
        monitor,
    }))
}

/// Deterministic renderer used by `--demo`: builds a synthetic document, paints
/// the overlay **and** the export crop, and writes both to disk. This is the
/// only way to eyeball every tool without driving real mouse input, and it
/// doubles as the 125 %/200 % layout regression check.
pub fn demo(shot: Shot, cfg: Config, out: &std::path::Path) -> Result<(), String> {
    let mut s = build(shot, cfg)?;
    let (w, h) = (s.w as f32, s.h as f32);
    let sel = Rect::from_xywh(w * 0.16, h * 0.18, w * 0.46, h * 0.54);
    s.sel = Some(sel);
    s.phase = Phase::Ready;
    s.monitor = monitor_local_for_point(
        s.origin,
        Pt::new((sel.x0 + sel.x1) * 0.5, (sel.y0 + sel.y1) * 0.5),
        Rect::from_xywh(0.0, 0.0, w, h),
    );
    s.tool = Tool::Rect;
    s.hint_until = 0;
    s.update_layout();

    let scale = s.scale;
    let c = Rgba::from_rgb(layout::SWATCHES[0].rgb);
    let c2 = Rgba::from_rgb(layout::SWATCHES[2].rgb);
    let c3 = Rgba::from_rgb(layout::SWATCHES[5].rgb);
    let push = |s: &mut Session, kind: Kind, color: Rgba, width: f32| {
        s.doc.objs.push(Obj::new(kind, color, width));
    };
    push(
        &mut s,
        Kind::Rect {
            r: Rect::from_xywh(w * 0.19, h * 0.24, w * 0.16, h * 0.14),
            radius: 8.0,
            filled: false,
        },
        c,
        4.0,
    );
    push(
        &mut s,
        Kind::Arrow {
            a: Pt::new(w * 0.40, h * 0.30),
            c1: Pt::new(w * 0.45, h * 0.22),
            c2: Pt::new(w * 0.50, h * 0.38),
            b: Pt::new(w * 0.55, h * 0.27),
        },
        c2,
        4.0,
    );
    push(
        &mut s,
        Kind::Text {
            at: Pt::new(w * 0.19, h * 0.45),
            text: "中文标注 Text 123".to_string(),
            size: 26.0 * scale,
            // Demo carries the styled case: bold + shadow must be visible in
            // the regression PNG, otherwise a broken mask offset ships unseen.
            style: TextStyle {
                bold: true,
                shadow: true,
                ..TextStyle::default()
            },
        },
        c3,
        3.0,
    );
    push(
        &mut s,
        Kind::Marker {
            pts: (0..24)
                .map(|i| {
                    Pt::new(
                        w * 0.19 + i as f32 * (w * 0.010),
                        h * 0.56 + (i as f32 * 0.6).sin() * 6.0,
                    )
                })
                .collect(),
        },
        Rgba::new(0xF2, 0xB0, 0x2C, 140),
        44.0,
    );
    push(
        &mut s,
        Kind::Blur {
            r: Rect::from_xywh(w * 0.40, h * 0.50, w * 0.14, h * 0.10),
        },
        c,
        0.0,
    );
    push(
        &mut s,
        Kind::Spot {
            r: Rect::from_xywh(w * 0.40, h * 0.62, w * 0.12, h * 0.08),
        },
        c,
        0.0,
    );
    s.hover = Some(Act::Tool(Tool::Arrow));
    s.hover_since = now_ms().saturating_sub(2000);
    s.tip_active = true;
    s.invalidate();

    s.compose(true);
    let full = s.surface.bytes().to_vec();
    let ui_path = out.with_file_name(format!(
        "{}-ui.png",
        out.file_stem()
            .map(|x| x.to_string_lossy().into_owned())
            .unwrap_or_else(|| "demo".into())
    ));
    crate::output::write_png(&ui_path, &full, s.w, s.h)?;

    s.compose_export();
    let stride = s.surface.stride;
    let (crop, cw, ch) = crate::output::crop(s.surface.bytes(), stride, s.w, s.h, &sel)
        .ok_or_else(|| "crop failed".to_string())?;
    crate::output::write_png(out, &crop, cw, ch)?;
    // Exercise the exact production clipboard path (CF_DIB + PNG) so a broken
    // paste can be caught without a human at the keyboard.
    match crate::output::to_clipboard(0, &crop, cw, ch) {
        Ok(()) => crate::app::log("demo: clipboard ok"),
        Err(e) => crate::app::log(&format!("demo: clipboard failed: {e}")),
    }
    crate::app::log(&format!("demo: {} + {}", out.display(), ui_path.display()));
    Ok(())
}

/// Offscreen UI workbench used by `--demo-synth`: a synthetic desktop, the new
/// serial / picker / radius / bend chrome, and a rounded export — all without
/// touching the live screen or the clipboard.
pub fn demo_synth(out: &std::path::Path) -> Result<(), String> {
    let (w, h) = (1600u32, 900u32);
    let mut bgra = vec![0u8; (w as usize) * (h as usize) * 4];
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = if x < 260 {
                (0x1E, 0x23, 0x2A)
            } else if y < 64 {
                (0xFF, 0xFF, 0xFF)
            } else if x >= 300 && x < 1500 && y >= 100 && y < 800 {
                (0xFA, 0xFB, 0xFC)
            } else {
                (0xEC, 0xF0, 0xF4)
            };
            let (mut r, mut g, mut b) = (r, g, b);
            if x >= 320 && x < 1500 && y >= 120 && y < 138 {
                (r, g, b) = (0xE2, 0xE8, 0xEE);
            }
            if x >= 330 && x < 900 && (160..760).contains(&y) && (y % 44) < 10 {
                (r, g, b) = (0xD8, 0xDE, 0xE5);
            }
            if (980..1180).contains(&x) && (160..340).contains(&y) {
                (r, g, b) = (0x9F, 0xD9, 0xCE);
            }
            if (330..520).contains(&x) && (820..880).contains(&y) {
                (r, g, b) = (0x2B, 0x7D, 0xE9);
            }
            if x < 12 || y < 12 || x >= w - 12 || y >= h - 12 {
                (r, g, b) = (0xD5, 0xDD, 0xE4);
            }
            let off = ((y as usize) * (w as usize) + x as usize) * 4;
            bgra[off] = b as u8;
            bgra[off + 1] = g as u8;
            bgra[off + 2] = r as u8;
            bgra[off + 3] = 255;
        }
    }
    let shot = Shot {
        origin: (0, 0),
        w,
        h,
        bgra,
    };
    let mut cfg = crate::config::Config::default();
    cfg.output.corner_radius = 16;
    let mut s = build(shot, cfg)?;
    s.monitor = Rect::from_xywh(0.0, 0.0, w as f32, h as f32);
    s.sel = Some(Rect::from_xywh(140.0, 140.0, 1320.0, 620.0));
    s.phase = Phase::Ready;
    s.tool = Tool::Serial;
    s.sel_radius = 16.0;
    s.cursor = Pt::new(720.0, 500.0);
    s.hint_until = 0;
    s.update_layout();

    let scale = s.scale;
    let red = Rgba::from_rgb(layout::SWATCHES[0].rgb);
    let amber = Rgba::from_rgb(layout::SWATCHES[2].rgb);
    let blue = Rgba::from_rgb(layout::SWATCHES[5].rgb);
    s.doc.objs.push(Obj::new(
        Kind::Serial {
            at: Pt::new(360.0, 260.0),
            n: 1,
            size: 42.0,
        },
        red,
        3.0,
    ));
    s.doc.objs.push(Obj::new(
        Kind::Serial {
            at: Pt::new(425.0, 260.0),
            n: 2,
            size: 42.0,
        },
        amber,
        3.0,
    ));
    s.doc.objs.push(Obj::new(
        Kind::Arrow {
            a: Pt::new(520.0, 330.0),
            c1: Pt::new(620.0, 330.0),
            c2: Pt::new(720.0, 330.0),
            b: Pt::new(820.0, 330.0),
        },
        red,
        4.0,
    ));
    let ax = Pt::new(520.0, 470.0);
    let bx = Pt::new(880.0, 540.0);
    let (dx, dy) = (bx.x - ax.x, bx.y - ax.y);
    let bow = 0.22;
    let curved = s.doc.objs.len();
    s.doc.objs.push(Obj::new(
        Kind::Arrow {
            a: ax,
            c1: Pt::new(ax.x + dx * 0.33 - dy * bow, ax.y + dy * 0.33 + dx * bow),
            c2: Pt::new(ax.x + dx * 0.66 - dy * bow, ax.y + dy * 0.66 + dx * bow),
            b: bx,
        },
        blue,
        4.0,
    ));
    s.doc.objs.push(Obj::new(
        Kind::Rect {
            r: Rect::from_xywh(330.0, 590.0, 300.0, 110.0),
            radius: 10.0,
            filled: false,
        },
        red,
        4.0,
    ));
    let text_idx = s.doc.objs.len();
    s.doc.objs.push(Obj::new(
        Kind::Text {
            at: Pt::new(700.0, 620.0),
            text: "中文标注 Text".to_string(),
            size: 26.0 * scale,
            style: TextStyle {
                bold: true,
                shadow: true,
                ..TextStyle::default()
            },
        },
        blue,
        3.0,
    ));
    s.doc.objs.push(Obj::new(
        Kind::Text {
            at: Pt::new(700.0, 690.0),
            text: "楷体斜体 KaiTi".to_string(),
            size: 26.0 * scale,
            style: TextStyle {
                face: "KaiTi".to_string(),
                italic: true,
                ..TextStyle::default()
            },
        },
        red,
        3.0,
    ));
    s.doc.objs.push(Obj::new(
        Kind::Marker {
            pts: (0..20)
                .map(|i| {
                    Pt::new(
                        340.0 + i as f32 * 14.0,
                        760.0 + (i as f32 * 0.5).sin() * 5.0,
                    )
                })
                .collect(),
        },
        Rgba::new(0xF2, 0xB0, 0x2C, 140),
        44.0,
    ));
    s.invalidate();
    s.compose(true);
    let full = s.surface.bytes().to_vec();
    crate::output::write_png(out, &full, w, h)?;

    // Rounded, chrome-free export.
    s.compose_export();
    let stride = s.surface.stride;
    let sel = s.sel.unwrap_or_default();
    let Some((mut crop, cw, ch)) = crate::output::crop(s.surface.bytes(), stride, w, h, &sel)
    else {
        return Err("crop failed".into());
    };
    crate::output::round_alpha(&mut crop, cw, ch, s.sel_radius);
    let export = out.with_file_name(format!(
        "{}-export.png",
        out.file_stem()
            .map(|x| x.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    crate::output::write_png(&export, &crop, cw, ch)?;

    // Picker open.
    s.tool = Tool::Rect;
    s.doc.selected = None;
    s.update_layout();
    if let Some(anchor) = s.popover_anchor(Act::CustomColor) {
        s.picker.open_with(red, anchor, s.monitor, s.scale);
        s.picker.set_sv(Pt::new(
            s.picker.sv.x0 + 40.0 * s.scale,
            s.picker.sv.y0 + 50.0 * s.scale,
        ));
        s.apply_picker();
    }
    s.compose(true);
    let full = s.surface.bytes().to_vec();
    crate::output::write_png(
        &out.with_file_name(format!(
            "{}-picker.png",
            out.file_stem()
                .map(|x| x.to_string_lossy().into_owned())
                .unwrap_or_default()
        )),
        &full,
        w,
        h,
    )?;

    // Continuous radius slider, for the offscreen UI check.
    s.picker.open = false;
    s.doc.selected = None;
    s.tool = Tool::Rect;
    s.update_layout();
    if let Some(anchor) = s.popover_anchor(Act::Radius) {
        s.radius_picker
            .open_with(s.sel_radius, anchor, s.monitor, s.scale);
        s.radius_picker.value = 48.0;
        s.radius_picker.set_from_x(
            s.radius_picker.track.x0 + 48.0 / crate::radius::MAX * s.radius_picker.track.width(),
        );
        s.apply_radius();
    }
    s.compose(true);
    let full = s.surface.bytes().to_vec();
    crate::output::write_png(
        &out.with_file_name(format!(
            "{}-radius.png",
            out.file_stem()
                .map(|x| x.to_string_lossy().into_owned())
                .unwrap_or_default()
        )),
        &full,
        w,
        h,
    )?;
    s.radius_picker.open = false;

    // Font-family popover, opened from the text attribute row. Rendered
    // offscreen so the specimen list and the row's own state can be checked
    // without a live session.
    s.tool = Tool::Text;
    s.doc.selected = Some(text_idx);
    s.update_layout();
    if let Some(anchor) = s.popover_anchor(Act::FontFace) {
        let cur = s.effective_text_style().face;
        s.font_menu.open_with(&cur, anchor, s.monitor, s.scale);
    }
    s.compose(true);
    let full = s.surface.bytes().to_vec();
    crate::output::write_png(
        &out.with_file_name(format!(
            "{}-font.png",
            out.file_stem()
                .map(|x| x.to_string_lossy().into_owned())
                .unwrap_or_default()
        )),
        &full,
        w,
        h,
    )?;
    s.font_menu.open = false;

    // Curved arrow selected: bend grip + object frame.
    s.tool = Tool::Hand;
    s.doc.selected = Some(curved);
    s.update_layout();
    s.compose(true);
    let full = s.surface.bytes().to_vec();
    crate::output::write_png(
        &out.with_file_name(format!(
            "{}-obj.png",
            out.file_stem()
                .map(|x| x.to_string_lossy().into_owned())
                .unwrap_or_default()
        )),
        &full,
        w,
        h,
    )?;

    // Long-capture bar + preview, rendered offscreen (no HWND).
    crate::longshot::demo(out)?;
    Ok(())
}

/// Frame-time probe used by `--bench`: full re-composite vs cache-hit compose.
pub fn bench(frames: usize) -> i32 {
    use std::time::Instant;
    let shot = match crate::capture::grab() {
        Ok(s) => s,
        Err(e) => {
            crate::app::log(&format!("bench: capture failed: {e}"));
            return 1;
        }
    };
    let mut s = match build(shot, crate::config::load()) {
        Ok(s) => s,
        Err(e) => {
            crate::app::log(&format!("bench: build failed: {e}"));
            return 1;
        }
    };
    let (w, h) = (s.w as f32, s.h as f32);
    s.sel = Some(Rect::from_xywh(w * 0.2, h * 0.2, w * 0.5, h * 0.5));
    s.phase = Phase::Ready;
    s.tool = Tool::Rect;
    s.update_layout();
    let n = frames.clamp(1, 500);

    let t = Instant::now();
    for _ in 0..n {
        s.invalidate();
        s.compose(true);
    }
    let full = t.elapsed().as_secs_f64() * 1000.0 / n as f64;

    let t = Instant::now();
    for _ in 0..n {
        s.compose(true);
    }
    let hot = t.elapsed().as_secs_f64() * 1000.0 / n as f64;

    crate::app::log(&format!(
        "bench: {}x{} frames={} full={:.2}ms hot={:.2}ms",
        s.w, s.h, n, full, hot
    ));
    0
}

fn ui_scale(cfg: &Config, w: u32, h: u32) -> f32 {
    if cfg.ui.scale > 0.01 {
        return cfg.ui.scale.clamp(0.75, 3.0);
    }
    let dpi = cursor_monitor_dpi().unwrap_or(96) as f32;
    let dpi_scale = dpi / 96.0;
    // Big panels need proportionally bigger chrome, or the toolbar reads as a
    // postage stamp in the corner of a 4K screen.
    let area = (w as f32) * (h as f32);
    let comp = (area / (1920.0 * 1080.0)).max(0.5).powf(0.3);
    dpi_scale.max(comp).clamp(1.0, 3.0)
}

fn cursor_pos() -> Option<Pt> {
    let mut pt = dcx_win32::POINT::default();
    let ok = unsafe { GetCursorPos(&mut pt) } != 0;
    if ok {
        Some(Pt::new(pt.x as f32, pt.y as f32))
    } else {
        None
    }
}

fn cursor_monitor_dpi() -> Option<u32> {
    unsafe {
        let shcore = LoadLibraryW(crate::wide("shcore.dll").as_ptr());
        if shcore != 0 {
            let f = GetProcAddress(shcore, b"GetDpiForMonitor\0".as_ptr());
            if !f.is_null() {
                type GetDpi = unsafe extern "system" fn(isize, i32, *mut u32, *mut u32) -> i32;
                let func: GetDpi = core::mem::transmute(f);
                let mut pt = dcx_win32::POINT::default();
                GetCursorPos(&mut pt);
                let mon = crate::ffi::MonitorFromPoint(pt, 2);
                let (mut dx, mut dy) = (96u32, 96u32);
                if mon != 0 && func(mon, 0, &mut dx, &mut dy) == 0 && dx > 0 {
                    return Some(dx);
                }
            }
        }
        let dpi = dcx_win32::GetDpiForSystem();
        if dpi > 0 {
            Some(dpi)
        } else {
            None
        }
    }
}

/// Monitor rectangle in overlay coordinates. `origin` is the virtual-screen
/// origin of the frozen frame; monitor APIs speak virtual-screen coordinates,
/// so the conversion lives here and nowhere else.
fn monitor_local_for_point(origin: (i32, i32), p: Pt, fallback: Rect) -> Rect {
    unsafe {
        let pt = dcx_win32::POINT {
            x: origin.0 + p.x.round() as i32,
            y: origin.1 + p.y.round() as i32,
        };
        let mon = crate::ffi::MonitorFromPoint(pt, 2);
        if mon != 0 {
            let mut mi = dcx_win32::MONITORINFO {
                cbSize: core::mem::size_of::<dcx_win32::MONITORINFO>() as u32,
                ..Default::default()
            };
            if dcx_win32::GetMonitorInfoW(mon, &mut mi) != 0 {
                return Rect::new(
                    (mi.rcMonitor.left - origin.0) as f32,
                    (mi.rcMonitor.top - origin.1) as f32,
                    (mi.rcMonitor.right - origin.0) as f32,
                    (mi.rcMonitor.bottom - origin.1) as f32,
                );
            }
        }
    }
    fallback
}

fn force_foreground(hwnd: HWND) {
    unsafe {
        let fg = GetForegroundWindow();
        let me = GetCurrentThreadId();
        if fg != 0 {
            let them = GetWindowThreadProcessId(fg, core::ptr::null_mut());
            if them != 0 && them != me {
                AttachThreadInput(me, them, 1);
                SetForegroundWindow(hwnd);
                SetFocus(hwnd);
                AttachThreadInput(me, them, 0);
                return;
            }
        }
        SetForegroundWindow(hwnd);
        SetFocus(hwnd);
    }
}

fn build_dim(src: &[u8]) -> Vec<u8> {
    // Pre-darkened copy of the whole frame: compositing then costs one memcpy
    // instead of a per-pixel blend every frame.
    let mut out = src.to_vec();
    let (dr, dg, db) = (
        layout::MASK[0] as u32,
        layout::MASK[1] as u32,
        layout::MASK[2] as u32,
    );
    const K: u32 = 158;
    let inv = 255 - K;
    for px in out.chunks_exact_mut(4) {
        px[0] = ((px[0] as u32 * inv + db * K) / 255) as u8;
        px[1] = ((px[1] as u32 * inv + dg * K) / 255) as u8;
        px[2] = ((px[2] as u32 * inv + dr * K) / 255) as u8;
        px[3] = 255;
    }
    out
}

fn create_window(session: &mut Session, origin: (i32, i32)) -> Result<HWND, String> {
    unsafe {
        let class = crate::wide("DCX.Snip.Overlay");
        let title = crate::wide("DCX Snip Overlay");
        if CLASS_ATOM.with(|c| c.get()) == 0 {
            let cls = dcx_win32::WNDCLASSW {
                style: 0x0008, // CS_DBLCLKS
                lpfnWndProc: Some(wnd_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: GetModuleHandleW(core::ptr::null()),
                hIcon: 0,
                hCursor: 0,
                hbrBackground: 0,
                lpszMenuName: core::ptr::null(),
                lpszClassName: class.as_ptr(),
            };
            let atom = dcx_win32::RegisterClassW(&cls);
            if atom == 0 {
                return Err("无法注册覆盖层窗口类".to_string());
            }
            CLASS_ATOM.with(|c| c.set(atom));
        }
        let hwnd = dcx_win32::CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP | WS_CLIPCHILDREN,
            origin.0,
            origin.1,
            session.w as i32,
            session.h as i32,
            0,
            0,
            GetModuleHandleW(core::ptr::null()),
            session as *mut Session as *mut core::ffi::c_void,
        );
        if hwnd == 0 {
            return Err("无法创建覆盖层窗口".to_string());
        }
        // DWMWA_WINDOW_CORNER_PREFERENCE = 34, DWMWCP_DONOTROUND = 1: a rounded
        // corner would clip the captured pixels at the screen edge.
        let no_round: u32 = 1;
        dcx_win32::DwmSetWindowAttribute(
            hwnd,
            34,
            &no_round as *const u32 as *const core::ffi::c_void,
            4,
        );
        Ok(hwnd)
    }
}

// ---------------------------------------------------------------------------
// Window procedure
// ---------------------------------------------------------------------------

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    let ptr = CURRENT.with(|c| c.get());
    if ptr.is_null() {
        return dcx_win32::DefWindowProcW(hwnd, msg, wp, lp);
    }
    let s = unsafe { &mut *ptr };
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            if dc != 0 {
                s.surface
                    .blit(dc, &Rect::from_xywh(0.0, 0.0, s.w as f32, s.h as f32));
                EndPaint(hwnd, &ps);
            }
            0
        }
        WM_ERASEBKGND => 1,
        WM_MOUSEMOVE => {
            s.on_mouse(lp);
            0
        }
        WM_LBUTTONDOWN => {
            s.on_down(lp);
            0
        }
        WM_LBUTTONUP => {
            s.on_up(lp);
            0
        }
        WM_LBUTTONDBLCLK => {
            s.on_double_click(lp);
            0
        }
        WM_MBUTTONDOWN => {
            s.on_middle_down(lp);
            0
        }
        WM_MBUTTONUP => {
            s.on_middle_up(lp);
            0
        }
        WM_RBUTTONDOWN => {
            s.on_right(lp);
            0
        }
        WM_MOUSEWHEEL => {
            s.on_wheel((wp >> 16) as i16 as i32);
            0
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            s.on_key(wp as i32);
            0
        }
        WM_TIMER if wp == TIMER_FRAME => {
            s.on_tick();
            0
        }
        WM_APP_TEXT_COMMIT => {
            s.commit_text();
            0
        }
        WM_APP_TEXT_CANCEL => {
            s.discard_text();
            0
        }
        WM_SETCURSOR => {
            let cur = s.cursor_for_hover();
            SetCursor(cur);
            1
        }
        WM_CLOSE => {
            // Long capture runs a nested message loop on this same stack:
            // tearing the session down here would free `s` under it.
            if s.long_active || crate::longshot::is_active() {
                crate::longshot::request_close();
            } else {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, TIMER_FRAME);
            s.close_editor();
            CURRENT.with(|c| c.set(core::ptr::null_mut()));
            REAP.with(|c| c.set(ptr));
            0
        }
        _ => dcx_win32::DefWindowProcW(hwnd, msg, wp, lp),
    }
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

fn point_of(lp: isize) -> Pt {
    // lParam carries two signed 16-bit client coordinates; a drag past the
    // left/top edge wraps into large positives without the sign fix.
    let x = (lp & 0xFFFF) as u16 as i16;
    let y = ((lp >> 16) & 0xFFFF) as u16 as i16;
    Pt::new(x as f32, y as f32)
}

impl Session {
    fn clamp(&self, p: Pt) -> Pt {
        Pt::new(p.x.clamp(0.0, self.w as f32), p.y.clamp(0.0, self.h as f32))
    }

    fn invalidate(&mut self) {
        self.content_valid = false;
    }

    fn attr_tool(&self) -> Option<Tool> {
        if self.tool.is_drawing() {
            return match self.tool {
                Tool::Blur | Tool::Spot => None,
                t => Some(t),
            };
        }
        let i = self.doc.selected?;
        let obj = self.doc.objs.get(i)?;
        let t = Tool::of_kind(&obj.kind);
        matches!(
            t,
            Tool::Rect | Tool::Arrow | Tool::Text | Tool::Marker | Tool::Serial
        )
        .then_some(t)
    }

    fn update_layout(&mut self) {
        self.ensure_monitor();
        let show = self.sel.is_some() && self.phase != Phase::Selecting;
        let sel = if show { self.sel } else { None };
        let attr = if show { self.attr_tool() } else { None };
        self.layout = Layout::build(sel, self.monitor, self.scale, attr);
        if self.layout.hits.is_empty() {
            self.hover = None;
            self.tip_active = false;
            self.pressed = None;
        }
    }

    fn ensure_monitor(&mut self) {
        let Some(sel) = self.sel else { return };
        let (cx, cy) = sel.center();
        if !self.monitor.contains(cx, cy) {
            let fallback = self.monitor;
            self.monitor = monitor_local_for_point(self.origin, Pt::new(cx, cy), fallback);
        }
    }

    /// Index of the selection edge near `p` (handle numbering), with a generous
    /// band on both sides of the border so resizing does not require hitting a
    /// 3-px square.
    fn border_zone(&self, sel: Rect, p: Pt) -> Option<usize> {
        let band = BORDER_BAND * self.scale;
        if p.x < sel.x0 - band || p.x > sel.x1 + band || p.y < sel.y0 - band || p.y > sel.y1 + band
        {
            return None;
        }
        let l = (p.x - sel.x0).abs() <= band;
        let r = (p.x - sel.x1).abs() <= band;
        let t = (p.y - sel.y0).abs() <= band;
        let b = (p.y - sel.y1).abs() <= band;
        match (l, r, t, b) {
            (true, _, true, _) => Some(0),
            (_, true, true, _) => Some(2),
            (_, true, _, true) => Some(4),
            (true, _, _, true) => Some(6),
            (_, _, true, _) => Some(1),
            (_, true, _, _) => Some(3),
            (_, _, _, true) => Some(5),
            (true, _, _, _) => Some(7),
            _ => None,
        }
    }

    fn hit_object(&self, p: Pt) -> Option<usize> {
        if let Some(sel) = self.sel {
            if !sel.contains(p.x, p.y) {
                return None;
            }
        }
        for i in (0..self.doc.objs.len()).rev() {
            let o = &self.doc.objs[i];
            if let Some(sel) = self.sel {
                if o.bounds().intersect(&sel).is_none() {
                    continue;
                }
            }
            if o.hit(p, 4.0 * self.scale) {
                return Some(i);
            }
        }
        None
    }

    /// Topmost object of `want`'s kind under the pointer. Drawing tools use it
    /// to tell "press on my own annotation" from "press on empty canvas".
    fn object_of_kind_at(&self, p: Pt, want: Tool) -> Option<usize> {
        let i = self.hit_object(p)?;
        (Tool::of_kind(&self.doc.objs.get(i)?.kind) == want).then_some(i)
    }

    fn text_object_at(&self, p: Pt) -> Option<usize> {
        self.object_of_kind_at(p, Tool::Text)
    }

    fn on_mouse(&mut self, lp: isize) {
        let p = self.clamp(point_of(lp));
        self.cursor = p;
        if self.font_scroll_drag {
            self.font_menu.scroll_to_y(p.y);
            self.redraw(false);
            return;
        }
        if self.radius_drag {
            self.radius_picker.set_from_x(p.x);
            self.apply_radius();
            return;
        }
        if let Some(drag) = self.picker_drag {
            match drag {
                PickerHit::Sv => self.picker.set_sv(p),
                PickerHit::Hue => self.picker.set_hue(p),
                _ => {}
            }
            self.apply_picker();
            return;
        }
        match self.phase {
            Phase::Selecting => {
                let r = Rect::new(self.anchor.x, self.anchor.y, p.x, p.y);
                self.sel = Some(clamp_rect(r, self.w, self.h));
                self.update_layout();
                self.invalidate();
            }
            Phase::MoveSel => {
                let dx = p.x - self.drag_start.x;
                let dy = p.y - self.drag_start.y;
                self.sel = Some(move_into(self.drag_from, dx, dy, self.w, self.h));
                self.update_layout();
                self.invalidate();
            }
            Phase::ResizeSel(h) => {
                let dx = p.x - self.drag_start.x;
                let dy = p.y - self.drag_start.y;
                self.sel = Some(clamp_rect(
                    drag_handle(self.drag_from, h, dx, dy),
                    self.w,
                    self.h,
                ));
                self.update_layout();
                self.invalidate();
            }
            Phase::Drawing => {
                self.update_draft(p);
                self.invalidate();
            }
            Phase::MoveObj => {
                let dx = p.x - self.drag_start.x;
                let dy = p.y - self.drag_start.y;
                if dx != 0.0 || dy != 0.0 {
                    if let Some(i) = self.doc.selected {
                        if !self.gesture_dirty {
                            self.doc.snapshot();
                            self.gesture_dirty = true;
                        }
                        // Rebuilt from the gesture snapshot on every move:
                        // incremental translation accumulates rounding and
                        // drifts away from the cursor.
                        let mut copy = self.drag_snapshot.clone();
                        copy.translate(dx, dy);
                        if let Some(slot) = self.doc.objs.get_mut(i) {
                            *slot = copy;
                        }
                    }
                    self.invalidate();
                }
            }
            Phase::ResizeObj(h) => {
                let dx = p.x - self.drag_start.x;
                let dy = p.y - self.drag_start.y;
                let new = drag_handle(self.drag_from, h, dx, dy);
                if new != self.drag_from {
                    if let Some(i) = self.doc.selected {
                        if !self.gesture_dirty {
                            self.doc.snapshot();
                            self.gesture_dirty = true;
                        }
                        let mut copy = self.drag_snapshot.clone();
                        copy.remap(self.drag_from, new);
                        if let Some(slot) = self.doc.objs.get_mut(i) {
                            *slot = copy;
                        }
                    }
                    self.invalidate();
                }
            }
            Phase::BendObj(which) => {
                let mut changed = false;
                let mut next = self.drag_snapshot.clone();
                // Keep the curve inside the captured area, like every other
                // annotation: outside it is clipped and invisible anyway.
                let q = if let Some(sel) = self.sel {
                    Pt::new(p.x.clamp(sel.x0, sel.x1), p.y.clamp(sel.y0, sel.y1))
                } else {
                    p
                };
                if let Kind::Arrow { c1, c2, .. } = &mut next.kind {
                    // Each control follows the pointer independently: any bow
                    // amount, either side, asymmetric or S-shaped.
                    if which == 0 {
                        *c1 = q;
                    } else {
                        *c2 = q;
                    }
                    changed = next != self.drag_snapshot;
                }
                if changed {
                    if !self.gesture_dirty {
                        self.doc.snapshot();
                        self.gesture_dirty = true;
                    }
                    if let Some(i) = self.doc.selected {
                        if let Some(slot) = self.doc.objs.get_mut(i) {
                            *slot = next;
                        }
                    }
                    self.invalidate();
                }
            }
            Phase::MaybeGrab => {
                if self.drag_start.dist(&p) >= 3.0 * self.scale {
                    self.phase = Phase::MoveObj;
                    let dx = p.x - self.drag_start.x;
                    let dy = p.y - self.drag_start.y;
                    if let Some(i) = self.doc.selected {
                        if !self.gesture_dirty {
                            self.doc.snapshot();
                            self.gesture_dirty = true;
                        }
                        let mut copy = self.drag_snapshot.clone();
                        copy.translate(dx, dy);
                        if let Some(slot) = self.doc.objs.get_mut(i) {
                            *slot = copy;
                        }
                    }
                    self.invalidate();
                }
            }
            Phase::Ready | Phase::Idle => {
                let hit = self.layout.hit(p.x, p.y);
                if hit != self.hover {
                    self.hover = hit;
                    self.hover_since = now_ms();
                    self.tip_active = false;
                }
            }
        }
        self.redraw(false);
    }

    fn on_down(&mut self, lp: isize) {
        let p = self.clamp(point_of(lp));
        self.cursor = p;
        if self.editor.is_some() {
            self.commit_text();
        }

        if self.font_menu.open {
            match self.font_menu.hit(p) {
                MenuHit::Row(i) => {
                    self.run(Act::Font(i));
                    return;
                }
                MenuHit::Scrollbar => {
                    unsafe { SetCapture(self.hwnd) };
                    self.font_scroll_drag = true;
                    self.font_menu.scroll_to_y(p.y);
                    self.redraw(false);
                    return;
                }
                // Padding is part of the popover: a click there must neither
                // pick a font nor fall through to the canvas.
                MenuHit::Panel => return,
                MenuHit::Outside => {
                    self.font_menu.open = false;
                    self.font_scroll_drag = false;
                    self.redraw(false);
                    return;
                }
            }
        }

        if self.radius_picker.open {
            match self.radius_picker.hit(p) {
                Some(RadiusHit::Track) => {
                    unsafe { SetCapture(self.hwnd) };
                    self.radius_drag = true;
                    self.radius_picker.set_from_x(p.x);
                    self.apply_radius();
                    return;
                }
                Some(RadiusHit::Panel) => return,
                None => {
                    self.close_radius_picker();
                    return;
                }
            }
        }

        if self.picker.open {
            match self.picker.hit(p) {
                Some(PickerHit::Sv) | Some(PickerHit::Hue) => {
                    unsafe { SetCapture(self.hwnd) };
                    self.picker_drag = self.picker.hit(p);
                    if self.picker_drag == Some(PickerHit::Sv) {
                        self.picker.set_sv(p);
                    } else {
                        self.picker.set_hue(p);
                    }
                    self.apply_picker();
                    return;
                }
                Some(PickerHit::Close) => {
                    self.close_picker();
                    return;
                }
                Some(PickerHit::Panel) => return,
                None => {
                    // Outside click dismisses the popover and is consumed.
                    self.close_picker();
                    return;
                }
            }
        }

        // Controls fire on release, so a press that slides off can be
        // cancelled the way every other button behaves.
        if let Some(act) = self.layout.hit(p.x, p.y) {
            unsafe { SetCapture(self.hwnd) };
            self.pressed = Some(act);
            self.hover = Some(act);
            self.hover_since = now_ms();
            self.tip_active = false;
            self.redraw(false);
            return;
        }
        // Padding, gaps and dividers inside the bar are still the bar: a click
        // there must not fall through and start a new selection underneath.
        if self.layout.bar.contains(p.x, p.y) {
            return;
        }

        unsafe { SetCapture(self.hwnd) };

        // Text tool with a selected annotation: its handles stay live, so a
        // freshly committed text can be resized without switching tools.
        if self.tool == Tool::Text {
            if let Some(i) = self.doc.selected {
                if let Some(obj) = self.doc.objs.get(i).cloned() {
                    let b = obj.bounds();
                    let r = HANDLE_HIT * self.scale;
                    if let Some(h) = (0..8).find(|h| handle_point(b, *h).dist(&p) <= r) {
                        self.drag_snapshot = obj;
                        self.drag_from = b;
                        self.drag_start = p;
                        self.gesture_dirty = false;
                        self.phase = Phase::ResizeObj(h);
                        return;
                    }
                }
            }
        }

        // 1. Hand mode: adjust an annotation before touching the selection.
        if self.tool == Tool::Hand {
            if let Some(i) = self.doc.selected {
                if let Some(obj) = self.doc.objs.get(i).cloned() {
                    let b = obj.bounds();
                    let r = HANDLE_HIT * self.scale;
                    // Frame handles first, then the two cubic controls (they
                    // often sit on the body and must win over "move object"),
                    // then the body itself.
                    let grabbed = (0..8)
                        .find(|h| handle_point(b, *h).dist(&p) <= r)
                        .map(Phase::ResizeObj)
                        .or_else(|| match &obj.kind {
                            Kind::Arrow { c1, c2, .. } => [*c1, *c2]
                                .iter()
                                .position(|q| q.dist(&p) <= r)
                                .map(Phase::BendObj),
                            _ => None,
                        })
                        .or_else(|| obj.hit(p, 4.0 * self.scale).then_some(Phase::MoveObj));
                    if let Some(phase) = grabbed {
                        self.drag_snapshot = obj;
                        self.drag_from = b;
                        self.drag_start = p;
                        self.gesture_dirty = false;
                        self.phase = phase;
                        return;
                    }
                }
            }
            if let Some(i) = self.hit_object(p) {
                self.doc.selected = Some(i);
                if let Some(obj) = self.doc.objs.get(i).cloned() {
                    self.drag_from = obj.bounds();
                    self.drag_snapshot = obj;
                    self.drag_start = p;
                    self.gesture_dirty = false;
                    self.phase = Phase::MoveObj;
                }
                self.update_layout();
                self.redraw(false);
                return;
            }
        }

        // A selected arrow keeps its frame and cubic grips live in Arrow mode:
        // drag a grip to bend, drag a frame handle to resize. Any other press
        // on the body is handled further down (select / move), so drawing over
        // an old arrow does not silently stack a second one.
        if self.tool == Tool::Arrow {
            if let Some(i) = self.doc.selected {
                if let Some(obj) = self.doc.objs.get(i).cloned() {
                    if matches!(obj.kind, Kind::Arrow { .. }) {
                        let b = obj.bounds();
                        let r = HANDLE_HIT * self.scale;
                        let grabbed = (0..8)
                            .find(|h| handle_point(b, *h).dist(&p) <= r)
                            .map(Phase::ResizeObj)
                            .or_else(|| match &obj.kind {
                                Kind::Arrow { c1, c2, .. } => [*c1, *c2]
                                    .iter()
                                    .position(|q| q.dist(&p) <= r)
                                    .map(Phase::BendObj),
                                _ => None,
                            });
                        if let Some(phase) = grabbed {
                            self.drag_snapshot = obj;
                            self.drag_from = b;
                            self.drag_start = p;
                            self.gesture_dirty = false;
                            self.phase = phase;
                            return;
                        }
                    }
                }
            }
        }

        if let Some(sel) = self.sel {
            // Space-drag is the escape hatch while a drawing tool is armed.
            if unsafe { GetKeyState(VK_SPACE) } < 0 && sel.contains(p.x, p.y) {
                self.drag_from = sel;
                self.drag_start = p;
                self.phase = Phase::MoveSel;
                return;
            }
            if let Some(h) = self.border_zone(sel, p) {
                self.drag_from = sel;
                self.drag_start = p;
                self.phase = Phase::ResizeSel(h);
                return;
            }
            if sel.contains(p.x, p.y) {
                if self.tool == Tool::Hand {
                    self.drag_from = sel;
                    self.drag_start = p;
                    self.phase = Phase::MoveSel;
                    return;
                }
                if self.tool == Tool::Text {
                    if let Some(i) = self.object_of_kind_at(p, Tool::Text) {
                        // Press may become a move (drag) or an edit (click).
                        self.doc.selected = Some(i);
                        if let Some(obj) = self.doc.objs.get(i).cloned() {
                            self.drag_from = obj.bounds();
                            self.drag_snapshot = obj;
                            self.drag_start = p;
                            self.gesture_dirty = false;
                            self.phase = Phase::MaybeGrab;
                            self.update_layout();
                            return;
                        }
                    } else {
                        self.open_editor(p, None);
                    }
                    return;
                }
                // Arrow tool: pressing an existing arrow selects it instead of
                // stacking a second head on top — the same "click again to
                // re-edit" flow the text tool has. Pressing empty space starts
                // nothing and, on release, clears the previous edit state.
                if self.tool == Tool::Arrow {
                    if let Some(i) = self.object_of_kind_at(p, Tool::Arrow) {
                        self.doc.selected = Some(i);
                        if let Some(obj) = self.doc.objs.get(i).cloned() {
                            self.drag_from = obj.bounds();
                            self.drag_snapshot = obj;
                            self.drag_start = p;
                            self.gesture_dirty = false;
                            self.phase = Phase::MaybeGrab;
                            self.update_layout();
                            self.redraw(false);
                            return;
                        }
                    }
                }
                if self.tool == Tool::Serial {
                    let n = next_serial(&self.doc, self.sel);
                    let size = self.font_size * 1.4;
                    self.doc.snapshot();
                    self.doc.objs.push(Obj::new(
                        Kind::Serial { at: p, n, size },
                        self.color,
                        self.width,
                    ));
                    self.invalidate();
                    self.phase = Phase::Ready;
                    self.redraw(false);
                    return;
                }
                self.start_draft(p);
                return;
            }
        }

        // Anywhere else: start a new selection.
        self.doc.selected = None;
        self.pressed = None;
        self.phase = Phase::Selecting;
        self.anchor = p;
        self.sel = Some(Rect::new(p.x, p.y, p.x, p.y));
        self.hint_until = 0;
        self.update_layout();
        self.invalidate();
        self.redraw(false);
    }

    fn on_up(&mut self, lp: isize) {
        let p = self.clamp(point_of(lp));
        unsafe { ReleaseCapture() };

        if self.font_scroll_drag {
            self.font_scroll_drag = false;
            self.redraw(false);
            return;
        }
        if self.picker_drag.take().is_some() {
            self.redraw(false);
            return;
        }
        if self.radius_drag {
            self.radius_drag = false;
            self.redraw(false);
            return;
        }

        if let Some(act) = self.pressed.take() {
            let hit = self.layout.hit(p.x, p.y);
            self.hover = hit;
            self.tip_active = false;
            if hit == Some(act) {
                self.run(act);
            } else {
                self.redraw(false);
            }
            return;
        }

        match self.phase {
            Phase::Selecting => {
                let r = Rect::new(self.anchor.x, self.anchor.y, p.x, p.y);
                let sel = clamp_rect(r, self.w, self.h);
                let tiny = sel.width() < 3.0 || sel.height() < 3.0;
                if tiny {
                    self.sel = None;
                    self.phase = Phase::Idle;
                } else {
                    self.sel = Some(sel);
                    self.phase = Phase::Ready;
                }
                self.hint_until = 0;
                self.update_layout();
                self.invalidate();
            }
            Phase::Drawing => {
                self.finish_draft(p);
            }
            Phase::MoveObj | Phase::ResizeObj(_) | Phase::BendObj(_) => {
                self.phase = Phase::Ready;
            }
            Phase::MaybeGrab => {
                // No drag: the press was a plain click. Text opens its editor;
                // an arrow stays selected with its frame and grips live. A
                // click *outside* it takes the other path (`finish_draft`)
                // and drops the selection entirely.
                self.phase = Phase::Ready;
                if let Some(i) = self.doc.selected {
                    if matches!(
                        self.doc.objs.get(i).map(|o| &o.kind),
                        Some(Kind::Text { .. })
                    ) {
                        self.open_editor_at_object(i);
                        return;
                    }
                }
            }
            Phase::MoveSel | Phase::ResizeSel(_) => {
                self.phase = Phase::Ready;
                self.update_layout();
            }
            _ => {}
        }
        self.redraw(false);
    }

    fn on_middle_down(&mut self, lp: isize) {
        if self.picker.open || self.radius_picker.open || self.font_menu.open {
            return;
        }
        let p = self.clamp(point_of(lp));
        if self.sel.is_some_and(|s| s.contains(p.x, p.y)) {
            unsafe { SetCapture(self.hwnd) };
            self.drag_from = self.sel.unwrap_or_default();
            self.drag_start = p;
            self.phase = Phase::MoveSel;
        }
    }

    fn on_middle_up(&mut self, _lp: isize) {
        if matches!(self.phase, Phase::MoveSel) {
            unsafe { ReleaseCapture() };
            self.phase = Phase::Ready;
            self.update_layout();
            self.redraw(false);
        }
    }

    fn on_double_click(&mut self, lp: isize) {
        let p = point_of(lp);
        if self.picker.open || self.radius_picker.open || self.font_menu.open {
            return;
        }
        if self.layout.bar.contains(p.x, p.y) {
            return;
        }
        // Double-click a cubic control straightens the arrow again, matching
        // the usual "drag to bend, double-click to reset" gesture. This works
        // in Arrow mode too, right after the stroke.
        let r = HANDLE_HIT * self.scale;
        let on_ctrl = self
            .doc
            .selected
            .and_then(|i| self.doc.objs.get(i))
            .is_some_and(|o| match &o.kind {
                Kind::Arrow { c1, c2, .. } => c1.dist(&p) <= r || c2.dist(&p) <= r,
                _ => false,
            });
        if on_ctrl {
            self.doc.snapshot();
            if let Some(i) = self.doc.selected {
                if let Some(o) = self.doc.objs.get_mut(i) {
                    if let Kind::Arrow { a, c1, c2, b } = &mut o.kind {
                        let (q1, q2) = crate::model::arrow_straight(*a, *b);
                        *c1 = q1;
                        *c2 = q2;
                    }
                }
            }
            self.invalidate();
            self.redraw(false);
            return;
        }
        // A drawing tool must be able to draw two quick strokes: the second
        // WM_LBUTTONDOWN arrives as WM_LBUTTONDBLCLK, and copying there used to
        // close the whole session mid-annotation.
        if self.tool != Tool::Hand {
            return;
        }
        if let Some(i) = self.text_object_at(p) {
            self.open_editor_at_object(i);
            return;
        }
        if self.sel.is_some_and(|sel| sel.contains(p.x, p.y)) {
            self.do_copy();
        }
    }

    fn on_right(&mut self, lp: isize) {
        let p = point_of(lp);
        if self.editor.is_some() {
            self.discard_text();
            return;
        }
        if self.picker.open {
            self.close_picker();
            return;
        }
        if self.radius_picker.open {
            self.close_radius_picker();
            return;
        }
        if self.font_menu.open {
            self.close_font_menu();
            self.redraw(false);
            return;
        }
        if self.layout.bar.contains(p.x, p.y) {
            return;
        }
        // Non-destructive: a right click first backs out of the selected
        // annotation; the next one ends the session. It never clears the
        // selection and leaves annotations floating over a dimmed desktop.
        if self.doc.selected.is_some() {
            self.doc.selected = None;
            self.update_layout();
            self.redraw(false);
            return;
        }
        unsafe { PostMessageW(self.hwnd, WM_CLOSE, 0, 0) };
    }

    fn on_wheel(&mut self, delta: i32) {
        if delta == 0 {
            return;
        }
        // An open font list owns the wheel: with ~150 families the list is a
        // viewport, and scrolling is the only thing the user can be reaching
        // for while it covers the canvas.
        if self.font_menu.open {
            self.font_menu.scroll_by(if delta > 0 { -1 } else { 1 });
            self.redraw(false);
            return;
        }
        if self.editor.is_some() || self.picker.open || self.radius_picker.open {
            return;
        }
        let step = if delta > 0 { 1.0 } else { -1.0 };

        if let Some(i) = self.doc.selected {
            if i >= self.doc.objs.len() {
                return;
            }
            let hud: String;
            let mut changed = false;
            match &self.doc.objs[i].kind {
                Kind::Text { size, .. } => {
                    let new = (*size + step * 2.0 * self.scale).clamp(8.0 * self.scale, 200.0);
                    hud = format!("字号 {}", (new / self.scale).round());
                    if (new - *size).abs() > 0.01 {
                        self.doc.snapshot();
                        if let Some(o) = self.doc.objs.get_mut(i) {
                            if let Kind::Text { size, .. } = &mut o.kind {
                                *size = new;
                            }
                        }
                        changed = true;
                    }
                }
                Kind::Serial { size, .. } => {
                    let new = (*size + step * 2.0 * self.scale).clamp(10.0, 300.0);
                    hud = format!("序号大小 {}", (new / self.scale).round());
                    if (new - *size).abs() > 0.01 {
                        self.doc.snapshot();
                        if let Some(o) = self.doc.objs.get_mut(i) {
                            if let Kind::Serial { size, .. } = &mut o.kind {
                                *size = new;
                            }
                        }
                        changed = true;
                    }
                }
                // Blur and spotlight have no width to adjust; a wheel there
                // used to push an undo entry while changing nothing visible.
                Kind::Blur { .. } | Kind::Spot { .. } => return,
                _ => {
                    let cur = self.doc.objs[i].width;
                    let new = (cur + step).clamp(1.0, 40.0);
                    hud = format!("线宽 {}", fmt_num(new));
                    if (new - cur).abs() > 0.01 {
                        self.doc.snapshot();
                        if let Some(o) = self.doc.objs.get_mut(i) {
                            o.width = new;
                        }
                        changed = true;
                    }
                }
            }
            self.hud = Some((hud, now_ms() + HUD_MS));
            if changed {
                self.invalidate();
            }
            self.redraw(false);
            return;
        }

        if self.sel.is_none() {
            return;
        }
        if self.tool == Tool::Text {
            self.font_size =
                (self.font_size + step * 2.0 * self.scale).clamp(8.0 * self.scale, 200.0);
            let shown = (self.font_size / self.scale).round();
            self.hud = Some((format!("字号 {shown}"), now_ms() + HUD_MS));
        } else if self.tool == Tool::Serial {
            self.font_size =
                (self.font_size + step * 2.0 * self.scale).clamp(8.0 * self.scale, 200.0);
            let shown = (self.font_size * 1.4 / self.scale).round();
            self.hud = Some((format!("序号大小 {shown}"), now_ms() + HUD_MS));
        } else if self.tool.is_drawing() {
            self.width = (self.width + step).clamp(1.0, 40.0);
            self.width_index = layout::WIDTHS
                .iter()
                .position(|v| (v - self.width).abs() < 0.01);
            self.hud = Some((format!("线宽 {}", fmt_num(self.width)), now_ms() + HUD_MS));
            self.update_layout();
        } else {
            return;
        }
        self.redraw(false);
    }

    fn on_key(&mut self, vk: i32) {
        if self.font_menu.open {
            if vk == VK_ESCAPE {
                self.close_font_menu();
                self.redraw(false);
                return;
            }
            // Any other shortcut also dismisses the popover before it acts.
            self.close_font_menu();
        }
        if self.radius_picker.open {
            if vk == VK_ESCAPE {
                self.close_radius_picker();
                return;
            }
            self.close_radius_picker();
        }
        if self.picker.open {
            if vk == VK_ESCAPE {
                self.close_picker();
                return;
            }
            // Any other shortcut also dismisses the popover before it acts.
            self.close_picker();
        }
        let ctrl = unsafe { GetKeyState(VK_CONTROL) } < 0;
        let shift = unsafe { GetKeyState(VK_SHIFT) } < 0;

        match vk {
            VK_ESCAPE => {
                // One Esc backs out of an annotation; the next ends the
                // session, the way every other capture tool behaves.
                if self.doc.selected.is_some() {
                    self.doc.selected = None;
                    self.update_layout();
                } else {
                    unsafe { PostMessageW(self.hwnd, WM_CLOSE, 0, 0) };
                    return;
                }
            }
            VK_RETURN => {
                self.do_copy();
                return;
            }
            C_KEY if ctrl => {
                self.do_copy();
                return;
            }
            S_KEY if ctrl => {
                self.do_save();
                return;
            }
            L_KEY if ctrl => {
                self.start_longshot();
                return;
            }
            Z_KEY if ctrl && shift => {
                let keep = self.doc.selected;
                self.doc.redo();
                self.doc.selected = keep.filter(|i| *i < self.doc.objs.len());
                self.invalidate();
                self.update_layout();
            }
            Z_KEY if ctrl => {
                let keep = self.doc.selected;
                self.doc.undo();
                self.doc.selected = keep.filter(|i| *i < self.doc.objs.len());
                self.invalidate();
                self.update_layout();
            }
            Y_KEY if ctrl => {
                let keep = self.doc.selected;
                self.doc.redo();
                self.doc.selected = keep.filter(|i| *i < self.doc.objs.len());
                self.invalidate();
                self.update_layout();
            }
            A_KEY if ctrl => {
                self.sel = Some(self.monitor);
                self.phase = Phase::Ready;
                self.hint_until = 0;
                self.update_layout();
                self.invalidate();
            }
            // Text styling has the usual word-processor shortcuts; they only
            // fire while the text attribute row is actually on screen.
            B_KEY if ctrl => {
                if self.attr_tool() == Some(Tool::Text) {
                    self.toggle_text_style(Act::Bold);
                    self.update_layout();
                }
            }
            I_KEY if ctrl => {
                if self.attr_tool() == Some(Tool::Text) {
                    self.toggle_text_style(Act::Italic);
                    self.update_layout();
                }
            }
            V_KEY if !ctrl => {
                self.tool = Tool::Hand;
                self.doc.selected = None;
                self.update_layout();
            }
            VK_DELETE | VK_BACK => {
                if let Some(i) = self.doc.selected {
                    if i < self.doc.objs.len() {
                        self.doc.snapshot();
                        self.doc.objs.remove(i);
                    }
                    self.doc.selected = None;
                    self.invalidate();
                    self.update_layout();
                }
            }
            VK_LEFT | VK_UP | VK_RIGHT | VK_DOWN => {
                let step = if shift { 10.0 } else { 1.0 };
                let (dx, dy) = match vk {
                    VK_LEFT => (-step, 0.0),
                    VK_UP => (0.0, -step),
                    VK_RIGHT => (step, 0.0),
                    _ => (0.0, step),
                };
                if let Some(i) = self.doc.selected {
                    self.doc.snapshot();
                    if let Some(o) = self.doc.objs.get_mut(i) {
                        o.translate(dx, dy);
                    }
                    self.invalidate();
                } else if let Some(sel) = self.sel {
                    self.sel = Some(move_into(sel, dx, dy, self.w, self.h));
                    self.update_layout();
                    self.invalidate();
                }
            }
            K1..=K8 => {
                self.tool = Tool::from_index((vk - K1) as usize);
                self.doc.selected = None;
                self.update_layout();
            }
            _ => return,
        }
        self.redraw(false);
    }

    fn on_tick(&mut self) {
        let now = now_ms();
        if self.pending {
            self.redraw(true);
        }
        if let Some((_, until, _)) = self.toast {
            if now >= until {
                self.toast = None;
                self.redraw(false);
            }
        }
        if let Some((_, until)) = self.hud {
            if now >= until {
                self.hud = None;
                self.redraw(false);
            }
        }
        if self.hover.is_some()
            && !self.tip_active
            && now.saturating_sub(self.hover_since) >= TIP_DELAY_MS
        {
            self.tip_active = true;
            self.redraw(true);
        }
        if self.sel.is_none() {
            if self.hint_until > now {
                // The hint is static for its first ~2 s; only the last fade
                // window needs frames. The old code recomposed 25 fps for the
                // whole hold time.
                if self.hint_until - now <= HINT_FADE_MS && now.saturating_sub(self.hint_last) >= 40
                {
                    self.hint_last = now;
                    self.redraw(true);
                }
            } else if self.hint_until > 0 {
                self.hint_until = 0;
                self.redraw(true);
            }
        }
    }

    fn cursor_for_hover(&self) -> isize {
        let p = self.cursor;
        let arrow = unsafe { LoadCursorW(0, IDC_ARROW as *const u16) };
        let cross = unsafe { LoadCursorW(0, IDC_CROSS as *const u16) };
        let sizeall = unsafe { LoadCursorW(0, IDC_SIZEALL as *const u16) };
        if self.radius_picker.open {
            return match self.radius_picker.hit(p) {
                Some(RadiusHit::Track) => unsafe { LoadCursorW(0, IDC_SIZEWE as *const u16) },
                _ => arrow,
            };
        }
        if self.picker.open {
            return match self.picker.hit(p) {
                Some(PickerHit::Sv) | Some(PickerHit::Hue) => cross,
                _ => arrow,
            };
        }
        match self.phase {
            Phase::ResizeSel(h) | Phase::ResizeObj(h) => {
                return unsafe { LoadCursorW(0, cursor_for(h) as *const u16) };
            }
            Phase::MoveSel | Phase::MoveObj => return sizeall,
            Phase::BendObj(_) => return unsafe { LoadCursorW(0, IDC_HAND as *const u16) },
            Phase::MaybeGrab => {
                let text = self
                    .doc
                    .selected
                    .and_then(|i| self.doc.objs.get(i))
                    .is_some_and(|o| matches!(o.kind, Kind::Text { .. }));
                let id = if text { IDC_IBEAM } else { IDC_SIZEALL };
                return unsafe { LoadCursorW(0, id as *const u16) };
            }
            Phase::Drawing => return cross,
            _ => {}
        }
        if self.layout.bar.contains(p.x, p.y) {
            return arrow;
        }
        if self.tool == Tool::Hand {
            if let Some(i) = self.doc.selected {
                if let Some(o) = self.doc.objs.get(i) {
                    let b = o.bounds();
                    let r = HANDLE_HIT * self.scale;
                    for h in 0..8 {
                        if handle_point(b, h).dist(&p) <= r {
                            return unsafe { LoadCursorW(0, cursor_for(h) as *const u16) };
                        }
                    }
                    if let Kind::Arrow { c1, c2, .. } = &o.kind {
                        if c1.dist(&p) <= r || c2.dist(&p) <= r {
                            return unsafe { LoadCursorW(0, IDC_HAND as *const u16) };
                        }
                    }
                    if o.hit(p, 4.0 * self.scale) {
                        return sizeall;
                    }
                }
            }
            if self.hit_object(p).is_some() {
                return sizeall;
            }
        }
        if self.tool == Tool::Text {
            if let Some(i) = self.doc.selected {
                if let Some(o) = self.doc.objs.get(i) {
                    let b = o.bounds();
                    let r = HANDLE_HIT * self.scale;
                    for h in 0..8 {
                        if handle_point(b, h).dist(&p) <= r {
                            return unsafe { LoadCursorW(0, cursor_for(h) as *const u16) };
                        }
                    }
                }
            }
            if self.text_object_at(p).is_some() {
                return unsafe { LoadCursorW(0, IDC_IBEAM as *const u16) };
            }
        }
        if let Some(sel) = self.sel {
            if let Some(h) = self.border_zone(sel, p) {
                return unsafe { LoadCursorW(0, cursor_for(h) as *const u16) };
            }
            if sel.contains(p.x, p.y) {
                return if self.tool == Tool::Hand {
                    sizeall
                } else {
                    cross
                };
            }
        }
        cross
    }
}

/// Top-level window under `(x, y)` below `top` in z-order, skipping this
/// process's own windows. Used before hiding the overlay: the window below is
/// the one that must own the foreground (and therefore the wheel).
fn window_under_point(top: HWND, x: i32, y: i32) -> HWND {
    let me = unsafe { GetCurrentProcessId() };
    unsafe {
        let mut h = top;
        loop {
            h = GetWindow(h, GW_HWNDNEXT);
            if h == 0 {
                return 0;
            }
            if IsWindowVisible(h) == 0 || IsIconic(h) != 0 {
                continue;
            }
            let mut pid = 0;
            GetWindowThreadProcessId(h, &mut pid);
            if pid == me {
                continue;
            }
            // UWP apps (notably TextInputHost) keep full-screen "visible"
            // windows that DWM cloaks. Activating one would swallow the wheel.
            let mut cloaked = 0u32;
            if dcx_win32::DwmGetWindowAttribute(
                h,
                14, // DWMWA_CLOAKED
                &mut cloaked as *mut u32 as *mut core::ffi::c_void,
                4,
            ) == 0
                && cloaked != 0
            {
                continue;
            }
            let mut rc = dcx_win32::RECT::default();
            if dcx_win32::GetWindowRect(h, &mut rc) == 0 {
                continue;
            }
            if x >= rc.left && x < rc.right && y >= rc.top && y < rc.bottom {
                return h;
            }
        }
    }
}

fn cursor_for(handle: usize) -> usize {
    match handle % 8 {
        0 | 4 => IDC_SIZENWSE,
        2 | 6 => IDC_SIZENESW,
        1 | 5 => IDC_SIZENS,
        _ => IDC_SIZEWE,
    }
}

fn clamp_rect(r: Rect, w: u32, h: u32) -> Rect {
    Rect::new(
        r.x0.clamp(0.0, w as f32),
        r.y0.clamp(0.0, h as f32),
        r.x1.clamp(0.0, w as f32),
        r.y1.clamp(0.0, h as f32),
    )
}

fn move_into(r: Rect, dx: f32, dy: f32, w: u32, h: u32) -> Rect {
    let mut x0 = r.x0 + dx;
    let mut y0 = r.y0 + dy;
    let mut x1 = r.x1 + dx;
    let mut y1 = r.y1 + dy;
    if x0 < 0.0 {
        x1 -= x0;
        x0 = 0.0;
    }
    if y0 < 0.0 {
        y1 -= y0;
        y0 = 0.0;
    }
    if x1 > w as f32 {
        x0 -= x1 - w as f32;
        x1 = w as f32;
    }
    if y1 > h as f32 {
        y0 -= y1 - h as f32;
        y1 = h as f32;
    }
    Rect::new(x0.max(0.0), y0.max(0.0), x1, y1)
}

fn fmt_num(v: f32) -> String {
    if (v - v.round()).abs() < 0.05 {
        format!("{}", v.round() as i32)
    } else {
        format!("{v:.1}")
    }
}

fn next_serial(doc: &Doc, sel: Option<Rect>) -> u32 {
    doc.objs
        .iter()
        .filter(|o| match &o.kind {
            // Only badges visible in the current selection count; old hidden
            // ones from a previous selection would leave numbering gaps.
            Kind::Serial { .. } => sel.is_none_or(|s| o.bounds().intersect(&s).is_some()),
            _ => false,
        })
        .filter_map(|o| match &o.kind {
            Kind::Serial { n, .. } => Some(*n),
            _ => None,
        })
        .max()
        .unwrap_or(0)
        + 1
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

impl Session {
    fn run(&mut self, act: Act) {
        if self.doc.selected.is_some_and(|i| i >= self.doc.objs.len()) {
            self.doc.selected = None;
            self.update_layout();
        }
        match act {
            Act::Tool(t) => {
                self.tool = t;
                if t.is_drawing() {
                    self.doc.selected = None;
                }
                self.phase = Phase::Ready;
                self.update_layout();
            }
            Act::FullScreen => {
                self.sel = Some(self.monitor);
                self.phase = Phase::Ready;
                self.hint_until = 0;
                self.update_layout();
                self.invalidate();
            }
            Act::Undo => {
                let keep = self.doc.selected;
                self.doc.undo();
                self.doc.selected = keep.filter(|i| *i < self.doc.objs.len());
                self.invalidate();
                self.update_layout();
            }
            Act::Redo => {
                let keep = self.doc.selected;
                self.doc.redo();
                self.doc.selected = keep.filter(|i| *i < self.doc.objs.len());
                self.invalidate();
                self.update_layout();
            }
            Act::Copy => {
                self.do_copy();
                return;
            }
            Act::Save => {
                self.do_save();
                return;
            }
            Act::Close => {
                unsafe { PostMessageW(self.hwnd, WM_CLOSE, 0, 0) };
                return;
            }
            Act::Color(i) => {
                let rgb = layout::SWATCHES[i % layout::SWATCHES.len()].rgb;
                let new = Rgba::from_rgb(rgb);
                self.color = new;
                if let Some(idx) = self.doc.selected {
                    let changed = self.doc.objs.get(idx).is_some_and(|o| o.color != new);
                    if changed {
                        self.doc.snapshot();
                        if let Some(o) = self.doc.objs.get_mut(idx) {
                            o.color = new;
                        }
                        self.invalidate();
                    }
                }
            }
            Act::Width(i) => {
                self.width_index = Some(i.min(layout::WIDTHS.len() - 1));
                self.width = layout::WIDTHS[self.width_index.unwrap()];
                self.hud = Some((format!("线宽 {}", fmt_num(self.width)), now_ms() + HUD_MS));
                if let Some(idx) = self.doc.selected {
                    let changed = self
                        .doc
                        .objs
                        .get(idx)
                        .is_some_and(|o| (o.width - self.width).abs() > 0.01);
                    if changed {
                        self.doc.snapshot();
                        if let Some(o) = self.doc.objs.get_mut(idx) {
                            o.width = self.width;
                        }
                        self.invalidate();
                    }
                }
            }
            Act::FontUp | Act::FontDown => {
                let step = if act == Act::FontUp { 2.0 } else { -2.0 };
                if let Some(idx) = self.doc.selected {
                    let info = self.doc.objs.get(idx).and_then(|o| match &o.kind {
                        Kind::Text { size, .. } => Some((true, *size)),
                        Kind::Serial { size, .. } => Some((false, *size)),
                        _ => None,
                    });
                    if let Some((is_text, cur)) = info {
                        let new = if is_text {
                            (cur + step * self.scale).clamp(8.0 * self.scale, 200.0)
                        } else {
                            (cur + step * self.scale).clamp(10.0, 300.0)
                        };
                        self.hud = Some((
                            if is_text {
                                format!("字号 {}", (new / self.scale).round())
                            } else {
                                format!("序号大小 {}", (new / self.scale).round())
                            },
                            now_ms() + HUD_MS,
                        ));
                        if (new - cur).abs() > 0.01 {
                            self.doc.snapshot();
                            if let Some(o) = self.doc.objs.get_mut(idx) {
                                match &mut o.kind {
                                    Kind::Text { size, .. } => *size = new,
                                    Kind::Serial { size, .. } => *size = new,
                                    _ => {}
                                }
                            }
                            self.invalidate();
                        }
                    }
                } else {
                    self.font_size =
                        (self.font_size + step * self.scale).clamp(8.0 * self.scale, 200.0);
                    let shown = if self.tool == Tool::Serial {
                        (self.font_size * 1.4 / self.scale).round()
                    } else {
                        (self.font_size / self.scale).round()
                    };
                    self.hud = Some((
                        if self.tool == Tool::Serial {
                            format!("序号大小 {shown}")
                        } else {
                            format!("字号 {shown}")
                        },
                        now_ms() + HUD_MS,
                    ));
                }
            }
            Act::FontFace => {
                // Resolve the anchor before touching anything: an empty cell
                // means the row was rebuilt under the click and nothing should
                // close or open.
                let Some(anchor) = self.popover_anchor(Act::FontFace) else {
                    return;
                };
                // One popover at a time: the colour picker and the radius
                // slider are anchored to the same bar.
                self.picker.open = false;
                self.picker_drag = None;
                self.radius_picker.open = false;
                self.radius_drag = false;
                let cur = self.effective_text_style().face;
                self.font_menu
                    .open_with(&cur, anchor, self.monitor, self.scale);
                self.hover = None;
                self.tip_active = false;
            }
            Act::Font(i) => {
                let Some(face) = crate::fonts::all().get(i) else {
                    return;
                };
                let mut next = self.effective_text_style();
                next.face = face.id.clone();
                self.apply_text_style(next);
                self.close_font_menu();
                self.hud = Some((format!("字体 {}", face.label), now_ms() + HUD_MS));
            }
            Act::Bold | Act::Italic | Act::Shadow => {
                self.toggle_text_style(act);
                self.update_layout();
            }
            Act::ToggleFill => {
                if let Some(idx) = self.doc.selected {
                    let is_rect = matches!(
                        self.doc.objs.get(idx).map(|o| &o.kind),
                        Some(Kind::Rect { .. })
                    );
                    if is_rect {
                        self.doc.snapshot();
                        if let Some(o) = self.doc.objs.get_mut(idx) {
                            if let Kind::Rect { filled, .. } = &mut o.kind {
                                *filled = !*filled;
                            }
                        }
                        self.invalidate();
                    } else {
                        self.fill = !self.fill;
                    }
                } else {
                    self.fill = !self.fill;
                }
            }
            Act::ToggleCurve => {
                if let Some(idx) = self.doc.selected {
                    let is_arrow = matches!(
                        self.doc.objs.get(idx).map(|o| &o.kind),
                        Some(Kind::Arrow { .. })
                    );
                    if is_arrow {
                        self.doc.snapshot();
                        if let Some(o) = self.doc.objs.get_mut(idx) {
                            if let Kind::Arrow { a, c1, c2, b } = &mut o.kind {
                                // The button cycles between the straight chord
                                // and a default bow; the two control grips are
                                // for everything in between and beyond.
                                let (q1, q2) = if crate::model::arrow_bent(*a, *c1, *c2, *b) {
                                    crate::model::arrow_straight(*a, *b)
                                } else {
                                    crate::model::arrow_bowed(*a, *b, DEFAULT_BOW)
                                };
                                *c1 = q1;
                                *c2 = q2;
                            }
                        }
                        self.invalidate();
                    } else {
                        self.curve = !self.curve;
                    }
                } else {
                    self.curve = !self.curve;
                }
            }
            Act::Radius => {
                // Continuous slider: any value 0–128, not a five-step cycle.
                self.picker.open = false;
                self.picker_drag = None;
                self.close_font_menu();
                let Some(anchor) = self.popover_anchor(Act::Radius) else {
                    return;
                };
                self.radius_picker
                    .open_with(self.sel_radius, anchor, self.monitor, self.scale);
                self.radius_drag = false;
                self.hover = None;
                self.tip_active = false;
            }
            Act::CustomColor => {
                self.radius_picker.open = false;
                self.radius_drag = false;
                self.close_font_menu();
                let Some(anchor) = self.popover_anchor(Act::CustomColor) else {
                    return;
                };
                let current = self
                    .doc
                    .selected
                    .and_then(|i| self.doc.objs.get(i))
                    .map(|o| o.color)
                    .unwrap_or(self.color);
                self.picker
                    .open_with(current, anchor, self.monitor, self.scale);
                self.picker_drag = None;
                self.picker_snapshot = false;
                self.hover = None;
                self.tip_active = false;
            }
            Act::LongShot => {
                self.start_longshot();
            }
            Act::Divider => {}
        }
        self.redraw(false);
    }

    fn start_draft(&mut self, p: Pt) {
        self.phase = Phase::Drawing;
        self.anchor = p;
        self.drag_start = p;
        // The arrow tool defers its stroke: pressing without moving must not
        // leave anything behind. A click on empty canvas is how the user
        // leaves the previous arrow's edit state (see `finish_draft`); the
        // arrow itself stays on the canvas. The stroke is created only on the
        // first move past DRAG_MIN (see `update_draft`).
        if self.tool == Tool::Arrow {
            return;
        }
        // Any other stroke starts with a clean slate: the previous object's
        // frame must not stay on screen while this one is dragged out.
        self.doc.selected = None;
        let (color, width) = (self.color, self.width);
        self.draft = Some(match self.tool {
            Tool::Rect => Obj::new(
                Kind::Rect {
                    r: Rect::new(p.x, p.y, p.x, p.y),
                    radius: self.cfg.draw.radius.max(0.0),
                    filled: self.fill,
                },
                color,
                width,
            ),
            Tool::Marker => Obj::new(
                Kind::Marker { pts: vec![p] },
                color,
                (width * 4.0).max(10.0),
            ),
            Tool::Blur => Obj::new(
                Kind::Blur {
                    r: Rect::new(p.x, p.y, p.x, p.y),
                },
                color,
                0.0,
            ),
            Tool::Spot => Obj::new(
                Kind::Spot {
                    r: Rect::new(p.x, p.y, p.x, p.y),
                },
                color,
                0.0,
            ),
            Tool::Arrow | Tool::Text | Tool::Hand | Tool::Serial => return,
        });
        self.invalidate();
        self.redraw(false);
    }

    fn update_draft(&mut self, p: Pt) {
        if self.draft.is_none() {
            if self.tool != Tool::Arrow || self.anchor.dist(&p) < DRAG_MIN * self.scale {
                return;
            }
            self.doc.selected = None;
            self.draft = Some(Obj::new(
                Kind::Arrow {
                    a: self.anchor,
                    c1: self.anchor,
                    c2: self.anchor,
                    b: p,
                },
                self.color,
                self.width,
            ));
            self.invalidate();
            self.update_layout();
            // Fall through: the block below sets `b` and both controls. Leaving
            // them on the anchor would bend the whole shaft towards it when the
            // release arrives without a final mouse-move.
        }
        let Some(draft) = self.draft.as_mut() else {
            return;
        };
        match &mut draft.kind {
            Kind::Rect { r, .. } | Kind::Blur { r } | Kind::Spot { r } => {
                *r = Rect::new(self.anchor.x, self.anchor.y, p.x, p.y);
            }
            Kind::Arrow { a, c1, c2, b } => {
                *b = p;
                let (q1, q2) = if self.curve {
                    crate::model::arrow_bowed(*a, *b, DEFAULT_BOW)
                } else {
                    crate::model::arrow_straight(*a, *b)
                };
                *c1 = q1;
                *c2 = q2;
            }
            Kind::Marker { pts } => {
                if let Some(last) = pts.last() {
                    if last.dist(&p) >= 2.0 {
                        pts.push(p);
                    }
                }
            }
            Kind::Text { .. } | Kind::Serial { .. } => {}
        }
    }

    fn finish_draft(&mut self, p: Pt) {
        self.update_draft(p);
        self.phase = Phase::Ready;
        let Some(draft) = self.draft.take() else {
            // No stroke was ever created: for the arrow (and hand) tool this
            // is a click on empty canvas. Drop the previous object's edit
            // state so its frame and grips stop covering the artwork. Drawing
            // tools clear the selection in `start_draft`, so none of them
            // reach here with one. The arrow itself is an object, not edit
            // state — it stays on the canvas.
            if self.doc.selected.take().is_some() {
                self.update_layout();
                self.invalidate();
            }
            return;
        };
        let bounds = draft.bounds();
        let tiny = match &draft.kind {
            Kind::Arrow { a, b, .. } => a.dist(b) < DRAG_MIN * self.scale,
            _ => bounds.width() < 3.0 && bounds.height() < 3.0,
        };
        if tiny {
            self.invalidate();
            return;
        }
        let is_arrow = matches!(draft.kind, Kind::Arrow { .. });
        self.doc.snapshot();
        self.doc.objs.push(draft);
        // Keep a freshly drawn arrow selected: its two cubic grips become
        // immediately draggable, the way AirSnip hands you the curve controls
        // right after the stroke.
        if is_arrow {
            self.doc.selected = Some(self.doc.objs.len() - 1);
        }
        self.invalidate();
        self.update_layout();
    }

    // -- text ---------------------------------------------------------------

    fn open_editor(&mut self, at: Pt, editing: Option<usize>) {
        self.text_at = at;
        self.editing = editing;
        self.doc.selected = None;
        self.phase = Phase::Ready;
        let style = self.text_style.clone();
        self.editor =
            crate::edit_text::open(self.hwnd, at, self.font_size, self.monitor, "", &style);
        if self.editor.is_none() {
            crate::app::log("text editor: open failed");
        }
        self.update_layout();
        self.redraw(false);
    }

    fn open_editor_at_object(&mut self, i: usize) {
        let Some(obj) = self.doc.objs.get(i).cloned() else {
            return;
        };
        let Kind::Text {
            at,
            text,
            size,
            style,
        } = &obj.kind
        else {
            return;
        };
        self.font_size = *size;
        self.color = obj.color;
        // Editing an object adopts its style, so the next text keeps the look
        // the user just worked in.
        self.text_style = style.clone();
        self.text_at = *at;
        self.editing = Some(i);
        self.doc.selected = None;
        self.phase = Phase::Ready;
        self.editor =
            crate::edit_text::open(self.hwnd, *at, *size, self.monitor, text, &self.text_style);
        if self.editor.is_none() {
            crate::app::log("text editor: open failed");
        }
        self.update_layout();
        self.redraw(false);
    }

    fn commit_text(&mut self) {
        let Some(editor) = self.editor.take() else {
            return;
        };
        let text = editor.text();
        editor.close();
        let editing = self.editing.take();
        if let Some(i) = editing {
            // Editing and committing unchanged text must not create an undo
            // step; emptying it deletes the object and does.
            let old = self.doc.objs.get(i).and_then(|o| match &o.kind {
                Kind::Text { text, .. } => Some(text.clone()),
                _ => None,
            });
            let mut empty = false;
            if let Some(old) = old {
                empty = text.trim().is_empty();
                if (empty && !old.is_empty()) || (!empty && old != text) {
                    self.doc.snapshot();
                    if empty {
                        if i < self.doc.objs.len() {
                            self.doc.objs.remove(i);
                        }
                    } else if let Some(o) = self.doc.objs.get_mut(i) {
                        if let Kind::Text { text: slot, .. } = &mut o.kind {
                            *slot = text;
                        }
                    }
                    self.invalidate();
                }
            }
            // Keep the text selected so it can be dragged/resized right away.
            self.doc.selected = if empty || i >= self.doc.objs.len() {
                None
            } else {
                Some(i)
            };
        } else if !text.trim().is_empty() {
            self.doc.snapshot();
            self.doc.objs.push(Obj::new(
                Kind::Text {
                    at: self.text_at,
                    text,
                    size: self.font_size,
                    style: self.text_style.clone(),
                },
                self.color,
                self.width,
            ));
            self.doc.selected = Some(self.doc.objs.len() - 1);
            self.invalidate();
        }
        unsafe { SetFocus(self.hwnd) };
        self.update_layout();
        self.redraw(false);
    }

    fn discard_text(&mut self) {
        if let Some(editor) = self.editor.take() {
            editor.close();
        }
        self.editing = None;
        unsafe { SetFocus(self.hwnd) };
        self.update_layout();
        self.redraw(false);
    }

    fn close_editor(&mut self) {
        if let Some(editor) = self.editor.take() {
            editor.close();
        }
        self.editing = None;
    }

    fn apply_picker(&mut self) {
        let c = self.picker.color();
        self.color = c;
        self.custom = c;
        self.custom_set = true;
        if let Some(i) = self.doc.selected {
            let changed = self.doc.objs.get(i).is_some_and(|o| o.color != c);
            if changed {
                if !self.picker_snapshot {
                    self.doc.snapshot();
                    self.picker_snapshot = true;
                }
                if let Some(o) = self.doc.objs.get_mut(i) {
                    o.color = c;
                }
                self.invalidate();
            }
        }
        self.redraw(false);
    }

    // -- text style ---------------------------------------------------------

    /// Anchor for a popover that must open **outside the whole bar**: with two
    /// rows, "above the FontFace cell" would put the list on top of the tool
    /// row. Keep the cell's x so the panel still points at the control.
    fn popover_anchor(&self, act: Act) -> Option<Rect> {
        let cell = self.layout.cell(act)?;
        if cell.is_empty() {
            return None;
        }
        Some(Rect::new(
            cell.x0,
            self.layout.bar.y0,
            cell.x1,
            self.layout.bar.y1,
        ))
    }

    /// Close the family popover and drop any scrollbar drag with it: the
    /// pointer capture belongs to the thumb, not to the panel.
    fn close_font_menu(&mut self) {
        self.font_menu.open = false;
        self.font_scroll_drag = false;
    }

    /// What the attribute row shows and what a toggle edits: the selected text
    /// object's own style when there is one, otherwise the defaults.
    fn effective_text_style(&self) -> TextStyle {
        self.doc
            .selected
            .and_then(|i| self.doc.objs.get(i))
            .and_then(|o| match &o.kind {
                Kind::Text { style, .. } => Some(style.clone()),
                _ => None,
            })
            .unwrap_or_else(|| self.text_style.clone())
    }

    /// Apply `next` to the selected text object (one undo step) and remember it
    /// as the default for the next one, mirroring how colour and width behave.
    fn apply_text_style(&mut self, next: TextStyle) {
        if let Some(i) = self.doc.selected {
            let cur = self.doc.objs.get(i).and_then(|o| match &o.kind {
                Kind::Text { style, .. } => Some(style.clone()),
                _ => None,
            });
            if let Some(cur) = cur {
                if cur != next {
                    self.doc.snapshot();
                    if let Some(o) = self.doc.objs.get_mut(i) {
                        if let Kind::Text { style, .. } = &mut o.kind {
                            *style = next.clone();
                        }
                    }
                    self.invalidate();
                }
            }
        }
        self.text_style = next;
    }

    fn toggle_text_style(&mut self, which: Act) {
        let mut next = self.effective_text_style();
        let (flag, label) = match which {
            Act::Bold => {
                next.bold = !next.bold;
                (next.bold, "粗体")
            }
            Act::Italic => {
                next.italic = !next.italic;
                (next.italic, "斜体")
            }
            _ => {
                next.shadow = !next.shadow;
                (next.shadow, "文字阴影")
            }
        };
        self.apply_text_style(next);
        self.hud = Some((
            format!("{label} {}", if flag { "开" } else { "关" }),
            now_ms() + HUD_MS,
        ));
    }

    fn close_picker(&mut self) {
        self.picker.open = false;
        self.picker_drag = None;
        self.picker_snapshot = false;
        unsafe { ReleaseCapture() };
        self.redraw(false);
    }

    fn apply_radius(&mut self) {
        let v = self.radius_picker.value.clamp(0.0, crate::radius::MAX);
        if (v - self.sel_radius).abs() > 0.01 {
            self.sel_radius = v;
            self.invalidate();
        }
        self.redraw(false);
    }

    fn close_radius_picker(&mut self) {
        self.radius_picker.open = false;
        self.radius_drag = false;
        unsafe { ReleaseCapture() };
        self.redraw(false);
    }

    // -- long capture -------------------------------------------------------

    /// Hand the selection to the standalone scrolling capture. This blocks in
    /// the longshot module's own message loop; the overlay is hidden first and
    /// deliberately **not** restored, because both a finished and a cancelled
    /// capture end the snapshot session (AirSnip behaviour).
    fn start_longshot(&mut self) {
        if crate::longshot::is_active() {
            return;
        }
        let Some(sel) = self.sel else {
            self.toast("先框选要长截图的区域", true, 2200);
            return;
        };
        let Some((x0, y0, x1, y1)) = sel.to_px(self.w, self.h) else {
            return;
        };
        let fw = (x1 - x0) as u32;
        let fh = (y1 - y0) as u32;
        if fw < 40 || fh < crate::stitch::MIN_SEL_H {
            self.toast("选区太小，长截图至少需要 40 × 32 px", true, 2600);
            return;
        }
        if (fw as u64) * (fh as u64) > crate::stitch::MAX_PIXELS {
            self.toast("选区太大，长截图最多 2400 万像素", true, 2800);
            return;
        }
        let ctx = crate::longshot::Ctx {
            region: (self.origin.0 + x0, self.origin.1 + y0, fw, fh),
            scale: self.scale,
            radius: self.sel_radius,
            out_dir: self.cfg.output_dir(),
            jpeg: self.cfg.jpeg(),
            quality: self.cfg.output.jpeg_quality,
        };
        self.long_active = true;
        // Hand focus to the page **before** hiding the overlay. A hidden
        // window stays the foreground window otherwise, and the wheel then
        // goes to this process instead of the page being captured.
        let cx = self.origin.0 + (x0 + x1) / 2;
        let cy = self.origin.1 + (y0 + y1) / 2;
        let target = window_under_point(self.hwnd, cx, cy);
        unsafe {
            KillTimer(self.hwnd, TIMER_FRAME);
            if target != 0 {
                let ok = SetForegroundWindow(target);
                crate::app::log(&format!(
                    "longshot: focus target=0x{target:X} ok={ok} fg=0x{:X}",
                    GetForegroundWindow()
                ));
            } else {
                crate::app::log("longshot: no window under the selection");
            }
            ShowWindow(self.hwnd, SW_HIDE);
        }
        let started = crate::longshot::run(ctx);
        self.long_active = false;
        if !started {
            unsafe {
                ShowWindow(self.hwnd, SW_SHOW);
                SetTimer(self.hwnd, TIMER_FRAME, FRAME_MS as u32, core::ptr::null());
                force_foreground(self.hwnd);
                SetFocus(self.hwnd);
            }
            self.invalidate();
            self.redraw(true);
            self.toast("无法启动长截图", true, 2600);
            return;
        }
        // Post, never destroy: this call is still on the session's stack.
        unsafe { PostMessageW(self.hwnd, WM_CLOSE, 0, 0) };
    }

    fn toast(&mut self, msg: impl Into<String>, error: bool, ms: u64) {
        self.toast = Some((msg.into(), now_ms() + ms, error));
        self.redraw(false);
    }

    fn do_copy(&mut self) {
        if self.sel.is_none() {
            self.toast("先用鼠标框选要截取的区域", true, 2400);
            return;
        }
        if self.editor.is_some() {
            self.commit_text();
        }
        let sel = self.sel.unwrap_or_default();
        self.compose_export();
        let stride = self.surface.stride;
        let Some((mut bytes, w, h)) =
            crate::output::crop(self.surface.bytes(), stride, self.w, self.h, &sel)
        else {
            return;
        };
        crate::output::round_alpha(&mut bytes, w, h, self.sel_radius);
        match crate::output::to_clipboard(self.hwnd, &bytes, w, h) {
            Ok(()) => {
                unsafe { PostMessageW(self.hwnd, WM_CLOSE, 0, 0) };
            }
            Err(e) => {
                self.toast(e, true, 2800);
            }
        }
    }

    fn do_save(&mut self) {
        if self.sel.is_none() {
            self.toast("先用鼠标框选要保存的区域", true, 2400);
            return;
        }
        if self.editor.is_some() {
            self.commit_text();
        }
        let sel = self.sel.unwrap_or_default();
        self.compose_export();
        let stride = self.surface.stride;
        let Some((mut bytes, w, h)) =
            crate::output::crop(self.surface.bytes(), stride, self.w, self.h, &sel)
        else {
            return;
        };
        crate::output::round_alpha(&mut bytes, w, h, self.sel_radius);
        let dir = self.cfg.output_dir();
        match crate::output::save(
            &dir,
            &bytes,
            w,
            h,
            self.cfg.jpeg(),
            self.cfg.output.jpeg_quality,
        ) {
            Ok(path) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "图片".into());
                // Staying open lets the user keep annotating or press Esc;
                // closing immediately left no trace of where the file went.
                self.toast(format!("已保存 · {name}"), false, 3600);
            }
            Err(e) => {
                self.toast(e, true, 3200);
            }
        }
    }

    // -- painting -----------------------------------------------------------

    fn redraw(&mut self, force: bool) {
        let now = now_ms();
        if !force && now.saturating_sub(self.last_frame) < FRAME_MS {
            self.pending = true;
            return;
        }
        self.last_frame = now;
        self.pending = false;
        self.compose(true);
        self.present();
    }

    fn present(&self) {
        // `--demo-synth` builds a session without a window; GetDC(0) would be
        // the desktop and blit the synthetic frame onto the user's screen.
        if self.hwnd == 0 {
            return;
        }
        unsafe {
            let dc = GetDC(self.hwnd);
            if dc != 0 {
                self.surface
                    .blit(dc, &Rect::from_xywh(0.0, 0.0, self.w as f32, self.h as f32));
                ReleaseDC(self.hwnd, dc);
            }
        }
    }

    fn build_content(&mut self, mask_corners: bool) {
        let w = self.w;
        let h = self.h;
        let sel = self.sel;
        let radius = self.sel_radius;
        {
            let Session {
                surface,
                base,
                dim,
                doc,
                draft,
                ..
            } = self;
            let stride = surface.stride;
            {
                let buf = surface.bytes_mut();
                buf.copy_from_slice(dim);
            }
            if let Some(sel) = sel {
                if let Some((x0, y0, x1, y1)) = sel.to_px(w, h) {
                    copy_rect(surface.bytes_mut(), stride, base, stride, x0, y0, x1, y1);
                }
            }
            // Effects read the frozen frame, never the composed result. The
            // in-progress object is part of the scene: without it a blur drag
            // showed nothing until mouse-up.
            if let Some(sel) = sel {
                let buf = surface.bytes_mut();
                for obj in doc.objs.iter().chain(draft.iter()) {
                    match &obj.kind {
                        Kind::Blur { r } => {
                            if let Some(clipped) = r.intersect(&sel) {
                                paint::blur_region(buf, stride, base, clipped, w, h, 1.0);
                            }
                        }
                        Kind::Spot { r } => paint::spotlight(buf, stride, *r, sel, w, h),
                        _ => {}
                    }
                }
            }
            // Annotations only exist in the context of a selection: without
            // one they would float over the dimmed desktop after Esc.
            if let Some(sel) = sel {
                surface.canvas(|sk| {
                    for obj in doc.objs.iter().chain(draft.iter()) {
                        paint::object(sk, obj);
                    }
                });
                if !doc.objs.is_empty() || draft.is_some() {
                    let buf = surface.bytes_mut();
                    paint::clip_outside(buf, stride, dim, sel, w, h);
                }
                if mask_corners && radius > 0.5 {
                    // Screen-side mask: the corners outside the rounded export
                    // fade into the dim surround. The export builds the same
                    // content *without* this mask, so its anti-aliased edge
                    // keeps the page colour instead of a dark dimmed fringe.
                    let buf = surface.bytes_mut();
                    paint::round_selection(buf, stride, dim, sel, radius, w, h);
                }
            }
        }
    }

    fn compose(&mut self, with_chrome: bool) {
        if !self.content_valid {
            self.build_content(true);
            let bytes = self.surface.bytes();
            self.content.copy_from_slice(bytes);
            self.content_valid = true;
        } else {
            let Session {
                surface, content, ..
            } = self;
            surface.bytes_mut().copy_from_slice(content);
        }

        if !with_chrome {
            return;
        }
        let now = now_ms();
        let sel_obj = self.doc.selected.and_then(|i| self.doc.objs.get(i));
        let obj = sel_obj.map(|o| o.bounds());
        // While an annotation is selected, the attribute row reflects *that
        // annotation*, not the last defaults — otherwise a red swatch ring sat
        // next to a yellow selected arrow.
        let (color, width_index, fill, curve) = if let Some(o) = sel_obj {
            let fill = match &o.kind {
                Kind::Rect { filled, .. } => *filled,
                _ => self.fill,
            };
            let curve = match &o.kind {
                Kind::Arrow { a, c1, c2, b } => crate::model::arrow_bent(*a, *c1, *c2, *b),
                _ => self.curve,
            };
            (
                o.color,
                layout::WIDTHS
                    .iter()
                    .position(|v| (*v - o.width).abs() < 0.01),
                fill,
                curve,
            )
        } else {
            (self.color, self.width_index, self.fill, self.curve)
        };
        let size = self
            .sel
            .map(|s| (s.width().round() as i32, s.height().round() as i32));
        let toast = self.toast.as_ref().map(|(t, _, _)| t.as_str());
        let toast_error = self.toast.as_ref().map(|(_, _, e)| *e).unwrap_or(false);
        let hud = self.hud.as_ref().map(|(t, _)| (t.as_str(), self.cursor));
        let tooltip = if self.tip_active && !self.picker.open && !self.radius_picker.open {
            self.hover.and_then(|a| {
                let (label, key) = layout::tip(a);
                if label.is_empty() {
                    return None;
                }
                let anchor = self.layout.cell(a)?;
                Some((label, key, anchor))
            })
        } else {
            None
        };
        let hint_alpha = if self.sel.is_none() && now < self.hint_until {
            ((self.hint_until - now) as f32 / HINT_FADE_MS as f32).min(1.0)
        } else {
            0.0
        };
        let curve_handles = sel_obj.and_then(|o| match &o.kind {
            Kind::Arrow { a, c1, c2, b } => Some((*a, *c1, *c2, *b)),
            _ => None,
        });
        // The attribute row reflects the selected text object's own font,
        // weight and shadow — not the session defaults, which is what the
        // first version showed next to an already-styled object.
        let text_style = self.effective_text_style();
        let serial_preview = if self.tool == Tool::Serial
            && self.editor.is_none()
            && !self.picker.open
            && !self.radius_picker.open
            && self
                .sel
                .is_some_and(|s| s.contains(self.cursor.x, self.cursor.y))
            && !self.layout.bar.contains(self.cursor.x, self.cursor.y)
        {
            Some((
                next_serial(&self.doc, self.sel),
                self.cursor,
                self.font_size * 1.4,
            ))
        } else {
            None
        };
        let chrome = Chrome {
            sel: self.sel,
            scale: self.scale,
            layout: &self.layout,
            monitor: self.monitor,
            hover: self.hover,
            pressed: self.pressed,
            tool: self.tool,
            color,
            width_index,
            fill,
            curve,
            text_style: &text_style,
            radius: self.sel_radius,
            custom: self.custom,
            custom_set: self.custom_set,
            obj,
            curve_handles,
            serial_preview,
            can_undo: self.doc.can_undo(),
            can_redo: self.doc.can_redo(),
            size,
            toast,
            toast_error,
            hud,
            hint_alpha,
            selecting: self.phase == Phase::Selecting,
            tooltip,
        };
        self.surface.canvas(|sk| paint::chrome(sk, &chrome));
        if self.radius_picker.open {
            let picker = &self.radius_picker;
            self.surface.canvas(|sk| picker.draw(sk, self.scale));
        }
        if self.picker.open {
            let picker = &self.picker;
            self.surface.canvas(|sk| picker.draw(sk, self.scale));
        }
        if self.font_menu.open {
            let menu = &self.font_menu;
            let cursor = self.cursor;
            self.surface.canvas(|sk| menu.draw(sk, self.scale, cursor));
        }
    }

    /// Chrome-free content for export. Deliberately skips the screen-side
    /// rounded-corner mask so the crop keeps the original corner pixels;
    /// `output::round_alpha` then writes only alpha (white outside).
    fn compose_export(&mut self) {
        self.build_content(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The in-progress object must be part of the composed frame. The first
    /// version only drew `doc.objs`, so a rectangle appeared on mouse-up and a
    /// blur showed nothing at all while dragging.
    #[test]
    fn draft_is_composed_before_release() {
        let (w, h) = (120u32, 100u32);
        let mut bgra = vec![0u8; (w as usize) * (h as usize) * 4];
        for px in bgra.chunks_exact_mut(4) {
            px[0] = 40;
            px[1] = 40;
            px[2] = 40;
            px[3] = 255;
        }
        let shot = Shot {
            origin: (0, 0),
            w,
            h,
            bgra,
        };
        let mut s = build(shot, Config::default()).expect("session");
        s.sel = Some(Rect::from_xywh(0.0, 0.0, w as f32, h as f32));
        s.phase = Phase::Ready;
        s.tool = Tool::Rect;
        s.color = Rgba::opaque(0xE5, 0x48, 0x4D);
        s.width = 3.0;
        s.draft = Some(Obj::new(
            Kind::Rect {
                r: Rect::from_xywh(20.0, 20.0, 60.0, 50.0),
                radius: 0.0,
                filled: false,
            },
            s.color,
            3.0,
        ));
        s.compose(true);

        let off = (40usize * w as usize + 20usize) * 4;
        let px = &s.surface.bytes()[off..off + 4];
        assert!(
            px[2] > 150 && px[1] < 120,
            "draft stroke missing from the frame: b={} g={} r={}",
            px[0],
            px[1],
            px[2]
        );
    }

    /// Text attributes are per object, not per session: toggling with a text
    /// object selected must edit that object (in one undo step) *and* leave the
    /// choice as the default for the next one. The row reads back the object's
    /// own style, so a bold object must not show a plain bold toggle.
    #[test]
    fn text_style_edits_the_selected_object_and_becomes_the_default() {
        let (w, h) = (200u32, 120u32);
        let mut bgra = vec![0u8; (w as usize) * (h as usize) * 4];
        for px in bgra.chunks_exact_mut(4) {
            px[3] = 255;
        }
        let shot = Shot {
            origin: (0, 0),
            w,
            h,
            bgra,
        };
        let mut s = build(shot, Config::default()).expect("session");
        s.sel = Some(Rect::from_xywh(0.0, 0.0, w as f32, h as f32));
        s.phase = Phase::Ready;
        s.tool = Tool::Text;
        s.update_layout();

        assert!(!s.text_style.bold);
        s.run(Act::Bold);
        assert!(
            s.text_style.bold,
            "no selection: the toggle sets the default"
        );

        s.doc.objs.push(Obj::new(
            Kind::Text {
                at: Pt::new(10.0, 40.0),
                text: "样式".to_string(),
                size: 20.0,
                style: TextStyle::default(),
            },
            Rgba::opaque(0xFF, 0xFF, 0xFF),
            2.0,
        ));
        s.doc.selected = Some(0);
        assert!(!s.effective_text_style().italic);

        s.run(Act::Italic);
        match &s.doc.objs[0].kind {
            Kind::Text { style, .. } => {
                assert!(style.italic, "the selected object keeps the toggle");
                assert!(!style.bold, "its own weight must survive the edit");
            }
            _ => panic!("object kind changed"),
        }
        assert!(s.text_style.italic, "the edit also seeds the next text");
        assert!(s.doc.can_undo());

        s.doc.undo();
        match &s.doc.objs[0].kind {
            Kind::Text { style, .. } => assert!(!style.italic, "one undo step per toggle"),
            _ => panic!("object kind changed"),
        }
    }

    /// Pressing the body of an existing arrow with the arrow tool must select
    /// it (and stop there) rather than stacking a second head on top; pressing
    /// empty canvas must still start a new stroke.
    /// Leaving the edit state must also unbind the attribute row: a colour
    /// picked afterwards is the default for the next stroke, not a silent
    /// recolour of the arrow the user just finished looking at.
    #[test]
    fn blank_click_unbinds_the_attribute_row() {
        fn lp(x: f32, y: f32) -> isize {
            (((y as i32) << 16) | (x as i32 & 0xFFFF)) as isize
        }

        let (w, h) = (300u32, 200u32);
        let shot = Shot {
            origin: (0, 0),
            w,
            h,
            bgra: vec![0u8; (w as usize) * (h as usize) * 4],
        };
        let mut s = build(shot, Config::default()).expect("session");
        s.sel = Some(Rect::from_xywh(0.0, 0.0, w as f32, h as f32));
        s.phase = Phase::Ready;
        s.tool = Tool::Arrow;
        s.doc.objs.push(Obj::new(
            Kind::Arrow {
                a: Pt::new(30.0, 100.0),
                c1: Pt::new(100.0, 100.0),
                c2: Pt::new(170.0, 100.0),
                b: Pt::new(240.0, 100.0),
            },
            Rgba::opaque(0xE5, 0x48, 0x4D),
            4.0,
        ));
        s.doc.selected = Some(0);
        s.update_layout();
        let old = s.doc.objs[0].color;

        s.on_down(lp(60.0, 40.0));
        s.on_up(lp(60.0, 40.0));
        assert_eq!(s.doc.selected, None, "the edit state is gone");

        let picked = Rgba::from_rgb(layout::SWATCHES[3].rgb);
        assert_ne!(picked, old);
        s.run(Act::Color(3));
        assert_eq!(s.doc.objs[0].color, old, "the old arrow is not recoloured");
        assert_eq!(s.color, picked, "the colour is the next stroke's default");
    }

    #[test]
    fn arrow_tool_reselects_an_existing_arrow() {
        fn lp(x: f32, y: f32) -> isize {
            (((y as i32) << 16) | (x as i32 & 0xFFFF)) as isize
        }

        let (w, h) = (300u32, 200u32);
        let shot = Shot {
            origin: (0, 0),
            w,
            h,
            bgra: vec![0u8; (w as usize) * (h as usize) * 4],
        };
        let mut s = build(shot, Config::default()).expect("session");
        s.sel = Some(Rect::from_xywh(0.0, 0.0, w as f32, h as f32));
        s.phase = Phase::Ready;
        s.tool = Tool::Arrow;
        s.doc.objs.push(Obj::new(
            Kind::Arrow {
                a: Pt::new(30.0, 100.0),
                c1: Pt::new(100.0, 100.0),
                c2: Pt::new(170.0, 100.0),
                b: Pt::new(240.0, 100.0),
            },
            Rgba::opaque(0xE5, 0x48, 0x4D),
            4.0,
        ));
        s.update_layout();
        let before = s.doc.objs.len();

        s.on_down(lp(120.0, 100.0));
        assert_eq!(s.doc.selected, Some(0), "the shaft selects the arrow");
        s.on_up(lp(120.0, 100.0));
        assert_eq!(s.doc.objs.len(), before, "no second arrow was started");
        assert_eq!(s.doc.selected, Some(0), "selection survives the release");

        // A click on empty canvas is not a stroke: nothing is created, and it
        // leaves the previous arrow's edit state (frame + grips disappear).
        // The arrow object itself must survive.
        s.on_down(lp(60.0, 40.0));
        assert!(s.draft.is_none(), "a click must not start a stroke");
        s.on_up(lp(60.0, 40.0));
        assert_eq!(s.doc.objs.len(), before, "a click must not add an arrow");
        assert_eq!(s.doc.selected, None, "a click ends the edit state");
        assert_eq!(s.phase, Phase::Ready);

        // Leaving the edit state is not destructive: clicking the shaft again
        // re-enters it.
        s.on_down(lp(120.0, 100.0));
        assert_eq!(s.doc.selected, Some(0), "the arrow can be re-selected");
        s.on_up(lp(120.0, 100.0));

        // A real drag still draws exactly one arrow.
        s.on_down(lp(60.0, 40.0));
        s.on_mouse(lp(150.0, 80.0));
        assert!(s.draft.is_some(), "dragging starts the stroke");
        s.on_up(lp(150.0, 80.0));
        assert_eq!(s.doc.objs.len(), before + 1, "one arrow per drag");
        match &s.doc.objs[before].kind {
            Kind::Arrow { a, b, .. } => {
                assert!((a.x - 60.0).abs() < 0.5 && (b.x - 150.0).abs() < 0.5)
            }
            _ => panic!("expected an arrow"),
        }

        // Press and release with no intermediate move: the stroke is still
        // built correctly (both controls on the chord, not on the anchor).
        s.on_down(lp(60.0, 120.0));
        s.on_up(lp(200.0, 120.0));
        assert_eq!(s.doc.objs.len(), before + 2, "a fast drag still draws");
        match &s.doc.objs[before + 1].kind {
            Kind::Arrow { a, c1, c2, b } => assert!(
                !crate::model::arrow_bent(*a, *c1, *c2, *b),
                "a straight drag must not leave the controls on the anchor"
            ),
            _ => panic!("expected an arrow"),
        }
    }

    /// The font controls must be real cells of the attribute row, hit-testable
    /// at their centres, and the popover must open from the family cell and
    /// map its middle row back to a family. Drawing and clicking share
    /// `Layout`, so this is the contract that keeps them from drifting.
    #[test]
    fn text_attribute_row_hits_font_controls() {
        let (w, h) = (320u32, 200u32);
        let shot = Shot {
            origin: (0, 0),
            w,
            h,
            bgra: vec![0u8; (w as usize) * (h as usize) * 4],
        };
        let mut s = build(shot, Config::default()).expect("session");
        s.sel = Some(Rect::from_xywh(8.0, 8.0, 200.0, 120.0));
        s.phase = Phase::Ready;
        s.tool = Tool::Text;
        s.update_layout();

        for act in [
            Act::FontFace,
            Act::Bold,
            Act::Italic,
            Act::Shadow,
            Act::FontUp,
            Act::FontDown,
        ] {
            let cell = s
                .layout
                .cell(act)
                .unwrap_or_else(|| panic!("{act:?} missing"));
            let (cx, cy) = cell.center();
            assert_eq!(s.layout.hit(cx, cy), Some(act), "{act:?} is not clickable");
        }

        s.run(Act::FontFace);
        assert!(s.font_menu.open, "the family cell opens the popover");
        let (cx, cy) = s.font_menu.rect.center();
        match s.font_menu.hit(Pt::new(cx, cy)) {
            // The centre lands on some row of the (scrolled) viewport.
            MenuHit::Row(i) => assert!(i < crate::fonts::all().len()),
            other => panic!("menu centre is not a row: {other:?}"),
        }

        // The wheel moves the viewport by whole rows, so the same screen point
        // must resolve to a later family afterwards.
        if crate::fonts::all().len() > 16 {
            let probe = Pt::new(
                s.font_menu.rect.x0 + 20.0 * s.scale,
                s.font_menu.rect.y0 + 12.0 * s.scale,
            );
            let before = match s.font_menu.hit(probe) {
                MenuHit::Row(i) => i,
                other => panic!("probe is not on a row: {other:?}"),
            };
            s.font_menu.scroll_by(1);
            let after = match s.font_menu.hit(probe) {
                MenuHit::Row(i) => i,
                other => panic!("probe is not on a row after scrolling: {other:?}"),
            };
            assert_eq!(after, before + 3, "one notch scrolls three rows");
            s.font_menu.scroll_by(-1);

            // The scrollbar band must be caught before the row underneath it,
            // and dragging it must reach both ends of the list.
            let grab = Pt::new(
                s.font_menu.rect.x1 - 8.0 * s.scale,
                s.font_menu.rect.y0 + 20.0 * s.scale,
            );
            assert_eq!(
                s.font_menu.hit(grab),
                MenuHit::Scrollbar,
                "the scrollbar must own the pointer over its band"
            );
            let top_row = Pt::new(
                s.font_menu.rect.x0 + 20.0 * s.scale,
                s.font_menu.rect.y0 + 12.0 * s.scale,
            );
            s.font_menu.scroll_to_y(f32::MAX);
            match s.font_menu.hit(top_row) {
                MenuHit::Row(i) => assert!(i > before + 3, "dragging must reach the end"),
                other => panic!("top row after drag: {other:?}"),
            }
            s.font_menu.scroll_to_y(f32::MIN);
            match s.font_menu.hit(top_row) {
                MenuHit::Row(i) => assert_eq!(i, 0, "dragging back must reach the top"),
                other => panic!("top row after drag back: {other:?}"),
            }
        }

        let pick = crate::fonts::all().len() - 1;
        s.run(Act::Font(pick));
        assert!(!s.font_menu.open, "picking a family closes the popover");
        assert_eq!(
            s.text_style.face,
            crate::fonts::all()[pick].id,
            "the pick becomes the default for the next text"
        );
    }
}

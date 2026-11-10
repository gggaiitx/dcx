//! Painting for the overlay. Pure functions over a [`Sk`] so the composition
//! order stays visible in one place (`session::Session::compose`).

use crate::icons::{self, Icon};
use crate::layout::{
    self, Act, Layout, ACCENT, ACTIVE, BORDER, DANGER, DISABLED, HOVER, INK, MUTED, PANEL,
    PANEL_DEEP, PRESSED, TEXT,
};
use crate::model::{Kind, Obj, Pt, TextStyle, Tool};
use crate::surface::{copy_rect, path_rect, tint_rect, Rect, Rgba, Sk};
use crate::text;

/// Drop shadow behind text objects. Black at ~55 % keeps white and coloured
/// glyphs legible over both light documents and dark terminals.
const SHADOW: Rgba = Rgba::new(0x00, 0x00, 0x00, 0x8C);

/// Draw a run of text with the glyph masks; returns the advance width.
pub fn label(sk: &mut Sk, s: &str, x: f32, y: f32, size: f32, color: Rgba) -> f32 {
    label_styled(sk, s, x, y, size, color, &TextStyle::ui())
}

/// Same, in an explicit family/weight. Used by the font popover, whose rows
/// are their own specimen, and by the attribute row's font box.
pub fn label_styled(
    sk: &mut Sk,
    s: &str,
    x: f32,
    y: f32,
    size: f32,
    color: Rgba,
    style: &TextStyle,
) -> f32 {
    let Some(cov) = text::coverage_styled(s, size, style) else {
        return 0.0;
    };
    let dx = (x - cov.bleed).round() as i32;
    let dy = (y - cov.bleed).round() as i32;
    sk.draw_coverage(dx, dy, cov.w, cov.h, &cov.alpha, color);
    text::measure_styled(s, size, style).0
}

pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Rgba::new(f(a.r, b.r), f(a.g, b.g), f(a.b, b.b), f(a.a, b.a))
}

/// One annotation, clipped by the caller to the selection.
pub fn object(sk: &mut Sk, obj: &Obj) {
    match &obj.kind {
        Kind::Rect { r, radius, filled } => {
            if *filled {
                sk.fill_round(r, *radius, obj.color);
            } else {
                sk.stroke_round(r, *radius, obj.width, obj.color);
            }
        }
        Kind::Arrow { a, c1, c2, b } => {
            // Head geometry first: the shaft has to stop inside the head, so
            // both share the tangent at `b`. A straight arrow keeps both
            // controls on the chord, so the tangent is still the chord.
            let from = if c2.dist(b) > 0.5 { *c2 } else { *a };
            let dx = b.x - from.x;
            let dy = b.y - from.y;
            let len = (dx * dx + dy * dy).sqrt().max(0.001);
            let (ux, uy) = (dx / len, dy / len);
            let head = (obj.width * 4.6).max(11.0);
            // Base at least as wide as the stroke, or a thick line steps out of
            // the head's flanks.
            let spread = (head * 0.52).max(obj.width * 0.8);
            // Stop the shaft ~80 % into the head. Drawn to `b` the butt cap's
            // two corners sit exactly on the tip and poke out on both sides —
            // the "line sticks through the arrow head" report.
            let back = (head * 0.8).min(len * 0.8);
            let stop = Pt::new(b.x - ux * back, b.y - uy * back);
            let mut pb = tiny_skia::PathBuilder::new();
            pb.move_to(a.x, a.y);
            pb.cubic_to(c1.x, c1.y, c2.x, c2.y, stop.x, stop.y);
            if let Some(path) = pb.finish() {
                sk.stroke_path(&path, obj.width, obj.color, false);
            }
            let bx = b.x - ux * head;
            let by = b.y - uy * head;
            let mut pb = tiny_skia::PathBuilder::new();
            pb.move_to(b.x, b.y);
            pb.line_to(bx - uy * spread, by + ux * spread);
            pb.line_to(bx + uy * spread, by - ux * spread);
            pb.close();
            if let Some(path) = pb.finish() {
                sk.fill_path(&path, obj.color);
            }
        }
        Kind::Text {
            at,
            text: body,
            size,
            style,
        } => {
            let Some(cov) = text::coverage_styled(body, *size, style) else {
                return;
            };
            let dx = (at.x - cov.bleed).round() as i32;
            let dy = (at.y - cov.bleed).round() as i32;
            if style.shadow {
                // Hard drop shadow from the same mask: one offset copy, so the
                // shadow always matches the glyph shape exactly (a GDI shadow
                // offset would need a second font render per frame).
                let off = text::shadow_offset(*size).round() as i32;
                sk.draw_coverage(dx + off, dy + off, cov.w, cov.h, &cov.alpha, SHADOW);
            }
            sk.draw_coverage(dx, dy, cov.w, cov.h, &cov.alpha, obj.color);
        }
        Kind::Marker { pts } => {
            let mut pb = tiny_skia::PathBuilder::new();
            if let Some(first) = pts.first() {
                pb.move_to(first.x, first.y);
                for p in &pts[1..] {
                    pb.line_to(p.x, p.y);
                }
                if pts.len() == 1 {
                    pb.line_to(first.x + 0.1, first.y);
                }
                if let Some(path) = pb.finish() {
                    sk.stroke_path(
                        &path,
                        obj.width,
                        obj.color.alpha(obj.color.a.min(150)),
                        false,
                    );
                }
            }
        }
        Kind::Serial { at, n, size } => {
            serial_badge(sk, *at, *n, *size, obj.color, 255);
        }
        // Effects are painted from the frozen frame, not from the vector layer.
        Kind::Blur { .. } | Kind::Spot { .. } => {}
    }
}

/// A numbered step badge. `alpha` scales the fill/number together so the same
/// drawing works for the solid object and the hover preview.
pub fn serial_badge(sk: &mut Sk, at: Pt, n: u32, size: f32, fill: Rgba, alpha: u8) {
    if size < 6.0 {
        return;
    }
    let a = alpha;
    let r = size * 0.5;
    let body = fill.alpha(((fill.a as u32 * a as u32) / 255) as u8);
    sk.circle(at.x, at.y, r, body);
    let ring = if luminance(fill) > 0.65 {
        Rgba::new(0, 0, 0, ((70u32 * a as u32) / 255) as u8)
    } else {
        Rgba::new(255, 255, 255, ((110u32 * a as u32) / 255) as u8)
    };
    sk.ring(at.x, at.y, r, (size * 0.045).max(1.0), ring);

    let body_text = n.to_string();
    let ink = if luminance(fill) > 0.65 {
        Rgba::new(0x0B, 0x12, 0x16, a)
    } else {
        Rgba::new(0xFF, 0xFF, 0xFF, a)
    };
    let mut num_size = size * 0.52;
    let mut tw = text::measure(&body_text, num_size).0;
    while tw > size * 0.72 && num_size > size * 0.22 {
        num_size *= 0.88;
        tw = text::measure(&body_text, num_size).0;
    }
    let th = text::measure(&body_text, num_size).1;
    label(
        sk,
        &body_text,
        at.x - tw * 0.5,
        at.y - th * 0.5 + num_size * 0.04,
        num_size,
        ink,
    );
}

fn luminance(c: Rgba) -> f32 {
    (0.2126 * c.r as f32 + 0.7152 * c.g as f32 + 0.0722 * c.b as f32) / 255.0
}

/// Gaussian-ish blur of one rectangle, sampled from `base` so stacked
/// annotations never smear into each other.
pub fn blur_region(
    dst: &mut [u8],
    stride: usize,
    base: &[u8],
    rect: Rect,
    w: u32,
    h: u32,
    strength: f32,
) {
    let Some((x0, y0, x1, y1)) = rect.to_px(w, h) else {
        return;
    };
    let rw = (x1 - x0) as usize;
    let rh = (y1 - y0) as usize;
    if rw < 3 || rh < 3 {
        return;
    }
    let mut buf = vec![0u8; rw * rh * 4];
    for y in 0..rh {
        let src = (y0 as usize + y) * stride + x0 as usize * 4;
        buf[y * rw * 4..(y + 1) * rw * 4].copy_from_slice(&base[src..src + rw * 4]);
    }
    let radius = ((rw.min(rh) as f32 / 16.0).clamp(2.0, 24.0) * strength).round() as usize;
    if radius >= 1 {
        box_blur(&mut buf, rw, rh, radius);
        box_blur(&mut buf, rw, rh, radius);
    }
    for y in 0..rh {
        let dst_off = (y0 as usize + y) * stride + x0 as usize * 4;
        dst[dst_off..dst_off + rw * 4].copy_from_slice(&buf[y * rw * 4..(y + 1) * rw * 4]);
    }
}

fn box_blur(buf: &mut [u8], w: usize, h: usize, r: usize) {
    // Separable sliding-window box blur, 4 channels. Edges are extended
    // (clamped), which keeps the border colour from creeping inward the way a
    // zero-padded window does.
    let mut tmp = vec![0u8; buf.len()];
    let win = (2 * r + 1) as i32;
    let idx = |x: i32| x.clamp(0, w as i32 - 1) as usize;

    for y in 0..h {
        let row = y * w * 4;
        for c in 0..4 {
            let mut sum: i32 = 0;
            for i in -(r as i32)..=(r as i32) {
                sum += buf[row + idx(i) * 4 + c] as i32;
            }
            for x in 0..w {
                tmp[row + x * 4 + c] = (sum / win) as u8;
                let out = idx(x as i32 - r as i32);
                let inc = idx(x as i32 + r as i32 + 1);
                sum += buf[row + inc * 4 + c] as i32 - buf[row + out * 4 + c] as i32;
            }
        }
    }
    let idx_y = |y: i32| y.clamp(0, h as i32 - 1) as usize;
    for x in 0..w {
        for c in 0..4 {
            let mut sum: i32 = 0;
            for i in -(r as i32)..=(r as i32) {
                sum += tmp[(idx_y(i) * w + x) * 4 + c] as i32;
            }
            for y in 0..h {
                buf[(y * w + x) * 4 + c] = (sum / win) as u8;
                let out = idx_y(y as i32 - r as i32);
                let inc = idx_y(y as i32 + r as i32 + 1);
                sum += tmp[(inc * w + x) * 4 + c] as i32 - tmp[(out * w + x) * 4 + c] as i32;
            }
        }
    }
}

/// Everything that is chrome rather than content.
pub struct Chrome<'a> {
    pub sel: Option<Rect>,
    pub scale: f32,
    pub layout: &'a Layout,
    pub monitor: Rect,
    pub hover: Option<Act>,
    pub pressed: Option<Act>,
    pub tool: Tool,
    pub color: Rgba,
    pub width_index: Option<usize>,
    pub fill: bool,
    pub curve: bool,
    /// Text attributes shown in the attribute row: the selected text object's
    /// style, or the defaults for the next one.
    pub text_style: &'a TextStyle,
    /// Selection/output corner radius in physical pixels.
    pub radius: f32,
    /// Last colour chosen in the free picker; `set` false until then.
    pub custom: Rgba,
    pub custom_set: bool,
    /// Bounds of the selected object, when there is one.
    pub obj: Option<Rect>,
    /// Selected arrow's cubic: (a, c1, c2, b). Both controls get their own
    /// draggable grip, so the bow can be adjusted freely (either side, any
    /// amount, even an S-curve).
    pub curve_handles: Option<(Pt, Pt, Pt, Pt)>,
    /// Ghost badge under the pointer while the serial tool is armed.
    pub serial_preview: Option<(u32, Pt, f32)>,
    pub can_undo: bool,
    pub can_redo: bool,
    pub size: Option<(i32, i32)>,
    pub toast: Option<&'a str>,
    pub toast_error: bool,
    pub hud: Option<(&'a str, Pt)>,
    pub hint_alpha: f32,
    /// Dragging out the first selection: frame only, no handles, no bar.
    pub selecting: bool,
    /// Hovered control: label, key hint and the cell it belongs to.
    pub tooltip: Option<(&'static str, &'static str, Rect)>,
}

pub fn chrome(sk: &mut Sk, c: &Chrome<'_>) {
    let s = c.scale.max(0.6);

    if let Some(sel) = c.sel {
        // Accent line over a dark halo: readable on white documents and on
        // dark terminals alike, without the double corner brackets that made
        // the first version look like it had two borders.
        if c.radius > 0.5 {
            sk.stroke_round(&sel, c.radius, 2.4 * s, Rgba::new(0, 0, 0, 0x66));
            sk.stroke_round(&sel, c.radius, 1.2 * s, ACCENT);
        } else {
            sk.stroke_rect(&sel, 2.4 * s, Rgba::new(0, 0, 0, 0x66));
            sk.stroke_rect(&sel, 1.2 * s, ACCENT);
        }
        if !c.selecting {
            for h in 0..8 {
                let p = crate::model::handle_point(sel, h);
                selection_handle(sk, p, s);
            }
        }
    }

    if let Some(b) = c.obj {
        if let Some(path) = path_rect(&b) {
            // Solid white over a dark underlay: distinct from the teal
            // marquee, and readable on both white documents and dark editors.
            sk.stroke_path(&path, 2.2 * s, Rgba::new(0, 0, 0, 0x55), false);
            sk.stroke_path(&path, 1.1 * s, Rgba::new(0xFF, 0xFF, 0xFF, 0xDD), false);
        }
        for h in 0..8 {
            let p = crate::model::handle_point(b, h);
            handle_square(sk, p, 3.0 * s, s);
        }
    }

    if let Some((a, c1, c2, b)) = c.curve_handles {
        // Both cubic controls are actual handles, with dashed guides to their
        // endpoints. Dragging either one bends the arrow freely; double-click
        // straightens it again.
        let guide = Rgba::new(0x1E, 0x9C, 0x8B, 0x99);
        dashed(sk, a, c1, 1.1 * s, 5.0 * s, 4.0 * s, guide);
        dashed(sk, c2, b, 1.1 * s, 5.0 * s, 4.0 * s, guide);
        for p in [c1, c2] {
            // Dark halo keeps the grip visible over white documents.
            sk.ring(p.x, p.y, 6.3 * s, 1.2 * s, Rgba::new(0, 0, 0, 0x55));
            sk.circle(p.x, p.y, 5.0 * s, Rgba::opaque(0xFF, 0xFF, 0xFF));
            sk.ring(p.x, p.y, 5.0 * s, 1.5 * s, ACCENT);
        }
    }

    if let Some((n, at, size)) = c.serial_preview {
        serial_badge(sk, at, n, size, c.color, 120);
    }

    if let Some((w, h)) = c.size {
        if let Some(sel) = c.sel {
            size_pill(sk, c, sel, w, h, s);
        }
    }

    bar(sk, c);

    if let Some((label, key, anchor)) = c.tooltip {
        tooltip(sk, c, label, key, anchor, s);
    }
    if let Some((text, at)) = c.hud {
        hud(sk, c, text, at, s);
    }
    if let Some(msg) = c.toast {
        toast(sk, c, msg, s);
    }
    if c.hint_alpha > 0.02 && c.sel.is_none() {
        initial_hint(sk, c, s);
    }
}

fn selection_handle(sk: &mut Sk, p: Pt, s: f32) {
    let r = 3.8 * s;
    sk.circle(p.x, p.y, r, Rgba::opaque(0xF3, 0xF5, 0xF7));
    sk.ring(p.x, p.y, r, 1.0 * s, Rgba::new(0, 0, 0, 0x66));
}

fn dashed(sk: &mut Sk, a: Pt, b: Pt, width: f32, dash: f32, gap: f32, color: Rgba) {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0 {
        return;
    }
    let (ux, uy) = (dx / len, dy / len);
    let mut t = 0.0;
    while t < len {
        let end = (t + dash).min(len);
        sk.line(
            (a.x + ux * t, a.y + uy * t),
            (a.x + ux * end, a.y + uy * end),
            width,
            color,
        );
        t += dash + gap;
    }
}

fn handle_square(sk: &mut Sk, p: Pt, half: f32, s: f32) {
    let r = Rect::from_xywh(p.x - half, p.y - half, half * 2.0, half * 2.0);
    sk.fill_rect(&r, Rgba::opaque(0xF3, 0xF5, 0xF7));
    sk.stroke_rect(&r, 1.0 * s, Rgba::new(0, 0, 0, 0x55));
}

fn size_pill(sk: &mut Sk, c: &Chrome<'_>, sel: Rect, w: i32, h: i32, s: f32) {
    let body = format!("{w} × {h}");
    let size = 12.0 * s;
    let tw = text::measure(&body, size).0;
    let pad = 7.0 * s;
    let bw = tw + pad * 2.0;
    let bh = 22.0 * s;
    // Inside the top-left corner: it never collides with the bar and stays
    // next to where the drag started. Falls outside only when the selection is
    // too small to hold it.
    let mut bx = sel.x0 + 6.0 * s;
    let mut by = sel.y0 + 6.0 * s;
    if bw + 12.0 * s > sel.width() || bh + 12.0 * s > sel.height() {
        bx = sel.x0;
        by = sel.y0 - bh - 5.0 * s;
    }
    bx = bx.clamp(
        c.monitor.x0 + 4.0 * s,
        (c.monitor.x1 - 4.0 * s - bw).max(c.monitor.x0 + 4.0 * s),
    );
    by = by.clamp(
        c.monitor.y0 + 4.0 * s,
        (c.monitor.y1 - 4.0 * s - bh).max(c.monitor.y0 + 4.0 * s),
    );
    let pill = Rect::from_xywh(bx, by, bw, bh);
    icons::panel(
        sk,
        pill,
        7.0 * s,
        Rgba::new(0x0E, 0x12, 0x16, 0xD8),
        Rgba::new(0xFF, 0xFF, 0xFF, 0x22),
    );
    label(sk, &body, bx + pad, by + (bh - 14.0 * s) * 0.5, size, TEXT);
}

fn icon_of(t: Tool) -> Icon {
    match t {
        Tool::Hand => Icon::Hand,
        Tool::Rect => Icon::Frame,
        Tool::Arrow => Icon::Vector,
        Tool::Text => Icon::Glyph,
        Tool::Marker => Icon::Marker,
        Tool::Blur => Icon::Defocus,
        Tool::Spot => Icon::Aperture,
        Tool::Serial => Icon::Serial,
    }
}

fn cell_bg(sk: &mut Sk, rect: &Rect, radius: f32, hot: bool, down: bool, on: bool) {
    if down {
        sk.fill_round(rect, radius, PRESSED);
    } else if on {
        sk.fill_round(rect, radius, ACTIVE);
    } else if hot {
        sk.fill_round(rect, radius, HOVER);
    }
}

fn bar(sk: &mut Sk, c: &Chrome<'_>) {
    let l = c.layout;
    if l.bar.is_empty() {
        return;
    }
    let s = l.scale;
    icons::panel(sk, l.bar, 12.0 * s, PANEL, BORDER);
    if let Some(y) = l.separator {
        // Hairline between the tool row and the attribute row: without it the
        // two rows read as one very tall panel with stray icons.
        sk.line(
            (l.bar.x0 + 10.0 * s, y),
            (l.bar.x1 - 10.0 * s, y),
            1.0,
            BORDER,
        );
    }

    for (rect, act) in &l.hits {
        if *act == Act::Divider {
            let x = rect.center().0;
            sk.line((x, rect.y0 + 9.0 * s), (x, rect.y1 - 9.0 * s), 1.0, BORDER);
            continue;
        }
        let hot = c.hover == Some(*act);
        let down = c.pressed == Some(*act) && hot;
        let radius = 8.0 * s;
        match act {
            Act::Tool(t) => {
                let on = *t == c.tool;
                cell_bg(sk, rect, radius, hot, down, on);
                let color = if on { ACCENT } else { TEXT };
                icons::draw(sk, icon_of(*t), rect.inflate(-7.0 * s), color, 1.6);
            }
            Act::FullScreen => {
                cell_bg(sk, rect, radius, hot, down, false);
                icons::draw(sk, Icon::Expand, rect.inflate(-7.0 * s), TEXT, 1.6);
            }
            Act::LongShot => {
                cell_bg(sk, rect, radius, hot, down, false);
                icons::draw(sk, Icon::Long, rect.inflate(-7.0 * s), TEXT, 1.6);
            }
            Act::Radius => {
                let on = c.radius > 0.5;
                cell_bg(sk, rect, radius, hot, down, on);
                icons::draw(
                    sk,
                    Icon::Radius,
                    rect.inflate(-7.0 * s),
                    if on { ACCENT } else { TEXT },
                    1.6,
                );
            }
            Act::CustomColor => {
                let on = c.custom_set
                    && c.color.r == c.custom.r
                    && c.color.g == c.custom.g
                    && c.color.b == c.custom.b;
                cell_bg(sk, rect, radius, hot, down, on);
                icons::draw_color_wheel(sk, rect.inflate(-3.0 * s), c.custom, c.custom_set, ACCENT);
            }
            Act::Undo | Act::Redo => {
                let enabled = if *act == Act::Undo {
                    c.can_undo
                } else {
                    c.can_redo
                };
                cell_bg(sk, rect, radius, hot && enabled, down && enabled, false);
                let color = if enabled { TEXT } else { DISABLED };
                icons::draw(
                    sk,
                    if *act == Act::Undo {
                        Icon::Rewind
                    } else {
                        Icon::Forward
                    },
                    rect.inflate(-7.0 * s),
                    color,
                    1.7,
                );
            }
            Act::Copy => {
                let base = if down {
                    mix(ACCENT, Rgba::new(0, 0, 0, 255), 0.18)
                } else if hot {
                    mix(ACCENT, Rgba::opaque(255, 255, 255), 0.10)
                } else {
                    ACCENT
                };
                sk.fill_round(rect, 9.0 * s, base);
                action_cell(sk, rect, Icon::Stack, "复制", INK, 1.7, s);
            }
            Act::Save => {
                cell_bg(sk, rect, radius, hot, down, false);
                action_cell(sk, rect, Icon::Slot, "保存", TEXT, 1.6, s);
            }
            Act::Close => {
                let hot_bg = if down {
                    Rgba::new(0xE5, 0x48, 0x4D, 0x44)
                } else if hot {
                    Rgba::new(0xE5, 0x48, 0x4D, 0x2A)
                } else {
                    Rgba::new(0, 0, 0, 0)
                };
                if hot_bg.a > 0 {
                    sk.fill_round(rect, radius, hot_bg);
                }
                icons::draw(
                    sk,
                    Icon::Dismiss,
                    rect.inflate(-9.0 * s),
                    if hot { DANGER } else { MUTED },
                    1.8,
                );
            }
            Act::Color(i) => {
                if hot || down {
                    sk.fill_round(rect, radius, if down { PRESSED } else { HOVER });
                }
                let rgb = layout::SWATCHES[*i % layout::SWATCHES.len()].rgb;
                let is_on = c.color.r == rgb[0] && c.color.g == rgb[1] && c.color.b == rgb[2];
                icons::draw_swatch(sk, *rect, Rgba::from_rgb(rgb), is_on, ACCENT);
            }
            Act::Width(i) => {
                let on = c.width_index == Some(*i);
                cell_bg(sk, rect, 7.0 * s, hot, down, on);
                icons::draw_width(sk, *i, *rect, if on { ACCENT } else { TEXT });
            }
            Act::FontFace => {
                cell_bg(sk, rect, 7.0 * s, hot, down, false);
                let style = c.text_style.without_shadow();
                let size = 11.5 * s;
                let caret = 7.0 * s;
                let body = text::fit_px(
                    crate::fonts::label_of(&c.text_style.face),
                    size,
                    &style,
                    rect.width() - caret - 12.0 * s,
                );
                let tw = text::measure_styled(&body, size, &style).0;
                let (cx, cy) = rect.center();
                let x = cx - (tw + 5.0 * s + caret) * 0.5;
                // Preview the family (and weight) inside the box, like a font
                // picker should: the label itself is the specimen.
                if let Some(cov) = text::coverage_styled(&body, size, &style) {
                    let dx = (x - cov.bleed).round() as i32;
                    let dy = (cy - 6.5 * s - cov.bleed).round() as i32;
                    sk.draw_coverage(dx, dy, cov.w, cov.h, &cov.alpha, TEXT);
                }
                let ax = x + tw + 5.0 * s;
                let ay = cy + 1.6 * s;
                sk.line(
                    (ax, ay - 2.6 * s),
                    (ax + caret * 0.5, ay + 1.2 * s),
                    1.4 * s,
                    MUTED,
                );
                sk.line(
                    (ax + caret * 0.5, ay + 1.2 * s),
                    (ax + caret, ay - 2.6 * s),
                    1.4 * s,
                    MUTED,
                );
            }
            Act::Bold | Act::Italic => {
                let on = if *act == Act::Bold {
                    c.text_style.bold
                } else {
                    c.text_style.italic
                };
                cell_bg(sk, rect, 7.0 * s, hot, down, on);
                // Serif italic has a readable "I"; the sans italic I is just a
                // slash and read as a separator in the first render. Both
                // glyphs are drawn bold: at 14 pt a regular italic I is a 2 px
                // hairline and disappears next to the B.
                let latin = TextStyle {
                    face: "Times New Roman".to_string(),
                    bold: true,
                    italic: *act == Act::Italic,
                    shadow: false,
                };
                let body = if *act == Act::Bold { "B" } else { "I" };
                let size = 14.0 * s;
                let tw = text::measure_styled(body, size, &latin).0;
                let (cx, cy) = rect.center();
                if let Some(cov) = text::coverage_styled(body, size, &latin) {
                    let dx = (cx - tw * 0.5 - cov.bleed).round() as i32;
                    let dy = (cy - 6.5 * s - cov.bleed).round() as i32;
                    sk.draw_coverage(
                        dx,
                        dy,
                        cov.w,
                        cov.h,
                        &cov.alpha,
                        if on { ACCENT } else { TEXT },
                    );
                }
            }
            Act::Shadow => {
                cell_bg(sk, rect, 7.0 * s, hot, down, c.text_style.shadow);
                // Two offset squares: the front one is the glyph, the dim one
                // behind it is the shadow. Reads at 14 px where an "S" would
                // look like a keyboard shortcut. The offset must be larger than
                // the radius, otherwise the two melt into one block.
                let col = if c.text_style.shadow { ACCENT } else { TEXT };
                let side = 8.5 * s;
                let (cx, cy) = rect.center();
                let back = Rect::from_xywh(
                    cx - side * 0.5 + 2.6 * s,
                    cy - side * 0.5 + 2.6 * s,
                    side,
                    side,
                );
                let front = Rect::from_xywh(
                    cx - side * 0.5 - 2.0 * s,
                    cy - side * 0.5 - 2.0 * s,
                    side,
                    side,
                );
                sk.fill_round(&back, 1.5 * s, Rgba::new(col.r, col.g, col.b, 0x55));
                sk.fill_round(&front, 1.5 * s, col);
            }
            Act::FontUp | Act::FontDown => {
                cell_bg(sk, rect, 7.0 * s, hot, down, false);
                let body = if *act == Act::FontUp { "A+" } else { "A−" };
                let size = 14.0 * s;
                let tw = text::measure(body, size).0;
                let (cx, cy) = rect.center();
                label(sk, body, cx - tw * 0.5, cy - 7.0 * s, size, TEXT);
            }
            Act::ToggleFill => {
                cell_bg(sk, rect, 7.0 * s, hot, down, c.fill);
                icons::draw(
                    sk,
                    Icon::Fill,
                    rect.inflate(-8.0 * s),
                    if c.fill { ACCENT } else { TEXT },
                    1.5,
                );
            }
            Act::ToggleCurve => {
                cell_bg(sk, rect, 7.0 * s, hot, down, c.curve);
                icons::draw(
                    sk,
                    Icon::Curve,
                    rect.inflate(-8.0 * s),
                    if c.curve { ACCENT } else { TEXT },
                    1.5,
                );
            }
            // Popover rows are painted by `font_menu`, never by the bar.
            Act::Font(_) => {}
            Act::Divider => {}
        }
    }
}

fn action_cell(sk: &mut Sk, rect: &Rect, icon: Icon, body: &str, color: Rgba, weight: f32, s: f32) {
    let size = 13.0 * s;
    let tw = text::measure(body, size).0;
    let icon_side = 17.0 * s;
    let gap = 5.0 * s;
    let total = icon_side + gap + tw;
    let (cx, cy) = rect.center();
    let x = cx - total * 0.5;
    let icon_area = Rect::from_xywh(x, cy - icon_side * 0.5, icon_side, icon_side);
    icons::draw(sk, icon, icon_area, color, weight);
    label(
        sk,
        body,
        x + icon_side + gap,
        cy - size * 0.5 - 1.0 * s,
        size,
        color,
    );
}

fn tooltip(sk: &mut Sk, c: &Chrome<'_>, body: &str, key: &str, anchor: Rect, s: f32) {
    let size = 12.5 * s;
    let key_size = 11.0 * s;
    let tw = text::measure(body, size).0;
    let kw = if key.is_empty() {
        0.0
    } else {
        text::measure(key, key_size).0
    };
    if tw <= 0.0 {
        return;
    }
    let gap = if key.is_empty() { 0.0 } else { 9.0 * s };
    let pad = 9.0 * s;
    let bw = tw + kw + gap + pad * 2.0;
    let bh = 25.0 * s;
    let mon = c.monitor;
    let cx = anchor.center().0;
    let bx = (cx - bw * 0.5).clamp(
        mon.x0 + 4.0 * s,
        (mon.x1 - 4.0 * s - bw).max(mon.x0 + 4.0 * s),
    );
    // Clear the whole bar, not just the hovered cell: with two rows a "below"
    // tooltip anchored to a tool row would land on the attribute row.
    let bar = c.layout.bar;
    let mut by = if c.layout.tip_below {
        bar.y1 + 7.0 * s
    } else {
        bar.y0 - 7.0 * s - bh
    };
    by = by.clamp(
        mon.y0 + 4.0 * s,
        (mon.y1 - 4.0 * s - bh).max(mon.y0 + 4.0 * s),
    );
    let pill = Rect::from_xywh(bx, by, bw, bh);
    icons::panel(
        sk,
        pill,
        7.0 * s,
        PANEL_DEEP,
        Rgba::new(0xFF, 0xFF, 0xFF, 0x24),
    );
    label(sk, body, bx + pad, by + (bh - 15.0 * s) * 0.5, size, TEXT);
    if !key.is_empty() {
        label(
            sk,
            key,
            bx + pad + tw + gap,
            by + (bh - 14.0 * s) * 0.5 + 1.0 * s,
            key_size,
            MUTED,
        );
    }
}

fn hud(sk: &mut Sk, c: &Chrome<'_>, body: &str, at: Pt, s: f32) {
    let size = 12.5 * s;
    let tw = text::measure(body, size).0;
    let pad = 9.0 * s;
    let bw = tw + pad * 2.0;
    let bh = 26.0 * s;
    let mon = c.monitor;
    let bx = (at.x - bw * 0.5).clamp(
        mon.x0 + 4.0 * s,
        (mon.x1 - 4.0 * s - bw).max(mon.x0 + 4.0 * s),
    );
    let mut by = at.y - bh - 18.0 * s;
    if by < mon.y0 + 4.0 * s {
        by = at.y + 20.0 * s;
    }
    by = by.clamp(
        mon.y0 + 4.0 * s,
        (mon.y1 - 4.0 * s - bh).max(mon.y0 + 4.0 * s),
    );
    let pill = Rect::from_xywh(bx, by, bw, bh);
    icons::panel(
        sk,
        pill,
        8.0 * s,
        PANEL_DEEP,
        Rgba::new(0x1E, 0x9C, 0x8B, 0x88),
    );
    label(sk, body, bx + pad, by + (bh - 15.0 * s) * 0.5, size, TEXT);
}

fn toast(sk: &mut Sk, c: &Chrome<'_>, body: &str, s: f32) {
    let size = 13.0 * s;
    let tw = text::measure(body, size).0;
    let icon_side = 16.0 * s;
    let gap = 7.0 * s;
    let pad = 12.0 * s;
    let mon = c.monitor;
    let max_w = (mon.width() - 48.0 * s).max(160.0 * s);
    let bw = (tw + icon_side + gap + pad * 2.0).min(max_w);
    let bh = 34.0 * s;
    let bx = ((mon.x0 + mon.x1) * 0.5 - bw * 0.5).clamp(
        mon.x0 + 12.0 * s,
        (mon.x1 - 12.0 * s - bw).max(mon.x0 + 12.0 * s),
    );
    // Top of the monitor: the toolbar lives at the selection's bottom edge, so
    // a bottom toast would fight it for the same strip of pixels.
    let by = mon.y0 + 38.0 * s;
    let pill = Rect::from_xywh(bx, by, bw, bh);
    let border = if c.toast_error {
        Rgba::new(0xE5, 0x48, 0x4D, 0x99)
    } else {
        Rgba::new(0x1E, 0x9C, 0x8B, 0x99)
    };
    icons::panel(sk, pill, 10.0 * s, PANEL_DEEP, border);
    let (ix, iy) = (bx + pad, by + (bh - icon_side) * 0.5);
    if c.toast_error {
        let (cx, cy) = (ix + icon_side * 0.5, iy + icon_side * 0.5);
        sk.ring(cx, cy, icon_side * 0.44, 1.5 * s, DANGER);
        sk.line(
            (cx - icon_side * 0.18, cy),
            (cx + icon_side * 0.18, cy),
            1.6 * s,
            DANGER,
        );
    } else {
        icons::draw(
            sk,
            Icon::Check,
            Rect::from_xywh(ix, iy, icon_side, icon_side),
            ACCENT,
            1.9,
        );
    }
    label(
        sk,
        body,
        ix + icon_side + gap,
        by + (bh - 15.0 * s) * 0.5,
        size,
        TEXT,
    );
}

fn initial_hint(sk: &mut Sk, c: &Chrome<'_>, s: f32) {
    let body = "拖动框选 · Esc 退出";
    let size = 12.5 * s;
    let tw = text::measure(body, size).0;
    if tw <= 0.0 {
        return;
    }
    let pad = 12.0 * s;
    let bw = tw + pad * 2.0;
    let bh = 28.0 * s;
    let mon = c.monitor;
    let bx = ((mon.x0 + mon.x1) * 0.5 - bw * 0.5).clamp(
        mon.x0 + 12.0 * s,
        (mon.x1 - 12.0 * s - bw).max(mon.x0 + 12.0 * s),
    );
    let by = mon.y1 - bh - 44.0 * s;
    let a = (c.hint_alpha.clamp(0.0, 1.0) * 255.0) as u8;
    let pill = Rect::from_xywh(bx, by, bw, bh);
    icons::panel(
        sk,
        pill,
        9.0 * s,
        Rgba::new(0x0E, 0x12, 0x16, (0xC8i32 * a as i32 / 255) as u8),
        Rgba::new(0xFF, 0xFF, 0xFF, (0x1E_i32 * a as i32 / 255) as u8),
    );
    label(
        sk,
        body,
        bx + pad,
        by + (bh - 15.0 * s) * 0.5,
        size,
        MUTED.alpha(a),
    );
}

/// Repaint the four bands outside `sel` from the dimmed copy, which is how an
/// annotation that spills out of the selection gets clipped.
pub fn clip_outside(dst: &mut [u8], stride: usize, dim: &[u8], sel: Rect, w: u32, h: u32) {
    let (x0, y0, x1, y1) = match sel.to_px(w, h) {
        Some(v) => v,
        None => return,
    };
    copy_rect(dst, stride, dim, stride, 0, 0, w as i32, y0);
    copy_rect(dst, stride, dim, stride, 0, y1, w as i32, h as i32);
    copy_rect(dst, stride, dim, stride, 0, y0, x0, y1);
    copy_rect(dst, stride, dim, stride, x1, y0, w as i32, y1);
}

/// Spotlight: dim everything inside the selection except the focus rectangle.
pub fn spotlight(dst: &mut [u8], stride: usize, focus: Rect, sel: Rect, w: u32, h: u32) {
    let Some(f) = focus.intersect(&sel) else {
        return;
    };
    let Some((x0, y0, x1, y1)) = sel.to_px(w, h) else {
        return;
    };
    let (fx0, fy0, fx1, fy1) = (f.x0 as i32, f.y0 as i32, f.x1 as i32, f.y1 as i32);
    tint_rect(dst, stride, x0, y0, x1, fy0, [0, 0, 0], 150);
    tint_rect(dst, stride, x0, fy1, x1, y1, [0, 0, 0], 150);
    tint_rect(dst, stride, x0, fy0, fx0, fy1, [0, 0, 0], 150);
    tint_rect(dst, stride, fx1, fy0, x1, fy1, [0, 0, 0], 150);
}

/// Fade the four corners of the selection back into the dimmed surround, so
/// the on-screen hole matches the rounded export. `radius` is in physical px.
pub fn round_selection(
    dst: &mut [u8],
    stride: usize,
    dim: &[u8],
    sel: Rect,
    radius: f32,
    w: u32,
    h: u32,
) {
    let Some((x0, y0, x1, y1)) = sel.to_px(w, h) else {
        return;
    };
    let r = radius
        .min(sel.width() * 0.5)
        .min(sel.height() * 0.5)
        .max(0.0);
    let ri = r.ceil() as i32;
    if ri < 1 {
        return;
    }
    let mut blend = |px: i32, py: i32, cov: f32| {
        if cov >= 0.999 {
            return;
        }
        let off = py as usize * stride + px as usize * 4;
        if off + 3 >= dst.len() || off + 3 >= dim.len() {
            return;
        }
        let t = (1.0 - cov).clamp(0.0, 1.0);
        for c in 0..3 {
            let base = dst[off + c] as f32;
            let back = dim[off + c] as f32;
            dst[off + c] = (base + (back - base) * t).round() as u8;
        }
        dst[off + 3] = 255;
    };
    // Corner centres sit `r` inside each corner; coverage is the signed
    // distance to the arc, 0.5 px anti-aliasing band.
    for yy in 0..ri {
        for xx in 0..ri {
            let dx = (r - (xx as f32 + 0.5)).abs();
            let dy = (r - (yy as f32 + 0.5)).abs();
            let dist = (dx * dx + dy * dy).sqrt();
            let cov = (r - dist + 0.5).clamp(0.0, 1.0);
            blend(x0 + xx, y0 + yy, cov);
            blend(x1 - 1 - xx, y0 + yy, cov);
            blend(x0 + xx, y1 - 1 - yy, cov);
            blend(x1 - 1 - xx, y1 - 1 - yy, cov);
        }
    }
}

//! Small painting helpers.
//!
//! Everything the island draws is a rounded rectangle, a bar, a ring or a
//! line glyph. egui's stock widgets bring a theme, padding and a minimum size
//! that do not fit a 34 pt capsule, so the shapes are painted directly and the
//! interaction is handled by the caller with explicit rectangles.

use egui::{Color32, FontId, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2};

use super::theme::Palette;

pub fn round_rect(
    ui: &Ui,
    rect: Rect,
    radius: f32,
    fill: Color32,
    stroke: Stroke,
) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    ui.painter()
        .add(Shape::rect_filled(rect, radius, fill));
    if stroke.width > 0.0 && stroke.color.a() > 0 {
    ui.painter().add(Shape::rect_stroke(rect, radius, stroke));
    }
}

/// A soft drop shadow under a rounded rectangle: four expanding outlines with
/// decreasing alpha. Cheaper and steadier than a blur texture, and it respects
/// the window's per-pixel alpha.
pub fn shadow(ui: &Ui, rect: Rect, radius: f32, color: Color32) {
    let base = color.a() as f32;
    for i in (1..=4).rev() {
        let grow = i as f32 * 2.2;
        let a = (base / (i as f32 * 2.6)) as u8;
        let c = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a);
        let r = rect.expand(grow);
        ui.painter()
            .add(Shape::rect_stroke(r, radius + grow, Stroke::new(2.0_f32, c)));
    }
}

/// A rounded rectangle: epaint fills it, and the border is stroked along a path
/// with a real arc at each corner (20 segments per corner).
///
/// The fill deliberately uses [`Shape::rect_filled`]. It used to be
/// `Shape::convex_polygon(round_path(..))`, on the theory that a hand-sampled
/// path would beat epaint's segment count on the card's own edge — that was
/// wrong twice over. epaint fills a path as a **triangle fan from the first
/// point**, and on a pill (where the two corner centres of a side coincide, so
/// half the path is a half-circle) the fan does not reproduce the outline: the
/// corners came out square. On the capsule that was a flat right end clipped by
/// the window edge; on the card it was the two bottom corners turning into pale
/// blocks. Fills are the one thing epaint already does correctly.
///
/// The stroke stays on the sampled path because `Shape::rect_stroke` centres
/// the line on the outline, and half of it lands outside the window — a 1 px
/// edge on a 14 pt radius reads as a shaved corner.
pub fn round_rect_smooth(ui: &Ui, rect: Rect, radius: f32, fill: Color32, stroke: Stroke) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let pts = round_path(rect, radius);
    if fill.a() > 0 {
        ui.painter()
            .add(Shape::rect_filled(rect, radius, fill));
    }
    if stroke.width > 0.0 && stroke.color.a() > 0 {
        ui.painter().add(Shape::closed_line(pts, stroke));
    }
}

/// Clockwise outline of a rounded rectangle, starting at the top-left corner's
/// top tangent. 20 points per corner: at a 20 px radius the chord error is
/// under 0.05 px, which is below one subpixel of the anti-aliasing.
pub fn round_path(rect: Rect, radius: f32) -> Vec<Pos2> {
    const SEGMENTS: usize = 20;
    let r = radius.max(0.0).min(rect.width() * 0.5).min(rect.height() * 0.5);
    let mut pts = Vec::with_capacity(SEGMENTS * 4 + 4);
    if r <= 0.5 {
        return vec![
            rect.left_top(),
            rect.right_top(),
            rect.right_bottom(),
            rect.left_bottom(),
        ];
    }
    // (centre, start angle, end angle) per corner; angles run clockwise in
    // screen space, where +y points down.
    let corners = [
        (
            Pos2::new(rect.left() + r, rect.top() + r),
            std::f32::consts::PI,
            std::f32::consts::PI * 1.5,
        ),
        (
            Pos2::new(rect.right() - r, rect.top() + r),
            std::f32::consts::PI * 1.5,
            std::f32::consts::TAU,
        ),
        (
            Pos2::new(rect.right() - r, rect.bottom() - r),
            0.0,
            std::f32::consts::FRAC_PI_2,
        ),
        (
            Pos2::new(rect.left() + r, rect.bottom() - r),
            std::f32::consts::FRAC_PI_2,
            std::f32::consts::PI,
        ),
    ];
    for (c, a0, a1) in corners {
        for i in 0..=SEGMENTS {
            let t = i as f32 / SEGMENTS as f32;
            let a = a0 + (a1 - a0) * t;
            pts.push(Pos2::new(c.x + r * a.cos(), c.y + r * a.sin()));
        }
    }
    pts
}

pub fn bar(ui: &Ui, rect: Rect, frac: f32, color: Color32, track: Color32) {
    let radius = rect.height() * 0.5;
    round_rect(ui, rect, radius, track, Stroke::NONE);
    let w = (rect.width() * frac.clamp(0.0, 1.0)).max(0.0);
    if w > 1.0 {
        let filled = Rect::from_min_size(rect.min, Vec2::new(w, rect.height()));
        round_rect(ui, filled, radius, color, Stroke::NONE);
    }
}

pub fn ring(
    ui: &Ui,
    center: Pos2,
    radius: f32,
    width: f32,
    frac: f32,
    color: Color32,
    track: Color32,
) {
    ui.painter()
        .circle_stroke(center, radius, Stroke::new(width, track));
    let frac = frac.clamp(0.0, 1.0);
    if frac <= 0.004 {
        return;
    }
    // Start at 12 o'clock and run clockwise.
    let steps = ((frac * 64.0).ceil() as usize).max(2);
    let mut pts = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let t = frac * (i as f32 / steps as f32);
        let angle = -std::f32::consts::FRAC_PI_2 + t * std::f32::consts::TAU;
        pts.push(center + Vec2::new(angle.cos(), angle.sin()) * radius);
    }
    ui.painter()
        .add(Shape::line(pts, Stroke::new(width, color)));
}

/// Text with a stable baseline. Returns the painted rectangle so callers can
/// chain a second label without measuring again.
pub fn text(
    ui: &Ui,
    pos: Pos2,
    align: egui::Align2,
    text: &str,
    size: f32,
    color: Color32,
) -> Rect {
    ui.painter().text(
        pos,
        align,
        text,
        FontId::proportional(size),
        color,
    )
}

/// Tabular digits: same size, monospace family, so a ticking clock does not
/// shift by a pixel every second.
pub fn mono_text(
    ui: &Ui,
    pos: Pos2,
    align: egui::Align2,
    text: &str,
    size: f32,
    color: Color32,
) -> Rect {
    ui.painter().text(
        pos,
        align,
        text,
        FontId::monospace(size),
        color,
    )
}

/// A hit area with a hover highlight, painted by `draw`.
pub fn hit(
    ui: &mut Ui,
    rect: Rect,
    draw: impl FnOnce(&Ui, bool),
) -> Response {
    let resp = ui.allocate_rect(rect, Sense::click());
    draw(ui, resp.hovered());
    resp
}

// ---------------------------------------------------------------------------
// Line glyphs — 16 px grid, same weight as the settings centre icons
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub enum Glyph {
    Close,
    Pin,
    Plus,
    Trash,
    Check,
    Gear,
    Clock,
    Clipboard,
    Inbox,
    Gauge,
    Bell,
    Copy,
    /// Document / export-to-file outline.
    Doc,
    /// Round chat bubble (QQ).
    Bubble,
    /// Two overlapping bubbles — WeChat's own mark, and the only silhouette
    /// that stays distinguishable from the single bubble at 12 pt.
    BubbleTwo,
    /// Briefcase — the enterprise-messaging channel.
    Briefcase,
    /// Envelope.
    Mail,
    /// Heartbeat / activity trace — the "live" push-card tab.
    Pulse,
    /// Transport keys for the now-playing row.
    Prev,
    Play,
    Pause,
    Next,
    /// Arrow into a tray — "收进盒子".
    Collect,
    /// Magnifier — the search tab.
    Search,
}

pub fn glyph(ui: &Ui, center: Pos2, size: f32, which: Glyph, color: Color32) {
    draw_glyph(ui, center, size, which, color, 1.5, None);
}

/// Same glyph, explicit stroke width. A 9 pt icon drawn with the default 1.5 pt
/// stem has more ink than the 10 pt text beside it and reads as bold; the title
/// row's close button therefore asks for a thinner stem than the panel's icon
/// buttons, which are larger.
pub fn glyph_weighted(
    ui: &Ui,
    center: Pos2,
    size: f32,
    which: Glyph,
    color: Color32,
    width: f32,
) {
    draw_glyph(ui, center, size, which, color, width, None);
}

/// A glyph that overlaps itself, with the background passed in so the front
/// shape can be filled and occlude the back one. Only the two-bubble mark needs
/// this: a stroke-only overlap reads as a bubble with a hook attached.
pub fn glyph_on(
    ui: &Ui,
    center: Pos2,
    size: f32,
    which: Glyph,
    color: Color32,
    width: f32,
    bg: Color32,
) {
    draw_glyph(ui, center, size, which, color, width, Some(bg));
}

fn draw_glyph(
    ui: &Ui,
    center: Pos2,
    size: f32,
    which: Glyph,
    color: Color32,
    width: f32,
    bg: Option<Color32>,
) {
    let p = ui.painter();
    let s = size * 0.5;
    let stroke = Stroke::new(width, color);
    let line = |a: Pos2, b: Pos2| {
        p.add(Shape::line_segment([a, b], stroke));
    };
    let c = center;
    match which {
        Glyph::Close => {
            line(c + Vec2::new(-s, -s), c + Vec2::new(s, s));
            line(c + Vec2::new(s, -s), c + Vec2::new(-s, s));
        }
        Glyph::Plus => {
            line(c + Vec2::new(-s, 0.0), c + Vec2::new(s, 0.0));
            line(c + Vec2::new(0.0, -s), c + Vec2::new(0.0, s));
        }
        Glyph::Check => {
            line(c + Vec2::new(-s, 0.0), c + Vec2::new(-s * 0.2, s * 0.75));
            line(c + Vec2::new(-s * 0.2, s * 0.75), c + Vec2::new(s, -s * 0.7));
        }
        Glyph::Pin => {
            line(c + Vec2::new(0.0, -s), c + Vec2::new(0.0, s * 0.9));
            p.add(Shape::circle_stroke(
                c + Vec2::new(0.0, -s * 0.25),
                s * 0.55,
                stroke,
            ));
            line(
                c + Vec2::new(-s * 0.55, -s * 0.25),
                c + Vec2::new(s * 0.55, -s * 0.25),
            );
        }
        Glyph::Trash => {
            line(c + Vec2::new(-s, -s * 0.55), c + Vec2::new(s, -s * 0.55));
            line(c + Vec2::new(-s * 0.45, -s * 0.9), c + Vec2::new(s * 0.45, -s * 0.9));
            p.add(Shape::rect_stroke(
                Rect::from_center_size(c + Vec2::new(0.0, s * 0.2), Vec2::new(s * 1.3, s * 1.4)),
                2.0,
                stroke,
            ));
        }
        Glyph::Gear => {
            p.add(Shape::circle_stroke(c, s * 0.52, stroke));
            for i in 0..6 {
                let a = i as f32 * std::f32::consts::TAU / 6.0;
                let d = Vec2::new(a.cos(), a.sin());
                line(c + d * (s * 0.72), c + d * s);
            }
        }
        Glyph::Clock => {
            p.add(Shape::circle_stroke(c, s * 0.9, stroke));
            line(c, c + Vec2::new(0.0, -s * 0.55));
            line(c, c + Vec2::new(s * 0.45, s * 0.15));
        }
        Glyph::Clipboard => {
            p.add(Shape::rect_stroke(
                Rect::from_center_size(c + Vec2::new(0.0, s * 0.1), Vec2::new(s * 1.3, s * 1.6)),
                2.5,
                stroke,
            ));
            line(c + Vec2::new(-s * 0.3, -s * 0.72), c + Vec2::new(s * 0.3, -s * 0.72));
        }
        Glyph::Inbox => {
            line(c + Vec2::new(-s, -s * 0.2), c + Vec2::new(-s, s * 0.7));
            line(c + Vec2::new(s, -s * 0.2), c + Vec2::new(s, s * 0.7));
            line(c + Vec2::new(-s, s * 0.7), c + Vec2::new(s, s * 0.7));
            line(c + Vec2::new(-s, -s * 0.2), c + Vec2::new(-s * 0.35, s * 0.1));
            line(c + Vec2::new(s, -s * 0.2), c + Vec2::new(s * 0.35, s * 0.1));
        }
        Glyph::Gauge => {
            let steps = 12;
            let mut pts = Vec::with_capacity(steps + 1);
            for i in 0..=steps {
                let t = i as f32 / steps as f32;
                let a = std::f32::consts::PI + t * std::f32::consts::PI;
                pts.push(c + Vec2::new(a.cos(), a.sin()) * s);
            }
            p.add(Shape::line(pts, stroke));
            line(c, c + Vec2::new(s * 0.55, -s * 0.45));
        }
        Glyph::Bell => {
            line(c + Vec2::new(-s * 0.75, s * 0.45), c + Vec2::new(s * 0.75, s * 0.45));
            line(c + Vec2::new(-s * 0.6, s * 0.45), c + Vec2::new(-s * 0.6, -s * 0.1));
            line(c + Vec2::new(s * 0.6, s * 0.45), c + Vec2::new(s * 0.6, -s * 0.1));
            p.add(Shape::circle_stroke(c + Vec2::new(0.0, -s * 0.15), s * 0.6, stroke));
            line(c + Vec2::new(-s * 0.2, s * 0.7), c + Vec2::new(s * 0.2, s * 0.7));
        }
        Glyph::Doc => {
            // A sheet with a folded corner and three text lines.
            line(c + Vec2::new(-s * 0.75, -s), c + Vec2::new(s * 0.35, -s));
            line(c + Vec2::new(s * 0.35, -s), c + Vec2::new(s * 0.75, -s * 0.6));
            line(c + Vec2::new(s * 0.75, -s * 0.6), c + Vec2::new(s * 0.75, s));
            line(c + Vec2::new(s * 0.75, s), c + Vec2::new(-s * 0.75, s));
            line(c + Vec2::new(-s * 0.75, s), c + Vec2::new(-s * 0.75, -s));
            line(c + Vec2::new(s * 0.35, -s), c + Vec2::new(s * 0.35, -s * 0.6));
            line(c + Vec2::new(s * 0.35, -s * 0.6), c + Vec2::new(s * 0.75, -s * 0.6));
            line(c + Vec2::new(-s * 0.4, s * 0.1), c + Vec2::new(s * 0.4, s * 0.1));
            line(c + Vec2::new(-s * 0.4, s * 0.5), c + Vec2::new(s * 0.4, s * 0.5));
        }
        Glyph::Copy => {
            p.add(Shape::rect_stroke(
                Rect::from_center_size(c + Vec2::new(s * 0.2, s * 0.2), Vec2::new(s * 1.1, s * 1.1)),
                2.0,
                stroke,
            ));
            p.add(Shape::rect_stroke(
                Rect::from_center_size(c + Vec2::new(-s * 0.25, -s * 0.25), Vec2::new(s * 1.1, s * 1.1)),
                2.0,
                stroke,
            ));
        }
        // -- channel marks --------------------------------------------------
        //
        // Four 12 pt marks have to be told apart at a glance, and detail does
        // not survive at 12 pt — only silhouette does. So each channel gets a
        // different outline: round bubble, double bubble, squared bubble,
        // envelope. Nothing is drawn inside them.
        Glyph::Bubble => {
            p.add(Shape::circle_stroke(
                c + Vec2::new(0.0, -s * 0.12),
                s * 0.76,
                stroke,
            ));
            line(c + Vec2::new(-s * 0.4, s * 0.5), c + Vec2::new(-s * 0.57, s * 0.94));
            line(c + Vec2::new(-s * 0.57, s * 0.94), c + Vec2::new(-s * 0.02, s * 0.6));
        }
        Glyph::BubbleTwo => {
            // Drawn back-to-front with a real occlusion: the back bubble is a
            // whole outline, the front bubble is filled with the surface colour
            // before it is stroked. The previous version drew the back one as a
            // bare arc, and an arc that stops in mid-air is what read as "a
            // bubble with a hook".
            let back = Rect::from_center_size(
                c + Vec2::new(s * 0.38, -s * 0.36),
                Vec2::new(s * 1.04, s * 0.78),
            );
            p.add(Shape::rect_stroke(back, s * 0.34, stroke));
            let front = Rect::from_center_size(
                c + Vec2::new(-s * 0.16, s * 0.18),
                Vec2::new(s * 1.38, s * 0.98),
            );
            if let Some(bg) = bg {
                p.add(Shape::rect_filled(front, s * 0.46, bg));
            }
            p.add(Shape::rect_stroke(front, s * 0.46, stroke));
            line(c + Vec2::new(-s * 0.5, s * 0.63), c + Vec2::new(-s * 0.62, s * 0.92));
            line(c + Vec2::new(-s * 0.62, s * 0.92), c + Vec2::new(-s * 0.22, s * 0.66));
        }
        Glyph::Briefcase => {
            let body = Rect::from_center_size(
                c + Vec2::new(0.0, s * 0.18),
                Vec2::new(s * 1.56, s * 1.1),
            );
            p.add(Shape::rect_stroke(body, s * 0.26, stroke));
            line(c + Vec2::new(-s * 0.32, -s * 0.37), c + Vec2::new(-s * 0.32, s * 0.05));
            line(c + Vec2::new(s * 0.32, -s * 0.37), c + Vec2::new(s * 0.32, s * 0.05));
            line(c + Vec2::new(-s * 0.32, -s * 0.37), c + Vec2::new(s * 0.32, -s * 0.37));
            // A single seam across the body: without it the briefcase is a
            // rounded rectangle, and the handle alone does not carry.
            line(c + Vec2::new(-s * 0.78, s * 0.05), c + Vec2::new(s * 0.78, s * 0.05));
        }
        Glyph::Mail => {
            let body = Rect::from_center_size(c, Vec2::new(s * 1.66, s * 1.24));
            p.add(Shape::rect_stroke(body, s * 0.2, stroke));
            let fold = Pos2::new(c.x, c.y + s * 0.3);
            line(Pos2::new(body.left() + s * 0.14, body.top() + s * 0.14), fold);
            line(Pos2::new(body.right() - s * 0.14, body.top() + s * 0.14), fold);
        }
        Glyph::Pulse => {
            // Flat lead-in, one spike up and one down, flat tail. At 12 pt the
            // spike is the only thing that survives, and it must not look like
            // the gauge arc or a check mark, so the down-stroke goes below the
            // baseline.
            let pts = [
                c + Vec2::new(-s, 0.0),
                c + Vec2::new(-s * 0.45, 0.0),
                c + Vec2::new(-s * 0.2, -s * 0.75),
                c + Vec2::new(s * 0.15, s * 0.75),
                c + Vec2::new(s * 0.42, 0.0),
                c + Vec2::new(s, 0.0),
            ];
            p.add(Shape::line(pts.to_vec(), stroke));
        }
        // Transport keys: filled triangles, because an outlined triangle at
        // 12 pt is a sliver of ink that reads as noise next to the stroked
        // icons elsewhere in the panel.
        Glyph::Prev => {
            line(c + Vec2::new(-s * 0.62, -s * 0.62), c + Vec2::new(-s * 0.62, s * 0.62));
            p.add(Shape::convex_polygon(
                vec![
                    c + Vec2::new(s * 0.72, -s * 0.62),
                    c + Vec2::new(-s * 0.42, 0.0),
                    c + Vec2::new(s * 0.72, s * 0.62),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Glyph::Next => {
            line(c + Vec2::new(s * 0.62, -s * 0.62), c + Vec2::new(s * 0.62, s * 0.62));
            p.add(Shape::convex_polygon(
                vec![
                    c + Vec2::new(-s * 0.72, -s * 0.62),
                    c + Vec2::new(s * 0.42, 0.0),
                    c + Vec2::new(-s * 0.72, s * 0.62),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Glyph::Play => {
            p.add(Shape::convex_polygon(
                vec![
                    c + Vec2::new(-s * 0.5, -s * 0.75),
                    c + Vec2::new(s * 0.78, 0.0),
                    c + Vec2::new(-s * 0.5, s * 0.75),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Glyph::Pause => {
            for x in [-s * 0.33, s * 0.33] {
                p.add(Shape::rect_filled(
                    Rect::from_center_size(c + Vec2::new(x, 0.0), Vec2::new(s * 0.34, s * 1.5)),
                    1.0,
                    color,
                ));
            }
        }
        Glyph::Collect => {
            // Down-arrow into a tray. Deliberately not a box outline: next to
            // the clipboard tab and the doc glyph, a third rectangular
            // silhouette would read as paper again.
            line(c + Vec2::new(0.0, -s * 0.8), c + Vec2::new(0.0, s * 0.15));
            line(c + Vec2::new(-s * 0.42, -s * 0.25), c + Vec2::new(0.0, s * 0.2));
            line(c + Vec2::new(s * 0.42, -s * 0.25), c + Vec2::new(0.0, s * 0.2));
            line(c + Vec2::new(-s * 0.8, s * 0.55), c + Vec2::new(-s * 0.8, s * 0.85));
            line(c + Vec2::new(s * 0.8, s * 0.55), c + Vec2::new(s * 0.8, s * 0.85));
            line(c + Vec2::new(-s * 0.8, s * 0.85), c + Vec2::new(s * 0.8, s * 0.85));
        }
        Glyph::Search => {
            p.add(Shape::circle_stroke(
                c + Vec2::new(-s * 0.15, -s * 0.15),
                s * 0.6,
                stroke,
            ));
            line(
                c + Vec2::new(s * 0.3, s * 0.3),
                c + Vec2::new(s * 0.85, s * 0.85),
            );
        }
    }
}

/// Tiny round icon button with a hover plate.
pub fn icon_button(
    ui: &mut Ui,
    rect: Rect,
    which: Glyph,
    p: &Palette,
    danger: bool,
) -> Response {
    icon_button_sized(ui, rect, which, p, danger, 13.0, 1.5)
}

/// [`icon_button`] with an explicit glyph size and stroke width. 13 pt is
/// right for a toolbar, but too heavy for a hover affordance that appears
/// beside 12 pt row text.
#[allow(clippy::too_many_arguments)]
pub fn icon_button_sized(
    ui: &mut Ui,
    rect: Rect,
    which: Glyph,
    p: &Palette,
    danger: bool,
    size: f32,
    width: f32,
) -> Response {
    let resp = ui.allocate_rect(rect, Sense::click());
    if resp.hovered() {
        let fill = if danger {
            Color32::from_rgba_unmultiplied(p.danger.r(), p.danger.g(), p.danger.b(), 0x22)
        } else {
            p.accent_soft
        };
        round_rect(ui, rect, rect.height() * 0.5, fill, Stroke::NONE);
    }
    let color = if danger && resp.hovered() {
        p.danger
    } else if resp.hovered() {
        p.accent
    } else {
        p.text_dim
    };
    glyph_weighted(ui, rect.center(), size, which, color, width);
    resp
}

/// One-line text field sized for the panel, with the island's own colours.
pub fn field(ui: &mut Ui, rect: Rect, text: &mut String, hint: &str, p: &Palette) -> Response {
    let id = ui.id().with(("island-field", rect.left() as i32, rect.top() as i32));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let out = child.add(
        egui::TextEdit::singleline(text)
            .id(id)
            .hint_text(hint)
            .desired_width(rect.width())
            .margin(egui::Margin::symmetric(8.0, 5.0))
            .frame(false)
            .text_color(p.text),
    );
    out
}

/// Trims `text` to a single line of `max_w` at `size`, ending in an ellipsis.
///
/// Measured, not counted: a 12 pt CJK glyph is about twice as wide as a 12 pt
/// Latin one, so "40 characters" is either a wasted line or two inches of
/// overflow depending on the string. Binary search over the prefix length keeps
/// this to a handful of layout calls per row.
pub fn fit_one_line(ui: &Ui, text: &str, size: f32, max_w: f32) -> String {
    if max_w <= 0.0 {
        return String::new();
    }
    let font = FontId::proportional(size);
    let width = |s: &str| {
        ui.fonts(|f| {
            f.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE)
                .size()
                .x
        })
    };
    if width(text) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut lo = 0usize;
    let mut hi = chars.len();
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let cand: String = chars[..mid].iter().collect::<String>() + "…";
        if width(&cand) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    if lo == 0 {
        return "…".to_string();
    }
    chars[..lo].iter().collect::<String>() + "…"
}

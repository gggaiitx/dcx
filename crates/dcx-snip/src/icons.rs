//! The overlay's own icon set, drawn as vectors on a 24 × 24 grid.
//!
//! Every glyph here is new work for DCX: the semantics come from the tool
//! names, not from any third-party icon font or path set. Stroke weight is
//! passed in rather than hard-coded so the same glyph stays legible at 16 px
//! (a 1.6 pt hairline) and at 28 px (a 2 pt stem) — the "optical weight"
//! problem in miniature.

use crate::surface::{Rect, Rgba, Sk};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Hand,
    Frame,
    Vector,
    Glyph,
    Marker,
    Defocus,
    Aperture,
    Rewind,
    Forward,
    Stack,
    Slot,
    Dismiss,
    Fill,
    Curve,
    Expand,
    Check,
    Serial,
    Radius,
    Long,
}

struct Pen<'a, 'b> {
    sk: &'a mut Sk<'b>,
    ox: f32,
    oy: f32,
    k: f32,
    color: Rgba,
    weight: f32,
}

impl<'a, 'b> Pen<'a, 'b> {
    fn p(&self, x: f32, y: f32) -> (f32, f32) {
        (self.ox + x * self.k, self.oy + y * self.k)
    }

    fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        let a = self.p(x0, y0);
        let b = self.p(x1, y1);
        let w = self.weight;
        let c = self.color;
        self.sk.line(a, b, w, c);
    }

    fn poly(&mut self, pts: &[(f32, f32)], close: bool) {
        if pts.len() < 2 {
            return;
        }
        let mut pb = tiny_skia::PathBuilder::new();
        let first = self.p(pts[0].0, pts[0].1);
        pb.move_to(first.0, first.1);
        for p in &pts[1..] {
            let q = self.p(p.0, p.1);
            pb.line_to(q.0, q.1);
        }
        if close {
            pb.close();
        }
        let w = self.weight;
        let c = self.color;
        if let Some(path) = pb.finish() {
            self.sk.stroke_path(&path, w, c, false);
        }
    }

    fn fill_poly(&mut self, pts: &[(f32, f32)]) {
        if pts.len() < 3 {
            return;
        }
        let mut pb = tiny_skia::PathBuilder::new();
        let first = self.p(pts[0].0, pts[0].1);
        pb.move_to(first.0, first.1);
        for p in &pts[1..] {
            let q = self.p(p.0, p.1);
            pb.line_to(q.0, q.1);
        }
        pb.close();
        let c = self.color;
        if let Some(path) = pb.finish() {
            self.sk.fill_path(&path, c);
        }
    }

    fn circle(&mut self, cx: f32, cy: f32, r: f32, filled: bool) {
        let c = self.p(cx, cy);
        let rr = r * self.k;
        if filled {
            let color = self.color;
            self.sk.circle(c.0, c.1, rr, color);
        } else {
            let w = self.weight;
            let color = self.color;
            self.sk.ring(c.0, c.1, rr, w, color);
        }
    }

    fn round_rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, rad: f32, filled: bool) {
        let a = self.p(x0, y0);
        let b = self.p(x1, y1);
        let r = Rect::new(a.0, a.1, b.0, b.1);
        let radius = rad * self.k;
        let color = self.color;
        if filled {
            self.sk.fill_round(&r, radius, color);
        } else {
            let w = self.weight;
            self.sk.stroke_round(&r, radius, w, color);
        }
    }

    /// Arc as a polyline; 32 segments is smooth at any size we draw.
    fn arc(&mut self, cx: f32, cy: f32, r: f32, a0: f32, a1: f32) {
        let n = 32;
        let mut pts = Vec::with_capacity(n + 1);
        for i in 0..=n {
            let t = a0 + (a1 - a0) * (i as f32 / n as f32);
            pts.push((cx + r * t.cos(), cy + r * t.sin()));
        }
        self.poly(&pts, false);
    }

    fn cubic(&mut self, a: (f32, f32), c1: (f32, f32), c2: (f32, f32), b: (f32, f32)) {
        let pa = self.p(a.0, a.1);
        let p1 = self.p(c1.0, c1.1);
        let p2 = self.p(c2.0, c2.1);
        let pb = self.p(b.0, b.1);
        let mut path = tiny_skia::PathBuilder::new();
        path.move_to(pa.0, pa.1);
        path.cubic_to(p1.0, p1.1, p2.0, p2.1, pb.0, pb.1);
        let w = self.weight;
        let c = self.color;
        if let Some(path) = path.finish() {
            self.sk.stroke_path(&path, w, c, false);
        }
    }

    fn head(&mut self, tip: (f32, f32), from: (f32, f32), len: f32, spread: f32) {
        let dx = tip.0 - from.0;
        let dy = tip.1 - from.1;
        let n = (dx * dx + dy * dy).sqrt().max(0.001);
        let (ux, uy) = (dx / n, dy / n);
        let (px, py) = (-uy, ux);
        let base = (tip.0 - ux * len, tip.1 - uy * len);
        let a = (base.0 + px * spread, base.1 + py * spread);
        let b = (base.0 - px * spread, base.1 - py * spread);
        self.fill_poly(&[tip, a, b]);
    }
}

/// Draw `icon` centred in `area` (a square box, device pixels).
pub fn draw(sk: &mut Sk, icon: Icon, area: Rect, color: Rgba, weight: f32) {
    let side = area.width().min(area.height());
    // 24-grid drawn inside 20/24 of the box: the same optical padding a system
    // icon set uses, so a row of glyphs reads as one set.
    let k = side * (20.0 / 24.0) / 24.0;
    let ox = area.x0 + (area.width() - 24.0 * k) * 0.5;
    let oy = area.y0 + (area.height() - 24.0 * k) * 0.5;
    let w = (weight * k * 1.35).max(1.0);
    let mut pen = Pen {
        sk,
        ox,
        oy,
        k,
        color,
        weight: w,
    };
    match icon {
        Icon::Hand => {
            // Mouse pointer: the universal "select" sign. The four-way arrow
            // that came before read as a plus button at 20 px.
            pen.fill_poly(&[
                (6.5, 3.0),
                (17.6, 12.0),
                (12.4, 12.6),
                (15.0, 18.6),
                (12.0, 20.0),
                (9.6, 14.2),
                (6.5, 16.8),
            ]);
        }
        Icon::Frame => {
            // Four detached corner brackets around an open square: the
            // viewfinder motif, readable at 16 px because the brackets are the
            // only ink at the edges.
            for (cx, cy, sx, sy) in [
                (3.0, 3.0, 1.0, 1.0),
                (21.0, 3.0, -1.0, 1.0),
                (3.0, 21.0, 1.0, -1.0),
                (21.0, 21.0, -1.0, -1.0),
            ] {
                pen.line(cx, cy, cx + 6.0 * sx, cy);
                pen.line(cx, cy, cx, cy + 6.0 * sy);
            }
            pen.round_rect(9.0, 9.0, 15.0, 15.0, 1.4, false);
        }
        Icon::Vector => {
            pen.circle(5.5, 18.5, 1.9, false);
            pen.line(6.8, 17.2, 16.2, 7.8);
            pen.head((17.6, 6.4), (16.2, 7.8), 4.6, 2.1);
        }
        Icon::Glyph => {
            // A serif "A" as two stems plus a crossbar, over a baseline tick.
            pen.line(6.0, 18.0, 11.0, 5.5);
            pen.line(11.0, 5.5, 16.0, 18.0);
            pen.line(8.0, 14.0, 14.0, 14.0);
            pen.line(5.0, 21.0, 19.0, 21.0);
        }
        Icon::Marker => {
            pen.fill_poly(&[(9.0, 4.0), (14.0, 6.0), (9.5, 15.0), (4.5, 13.0)]);
            pen.poly(&[(9.0, 4.0), (14.0, 6.0), (9.5, 15.0), (4.5, 13.0)], true);
            pen.line(4.0, 20.5, 20.0, 20.5);
        }
        Icon::Defocus => {
            // Two concentric dotted rings with a steady core: defocus as
            // scattered points, not as a spiral that read like a "C".
            for r in [8.2f32, 5.2] {
                for k in 0..4 {
                    let a0 = k as f32 * core::f32::consts::TAU / 4.0 + 0.34;
                    let a1 = (k + 1) as f32 * core::f32::consts::TAU / 4.0 - 0.34;
                    pen.arc(12.0, 12.0, r, a0, a1);
                }
            }
            pen.circle(12.0, 12.0, 1.5, true);
        }
        Icon::Aperture => {
            for i in 0..6 {
                let a = i as f32 * core::f32::consts::TAU / 6.0;
                let (s, c) = (a.sin(), a.cos());
                pen.line(12.0 + 3.4 * c, 12.0 + 3.4 * s, 12.0 + 8.0 * c, 12.0 + 8.0 * s);
                let a2 = a + 0.9;
                pen.line(
                    12.0 + 8.0 * c,
                    12.0 + 8.0 * s,
                    12.0 + 8.0 * a2.cos(),
                    12.0 + 8.0 * a2.sin(),
                );
            }
            pen.circle(12.0, 12.0, 2.0, true);
        }
        Icon::Rewind => {
            // Hook arrow: the curve comes back to the left and the head points
            // down-left. The old 250° ring read as "refresh" at 20 px.
            pen.cubic((19.0, 15.0), (20.0, 6.5), (12.5, 3.0), (6.5, 7.5));
            pen.head((4.0, 9.8), (6.5, 7.5), 5.2, 2.7);
        }
        Icon::Forward => {
            pen.cubic((5.0, 15.0), (4.0, 6.5), (11.5, 3.0), (17.5, 7.5));
            pen.head((20.0, 9.8), (17.5, 7.5), 5.2, 2.7);
        }
        Icon::Stack => {
            pen.round_rect(4.0, 4.0, 14.0, 14.0, 2.0, false);
            pen.round_rect(9.0, 9.0, 20.0, 20.0, 2.0, true);
            pen.round_rect(9.0, 9.0, 20.0, 20.0, 2.0, false);
        }
        Icon::Slot => {
            // Tray with a downward slot: "save to disk" without the floppy.
            pen.poly(&[(4.0, 10.0), (4.0, 19.0), (20.0, 19.0), (20.0, 10.0)], false);
            pen.line(12.0, 4.0, 12.0, 13.0);
            pen.head((12.0, 16.0), (12.0, 11.5), 4.6, 2.4);
        }
        Icon::Dismiss => {
            pen.line(6.0, 6.0, 18.0, 18.0);
            pen.line(18.0, 6.0, 6.0, 18.0);
        }
        Icon::Fill => {
            // Half-filled square: clearly "fill", not the "forbidden" slash
            // the hatch pattern produced.
            pen.round_rect(4.0, 4.0, 20.0, 20.0, 2.5, false);
            pen.fill_poly(&[(6.0, 18.0), (18.0, 6.0), (18.0, 18.0)]);
        }
        Icon::Curve => {
            let mut pb = tiny_skia::PathBuilder::new();
            let a = pen.p(4.5, 17.5);
            let c1 = pen.p(9.0, 4.0);
            let c2 = pen.p(15.0, 20.0);
            let b = pen.p(19.5, 6.5);
            pb.move_to(a.0, a.1);
            pb.cubic_to(c1.0, c1.1, c2.0, c2.1, b.0, b.1);
            let w = pen.weight;
            let c = pen.color;
            if let Some(path) = pb.finish() {
                pen.sk.stroke_path(&path, w, c, false);
            }
            pen.circle(19.5, 6.5, 1.6, true);
            pen.circle(4.5, 17.5, 1.6, false);
        }
        Icon::Expand => {
            // A monitor: "whole screen" without becoming a second close X.
            pen.round_rect(3.0, 4.0, 21.0, 16.5, 2.0, false);
            pen.line(12.0, 16.5, 12.0, 20.0);
            pen.line(8.0, 20.0, 16.0, 20.0);
        }
        Icon::Check => {
            pen.poly(&[(5.0, 12.5), (10.0, 17.5), (19.0, 6.5)], false);
        }
        Icon::Serial => {
            // A numbered badge: circle plus a numeral "1".
            pen.circle(12.0, 12.0, 8.0, false);
            pen.line(12.0, 7.5, 12.0, 16.5);
            pen.line(9.8, 9.6, 12.0, 7.5);
            pen.line(9.5, 16.5, 14.5, 16.5);
        }
        Icon::Radius => {
            // A square with one rounded corner and a radius tick: clearly the
            // corner-radius control, not a generic "corner" glyph.
            pen.arc(5.0, 5.0, 7.0, 0.0, core::f32::consts::FRAC_PI_2);
            pen.line(12.0, 5.0, 19.0, 5.0);
            pen.line(19.0, 5.0, 19.0, 19.0);
            pen.line(19.0, 19.0, 5.0, 19.0);
            pen.line(5.0, 19.0, 5.0, 12.0);
            pen.line(5.0, 5.0, 8.5, 8.5);
        }
        Icon::Long => {
            // A page with a seam and a downward arrow: scrolling capture.
            pen.round_rect(7.0, 3.0, 17.0, 16.0, 2.0, false);
            pen.line(9.5, 9.0, 14.5, 9.0);
            pen.line(9.5, 12.0, 14.5, 12.0);
            pen.line(12.0, 17.0, 12.0, 21.0);
            pen.head((12.0, 21.5), (12.0, 19.0), 3.4, 1.8);
        }
    }
}

/// Three horizontal lines of increasing weight — "line width" as a scale, not
/// as a number. All three sit on the same baseline so the segmented control
/// reads as one control.
pub fn draw_width(sk: &mut Sk, index: usize, area: Rect, color: Rgba) {
    let side = area.width().min(area.height());
    let k = side * (20.0 / 24.0) / 24.0;
    let ox = area.x0 + (area.width() - 24.0 * k) * 0.5;
    let oy = area.y0 + (area.height() - 24.0 * k) * 0.5;
    let thickness = [1.4f32, 2.6, 4.2][index.min(2)];
    let mut pb = tiny_skia::PathBuilder::new();
    let y = 12.0f32;
    let (x0, y0) = (ox + 4.5 * k, oy + y * k);
    let (x1, _) = (ox + 19.5 * k, 0.0);
    pb.move_to(x0, y0);
    pb.line_to(x1, y0);
    if let Some(path) = pb.finish() {
        sk.stroke_path(&path, thickness * k * 1.35, color, false);
    }
}

/// Colour swatch: a filled disc with a hairline ring so pale colours stay
/// visible on a pale panel — and dark colours (black is in the palette now)
/// stay visible on the dark one.
pub fn draw_swatch(sk: &mut Sk, area: Rect, color: Rgba, selected: bool, ring: Rgba) {
    let (cx, cy) = area.center();
    let r = area.width().min(area.height()) * 0.5 - 2.0;
    let luma = (0.2126 * color.r as f32 + 0.7152 * color.g as f32 + 0.0722 * color.b as f32) / 255.0;
    let border = if luma < 0.12 {
        Rgba::new(0xFF, 0xFF, 0xFF, 0x55)
    } else {
        Rgba::new(0, 0, 0, 0x40)
    };
    if selected {
        sk.ring(cx, cy, r, 2.0, ring);
        sk.circle(cx, cy, r - 4.0, color);
    } else {
        sk.circle(cx, cy, r - 3.0, color);
        sk.ring(cx, cy, r - 1.5, 1.0, border);
    }
}

/// Colour wheel for the "custom colour" cell: twelve hue dots in a ring with
/// the last custom colour in the middle. Replaces a bare "+", which read as
/// "add swatch" and told nobody that a picker was behind it.
pub fn draw_color_wheel(sk: &mut Sk, area: Rect, custom: Rgba, set: bool, accent: Rgba) {
    const RING: [u8; 12 * 3] = [
        0xE5, 0x48, 0x4D, // red
        0xF7, 0x6B, 0x15, // orange
        0xF2, 0xB0, 0x2C, // amber
        0x8F, 0xB0, 0x20, // lime
        0x3E, 0xC1, 0x76, // green
        0x12, 0xA5, 0x94, // teal
        0x2B, 0x9C, 0xE9, // blue
        0x4C, 0x6F, 0xFF, // indigo
        0x8E, 0x4E, 0xC6, // violet
        0xC7, 0x4A, 0xA8, // magenta
        0xF3, 0xF5, 0xF7, // white
        0x9A, 0xA7, 0xB1, // grey
    ];
    let (cx, cy) = area.center();
    let r = area.width().min(area.height()) * 0.5 - 1.0;
    let dot = (r * 0.23).max(1.3);
    let ring_r = r - dot - 0.5;
    for i in 0..12 {
        let a = i as f32 / 12.0 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let c = Rgba::from_rgb([RING[i * 3], RING[i * 3 + 1], RING[i * 3 + 2]]);
        sk.circle(cx + a.cos() * ring_r, cy + a.sin() * ring_r, dot, c);
    }
    let well = (r - dot * 2.2).max(1.2);
    if set {
        sk.circle(cx, cy, well, custom);
        sk.ring(cx, cy, well, 1.0, Rgba::new(0, 0, 0, 0x50));
    } else {
        sk.ring(cx, cy, well, 1.3, accent);
    }
}

/// A rounded panel with a hairline border — the one surface every floating
/// piece of chrome is built from.
pub fn panel(sk: &mut Sk, rect: Rect, radius: f32, fill: Rgba, border: Rgba) {
    sk.fill_round(&rect, radius, fill);
    if border.a > 0 {
        sk.stroke_round(&rect, radius, 1.0, border);
    }
}

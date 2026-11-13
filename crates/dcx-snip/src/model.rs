//! Annotation objects, their geometry and the undo stack.
//!
//! Everything is stored in **physical pixels of the virtual screen**, the same
//! space the frozen frame was captured in. No UI scaling ever touches these
//! values: the exported image must not change when the overlay is drawn at
//! 200 % instead of 100 %.

use crate::surface::{Rect, Rgba};

pub const MAX_HISTORY: usize = 100;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

impl Pt {
    pub const fn new(x: f32, y: f32) -> Self {
        Pt { x, y }
    }
    pub fn dist(&self, o: &Pt) -> f32 {
        let dx = self.x - o.x;
        let dy = self.y - o.y;
        (dx * dx + dy * dy).sqrt()
    }
    pub fn lerp(&self, o: &Pt, t: f32) -> Pt {
        Pt::new(
            self.x + (o.x - self.x) * t,
            self.y + (o.y - self.y) * t,
        )
    }
}

/// Per-object text appearance. Lives on the object (not on the session) so a
/// document with mixed fonts, weights and shadows survives undo/redo and
/// exports exactly as it was composed.
///
/// The face is a GDI family name — the same string `CreateFontW` receives, so
/// `dcx-host`'s settings page and the toolbar popup can write/read one value.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    pub face: String,
    pub bold: bool,
    pub italic: bool,
    /// Hard drop shadow, offset by ~7 % of the glyph size.
    pub shadow: bool,
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            face: dcx_win32::font_catalog::DEFAULT_FACE.to_string(),
            bold: false,
            italic: false,
            shadow: false,
        }
    }
}

impl TextStyle {
    /// Font used by the overlay's own chrome. Deliberately **not** the
    /// annotation default: changing the annotation font must not restyle
    /// tooltips, toasts or the size readout.
    pub fn ui() -> Self {
        TextStyle::default()
    }

    /// Same face/weight, shadow suppressed. The toolbar's font box previews
    /// the family and the weight; forcing a shadow there would be noise.
    pub fn without_shadow(&self) -> TextStyle {
        TextStyle {
            shadow: false,
            ..self.clone()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// Selection / move. The only mode in which annotations can be picked up,
    /// moved, resized or edited — a drawing tool never lets an old annotation
    /// steal the drag that was meant to draw a new one.
    Hand = 0,
    Rect = 1,
    Ellipse = 2,
    Arrow = 3,
    /// Cubic connector: an arrow without the head, same two draggable
    /// controls, so its curvature stays editable after the stroke.
    Connector = 4,
    Text = 5,
    Marker = 6,
    Blur = 7,
    Spot = 8,
    /// Numbered step badge (1, 2, 3 …).
    Serial = 9,
}

impl Tool {
    pub const ALL: [Tool; 10] = [
        Tool::Hand,
        Tool::Rect,
        Tool::Ellipse,
        Tool::Arrow,
        Tool::Connector,
        Tool::Text,
        Tool::Marker,
        Tool::Blur,
        Tool::Spot,
        Tool::Serial,
    ];

    pub fn from_index(i: usize) -> Tool {
        Tool::ALL[i.min(Tool::ALL.len() - 1)]
    }

    pub fn is_drawing(self) -> bool {
        self != Tool::Hand
    }

    /// Tool that owns the attribute row for this object kind.
    pub fn of_kind(k: &Kind) -> Tool {
        match k {
            Kind::Rect { .. } => Tool::Rect,
            Kind::Ellipse { .. } => Tool::Ellipse,
            Kind::Arrow { .. } => Tool::Arrow,
            Kind::Connector { .. } => Tool::Connector,
            Kind::Text { .. } => Tool::Text,
            Kind::Marker { .. } => Tool::Marker,
            Kind::Blur { .. } => Tool::Blur,
            Kind::Spot { .. } => Tool::Spot,
            Kind::Serial { .. } => Tool::Serial,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Rect {
        r: Rect,
        radius: f32,
        filled: bool,
    },
    /// Inscribed in `r`, so a drag draws an ellipse and a square drag a circle.
    Ellipse {
        r: Rect,
        filled: bool,
    },
    /// Cubic Bézier arrow. A straight arrow simply keeps both controls on the
    /// chord (at the thirds); the two are separately draggable, which is what
    /// makes the bow amount and side freely adjustable.
    Arrow {
        a: Pt,
        c1: Pt,
        c2: Pt,
        b: Pt,
    },
    /// The same cubic without the head: a connective line whose bend is still
    /// re-editable through its two control grips.
    Connector {
        a: Pt,
        c1: Pt,
        c2: Pt,
        b: Pt,
    },
    Text {
        at: Pt,
        text: String,
        size: f32,
        style: TextStyle,
    },
    /// Freehand highlighter: a polyline drawn with a wide, semi-transparent
    /// round-capped stroke.
    Marker {
        pts: Vec<Pt>,
    },
    Blur {
        r: Rect,
    },
    Spot {
        r: Rect,
    },
    /// A filled circular badge with a sequence number in the middle.
    Serial {
        at: Pt,
        n: u32,
        size: f32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Obj {
    pub kind: Kind,
    pub color: Rgba,
    pub width: f32,
}

impl Obj {
    pub fn new(kind: Kind, color: Rgba, width: f32) -> Self {
        Obj { kind, color, width }
    }

    /// Bounding box, padded by half the stroke so handles enclose the ink.
    pub fn bounds(&self) -> Rect {
        let pad = match &self.kind {
            Kind::Marker { .. } => self.width * 0.5,
            Kind::Rect { .. }
            | Kind::Ellipse { .. }
            | Kind::Arrow { .. }
            | Kind::Connector { .. } => self.width * 0.5,
            Kind::Text { .. } | Kind::Blur { .. } | Kind::Spot { .. } | Kind::Serial { .. } => 0.0,
        };
        let raw = match &self.kind {
            Kind::Rect { r, .. }
            | Kind::Ellipse { r, .. }
            | Kind::Blur { r }
            | Kind::Spot { r } => *r,
            Kind::Arrow { a, c1, c2, b } | Kind::Connector { a, c1, c2, b } => {
                // The curve lies inside the control hull, so this is tight for
                // a straight arrow (controls on the chord) and exact for a bow.
                Rect::new(
                    a.x.min(b.x).min(c1.x).min(c2.x),
                    a.y.min(b.y).min(c1.y).min(c2.y),
                    a.x.max(b.x).max(c1.x).max(c2.x),
                    a.y.max(b.y).max(c1.y).max(c2.y),
                )
            }
            Kind::Text { at, text, size, style } => {
                // Bold and italic change the advance width, so the box must be
                // measured with the object's own style, not the default face.
                let (w, h) = crate::text::measure_styled(text, *size, style);
                Rect::from_xywh(at.x, at.y, w, h)
            }
            Kind::Serial { at, size, .. } => {
                Rect::from_xywh(at.x - size * 0.5, at.y - size * 0.5, *size, *size)
            }
            Kind::Marker { pts } => {
                let mut r = Rect::new(pts[0].x, pts[0].y, pts[0].x, pts[0].y);
                for p in pts {
                    r.x0 = r.x0.min(p.x);
                    r.y0 = r.y0.min(p.y);
                    r.x1 = r.x1.max(p.x);
                    r.y1 = r.y1.max(p.y);
                }
                r
            }
        };
        raw.inflate(pad)
    }

    pub fn hit(&self, p: Pt, tol: f32) -> bool {
        match &self.kind {
            Kind::Rect { r, filled, .. } => {
                if *filled {
                    r.inflate(tol).contains(p.x, p.y)
                } else {
                    let outer = r.inflate(tol + self.width * 0.5);
                    let inner = r.inflate(-(tol + self.width * 0.5));
                    outer.contains(p.x, p.y) && !inner.contains(p.x, p.y)
                }
            }
            Kind::Ellipse { r, filled } => {
                let (cx, cy) = r.center();
                let (rx, ry) = (r.width() * 0.5, r.height() * 0.5);
                if rx < 0.01 || ry < 0.01 {
                    return r.inflate(tol).contains(p.x, p.y);
                }
                let nx = (p.x - cx) / rx;
                let ny = (p.y - cy) / ry;
                let d = (nx * nx + ny * ny).sqrt();
                // Distance to the rim in pixels, via the smaller semi-axis.
                let band = (tol + self.width * 0.5) / rx.min(ry);
                if *filled {
                    d <= 1.0 + band
                } else {
                    (d - 1.0).abs() <= band
                }
            }
            Kind::Blur { r } | Kind::Spot { r } => r.contains(p.x, p.y),
            Kind::Text { .. } => self.bounds().contains(p.x, p.y),
            Kind::Serial { at, size, .. } => at.dist(&p) <= size * 0.5 + tol,
            Kind::Arrow { a, c1, c2, b } | Kind::Connector { a, c1, c2, b } => {
                let n = 24;
                let mut prev = *a;
                for i in 1..=n {
                    let t = i as f32 / n as f32;
                    let q = cubic(*a, *c1, *c2, *b, t);
                    if dist_to_seg(p, prev, q) <= tol + self.width * 0.5 {
                        return true;
                    }
                    prev = q;
                }
                false
            }
            Kind::Marker { pts } => {
                if pts.len() == 1 {
                    return pts[0].dist(&p) <= tol + self.width * 0.5;
                }
                for w in pts.windows(2) {
                    if dist_to_seg(p, w[0], w[1]) <= tol + self.width * 0.5 {
                        return true;
                    }
                }
                false
            }
        }
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        match &mut self.kind {
            Kind::Rect { r, .. }
            | Kind::Ellipse { r, .. }
            | Kind::Blur { r }
            | Kind::Spot { r } => {
                r.x0 += dx;
                r.x1 += dx;
                r.y0 += dy;
                r.y1 += dy;
            }
            Kind::Arrow { a, c1, c2, b, .. } | Kind::Connector { a, c1, c2, b, .. } => {
                for p in [a, c1, c2, b] {
                    p.x += dx;
                    p.y += dy;
                }
            }
            Kind::Text { at, .. } | Kind::Serial { at, .. } => {
                at.x += dx;
                at.y += dy;
            }
            Kind::Marker { pts } => {
                for p in pts {
                    p.x += dx;
                    p.y += dy;
                }
            }
        }
    }

    /// Remap this object from `old` bounds into `new` bounds. Used by both the
    /// object handles and (for the selection-independent parts) by nothing
    /// else — the selection rect has its own path.
    pub fn remap(&mut self, old: Rect, new: Rect) {
        let sx = if old.width().abs() < 0.01 {
            1.0
        } else {
            new.width() / old.width()
        };
        let sy = if old.height().abs() < 0.01 {
            1.0
        } else {
            new.height() / old.height()
        };
        let map = |p: &mut Pt| {
            p.x = new.x0 + (p.x - old.x0) * sx;
            p.y = new.y0 + (p.y - old.y0) * sy;
        };
        match &mut self.kind {
            Kind::Rect { r, .. }
            | Kind::Ellipse { r, .. }
            | Kind::Blur { r }
            | Kind::Spot { r } => {
                let mut a = Pt::new(r.x0, r.y0);
                let mut b = Pt::new(r.x1, r.y1);
                map(&mut a);
                map(&mut b);
                *r = Rect::new(a.x, a.y, b.x, b.y);
            }
            Kind::Arrow { a, c1, c2, b, .. } | Kind::Connector { a, c1, c2, b, .. } => {
                map(a);
                map(c1);
                map(c2);
                map(b);
            }
            Kind::Text { at, size, .. } => {
                let mut p = *at;
                map(&mut p);
                *at = p;
                // Text scales with the box on the vertical axis only; changing
                // the glyph size on a horizontal drag would reflow the string.
                *size = (*size * sy).clamp(6.0, 400.0);
            }
            Kind::Marker { pts } => {
                for p in pts {
                    map(p);
                }
            }
            Kind::Serial { at, size, .. } => {
                let mut p = *at;
                map(&mut p);
                *at = p;
                *size = (*size * ((sx.abs() + sy.abs()) * 0.5)).clamp(10.0, 400.0);
            }
        }
    }
}

/// Controls of a straight arrow: on the chord at the thirds.
pub fn arrow_straight(a: Pt, b: Pt) -> (Pt, Pt) {
    (a.lerp(&b, 1.0 / 3.0), a.lerp(&b, 2.0 / 3.0))
}

/// Controls of a bowed arrow. `bow` is signed and in the chord's normal
/// direction, so any amount and either side comes from the same formula.
pub fn arrow_bowed(a: Pt, b: Pt, bow: f32) -> (Pt, Pt) {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let (c1, c2) = arrow_straight(a, b);
    (
        Pt::new(c1.x - dy * bow, c1.y + dx * bow),
        Pt::new(c2.x - dy * bow, c2.y + dx * bow),
    )
}

/// Highlighter stroke width for one of the shared width presets. The nib is
/// wider than a pen but no longer floors at 10 px, so the thin preset actually
/// draws a thin highlighter.
pub fn marker_stroke(width: f32) -> f32 {
    (width * 3.0).max(4.0)
}

/// Whether the controls differ from the straight thirds. Comparing controls
/// (not the curve midpoint) also catches symmetric S-curves, whose midpoint
/// lies back on the chord.
pub fn arrow_bent(a: Pt, c1: Pt, c2: Pt, b: Pt) -> bool {
    let (s1, s2) = arrow_straight(a, b);
    c1.dist(&s1) > 0.75 || c2.dist(&s2) > 0.75
}

pub fn cubic(a: Pt, c1: Pt, c2: Pt, b: Pt, t: f32) -> Pt {
    let u = 1.0 - t;
    let (w0, w1, w2, w3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    Pt::new(
        a.x * w0 + c1.x * w1 + c2.x * w2 + b.x * w3,
        a.y * w0 + c1.y * w1 + c2.y * w2 + b.y * w3,
    )
}

pub fn dist_to_seg(p: Pt, a: Pt, b: Pt) -> f32 {
    let vx = b.x - a.x;
    let vy = b.y - a.y;
    let len2 = vx * vx + vy * vy;
    if len2 < 0.0001 {
        return p.dist(&a);
    }
    let t = (((p.x - a.x) * vx + (p.y - a.y) * vy) / len2).clamp(0.0, 1.0);
    p.dist(&Pt::new(a.x + vx * t, a.y + vy * t))
}

/// Handle indices: 0 NW, 1 N, 2 NE, 3 E, 4 SE, 5 S, 6 SW, 7 W.
pub const HANDLES: [(f32, f32); 8] = [
    (0.0, 0.0),
    (0.5, 0.0),
    (1.0, 0.0),
    (1.0, 0.5),
    (1.0, 1.0),
    (0.5, 1.0),
    (0.0, 1.0),
    (0.0, 0.5),
];

pub fn handle_point(r: Rect, i: usize) -> Pt {
    let (fx, fy) = HANDLES[i % 8];
    Pt::new(r.x0 + r.width() * fx, r.y0 + r.height() * fy)
}

/// Apply a drag on handle `i` to a rect, keeping the opposite edge anchored.
pub fn drag_handle(r: Rect, i: usize, dx: f32, dy: f32) -> Rect {
    let mut out = r;
    match i % 8 {
        0 => {
            out.x0 += dx;
            out.y0 += dy;
        }
        1 => out.y0 += dy,
        2 => {
            out.x1 += dx;
            out.y0 += dy;
        }
        3 => out.x1 += dx,
        4 => {
            out.x1 += dx;
            out.y1 += dy;
        }
        5 => out.y1 += dy,
        6 => {
            out.x0 += dx;
            out.y1 += dy;
        }
        _ => out.x0 += dx,
    }
    Rect::new(out.x0, out.y0, out.x1, out.y1)
}

#[derive(Clone, Debug, Default)]
pub struct Doc {
    pub objs: Vec<Obj>,
    /// Index of the object whose handles are shown. Cleared by any click that
    /// does not land on it.
    pub selected: Option<usize>,
    undo: Vec<Vec<Obj>>,
    redo: Vec<Vec<Obj>>,
}

impl Doc {
    /// Record the state *before* an edit. Call once per user gesture.
    pub fn snapshot(&mut self) {
        // Selection-only gestures and a click on an already-selected object
        // used to push an identical state, so Ctrl+Z looked broken for the
        // first few presses. Never keep two identical states on top.
        if self.undo.last().is_some_and(|last| last == &self.objs) {
            self.redo.clear();
            return;
        }
        self.undo.push(self.objs.clone());
        if self.undo.len() > MAX_HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else {
            return false;
        };
        self.redo.push(core::mem::replace(&mut self.objs, prev));
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(core::mem::replace(&mut self.objs, next));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_controls_sit_on_the_chord() {
        let (a, b) = (Pt::new(0.0, 0.0), Pt::new(90.0, 30.0));
        let (c1, c2) = arrow_straight(a, b);
        assert!(!arrow_bent(a, c1, c2, b));
        let mid = cubic(a, c1, c2, b, 0.5);
        let chord = Pt::new(45.0, 15.0);
        assert!(mid.dist(&chord) < 0.01);
    }

    /// The circle tool must hit its rim, not its hollow centre, and the fill
    /// toggle must switch that contract.
    #[test]
    fn ellipse_hit_follows_the_rim() {
        let color = crate::surface::Rgba::opaque(0, 0, 0);
        let r = Rect::from_xywh(10.0, 10.0, 100.0, 60.0);
        let hollow = Obj::new(Kind::Ellipse { r, filled: false }, color, 2.0);
        assert!(hollow.hit(Pt::new(110.0, 40.0), 2.0), "right rim");
        assert!(!hollow.hit(Pt::new(60.0, 40.0), 2.0), "centre is empty");
        assert!(!hollow.hit(Pt::new(110.0, 10.0), 2.0), "corner is outside");
        let filled = Obj::new(Kind::Ellipse { r, filled: true }, color, 2.0);
        assert!(filled.hit(Pt::new(60.0, 40.0), 2.0), "filled centre");
        assert!(!filled.hit(Pt::new(115.0, 10.0), 2.0), "outside the oval");
    }

    #[test]
    fn highlighter_thin_preset_is_thinner_than_before() {
        assert_eq!(marker_stroke(1.5), 4.5);
        assert_eq!(marker_stroke(3.0), 9.0);
        assert_eq!(marker_stroke(6.0), 18.0);
    }

    /// Both sides and any amount come from the same signed bow, and the two
    /// controls stay independent (an asymmetric S is just two different
    /// offsets).
    #[test]
    fn bow_is_signed_and_symmetric() {
        let (a, b) = (Pt::new(0.0, 0.0), Pt::new(100.0, 0.0));
        // `bow` is a fraction of the chord: 0.2 of 100 px = a 20 px control
        // offset, 15 px at the curve midpoint.
        let (c1, c2) = arrow_bowed(a, b, 0.2);
        assert!(arrow_bent(a, c1, c2, b));
        let (d1, d2) = arrow_bowed(a, b, -0.2);
        assert!(cubic(a, c1, c2, b, 0.5).y > 10.0);
        assert!(cubic(a, d1, d2, b, 0.5).y < -10.0);
        // An asymmetric S-curve is still a valid arrow.
        let s = (Pt::new(20.0, 30.0), Pt::new(80.0, -30.0));
        assert!(arrow_bent(a, s.0, s.1, b));
        // Bounds always contain the controls.
        let obj = Obj::new(Kind::Arrow { a, c1: s.0, c2: s.1, b }, crate::surface::Rgba::opaque(0, 0, 0), 2.0);
        let bounds = obj.bounds();
        assert!(bounds.y0 <= -30.0 && bounds.y1 >= 30.0);
    }
}

//! Free colour picker popover: SV square + hue strip + preview.
//!
//! Drawn as chrome over the overlay. The panel is deliberately a single
//! self-contained cell so hit testing stays in one place (`Picker::hit`) and
//! the session never has to know how the gradient is painted.

use crate::icons::{self, Icon};
use crate::layout::{BORDER, MUTED, PANEL, TEXT};
use crate::paint;
use crate::surface::{Rect, Rgba, Sk};

const PW: f32 = 248.0;
const PH: f32 = 230.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickerHit {
    Sv,
    Hue,
    Close,
    Panel,
}

pub struct Picker {
    pub open: bool,
    pub h: f32,
    pub s: f32,
    pub v: f32,
    pub rect: Rect,
    pub sv: Rect,
    pub hue: Rect,
    pub body: Rect,
    pub close: Rect,
}

impl Default for Picker {
    fn default() -> Self {
        Picker {
            open: false,
            h: 0.0,
            s: 0.0,
            v: 1.0,
            rect: Rect::default(),
            sv: Rect::default(),
            hue: Rect::default(),
            body: Rect::default(),
            close: Rect::default(),
        }
    }
}

impl Picker {
    /// Anchor the panel to the custom-swatch cell, flipping below the bar when
    /// there is no room above, and clamp it into the monitor.
    pub fn open_with(&mut self, color: Rgba, anchor: Rect, monitor: Rect, scale: f32) {
        let s = scale.max(0.6);
        let (h, sat, v) = rgb_to_hsv(color);
        self.h = h;
        self.s = sat;
        self.v = v;

        let w = PW * s;
        let hgt = PH * s;
        let gap = 10.0 * s;
        let mut x = anchor.center().0 - w * 0.5;
        let mut y = anchor.y0 - gap - hgt;
        if y < monitor.y0 + 8.0 * s {
            y = anchor.y1 + gap;
        }
        x = x.clamp(
            monitor.x0 + 8.0 * s,
            (monitor.x1 - 8.0 * s - w).max(monitor.x0 + 8.0 * s),
        );
        y = y.clamp(
            monitor.y0 + 8.0 * s,
            (monitor.y1 - 8.0 * s - hgt).max(monitor.y0 + 8.0 * s),
        );
        self.rect = Rect::from_xywh(x, y, w, hgt);

        let pad = 12.0 * s;
        let title = 24.0 * s;
        self.close = Rect::from_xywh(x + w - pad - 18.0 * s, y + pad + 2.0 * s, 18.0 * s, 18.0 * s);
        self.sv = Rect::from_xywh(x + pad, y + pad + title, w - pad * 2.0, 122.0 * s);
        self.hue = Rect::from_xywh(self.sv.x0, self.sv.y1 + 9.0 * s, self.sv.width(), 14.0 * s);
        self.body = Rect::from_xywh(self.sv.x0, self.hue.y1 + 10.0 * s, self.sv.width(), 22.0 * s);
        self.open = true;
    }

    pub fn hit(&self, p: crate::model::Pt) -> Option<PickerHit> {
        if !self.open {
            return None;
        }
        if self.close.contains(p.x, p.y) {
            return Some(PickerHit::Close);
        }
        if self.sv.contains(p.x, p.y) {
            return Some(PickerHit::Sv);
        }
        if self.hue.contains(p.x, p.y) {
            return Some(PickerHit::Hue);
        }
        if self.rect.contains(p.x, p.y) {
            return Some(PickerHit::Panel);
        }
        None
    }

    pub fn set_sv(&mut self, p: crate::model::Pt) {
        let s = ((p.x - self.sv.x0) / self.sv.width().max(1.0)).clamp(0.0, 1.0);
        let v = 1.0 - ((p.y - self.sv.y0) / self.sv.height().max(1.0)).clamp(0.0, 1.0);
        self.s = s;
        self.v = v;
    }

    pub fn set_hue(&mut self, p: crate::model::Pt) {
        let t = ((p.x - self.hue.x0) / self.hue.width().max(1.0)).clamp(0.0, 1.0);
        self.h = t * 360.0;
    }

    pub fn color(&self) -> Rgba {
        hsv_to_rgb(self.h, self.s, self.v)
    }

    pub fn draw(&self, sk: &mut Sk, scale: f32) {
        if !self.open {
            return;
        }
        let s = scale.max(0.6);
        icons::panel(sk, self.rect, 12.0 * s, PANEL, BORDER);
        let title = "自定义颜色";
        let size = 12.5 * s;
        paint::label(
            sk,
            title,
            self.rect.x0 + 12.0 * s,
            self.rect.y0 + 12.0 * s,
            size,
            TEXT,
        );
        icons::draw(
            sk,
            Icon::Dismiss,
            self.close.inflate(-3.0 * s),
            MUTED,
            1.6,
        );

        // SV square as a solid-cell grid: the painter's channel constraints
        // mean no gradients, and 36×20 cells are visually smooth at this size.
        let cols = 36;
        let rows = 20;
        let cw = self.sv.width() / cols as f32;
        let ch = self.sv.height() / rows as f32;
        for row in 0..rows {
            let v = 1.0 - (row as f32 + 0.5) / rows as f32;
            for col in 0..cols {
                let sat = (col as f32 + 0.5) / cols as f32;
                let c = hsv_to_rgb(self.h, sat, v);
                let cell = Rect::from_xywh(
                    self.sv.x0 + col as f32 * cw,
                    self.sv.y0 + row as f32 * ch,
                    cw + 0.5,
                    ch + 0.5,
                );
                sk.fill_rect(&cell, c);
            }
        }
        sk.stroke_rect(&self.sv, 1.0, Rgba::new(0, 0, 0, 0x40));

        let mx = self.sv.x0 + self.s * self.sv.width();
        let my = self.sv.y0 + (1.0 - self.v) * self.sv.height();
        sk.ring(mx, my, 7.0 * s, 2.0 * s, Rgba::opaque(0xFF, 0xFF, 0xFF));
        sk.ring(mx, my, 7.0 * s, 1.0, Rgba::new(0, 0, 0, 0x88));

        let hue_cols = 48;
        let hw = self.hue.width() / hue_cols as f32;
        for i in 0..hue_cols {
            let c = hsv_to_rgb((i as f32 + 0.5) / hue_cols as f32 * 360.0, 1.0, 1.0);
            let cell = Rect::from_xywh(self.hue.x0 + i as f32 * hw, self.hue.y0, hw + 0.5, self.hue.height());
            sk.fill_rect(&cell, c);
        }
        let hx = self.hue.x0 + (self.h / 360.0) * self.hue.width();
        sk.fill_rect(
            &Rect::from_xywh(hx - 1.5 * s, self.hue.y0 - 2.0 * s, 3.0 * s, self.hue.height() + 4.0 * s),
            Rgba::opaque(0xFF, 0xFF, 0xFF),
        );
        sk.stroke_rect(
            &Rect::from_xywh(hx - 1.5 * s, self.hue.y0 - 2.0 * s, 3.0 * s, self.hue.height() + 4.0 * s),
            1.0,
            Rgba::new(0, 0, 0, 0x88),
        );

        let c = self.color();
        let cy = self.body.center().1;
        sk.circle(self.body.x0 + 8.0 * s, cy, 8.0 * s, c);
        sk.ring(self.body.x0 + 8.0 * s, cy, 8.0 * s, 1.0, Rgba::new(0, 0, 0, 0x66));
        let hex = format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b);
        paint::label(
            sk,
            &hex,
            self.body.x0 + 24.0 * s,
            self.body.y0 + (self.body.height() - 15.0 * s) * 0.5,
            12.5 * s,
            TEXT,
        );
        paint::label(
            sk,
            "拖动取色 · 立即生效",
            self.body.x0 + 96.0 * s,
            self.body.y0 + (self.body.height() - 14.0 * s) * 0.5,
            11.0 * s,
            MUTED,
        );
    }
}

pub fn rgb_to_hsv(c: Rgba) -> (f32, f32, f32) {
    let r = c.r as f32 / 255.0;
    let g = c.g as f32 / 255.0;
    let b = c.b as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d < 1e-6 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };
    let s = if max <= 1e-6 { 0.0 } else { d / max };
    (h, s, max)
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Rgba {
    let h = h.rem_euclid(360.0);
    let s = s.clamp(0.0, 1.0);
    let v = v.clamp(0.0, 1.0);
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    Rgba::opaque(
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8,
    )
}

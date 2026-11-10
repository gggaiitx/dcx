//! Continuous corner-radius popover. The old button cycled five presets; the
//! product needs any value in a wider range, chosen by dragging.

use crate::icons;
use crate::layout::{ACCENT, BORDER, MUTED, PANEL, TEXT};
use crate::paint;
use crate::surface::{Rect, Rgba, Sk};

pub const MAX: f32 = 128.0;
const PW: f32 = 264.0;
const PH: f32 = 86.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RadiusHit {
    Track,
    Panel,
}

pub struct RadiusPicker {
    pub open: bool,
    pub value: f32,
    pub rect: Rect,
    pub track: Rect,
    /// Generous vertical target around the thin track.
    hit: Rect,
}

impl Default for RadiusPicker {
    fn default() -> Self {
        RadiusPicker {
            open: false,
            value: 0.0,
            rect: Rect::default(),
            track: Rect::default(),
            hit: Rect::default(),
        }
    }
}

impl RadiusPicker {
    pub fn open_with(&mut self, current: f32, anchor: Rect, monitor: Rect, scale: f32) {
        let s = scale.max(0.6);
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
        self.track = Rect::from_xywh(
            x + 14.0 * s,
            y + 48.0 * s,
            w - 28.0 * s,
            6.0 * s,
        );
        self.hit = self.track.inflate(14.0 * s);
        self.value = current.clamp(0.0, MAX);
        self.open = true;
    }

    pub fn hit(&self, p: crate::model::Pt) -> Option<RadiusHit> {
        if !self.open {
            return None;
        }
        if self.hit.contains(p.x, p.y) {
            return Some(RadiusHit::Track);
        }
        if self.rect.contains(p.x, p.y) {
            return Some(RadiusHit::Panel);
        }
        None
    }

    pub fn set_from_x(&mut self, x: f32) {
        let t = ((x - self.track.x0) / self.track.width().max(1.0)).clamp(0.0, 1.0);
        self.value = (t * MAX).round();
    }

    pub fn draw(&self, sk: &mut Sk, scale: f32) {
        if !self.open {
            return;
        }
        let s = scale.max(0.6);
        icons::panel(sk, self.rect, 12.0 * s, PANEL, BORDER);
        paint::label(
            sk,
            "截图圆角",
            self.rect.x0 + 14.0 * s,
            self.rect.y0 + 12.0 * s,
            12.5 * s,
            TEXT,
        );
        let val = format!("{} px", self.value.round() as i32);
        let tw = crate::text::measure(&val, 12.5 * s).0;
        paint::label(
            sk,
            &val,
            self.rect.x1 - 14.0 * s - tw,
            self.rect.y0 + 12.0 * s,
            12.5 * s,
            ACCENT,
        );

        // Groove, filled portion, knob.
        sk.fill_round(&self.track, 3.0 * s, Rgba::new(0xFF, 0xFF, 0xFF, 0x16));
        let t = (self.value / MAX).clamp(0.0, 1.0);
        let kx = self.track.x0 + t * self.track.width();
        if t > 0.0 {
            let fill = Rect::from_xywh(
                self.track.x0,
                self.track.y0,
                (kx - self.track.x0).max(3.0 * s),
                self.track.height(),
            );
            sk.fill_round(&fill, 3.0 * s, ACCENT);
        }
        sk.circle(kx, self.track.center().1, 7.0 * s, Rgba::opaque(0xF3, 0xF5, 0xF7));
        sk.ring(kx, self.track.center().1, 7.0 * s, 1.2 * s, ACCENT);

        // Ticks at 0/32/64/96/128 so the range reads at a glance.
        for i in 0..=4 {
            let x = self.track.x0 + self.track.width() * i as f32 / 4.0;
            sk.line(
                (x, self.track.y1 + 8.0 * s),
                (x, self.track.y1 + 14.0 * s),
                1.0,
                MUTED,
            );
        }
        let hint = "拖动滑块，0 = 直角";
        paint::label(
            sk,
            hint,
            self.track.x0,
            self.track.y1 + 18.0 * s,
            10.5 * s,
            MUTED,
        );
    }
}

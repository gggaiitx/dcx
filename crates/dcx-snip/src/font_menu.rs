//! Font-family popover for the text tool.
//!
//! Same interaction as the radius slider and the colour picker: the toolbar
//! cell opens a panel anchored to itself, a click picks a row, a click outside
//! dismisses it and is consumed (so dismissing never starts a new selection).
//!
//! The list is every family GDI reports (see [`crate::fonts`]), not a hard-coded
//! seven: a screenshot tool that cannot use the font the document is set in is
//! useless for hand-off. That also means the list does not fit on screen, so
//! the popover is a **fixed-height viewport scrolled a whole row at a time**.
//! Whole rows matter: `Sk` has no clip stack, and pixel-smooth scrolling would
//! need one to stop half rows from painting outside the panel.

use crate::fonts;
use crate::icons::{self, Icon};
use crate::layout::{ACCENT, ACTIVE, BORDER, HOVER, MUTED, PANEL, TEXT};
use crate::model::{Pt, TextStyle};
use crate::paint;
use crate::surface::{Rect, Rgba, Sk};
use crate::text;

const ROW_H: f32 = 28.0;
const PW: f32 = 208.0;
const PAD: f32 = 6.0;
/// Share of the monitor height the popover may take, and an absolute row cap:
/// a font list that covers half the screen is not a picker, it is a dialog.
const MAX_FRACTION: f32 = 0.45;
const MAX_ROWS: usize = 10;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuHit {
    Row(usize),
    /// Inside the panel but not on a row (padding): consumed.
    Panel,
    /// Scrollbar: press starts a drag that maps the pointer to a position.
    Scrollbar,
    Outside,
}

pub struct FontMenu {
    pub open: bool,
    pub rect: Rect,
    /// Area the rows are drawn in (panel minus padding).
    view: Rect,
    /// Scrollbar groove, and a wider band that receives the pointer.
    track: Rect,
    grab: Rect,
    row_h: f32,
    /// Index of the first visible row. Scrolling is row-granular; only the
    /// scrollbar drag maps to a proportion (a thumb cannot be row-granular).
    first: usize,
    visible: usize,
    len: usize,
    active: Option<usize>,
}

impl Default for FontMenu {
    fn default() -> Self {
        FontMenu {
            open: false,
            rect: Rect::default(),
            view: Rect::default(),
            track: Rect::default(),
            grab: Rect::default(),
            row_h: ROW_H,
            first: 0,
            visible: 0,
            len: 0,
            active: None,
        }
    }
}

impl FontMenu {
    pub fn open_with(&mut self, current: &str, anchor: Rect, monitor: Rect, scale: f32) {
        let s = scale.max(0.6);
        let list = fonts::all();
        self.len = list.len().max(1);
        self.row_h = (ROW_H * s).max(16.0);
        self.active = fonts::index(current);

        let max_h = (monitor.height() * MAX_FRACTION).max(self.row_h * 4.0);
        let fits = (((max_h - PAD * 2.0 * s) / self.row_h).floor() as usize).max(4);
        self.visible = fits.min(self.len).min(MAX_ROWS);

        let w = PW * s;
        let h = self.visible as f32 * self.row_h + PAD * 2.0 * s;
        let gap = 10.0 * s;
        // Above the bar when it fits: the bar is under the selection, so the
        // side away from it is the side that is not covering the annotation.
        let mut x = anchor.x0;
        let mut y = anchor.y0 - gap - h;
        if y < monitor.y0 + 8.0 * s {
            y = anchor.y1 + gap;
        }
        x = x.clamp(
            monitor.x0 + 8.0 * s,
            (monitor.x1 - 8.0 * s - w).max(monitor.x0 + 8.0 * s),
        );
        y = y.clamp(
            monitor.y0 + 8.0 * s,
            (monitor.y1 - 8.0 * s - h).max(monitor.y0 + 8.0 * s),
        );
        self.rect = Rect::from_xywh(x, y, w, h);
        self.view = Rect::from_xywh(
            x + PAD * 0.5 * s,
            y + PAD * s,
            w - PAD * s,
            h - PAD * 2.0 * s,
        );
        // Groove on the right edge; the grab band is three times wider so the
        // thumb can actually be caught with a mouse.
        self.track = Rect::from_xywh(
            x + w - 7.0 * s,
            y + PAD * s + 2.0 * s,
            4.0 * s,
            h - PAD * 2.0 * s - 4.0 * s,
        );
        self.grab = Rect::from_xywh(x + w - 16.0 * s, y + PAD * s, 16.0 * s, h - PAD * 2.0 * s);

        // Open with the current family in the middle of the viewport: at the
        // bottom edge it reads as "list ended here" instead of "this is what
        // you have".
        self.first = match self.active {
            Some(i) => {
                let max_first = self.len.saturating_sub(self.visible);
                i.saturating_sub(self.visible / 2).min(max_first)
            }
            None => 0,
        };
        self.open = true;
    }

    pub fn hit(&self, p: Pt) -> MenuHit {
        if !self.open {
            return MenuHit::Outside;
        }
        // The scrollbar wins over rows: it sits on top of the row band.
        if self.len > self.visible && self.grab.contains(p.x, p.y) {
            return MenuHit::Scrollbar;
        }
        if self.view.contains(p.x, p.y) {
            let row = ((p.y - self.view.y0) / self.row_h).floor().max(0.0);
            let idx = self.first + row as usize;
            if idx < self.len {
                return MenuHit::Row(idx);
            }
        }
        if self.rect.contains(p.x, p.y) {
            MenuHit::Panel
        } else {
            MenuHit::Outside
        }
    }

    /// Map a pointer position on the scrollbar to a scroll offset. Called
    /// continuously while the thumb is dragged.
    pub fn scroll_to_y(&mut self, y: f32) {
        if !self.open || self.len <= self.visible {
            return;
        }
        let max_first = (self.len - self.visible) as f32;
        let t = ((y - self.track.y0) / self.track.height().max(1.0)).clamp(0.0, 1.0);
        self.first = (t * max_first).round() as usize;
    }

    /// Thumb rectangle for the current offset; `None` when everything fits.
    fn thumb(&self) -> Option<Rect> {
        if self.len <= self.visible {
            return None;
        }
        let frac = self.visible as f32 / self.len as f32;
        let h = (self.track.height() * frac).max(22.0 * (self.row_h / ROW_H));
        let max_first = (self.len - self.visible).max(1) as f32;
        let y = self.track.y0 + (self.track.height() - h) * self.first as f32 / max_first;
        Some(Rect::from_xywh(self.track.x0, y, self.track.width(), h))
    }

    /// Wheel input: positive `notches` scrolls down. Three rows per notch is
    /// enough to cross a long list without losing the place.
    pub fn scroll_by(&mut self, notches: i32) {
        if !self.open || self.len <= self.visible {
            return;
        }
        let max_first = self.len - self.visible;
        let next = self.first as i32 + 3 * notches;
        self.first = next.clamp(0, max_first as i32) as usize;
    }

    pub fn draw(&self, sk: &mut Sk, scale: f32, cursor: Pt) {
        if !self.open {
            return;
        }
        let s = scale.max(0.6);
        icons::panel(sk, self.rect, 10.0 * s, PANEL, BORDER);

        let list = fonts::all();
        let size = 13.0 * s;
        for i in self.first..(self.first + self.visible).min(self.len) {
            let Some(face) = list.get(i) else { continue };
            let y = self.view.y0 + (i - self.first) as f32 * self.row_h;
            let row = Rect::from_xywh(
                self.view.x0,
                y + 1.0 * s,
                self.view.width() - 12.0 * s,
                self.row_h - 2.0 * s,
            );
            let on = self.active == Some(i);
            if row.contains(cursor.x, cursor.y) {
                sk.fill_round(&row, 7.0 * s, HOVER);
            }
            if on {
                sk.fill_round(&row, 7.0 * s, ACTIVE);
            }
            let color = if on { ACCENT } else { TEXT };
            let style = TextStyle {
                face: face.id.clone(),
                bold: false,
                italic: false,
                shadow: false,
            };
            let right = if on { 26.0 * s } else { 8.0 * s };
            let label = text::fit_px(&face.label, size, &style, row.width() - 12.0 * s - right);
            paint::label_styled(
                sk,
                &label,
                row.x0 + 9.0 * s,
                y + 5.0 * s,
                size,
                color,
                &style,
            );
            if on {
                let check = Rect::from_xywh(
                    row.x1 - 19.0 * s,
                    y + self.row_h * 0.5 - 6.0 * s,
                    12.0 * s,
                    12.0 * s,
                );
                icons::draw(sk, Icon::Check, check, ACCENT, 1.6);
            }
        }

        // Scrollbar: proportional thumb, drawn only when the list overflows.
        if let Some(thumb) = self.thumb() {
            sk.fill_round(&self.track, 2.0 * s, Rgba::new(0xFF, 0xFF, 0xFF, 0x14));
            sk.fill_round(&thumb, 2.0 * s, MUTED);
        }
    }
}

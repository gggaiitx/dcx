//! The floating long-capture bar: geometry, hit testing and painting.
//!
//! One row, same vocabulary as the overlay toolbar: dark rounded panel, icon
//! buttons, a short hint. It sits under the viewport (above it when the bottom
//! of the screen is closer than the bar), is centred on the selection and is a
//! `WS_EX_NOACTIVATE` window so clicking it never takes focus from the page.

use super::{px, BoxRect};
use crate::icons::{self, Icon};
use crate::layout::{ACCENT, BORDER, DANGER, DISABLED, HOVER, INK, MUTED, PANEL, PRESSED, TEXT};
use crate::paint::{self, mix};
use crate::surface::{Rect, Rgba, Surface};
use crate::text;

const H_BASE: f32 = 58.0;
const MIN_W_BASE: f32 = 520.0;
const MAX_W_BASE: f32 = 640.0;
const BTN_W_BASE: f32 = 74.0;
const BTN_H_BASE: f32 = 34.0;
const PAD_BASE: f32 = 11.0;
const GAP_BASE: f32 = 4.0;
const MARGIN_BASE: f32 = 8.0;
const RADIUS_BASE: f32 = 11.0;
const FONT_TITLE: f32 = 13.5;
const FONT_HINT: f32 = 11.5;
const FONT_HEIGHT: f32 = 10.5;
const FONT_BTN: f32 = 12.0;
const ICON_BASE: f32 = 16.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Btn {
    Undo,
    Copy,
    Save,
    Close,
}

pub(super) struct Bar {
    pub rect: BoxRect,
    hits: Vec<(Rect, Btn)>,
}

impl Bar {
    pub fn hit(&self, x: f32, y: f32) -> Option<Btn> {
        self.hits
            .iter()
            .find(|(r, _)| r.contains(x, y))
            .map(|(_, b)| *b)
    }
}

pub(super) struct State<'a> {
    pub can_undo: bool,
    pub hint: &'a str,
    pub error: bool,
    pub height: u32,
    pub hover: Option<Btn>,
    pub pressed: Option<Btn>,
}

/// Below the selection when it fits, above it otherwise, and wedged inside
/// the bottom edge when the selection covers the whole screen.
pub(super) fn layout(vp: BoxRect, screen: (i32, i32, i32, i32), scale: f32) -> Bar {
    let (sx, sy, sw, sh) = screen;
    let bar_h = px(H_BASE, scale).max(px(30.0, scale));
    let margin = px(MARGIN_BASE, scale);
    let max_w = (sw - margin * 2).max(px(240.0, scale));
    let bar_w = (vp.w as i32)
        .clamp(px(MIN_W_BASE, scale), px(MAX_W_BASE, scale))
        .min(max_w);

    let x = (vp.x + (vp.w as i32 - bar_w) / 2)
        .clamp(sx + margin, (sx + sw - margin - bar_w).max(sx + margin));

    let below = vp.y + vp.h as i32 + margin;
    let above = vp.y - margin - bar_h;
    let y = if below + bar_h <= sy + sh {
        below
    } else if above >= sy {
        above
    } else {
        (vp.y + vp.h as i32 - bar_h - margin).max(sy)
    };

    let btn_w = px(BTN_W_BASE, scale);
    let btn_h = px(BTN_H_BASE, scale);
    let pad = px(PAD_BASE, scale);
    let gap = px(GAP_BASE, scale);
    let btn_y = ((bar_h - btn_h) / 2) as f32;
    let mut hits = Vec::with_capacity(4);
    let mut bx = bar_w - pad - btn_w;
    for button in [Btn::Close, Btn::Save, Btn::Copy, Btn::Undo] {
        hits.push((
            Rect::from_xywh(bx as f32, btn_y, btn_w as f32, btn_h as f32),
            button,
        ));
        bx -= btn_w + gap;
    }

    Bar {
        rect: BoxRect {
            x,
            y,
            w: bar_w.max(1) as u32,
            h: bar_h.max(1) as u32,
        },
        hits,
    }
}

pub(super) fn render(surface: &mut Surface, bar: &Bar, state: &State, scale: f32) {
    let (w, h) = (surface.w as f32, surface.h as f32);
    // Layered pixels accumulate: every frame starts from full transparency.
    surface.bytes_mut().fill(0);
    let radius = px(RADIUS_BASE, scale) as f32;
    let pad = px(PAD_BASE, scale) as f32;
    let undo_x = bar
        .hits
        .iter()
        .find(|(_, b)| *b == Btn::Undo)
        .map(|(r, _)| r.x0)
        .unwrap_or(w);

    surface.canvas(|sk| {
        let full = Rect::from_xywh(0.5, 0.5, w - 1.0, h - 1.0);
        sk.fill_round(&full, radius, PANEL);
        sk.stroke_round(&full, radius, 1.0, BORDER);

        let title = "长截图";
        let title_w = paint::label(sk, title, pad, px(9.0, scale) as f32, FONT_TITLE * scale, TEXT);
        if state.height > 0 {
            let meta = format!("{} px", state.height);
            paint::label(
                sk,
                &meta,
                pad + title_w + 8.0 * scale,
                px(11.0, scale) as f32,
                FONT_HEIGHT * scale,
                MUTED,
            );
        }

        let available = (undo_x - pad - 8.0 * scale).max(0.0);
        let hint = fit(state.hint, FONT_HINT * scale, available);
        paint::label(
            sk,
            &hint,
            pad,
            px(31.0, scale) as f32,
            FONT_HINT * scale,
            if state.error { DANGER } else { MUTED },
        );

        for (r, button) in &bar.hits {
            let hot = state.hover == Some(*button);
            let down = state.pressed == Some(*button) && hot;
            let disabled = *button == Btn::Undo && !state.can_undo;
            let (glyph, label) = match button {
                Btn::Undo => (Icon::Rewind, "撤销"),
                Btn::Copy => (Icon::Stack, "复制"),
                Btn::Save => (Icon::Slot, "保存"),
                Btn::Close => (Icon::Dismiss, "关闭"),
            };
            let area = r.inflate(-1.0 * scale);
            if *button == Btn::Copy {
                let base = if down {
                    mix(ACCENT, Rgba::opaque(0, 0, 0), 0.18)
                } else if hot {
                    mix(ACCENT, Rgba::opaque(255, 255, 255), 0.10)
                } else {
                    ACCENT
                };
                sk.fill_round(&area, 9.0 * scale, base);
            } else if down {
                sk.fill_round(&area, 9.0 * scale, PRESSED);
            } else if hot && *button == Btn::Close {
                sk.fill_round(&area, 9.0 * scale, Rgba::new(0xE5, 0x48, 0x4D, 0x2A));
            } else if hot && !disabled {
                sk.fill_round(&area, 9.0 * scale, HOVER);
            }
            let color = if *button == Btn::Copy {
                INK
            } else if disabled {
                DISABLED
            } else if *button == Btn::Close && hot {
                DANGER
            } else {
                TEXT
            };
            // Icon and label are one centred group: the old icon-only row had
            // a huge primary block and three faint glyphs; labels make the
            // four actions legible without hover tooltips.
            let icon = px(ICON_BASE, scale) as f32;
            let fs = FONT_BTN * scale;
            let tw = text::measure(label, fs).0;
            let gap = 5.0 * scale;
            let x0 = r.x0 + (r.width() - (icon + gap + tw)) * 0.5;
            let icon_rect = Rect::from_xywh(x0, r.y0 + (r.height() - icon) * 0.5, icon, icon);
            icons::draw(sk, glyph, icon_rect, color, 1.6);
            let th = text::measure(label, fs).1;
            paint::label(sk, label, x0 + icon + gap, r.y0 + (r.height() - th) * 0.5, fs, color);
        }
    });
}

/// Truncate with an ellipsis so a long error can never run under the buttons.
fn fit(body: &str, size: f32, max: f32) -> String {
    if max <= 0.0 {
        return String::new();
    }
    if text::measure(body, size).0 <= max {
        return body.to_string();
    }
    let mut out = String::new();
    for ch in body.chars() {
        let next = format!("{out}{ch}…");
        if text::measure(&next, size).0 > max {
            break;
        }
        out.push(ch);
    }
    if out.is_empty() {
        String::new()
    } else {
        format!("{out}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp(w: u32) -> BoxRect {
        BoxRect {
            x: 300,
            y: 200,
            w,
            h: 500,
        }
    }

    #[test]
    fn all_four_buttons_hit() {
        let scale = 1.0;
        let bar = layout(vp(800), (0, 0, 1920, 1080), scale);
        let cy = bar.rect.h as f32 * 0.5;
        let pad = px(PAD_BASE, scale);
        let btn = px(BTN_W_BASE, scale);
        let gap = px(GAP_BASE, scale);
        let at = |i: i32| (bar.rect.w as i32 - pad - i * (btn + gap) - btn / 2) as f32;
        assert_eq!(bar.hit(at(0), cy), Some(Btn::Close));
        assert_eq!(bar.hit(at(1), cy), Some(Btn::Save));
        assert_eq!(bar.hit(at(2), cy), Some(Btn::Copy));
        assert_eq!(bar.hit(at(3), cy), Some(Btn::Undo));
        assert_eq!(bar.hit(bar.rect.w as f32 * 0.5, cy), None);
        // Above the pills is panel, not a target.
        assert_eq!(bar.hit(at(2), 2.0), None);
    }

    #[test]
    fn sits_below_selection_when_there_is_room() {
        let bar = layout(vp(800), (0, 0, 1920, 1080), 1.0);
        assert_eq!(bar.rect.y, 200 + 500 + px(MARGIN_BASE, 1.0));
        assert_eq!(bar.rect.x + bar.rect.w as i32 / 2, 300 + 400, "centred on the selection");
    }

    #[test]
    fn flips_above_near_the_bottom() {
        let near = BoxRect {
            x: 200,
            y: 750,
            w: 900,
            h: 300,
        };
        let bar = layout(near, (0, 0, 1920, 1080), 1.0);
        assert!(bar.rect.bottom() <= near.y, "bar must not cover the viewport");
        assert!(bar.rect.y >= 0);
    }

    #[test]
    fn wedges_inside_a_full_height_selection() {
        let full = BoxRect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
        };
        let bar = layout(full, (0, 0, 1920, 1080), 1.0);
        assert!(bar.rect.y >= 0);
        assert!(bar.rect.bottom() <= 1080);
        assert!(bar.rect.intersects(&full), "no room left: overlap is expected");
    }

    #[test]
    fn scales_with_dpi() {
        let base = layout(vp(800), (0, 0, 1920, 1080), 1.0);
        let hi = layout(
            BoxRect {
                x: 600,
                y: 400,
                w: 1600,
                h: 1000,
            },
            (0, 0, 3840, 2160),
            2.0,
        );
        assert_eq!(hi.rect.h, base.rect.h * 2);
        assert_eq!(hi.rect.w, px(MAX_W_BASE, 2.0) as u32, "capped at the scaled max width");
    }

    #[test]
    fn fit_shortens_only_when_needed() {
        assert_eq!(fit("短提示", FONT_HINT, 500.0), "短提示");
        let long = "检测到滚动但拼接失败，请放慢滚动速度，不要一次滚太远";
        let short = fit(long, FONT_HINT, 90.0);
        assert!(short.ends_with('…'));
        assert!(text::measure(&short, FONT_HINT).0 <= 90.0);
    }
}

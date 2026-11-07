//! The capsule — the always-visible part of the island.
//!
//! Layout is computed right-to-left from measured text galleys instead of using
//! a layout pass: the capsule must keep the clock at a **fixed** x while the
//! metrics on the right appear and disappear, otherwise the time jumps every
//! time a bar changes width. Every element is placed from the capsule's own
//! rectangle, so a resize never moves the clock either.

use egui::{Align2, Color32, FontId, Pos2, Rect, Stroke, Vec2};

use super::theme::Palette;
use super::widgets::{self, Glyph};

pub struct CapsuleState {
    pub time: String,
    pub weekday: String,
    pub cpu: Option<f32>,
    pub mem: Option<f32>,
    /// Down/up rate in bytes per second, already summed for display.
    pub net: Option<(f64, f64)>,
    pub task: Option<String>,
    pub unread: usize,
    pub hovered: bool,
    pub open: bool,
}

/// What the capsule wants the app to do after this frame.
#[derive(Default)]
pub struct CapsuleHit {
    pub clicked: bool,
}

pub fn draw(
    ui: &mut egui::Ui,
    rect: Rect,
    st: &CapsuleState,
    p: &Palette,
    opacity: f32,
    time_size: f32,
) -> CapsuleHit {
    let radius = rect.height() * 0.5;
    let surface = with_alpha(p.surface, opacity);
    let stroke = if st.open {
        Stroke::new(1.0_f32, with_alpha(p.accent, 0.55))
    } else if st.hovered {
        Stroke::new(1.0_f32, with_alpha(p.stroke, 0.9))
    } else {
        Stroke::new(1.0_f32, p.stroke)
    };

    widgets::shadow(ui, rect, radius, p.shadow);
    widgets::round_rect(ui, rect, radius, surface, stroke);

    // A very light top sheen, the same trick the taskbar pill uses: it makes a
    // translucent capsule read as glass instead of as flat grey.
    if !p.light {
        let sheen = Rect::from_min_max(
            rect.min + Vec2::new(radius * 0.6, 1.0),
            Pos2::new(rect.right() - radius * 0.6, rect.center().y),
        );
        widgets::round_rect(
            ui,
            sheen,
            sheen.height() * 0.5,
            Color32::from_rgba_unmultiplied(255, 255, 255, 10),
            Stroke::NONE,
        );
    }

    let pad = (rect.height() * 0.34).clamp(9.0, 14.0);
    let mut left = rect.left() + pad;
    let mid_y = rect.center().y;

    // -- clock -------------------------------------------------------------
    let time_rect = widgets::mono_text(
        ui,
        Pos2::new(left, mid_y),
        Align2::LEFT_CENTER,
        &st.time,
        time_size,
        p.text,
    );
    left = time_rect.right() + 6.0;

    // Weekday disappears first when the capsule is narrow: the clock is the
    // reason the island exists.
    let weekday_w = ui
        .painter()
        .layout_no_wrap(
            st.weekday.clone(),
            FontId::proportional(10.5),
            p.text_dim,
        )
        .size()
        .x;
    if rect.width() > 196.0 && !st.weekday.is_empty() {
        widgets::text(
            ui,
            Pos2::new(left, mid_y + 0.5),
            Align2::LEFT_CENTER,
            &st.weekday,
            10.5,
            p.text_dim,
        );
        left += weekday_w + 8.0;
    }

    // -- right cluster -----------------------------------------------------
    let mut right = rect.right() - pad;

    // Unread badge: a filled pill with the count, not a bare dot — a dot is
    // indistinguishable from a rendering artefact at this size.
    if st.unread > 0 && rect.width() > 180.0 {
        let label = if st.unread > 99 {
            "99+".to_string()
        } else {
            st.unread.to_string()
        };
        let galley = ui.painter().layout_no_wrap(
            label,
            FontId::proportional(9.5),
            Color32::WHITE,
        );
        let w = (galley.size().x + 8.0).max(14.0);
        let badge = Rect::from_min_size(
            Pos2::new(right - w, mid_y - 6.5),
            Vec2::new(w, 13.0),
        );
        widgets::round_rect(ui, badge, 6.5, p.danger, Stroke::NONE);
        ui.painter().galley(
            Pos2::new(badge.center().x - galley.size().x * 0.5, badge.center().y - galley.size().y * 0.5),
            galley,
            Color32::WHITE,
        );
        right = badge.left() - 8.0;
    }

    // Metrics: a 22 pt bar plus a percentage. Two decimals of hardware would be
    // noise; one number per metric is the point of a capsule.
    let mut metrics: Vec<(&str, f32, Color32)> = Vec::new();
    if let Some(cpu) = st.cpu {
        metrics.push(("CPU", cpu, p.level(crate::metrics::level(cpu))));
    }
    if let Some(mem) = st.mem {
        metrics.push(("MEM", mem, p.level(crate::metrics::level(mem))));
    }
    if rect.width() >= 250.0 {
        for (label, value, color) in metrics.iter().rev() {
            let text = format!("{value:.0}%");
            let galley = ui.painter().layout_no_wrap(
                text.clone(),
                FontId::monospace(10.0),
                p.text_dim,
            );
            let text_w = galley.size().x;
            let x = right - text_w;
            ui.painter()
                .galley(Pos2::new(x, mid_y - galley.size().y * 0.5), galley, p.text_dim);
            let bar_w = 18.0;
            let bar = Rect::from_min_size(
                Pos2::new(x - bar_w - 5.0, mid_y - 1.5),
                Vec2::new(bar_w, 3.0),
            );
            widgets::bar(ui, bar, *value / 100.0, *color, p.track);
            right = bar.left() - 8.0;
            let _ = label;
        }
    }

    // Network rates live in the capsule only when there is room left over; the
    // panel's performance tab is the detailed view.
    if let Some((down, up)) = st.net {
        if rect.width() >= 300.0 {
            let text = format!(
                "↓{} ↑{}",
                crate::metrics::human_rate(down),
                crate::metrics::human_rate(up)
            );
            let galley =
                ui.painter()
                    .layout_no_wrap(text, FontId::monospace(9.0), p.text_faint);
            let w = galley.size().x;
            if right - w > left + 20.0 {
                ui.painter().galley(
                    Pos2::new(right - w, mid_y - galley.size().y * 0.5),
                    galley,
                    p.text_faint,
                );
                right -= w + 8.0;
            }
        }
    }

    // -- current task ------------------------------------------------------
    if let Some(task) = &st.task {
        let avail = (right - left).max(0.0);
        if avail > 40.0 {
            let galley = ui.painter().layout(
                task.clone(),
                FontId::proportional(11.0),
                p.text_dim,
                avail,
            );
            let h = galley.size().y.min(rect.height() - 6.0);
            let pos = Pos2::new(left, mid_y - h * 0.5);
            let clip = Rect::from_min_size(pos, Vec2::new(avail, h));
            ui.painter().with_clip_rect(clip).galley(pos, galley, p.text_dim);
        }
    }

    let resp = ui.allocate_rect(rect, egui::Sense::click());
    CapsuleHit {
        clicked: resp.clicked(),
    }
}

fn with_alpha(color: Color32, factor: f32) -> Color32 {
    let a = (color.a() as f32 * factor.clamp(0.0, 1.0)) as u8;
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
}

/// Tiny resize handle drawn in the panel's bottom-right corner.
pub fn resize_grip(ui: &egui::Ui, rect: Rect, p: &Palette) {
    let c = p.text_faint;
    for i in 0..3 {
        let off = i as f32 * 3.0;
        ui.painter().line_segment(
            [
                Pos2::new(rect.right() - 4.0 - off, rect.bottom() - 4.0),
                Pos2::new(rect.right() - 4.0, rect.bottom() - 4.0 - off),
            ],
            Stroke::new(1.0_f32, c),
        );
    }
    let _ = Glyph::Gear;
}

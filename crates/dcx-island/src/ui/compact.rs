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

/// How the row should be drawn. The same layout runs in both states; only the
/// chrome and the corner buttons change, so the content can grow out of the
/// capsule into the panel's title row without a second code path.
#[derive(Clone, Copy)]
pub struct CapsuleOpts {
    /// Paint the pill itself (shadow, fill, sheen, border). False once the panel
    /// owns the card: a second rounded shape inside the card is what made the
    /// island look like a separate object sitting on top of the panel.
    pub shell: bool,
    /// Horizontal padding of the content row, in points. Interpolated by the
    /// caller because the pill pads by its own height while the panel pads by
    /// its own grid.
    pub pad: f32,
    /// 0..1 — the close button fades in as the panel unfolds.
    pub corner: f32,
}

/// What the capsule wants the app to do after this frame.
#[derive(Default)]
pub struct CapsuleHit {
    pub clicked: bool,
    pub close: bool,
}

pub fn draw(
    ui: &mut egui::Ui,
    rect: Rect,
    st: &CapsuleState,
    p: &Palette,
    opacity: f32,
    time_size: f32,
    opts: CapsuleOpts,
) -> CapsuleHit {
    let mut hit = CapsuleHit::default();
    let radius = rect.height() * 0.5;

    if opts.shell {
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

        // A very light top sheen, the same trick the taskbar pill uses: it makes
        // a translucent capsule read as glass instead of as flat grey.
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
    }

    let pad = opts.pad;
    let mut left = rect.left() + pad;
    let mid_y = rect.center().y;

    // The row's own click area is allocated **before** everything it contains.
    // egui gives the click to the last widget registered in a layer, so a
    // full-row hit area allocated at the end would sit on top of the close and
    // export buttons and eat their clicks. Allocating it first puts it
    // underneath them, where it belongs.
    let row_resp = ui.allocate_rect(rect, egui::Sense::click());

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

    // Close, pinned to the row's right end. Only the close button lives up
    // here: the title row carries the island's own information (clock, date,
    // metrics) and exactly one global action. Everything else — exporting the
    // task list, clearing the clipboard, marking messages read — belongs to a
    // tab and is drawn in that tab's own tool row, so the header never grows a
    // toolbar that means different things on different tabs.
    if opts.corner > 0.01 {
        // Optical size, not tap size. At 12 pt with a 1.5 pt stem the ✕ stood
        // taller than the clock digits and carried more ink than any text in
        // the row, so it read as a caption button borrowed from another window.
        //
        // 8 pt puts the cross at the cap height of the 10 pt metrics beside it
        // (≈10 px at 125 %). A cross has two diagonals, so equal cap height
        // already reads *larger* than a digit; anything above that and it starts
        // competing with the data instead of sitting beside it.
        //
        // The glyph's right edge lands on the row's right inset, mirroring the
        // clock's left inset, so the title row is symmetric. The hit box is
        // larger than the glyph and spills a few points past the row — it is
        // invisible, and the 16 pt row padding keeps it inside the card.
        const GLYPH: f32 = 8.0;
        const HIT: f32 = 22.0;
        let close = Rect::from_center_size(
            Pos2::new(right - GLYPH * 0.5, mid_y),
            Vec2::splat(HIT),
        );
        if corner_button(ui, close, Glyph::Close, p, opts.corner, GLYPH).clicked() {
            hit.close = true;
        }
        right -= GLYPH + 10.0;
    }

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

    // Metrics: a 18 pt bar plus a percentage. Two decimals of hardware would be
    // noise; one number per metric is the point of a capsule.
    //
    // Fitting is decided per metric against the space the clock has already
    // taken, not against a fixed width: a 232 pt capsule used to show **no**
    // metrics at all because of a `>= 250.0` gate, and a capsule the user
    // narrows must lose the bars rather than overlap the time.
    //
    // The names appear only when the row is wide enough to afford them, which is
    // exactly the open panel. Collapsed, "44% 31%" is read at a glance by the
    // person who set the island up and the labels would cost both numbers; open,
    // the same two numbers sit next to a task list with no legend, and two bare
    // percentages of nothing is a genuinely unreadable header.
    let labelled = rect.width() > 330.0;
    let mut metrics: Vec<(f32, Color32, &'static str)> = Vec::new();
    if let Some(cpu) = st.cpu {
        metrics.push((cpu, p.level(crate::metrics::level(cpu)), "CPU"));
    }
    if let Some(mem) = st.mem {
        metrics.push((mem, p.level(crate::metrics::level(mem)), "内存"));
    }
    for (value, color, name) in metrics.iter().rev() {
        let text = format!("{value:.0}%");
        let galley = ui.painter().layout_no_wrap(
            text,
            FontId::monospace(10.0),
            p.text_dim,
        );
        let text_w = galley.size().x;
        let bar_w = 18.0;
        let name_w = if labelled {
            ui.painter()
                .layout_no_wrap(name.to_string(), FontId::proportional(9.0), p.text_faint)
                .size()
                .x
                + 4.0
        } else {
            0.0
        };
        // name + bar + gap + text + the 8 pt gap that separates two clusters.
        let need = name_w + bar_w + 5.0 + text_w + 8.0;
        // The task title keeps its space even when a second metric would fit:
        // dropping it to show a percentage is the wrong trade.
        let reserve = if st.task.is_some() { 70.0 } else { 24.0 };
        if right - need < left + reserve {
            continue;
        }

        if labelled {
            widgets::text(
                ui,
                Pos2::new(right - need + 8.0, mid_y),
                Align2::LEFT_CENTER,
                name,
                9.0,
                p.text_faint,
            );
        }
        let x = right - text_w;
        ui.painter()
            .galley(Pos2::new(x, mid_y - galley.size().y * 0.5), galley, p.text_dim);
        let bar = Rect::from_min_size(
            Pos2::new(x - bar_w - 5.0, mid_y - 1.5),
            Vec2::new(bar_w, 3.0),
        );
        widgets::bar(ui, bar, *value / 100.0, *color, p.track);
        right = bar.left() - name_w - 8.0;
    }

    // Network rates live in the capsule only when there is room left over; the
    // panel's performance tab is the detailed view. What "left over" means is
    // decided by the task title: it is the reason to look at the capsule, so it
    // keeps its space and the network readout is the first thing to drop.
    if let Some((down, up)) = st.net {
        let text = format!(
            "↓{} ↑{}",
            crate::metrics::human_rate(down),
            crate::metrics::human_rate(up)
        );
        let galley =
            ui.painter()
                .layout_no_wrap(text, FontId::monospace(9.0), p.text_faint);
        let w = galley.size().x;
        let reserve = if st.task.is_some() { 70.0 } else { 20.0 };
        if right - w > left + reserve {
            ui.painter().galley(
                Pos2::new(right - w, mid_y - galley.size().y * 0.5),
                galley,
                p.text_faint,
            );
            right -= w + 8.0;
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

    hit.clicked = row_resp.clicked() && !hit.close;
    hit
}

/// Small icon button whose whole glyph — not just its hover plate — fades with
/// `alpha`, so the close button can grow in with the panel instead of popping
/// into place on the frame the unfold ends.
///
/// The glyph size is a parameter because this is the one button that shares a
/// row with text: it has to be measured against the text's optical size, not
/// against the panel's 13 pt icon grid. The hover plate follows the glyph
/// (2×) rather than the hit box, so a small ✕ does not get a bubble around it.
fn corner_button(
    ui: &mut egui::Ui,
    rect: Rect,
    which: Glyph,
    p: &Palette,
    alpha: f32,
    glyph: f32,
) -> egui::Response {
    let resp = ui.allocate_rect(rect, egui::Sense::click());
    if resp.hovered() {
        let plate = Rect::from_center_size(rect.center(), Vec2::splat(glyph * 2.0));
        widgets::round_rect(ui, plate, plate.height() * 0.5, p.accent_soft, Stroke::NONE);
    }
    let base = if resp.hovered() { p.accent } else { p.text_dim };
    widgets::glyph_weighted(ui, rect.center(), glyph, which, with_alpha(base, alpha), 1.25);
    resp
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

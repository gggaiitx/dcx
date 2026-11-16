//! Scrolling stitch engine.
//!
//! Manual scrolling is matched with tolerant row signatures (see
//! `signature.rs`) and an absolute viewport position. The canvas only grows
//! when the viewport leaves it: downward appends rows, upward **prepends**
//! them, and moving back inside already captured content changes nothing. That
//! position model is what makes reverse scrolling work and what prevents the
//! duplicated blocks the previous "smallest shift, append only" engine
//! produced after an up-scroll.
//!
//! Sticky page headers/footers are measured once the page actually moves (the
//! first two frames may be identical while the user waits), kept out of the
//! content stream, and composed once at the top/bottom. A dropped match no
//! longer poisons the chain: the last located frame stays the anchor, and the
//! current frame may also be located directly in the canvas.
//!
//! Pure data in/out: no Win32, no UI.

mod signature;

use std::collections::VecDeque;

use signature::{row_diff, Sigs, ROW_TOL};

pub const MAX_HEIGHT: u32 = 30_000;
/// Four BGRA buffers exist at the clipboard stage: 24 MP ≈ 96 MB each.
pub const MAX_PIXELS: u64 = 24_000_000;
/// Smallest useful selection height (`MIN_CONTENT` + a header and footer).
pub const MIN_SEL_H: u32 = 32;

const MIN_CONTENT: u32 = 16;
/// A static band shorter than this is page background, not a fixed header.
const MIN_FIXED: u32 = 8;
/// Fraction of the overlap that must be row-similar.
const MATCH_RATIO: f32 = 0.90;
/// A first movement smaller than this cannot prove the sticky bands yet.
const SEED_MOVE_MIN: i32 = 8;
/// After this many unmatched frames the provisional seed stops waiting for
/// band evidence and starts behaving like a normal canvas.
const MAX_SEED_MISSES: u32 = 4;
/// A non-zero match only wins if it clearly beats the no-move match.
const ZERO_BIAS: f32 = 0.04;
/// Scores this close are a tie; prefer the offset nearer the prediction.
/// Kept tight so a 1 px shift that scores 0.994 does not beat an exact 1.0.
const TIE_MARGIN: f32 = 0.002;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Appended {
    /// Pixels the page moved: positive = down (new rows appended), negative =
    /// up (new rows prepended).
    pub offset: i32,
    pub added: u32,
    pub total_h: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Appended(Appended),
    /// No vertical movement, or the viewport is back inside captured content.
    Still,
    /// No trustworthy overlap: the page jumped too far or changed completely.
    Failed,
    /// Height or pixel cap reached.
    Limit,
}

struct Segment {
    prepend: bool,
    rows: u32,
}

struct Seed {
    raw: Vec<u8>,
    sigs: Sigs,
}

struct Found {
    offset: i32,
    score: f32,
}

/// One matched viewport position inside the canvas.
pub struct Stitcher {
    w: u32,
    frame_h: u32,
    /// First frame, buffered until the first real scroll proves the bands.
    seed: Option<Seed>,
    /// Sticky bands were measured; content rows exclude them.
    bands_known: bool,
    header: Vec<u8>,
    footer: Vec<u8>,
    header_h: u32,
    footer_h: u32,
    /// Scrollable rows between the fixed bands.
    content: VecDeque<Vec<u8>>,
    /// Row signatures parallel to `content`.
    content_sigs: VecDeque<signature::RowSig>,
    /// Last located frame (full viewport, bands included) and its signatures.
    prev: Vec<u8>,
    prev_sigs: Option<Sigs>,
    /// Absolute canvas-coordinate y of the previous frame's content top.
    prev_pos: i64,
    /// Absolute canvas-coordinate y of `content[0]`.
    origin: i64,
    last_offset: i32,
    seed_misses: u32,
    segments: Vec<Segment>,
}

impl Stitcher {
    pub fn new(frame: &[u8], w: u32, h: u32) -> Option<Stitcher> {
        if w < 2 || h < MIN_CONTENT || frame.len() != (w as usize) * (h as usize) * 4 {
            return None;
        }
        if (w as u64) * (h as u64) > MAX_PIXELS {
            return None;
        }
        let sigs = signature::compute(frame, w, h);
        Some(Stitcher {
            w,
            frame_h: h,
            seed: Some(Seed {
                raw: frame.to_vec(),
                sigs,
            }),
            bands_known: false,
            header: Vec::new(),
            footer: Vec::new(),
            header_h: 0,
            footer_h: 0,
            content: VecDeque::new(),
            content_sigs: VecDeque::new(),
            prev: Vec::new(),
            prev_sigs: None,
            prev_pos: 0,
            origin: 0,
            last_offset: 0,
            seed_misses: 0,
            segments: Vec::new(),
        })
    }

    pub fn height(&self) -> u32 {
        self.header_h + self.content.len() as u32 + self.footer_h
    }

    fn content_h(&self) -> u32 {
        self.frame_h.saturating_sub(self.header_h + self.footer_h)
    }

    pub fn unchanged(&self, frame: &[u8]) -> bool {
        !self.prev.is_empty() && frame == self.prev.as_slice()
    }

    pub fn can_undo(&self) -> bool {
        !self.segments.is_empty()
    }

    /// Composed image: fixed header + content + fixed footer.
    pub fn image(&self) -> (Vec<u8>, u32, u32) {
        let stride = self.w as usize * 4;
        let mut out = Vec::with_capacity(stride * self.height() as usize);
        out.extend_from_slice(&self.header);
        for row in &self.content {
            out.extend_from_slice(row);
        }
        out.extend_from_slice(&self.footer);
        (out, self.w, self.height())
    }

    /// Downsampled whole-image view for the preview card.
    pub fn thumbnail(&self, max_w: u32, max_h: u32) -> Option<(Vec<u8>, u32, u32)> {
        let (sw, sh) = (self.w, self.height());
        if sw == 0 || sh == 0 || max_w == 0 || max_h == 0 {
            return None;
        }
        let scale = (max_w as f64 / sw as f64).min(max_h as f64 / sh as f64);
        let tw = ((sw as f64 * scale) as u32).clamp(1, max_w);
        let th = ((sh as f64 * scale) as u32).clamp(1, max_h);
        let mut out = vec![0u8; (tw as usize) * (th as usize) * 4];
        for ty in 0..th {
            let sy = (ty as u64 * sh as u64 / th as u64) as u32;
            let Some(row) = self.row_at(sy) else { continue };
            let drow = ty as usize * tw as usize * 4;
            for tx in 0..tw {
                let sx = (tx as u64 * sw as u64 / tw as u64) as usize * 4;
                if sx + 4 <= row.len() {
                    let di = drow + tx as usize * 4;
                    out[di..di + 4].copy_from_slice(&row[sx..sx + 4]);
                }
            }
        }
        Some((out, tw, th))
    }

    fn row_at(&self, y: u32) -> Option<&[u8]> {
        let stride = self.w as usize * 4;
        if y < self.header_h {
            let o = y as usize * stride;
            return self.header.get(o..o + stride);
        }
        let y = y - self.header_h;
        if let Some(row) = self.content.get(y as usize) {
            return Some(row.as_slice());
        }
        let y = y - self.content.len() as u32;
        if y < self.footer_h {
            let o = y as usize * stride;
            return self.footer.get(o..o + stride);
        }
        None
    }

    /// Finish a capture that never received a second frame (the user pressed
    /// Enter right after starting): the seed becomes the whole image.
    pub fn settle(&mut self) {
        if let Some(seed) = self.seed.take() {
            self.adopt_whole(seed);
            self.bands_known = true;
        }
    }

    /// Feed one live frame. `frame` must match the seed size.
    pub fn push(&mut self, frame: &[u8], w: u32, h: u32) -> Step {
        if w != self.w || h != self.frame_h || frame.len() != (w as usize) * (h as usize) * 4 {
            return Step::Failed;
        }
        let sigs = signature::compute(frame, w, h);

        if let Some(seed) = self.seed.take() {
            if seed.raw == frame {
                self.adopt_whole(seed);
                return Step::Still;
            }
            // Sticky bands have to be known before the first locate: including
            // a 20 px page header in a 160 px overlap can pull the true offset
            // below the match ratio.
            let (mut top, mut bottom) = fixed_regions(&seed.raw, frame, w, h);
            let mut found = locate(&seed.sigs, &sigs, h, top, bottom, 0);
            if found.is_none() && (top != 0 || bottom != 0) {
                // The detected bands may belong to an animated edge. Retrying
                // the whole frame is safer than discarding the movement.
                top = 0;
                bottom = 0;
                found = locate(&seed.sigs, &sigs, h, 0, 0, 0);
            }
            return match found {
                Some(m) if m.offset.unsigned_abs() >= SEED_MOVE_MIN as u32 => {
                    self.adopt_with_bands(seed, top, bottom);
                    self.push_inner(frame, &sigs, Some(m.offset))
                }
                Some(_) => {
                    // The page has not really moved yet. Show the seed, keep
                    // waiting for evidence of the sticky bands.
                    self.adopt_whole(seed);
                    Step::Still
                }
                None => {
                    self.adopt_whole(seed);
                    Step::Failed
                }
            };
        }

        self.push_inner(frame, &sigs, None)
    }

    fn push_inner(&mut self, frame: &[u8], sigs: &Sigs, forced: Option<i32>) -> Step {
        if !self.bands_known {
            // Provisional seed: keep comparing against the still frame until a
            // real movement can prove which bands stay put.
            let (mut top, mut bottom) = fixed_regions(&self.prev, frame, self.w, self.frame_h);
            let mut found = locate(
                self.prev_sigs.as_ref().expect("provisional"),
                sigs,
                self.frame_h,
                top,
                bottom,
                0,
            );
            if found.is_none() && (top != 0 || bottom != 0) {
                top = 0;
                bottom = 0;
                found = locate(
                    self.prev_sigs.as_ref().expect("provisional"),
                    sigs,
                    self.frame_h,
                    0,
                    0,
                    0,
                );
            }
            if let Some(m) = found {
                if m.offset.unsigned_abs() >= SEED_MOVE_MIN as u32 {
                    self.rebuild_with_bands(top, bottom);
                    return self.push_inner(frame, sigs, Some(m.offset));
                }
                return Step::Still;
            }
            self.seed_misses += 1;
            if self.seed_misses >= MAX_SEED_MISSES {
                self.bands_known = true;
                self.seed_misses = 0;
                return self.push_inner(frame, sigs, None);
            }
            return Step::Failed;
        }

        if self.prev.is_empty() || self.prev_sigs.is_none() {
            return Step::Failed;
        }

        let found = match forced {
            Some(offset) => Some(Found { offset, score: 1.0 }),
            None => locate(
                self.prev_sigs.as_ref().expect("checked above"),
                sigs,
                self.frame_h,
                self.header_h,
                self.footer_h,
                self.last_offset,
            ),
        };

        if let Some(m) = found {
            let new_pos = self.prev_pos + m.offset as i64;
            return self.apply_position(frame, sigs, new_pos, m.offset);
        }

        // The previous frame may be stale (a repaint, a dropped grab). The
        // current frame can still overlap the canvas directly.
        if let Some((pos, _score)) = locate_in_canvas(
            &self.content_sigs,
            sigs,
            self.header_h,
            self.content_h(),
            self.origin,
            self.content.len() as u32,
            self.prev_pos,
        ) {
            let offset = (pos - self.prev_pos).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
            return self.apply_position(frame, sigs, pos, offset);
        }

        Step::Failed
    }

    /// Apply a located absolute viewport position: grow at whichever canvas
    /// edge the viewport has left, update the anchor, and report the result.
    fn apply_position(&mut self, frame: &[u8], sigs: &Sigs, new_pos: i64, offset: i32) -> Step {
        let content_h = self.content_h();
        let start = self.origin;
        let end = self.origin + self.content.len() as i64;
        let frame_end = new_pos + content_h as i64;

        // The viewport left the canvas entirely (the page scrolled past the
        // capped height, or jumped back above the top). Appending or
        // prepending here would place rows at the wrong coordinate; keep the
        // position anchor and wait until the viewport overlaps the canvas
        // again.
        if frame_end <= start || new_pos >= end {
            self.remember(frame, sigs, new_pos, offset);
            return Step::Still;
        }

        let mut prepend = 0u32;
        if new_pos < start {
            prepend = ((start - new_pos) as u64).min(content_h as u64) as u32;
        }
        let mut append = 0u32;
        if frame_end > end {
            let skip = (end - new_pos).max(0) as u64;
            append = content_h.saturating_sub(skip.min(content_h as u64) as u32);
        }
        debug_assert!(
            prepend == 0 || append == 0,
            "canvas cannot be shorter than one viewport"
        );

        let added = prepend + append;
        if added > 0 {
            let total = self.header_h as u64
                + self.content.len() as u64
                + added as u64
                + self.footer_h as u64;
            if total > MAX_HEIGHT as u64 || (self.w as u64) * total > MAX_PIXELS {
                self.remember(frame, sigs, new_pos, offset);
                return Step::Limit;
            }
        }

        let stride = self.w as usize * 4;
        if prepend > 0 {
            let base = (self.header_h) as usize * stride;
            for i in (0..prepend as usize).rev() {
                let o = base + i * stride;
                self.content.push_front(frame[o..o + stride].to_vec());
                self.content_sigs
                    .push_front(sigs.rows[self.header_h as usize + i]);
            }
            self.origin -= prepend as i64;
            self.segments.push(Segment {
                prepend: true,
                rows: prepend,
            });
        }
        if append > 0 {
            let start_content = content_h - append;
            let base = (self.header_h + start_content) as usize * stride;
            for i in 0..append as usize {
                let o = base + i * stride;
                self.content.push_back(frame[o..o + stride].to_vec());
                self.content_sigs
                    .push_back(sigs.rows[self.header_h as usize + start_content as usize + i]);
            }
            self.segments.push(Segment {
                prepend: false,
                rows: append,
            });
        }

        self.remember(frame, sigs, new_pos, offset);
        if added > 0 {
            Step::Appended(Appended {
                offset,
                added,
                total_h: self.height(),
            })
        } else {
            Step::Still
        }
    }

    fn remember(&mut self, frame: &[u8], sigs: &Sigs, new_pos: i64, offset: i32) {
        self.prev.clear();
        self.prev.extend_from_slice(frame);
        self.prev_sigs = Some(sigs.clone());
        self.prev_pos = new_pos;
        self.last_offset = offset;
    }

    /// The first real movement: measure the sticky bands on the seed, then
    /// rebuild the canvas from its content area.
    fn adopt_with_bands(&mut self, seed: Seed, top: u32, bottom: u32) {
        self.prev = seed.raw;
        self.prev_sigs = Some(seed.sigs);
        self.rebuild_with_bands(top, bottom);
    }

    /// Seed without band evidence (still page): the whole frame is content.
    fn adopt_whole(&mut self, seed: Seed) {
        let stride = self.w as usize * 4;
        self.header.clear();
        self.header_h = 0;
        self.footer.clear();
        self.footer_h = 0;
        self.content.clear();
        self.content_sigs.clear();
        for y in 0..self.frame_h as usize {
            let o = y * stride;
            self.content.push_back(seed.raw[o..o + stride].to_vec());
            self.content_sigs.push_back(seed.sigs.rows[y]);
        }
        self.prev = seed.raw;
        self.prev_sigs = Some(seed.sigs);
        self.prev_pos = 0;
        self.origin = 0;
        self.last_offset = 0;
        self.segments.clear();
        self.seed_misses = 0;
    }

    /// Rebuild `header`/`content`/`footer` from the current anchor frame. The
    /// anchor must be the seed at canvas position 0.
    fn rebuild_with_bands(&mut self, top: u32, bottom: u32) {
        let h = self.frame_h;
        let top = top.min(h);
        let bottom = bottom.min(h - top);
        let stride = self.w as usize * 4;
        self.header.clear();
        self.header
            .extend_from_slice(&self.prev[..top as usize * stride]);
        self.header_h = top;
        self.footer.clear();
        if bottom > 0 {
            self.footer
                .extend_from_slice(&self.prev[(h - bottom) as usize * stride..]);
        }
        self.footer_h = bottom;
        self.content.clear();
        self.content_sigs.clear();
        let sigs = self.prev_sigs.as_ref().expect("anchor signatures");
        for y in top..h - bottom {
            let o = y as usize * stride;
            self.content.push_back(self.prev[o..o + stride].to_vec());
            self.content_sigs.push_back(sigs.rows[y as usize]);
        }
        self.origin = 0;
        self.prev_pos = 0;
        self.last_offset = 0;
        self.segments.clear();
        self.bands_known = true;
        self.seed_misses = 0;
    }

    /// Drop the last appended/prepended segment and rebuild the anchor at that
    /// edge so a re-scroll of the same rows stitches cleanly.
    pub fn undo_last(&mut self) -> bool {
        let Some(seg) = self.segments.pop() else {
            return false;
        };
        if seg.prepend {
            for _ in 0..seg.rows {
                self.content.pop_front();
                self.content_sigs.pop_front();
            }
            self.origin += seg.rows as i64;
        } else {
            for _ in 0..seg.rows {
                self.content.pop_back();
                self.content_sigs.pop_back();
            }
        }
        self.rebuild_prev(seg.prepend);
        true
    }

    fn rebuild_prev(&mut self, at_top: bool) {
        let content_h = self.content_h() as usize;
        let len = self.content.len();
        if len < content_h {
            self.prev.clear();
            self.prev_sigs = None;
            return;
        }
        let start = if at_top { 0 } else { len - content_h };
        let stride = self.w as usize * 4;
        let mut frame = Vec::with_capacity(stride * self.frame_h as usize);
        frame.extend_from_slice(&self.header);
        for row in self.content.iter().skip(start).take(content_h) {
            frame.extend_from_slice(row);
        }
        frame.extend_from_slice(&self.footer);
        self.prev = frame;
        self.prev_sigs = Some(signature::compute(&self.prev, self.w, self.frame_h));
        self.prev_pos = if at_top {
            self.origin
        } else {
            self.origin + (len - content_h) as i64
        };
        self.last_offset = 0;
    }
}

/// Find the smallest shift whose overlap is convincingly row-similar.
fn locate(
    prev: &Sigs,
    cur: &Sigs,
    h: u32,
    fixed_top: u32,
    fixed_bottom: u32,
    prediction: i32,
) -> Option<Found> {
    let ct = fixed_top.min(h.saturating_sub(1));
    let cb = fixed_bottom.min(h.saturating_sub(ct + 1));
    let content_h = h - ct - cb;
    if content_h < MIN_CONTENT {
        return None;
    }
    let min_overlap = (content_h / 8).max(16).min(content_h - 1);
    let max_shift = content_h - min_overlap;
    if max_shift == 0 {
        return None;
    }

    let score_at = |d: i32, step: u32| -> Option<f32> {
        let ad = d.unsigned_abs();
        if ad >= content_h {
            return None;
        }
        let overlap = content_h - ad;
        let (po, co) = if d >= 0 { (ad, 0) } else { (0, ad) };
        let base_p = (ct + po) as usize;
        let base_c = (ct + co) as usize;
        let total = overlap.div_ceil(step);
        let mut matched = 0u32;
        let mut sampled = 0u32;
        let mut i = 0u32;
        while i < overlap {
            let a = &prev.rows[base_p + i as usize];
            let b = &cur.rows[base_c + i as usize];
            if row_matches(prev, base_p + i as usize, a, b) {
                matched += 1;
            }
            sampled += 1;
            let remaining = total - sampled;
            if (matched + remaining) as f32 / (total as f32) < MATCH_RATIO {
                return None;
            }
            i += step;
        }
        let tex = cur.tex_prefix[base_c + overlap as usize] - cur.tex_prefix[base_c];
        if tex == 0 {
            return None;
        }
        Some(matched as f32 / sampled as f32)
    };

    let zero = score_at(0, 1);
    if let Some(z) = zero {
        if z >= 0.999 {
            return Some(Found {
                offset: 0,
                score: z,
            });
        }
    }
    let max = max_shift as i32;
    let pred = prediction.clamp(-max, max);
    let mut best: Option<Found> = None;

    // The last movement is the strongest prior. A perfect score there means
    // the page kept moving at the same speed; skip the full-range scan.
    if pred != 0 {
        if let Some(score) = score_at(pred, 2) {
            if score >= 0.995 && zero.is_none_or(|z| z < MATCH_RATIO) {
                return Some(Found {
                    offset: pred,
                    score,
                });
            }
            consider(&mut best, pred, score, pred);
        }
    }
    for dist in 1..=48i32 {
        for d in [pred - dist, pred + dist] {
            if d == 0 || d < -max || d > max {
                continue;
            }
            if let Some(score) = score_at(d, 2) {
                consider(&mut best, d, score, pred);
            }
        }
    }

    // Coarse pass covers every offset. A step-3 grid looked cheaper but high
    // frequency content scores zero one pixel away, so a true offset between
    // grid points was never seen. Row sampling plus early pruning keeps this
    // linear enough for a 60 ms capture cadence.
    let raw_at = |d: i32, step: u32| -> Option<f32> {
        let ad = d.unsigned_abs();
        if ad >= content_h {
            return None;
        }
        let overlap = content_h - ad;
        let (po, co) = if d >= 0 { (ad, 0) } else { (0, ad) };
        let base_p = (ct + po) as usize;
        let base_c = (ct + co) as usize;
        let total = overlap.div_ceil(step);
        let mut matched = 0u32;
        let mut sampled = 0u32;
        let mut i = 0u32;
        while i < overlap {
            let a = &prev.rows[base_p + i as usize];
            let b = &cur.rows[base_c + i as usize];
            if row_matches(prev, base_p + i as usize, a, b) {
                matched += 1;
            }
            sampled += 1;
            let remaining = total - sampled;
            if (matched + remaining) as f32 / (total as f32) < 0.30 {
                return None;
            }
            i += step;
        }
        Some(matched as f32 / sampled as f32)
    };

    let mut basins: Vec<Found> = Vec::new();
    for d in -max..=max {
        if (d - pred).abs() <= 48 {
            continue;
        }
        let Some(score) = raw_at(d, 4) else { continue };
        if score < 0.30 {
            continue;
        }
        if let Some(b) = basins.iter_mut().find(|b| (b.offset - d).abs() <= 3) {
            if score > b.score {
                b.offset = d;
                b.score = score;
            }
        } else {
            basins.push(Found { offset: d, score });
            if basins.len() > 5 {
                let (worst, _) = basins
                    .iter()
                    .enumerate()
                    .min_by(|a, b| a.1.score.total_cmp(&b.1.score))
                    .expect("non-empty");
                basins.swap_remove(worst);
            }
        }
    }
    for b in basins {
        for d in (b.offset - 2).max(-max)..=(b.offset + 2).min(max) {
            if let Some(score) = score_at(d, 1) {
                consider(&mut best, d, score, pred);
            }
        }
    }

    match (zero, best) {
        (Some(z), Some(b)) if b.offset != 0 && b.score - z < ZERO_BIAS => Some(Found {
            offset: 0,
            score: z,
        }),
        (_, Some(b)) => Some(b),
        (Some(z), None) => Some(Found {
            offset: 0,
            score: z,
        }),
        (None, None) => None,
    }
}

fn consider(best: &mut Option<Found>, d: i32, score: f32, pred: i32) {
    let better = match best {
        None => true,
        Some(b) => {
            if score > b.score + TIE_MARGIN {
                true
            } else if score + TIE_MARGIN >= b.score {
                let dist = (d - pred).abs();
                let prev_dist = (b.offset - pred).abs();
                dist < prev_dist || (dist == prev_dist && d.abs() < b.offset.abs())
            } else {
                false
            }
        }
    };
    if better {
        *best = Some(Found { offset: d, score });
    }
}

/// Row equality with a half-pixel escape hatch: a frame captured between two
/// integer scroll positions blends two adjacent source rows. Comparing against
/// the blend in either direction keeps such frames usable.
fn row_matches(prev: &Sigs, index: usize, a: &signature::RowSig, b: &signature::RowSig) -> bool {
    if row_diff(a, b) <= ROW_TOL {
        return true;
    }
    if index > 0 {
        let up = blend(&prev.rows[index - 1], a);
        if row_diff(&up, b) <= ROW_TOL {
            return true;
        }
    }
    if let Some(next) = prev.rows.get(index + 1) {
        let down = blend(next, a);
        if row_diff(&down, b) <= ROW_TOL {
            return true;
        }
    }
    false
}

/// Same half-pixel tolerance for canvas rows.
fn canvas_row_matches(
    canvas: &VecDeque<signature::RowSig>,
    index: usize,
    b: &signature::RowSig,
) -> bool {
    let Some(a) = canvas.get(index) else {
        return false;
    };
    if row_diff(a, b) <= ROW_TOL {
        return true;
    }
    if index > 0 {
        if let Some(up) = canvas.get(index - 1) {
            if row_diff(&blend(up, a), b) <= ROW_TOL {
                return true;
            }
        }
    }
    if let Some(next) = canvas.get(index + 1) {
        if row_diff(&blend(next, a), b) <= ROW_TOL {
            return true;
        }
    }
    false
}

fn blend(a: &signature::RowSig, b: &signature::RowSig) -> signature::RowSig {
    let mut out = [0u8; signature::SAMPLES];
    for i in 0..signature::SAMPLES {
        out[i] = ((a[i] as u16 + b[i] as u16) / 2) as u8;
    }
    out
}

/// Locate the current frame directly in the canvas. Used when the previous
/// frame is stale or was never located; the overlap must still be convincing.
fn locate_in_canvas(
    canvas: &VecDeque<signature::RowSig>,
    cur: &Sigs,
    ct: u32,
    content_h: u32,
    origin: i64,
    len: u32,
    prediction: i64,
) -> Option<(i64, f32)> {
    if len < content_h || content_h < MIN_CONTENT {
        return None;
    }
    let min_overlap = (content_h / 8).max(16).min(content_h - 1);
    let lo = origin - content_h as i64 + min_overlap as i64;
    let hi = origin + len as i64 - min_overlap as i64;
    if lo > hi {
        return None;
    }

    let score_at = |p: i64, step: u32| -> Option<f32> {
        let start = p.max(origin);
        let end = (p + content_h as i64).min(origin + len as i64);
        let overlap = (end - start) as u32;
        if overlap < min_overlap {
            return None;
        }
        let foff = (start - p) as u32;
        let coff = (start - origin) as u32;
        let total = overlap.div_ceil(step);
        let mut matched = 0u32;
        let mut sampled = 0u32;
        let mut i = 0u32;
        while i < overlap {
            let b = &cur.rows[(ct + foff + i) as usize];
            if canvas_row_matches(canvas, (coff + i) as usize, b) {
                matched += 1;
            }
            sampled += 1;
            let remaining = total - sampled;
            if (matched + remaining) as f32 / (total as f32) < MATCH_RATIO {
                return None;
            }
            i += step;
        }
        let tex =
            cur.tex_prefix[(ct + foff + overlap) as usize] - cur.tex_prefix[(ct + foff) as usize];
        if tex == 0 {
            return None;
        }
        Some(matched as f32 / sampled as f32)
    };

    let pred = prediction.clamp(lo, hi);
    let mut best: Option<(i64, f32)> = None;
    let consider = |best: &mut Option<(i64, f32)>, p: i64, score: f32| {
        let better = match best {
            None => true,
            Some((bp, bs)) => {
                if score > *bs + TIE_MARGIN {
                    true
                } else if score + TIE_MARGIN >= *bs {
                    let dist = (p - pred).abs();
                    let prev_dist = (*bp - pred).abs();
                    dist < prev_dist || (dist == prev_dist && p.abs() < bp.abs())
                } else {
                    false
                }
            }
        };
        if better {
            *best = Some((p, score));
        }
    };

    if let Some(score) = score_at(pred, 2) {
        consider(&mut best, pred, score);
    }
    // High-frequency content scores zero two pixels off, so a coarse grid can
    // miss the real position. Scan every pixel, but only in the windows where
    // a recoverable frame can be: around the stale anchor and at both edges.
    let span = content_h as i64;
    let mut ranges = [
        ((pred - span).max(lo), (pred + span).min(hi)),
        (lo, (lo + span).min(hi)),
        ((hi - span).max(lo), hi),
    ];
    ranges.sort();
    let mut scanned_to = lo - 1;
    for (a, b) in ranges {
        let a = a.max(scanned_to + 1);
        if a > b {
            continue;
        }
        for p in a..=b {
            if let Some(score) = score_at(p, 4) {
                consider(&mut best, p, score);
            }
        }
        scanned_to = scanned_to.max(b);
    }
    if let Some((center, _)) = best {
        for p in (center - 2).max(lo)..=(center + 2).min(hi) {
            if let Some(score) = score_at(p, 1) {
                consider(&mut best, p, score);
            }
        }
    }
    best
}

/// Bands at the top/bottom that are pixel-identical between the seed and the
/// first moved frame: sticky site headers, toolbars, chat input boxes.
///
/// Exact rows deliberately: tolerant comparison would misread repetitive text
/// scrolled by exactly one line as a huge fixed header.
fn fixed_regions(seed: &[u8], cur: &[u8], w: u32, h: u32) -> (u32, u32) {
    let stride = w as usize * 4;
    let same = |y: u32| {
        let o = y as usize * stride;
        seed.get(o..o + stride) == cur.get(o..o + stride)
    };
    let mut top = 0u32;
    while top < h && same(top) {
        top += 1;
    }
    let mut bottom = 0u32;
    while bottom < h && same(h - 1 - bottom) {
        bottom += 1;
    }

    let cap = h / 2;
    top = top.min(cap);
    bottom = bottom.min(cap);
    if top + bottom + MIN_CONTENT > h {
        let excess = top + bottom + MIN_CONTENT - h;
        if bottom >= top {
            bottom = bottom.saturating_sub(excess);
        } else {
            top = top.saturating_sub(excess);
        }
    }
    if top + bottom + MIN_CONTENT > h {
        bottom = (h.saturating_sub(top + MIN_CONTENT)).min(bottom);
    }
    if top < MIN_FIXED {
        top = 0;
    }
    if bottom < MIN_FIXED {
        bottom = 0;
    }
    if top + bottom + MIN_CONTENT > h {
        top = 0;
        bottom = 0;
    }
    (top, bottom)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page where every pixel identifies its absolute row/column.
    fn page(w: u32, h: u32) -> Vec<u8> {
        let mut out = vec![0u8; (w as usize) * (h as usize) * 4];
        for y in 0..h {
            for x in 0..w {
                let off = ((y as usize) * (w as usize) + x as usize) * 4;
                let hash = x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663);
                out[off] = (hash & 0xff) as u8;
                out[off + 1] = ((hash >> 8) & 0xff) as u8;
                out[off + 2] = ((hash >> 16) & 0xff) as u8;
                out[off + 3] = 255;
            }
        }
        out
    }

    fn frame(tall: &[u8], w: u32, y: u32, h: u32) -> Vec<u8> {
        let stride = (w as usize) * 4;
        let start = (y as usize) * stride;
        tall[start..start + (h as usize) * stride].to_vec()
    }

    fn seed(tall: &[u8], w: u32, h: u32) -> Stitcher {
        Stitcher::new(&frame(tall, w, 0, h), w, h).expect("stitcher")
    }

    #[test]
    fn stitches_three_scrolls() {
        let (w, h) = (80u32, 200u32);
        let tall = page(w, 900);
        let mut s = seed(&tall, w, h);
        for y in [80u32, 160, 240] {
            let step = s.push(&frame(&tall, w, y, h), w, h);
            assert_eq!(
                step,
                Step::Appended(Appended {
                    offset: 80,
                    added: 80,
                    total_h: h + y,
                }),
                "scroll to {y}"
            );
        }
        assert_eq!(s.height(), 200 + 240);
        let (img, iw, ih) = s.image();
        assert_eq!((iw, ih), (w, 440));
        assert_eq!(img, tall[..(w as usize) * 440 * 4]);
    }

    /// Text-like page: every line has the same body plus a distinct line
    /// number. A single-strip matcher finds the wrong strip here; full-overlap
    /// verification finds the true offset.
    fn text_page(w: u32, h: u32) -> Vec<u8> {
        let mut out = vec![255u8; (w as usize) * (h as usize) * 4];
        let line = 20u32;
        for n in 0..(h / line) {
            let y0 = n * line;
            for y in (y0 + 4)..(y0 + 15) {
                for x in 50..(w - 6) {
                    let on = ((x / 5 + y / 3) % 3) == 0 || ((x + 2 * y) % 17) < 2;
                    if on {
                        let off = ((y as usize) * (w as usize) + x as usize) * 4;
                        out[off] = 20;
                        out[off + 1] = 20;
                        out[off + 2] = 20;
                        out[off + 3] = 255;
                    }
                }
            }
            let bits = n % 16;
            for bit in 0..4 {
                if (bits >> bit) & 1 == 1 {
                    for y in (y0 + 4)..(y0 + 11) {
                        for x in (6 + bit * 6)..(10 + bit * 6) {
                            let off = ((y as usize) * (w as usize) + x as usize) * 4;
                            out[off] = 0;
                            out[off + 1] = 0;
                            out[off + 2] = 0;
                            out[off + 3] = 255;
                        }
                    }
                }
            }
        }
        out
    }

    #[test]
    fn stitches_repeating_text_lines() {
        let (w, h) = (120u32, 200u32);
        let tall = text_page(w, 700);
        let mut s = seed(&tall, w, h);
        assert_eq!(
            s.push(&frame(&tall, w, 60, h), w, h),
            Step::Appended(Appended {
                offset: 60,
                added: 60,
                total_h: 260,
            })
        );
        let (img, _, _) = s.image();
        assert_eq!(img, &tall[..(w as usize) * 260 * 4]);
    }

    /// Real-world shape: 980 px-wide text box, three lines of wheel scroll.
    #[test]
    fn finds_a_three_line_scroll_on_a_wide_text_page() {
        let (w, h) = (980u32, 460u32);
        let tall = text_page(w, 2000);
        let mut s = seed(&tall, w, h);
        assert_eq!(
            s.push(&frame(&tall, w, 60, h), w, h),
            Step::Appended(Appended {
                offset: 60,
                added: 60,
                total_h: 520,
            })
        );
        assert_eq!(s.height(), 520);
    }

    /// The page scrollbar moves with the page; the side margins keep it out
    /// of the signature, so the true offset still wins.
    #[test]
    fn stitches_with_a_moving_scrollbar() {
        let (w, h) = (400u32, 460u32);
        let text_w = w - 18;
        let tall = text_page(text_w, 3000);
        let with_bar = |y: u32, thumb: u32| {
            let win = frame(&tall, text_w, y, h);
            let mut out = vec![0u8; (w as usize) * (h as usize) * 4];
            for row in 0..h as usize {
                let src = row * text_w as usize * 4;
                let dst = row * w as usize * 4;
                out[dst..dst + text_w as usize * 4]
                    .copy_from_slice(&win[src..src + text_w as usize * 4]);
                for x in text_w..w {
                    let o = dst + x as usize * 4;
                    let v = if row as u32 >= thumb && (row as u32) < thumb + 30 {
                        90
                    } else {
                        238
                    };
                    out[o] = v;
                    out[o + 1] = v;
                    out[o + 2] = v;
                    out[o + 3] = 255;
                }
            }
            out
        };
        let mut s = Stitcher::new(&with_bar(0, 200), w, h).expect("stitcher");
        let step = s.push(&with_bar(60, 212), w, h);
        assert_eq!(
            step,
            Step::Appended(Appended {
                offset: 60,
                added: 60,
                total_h: 520,
            })
        );
    }

    /// Sticky header/footer: scrolling must not duplicate them.
    #[test]
    fn keeps_fixed_header_and_footer_out_of_the_content() {
        let (w, h) = (64u32, 200u32);
        let hdr = 20u32;
        let ftr = 16u32;
        let content = 600u32;
        let tall = page(w, hdr + content + ftr);
        let view = |y0: u32| {
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            out.extend_from_slice(&frame(&tall, w, 0, hdr));
            out.extend_from_slice(&frame(&tall, w, hdr + y0, h - hdr - ftr));
            out.extend_from_slice(&frame(&tall, w, hdr + content, ftr));
            out
        };
        let mut s = Stitcher::new(&view(0), w, h).expect("stitcher");
        let step = s.push(&view(40), w, h);
        assert_eq!(
            step,
            Step::Appended(Appended {
                offset: 40,
                added: 40,
                total_h: h + 40,
            })
        );
        let (img, _, ih) = s.image();
        assert_eq!(ih, h + 40);
        assert_eq!(
            &img[..(w * hdr * 4) as usize],
            &tall[..(w * hdr * 4) as usize]
        );
        let foot = (w * ftr * 4) as usize;
        let img_bottom = &img[img.len() - foot..];
        let page_bottom = &tall[tall.len() - foot..];
        assert_eq!(img_bottom, page_bottom);
    }

    /// The first two frames are often identical while the user waits. Bands
    /// must be measured on the first real movement, not misread as "no fixed
    /// bands" from the still pair.
    #[test]
    fn delayed_band_detection_after_a_still_first_frame() {
        let (w, h) = (64u32, 200u32);
        let hdr = 20u32;
        let ftr = 16u32;
        let content = 600u32;
        let tall = page(w, hdr + content + ftr);
        let view = |y0: u32| {
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            out.extend_from_slice(&frame(&tall, w, 0, hdr));
            out.extend_from_slice(&frame(&tall, w, hdr + y0, h - hdr - ftr));
            out.extend_from_slice(&frame(&tall, w, hdr + content, ftr));
            out
        };
        let mut s = Stitcher::new(&view(0), w, h).expect("stitcher");
        assert_eq!(s.push(&view(0), w, h), Step::Still);
        assert_eq!(s.height(), h);
        let step = s.push(&view(40), w, h);
        assert_eq!(
            step,
            Step::Appended(Appended {
                offset: 40,
                added: 40,
                total_h: h + 40,
            })
        );
        let (img, _, ih) = s.image();
        assert_eq!(ih, h + 40);
        assert_eq!(
            &img[..(w * hdr * 4) as usize],
            &tall[..(w * hdr * 4) as usize]
        );
        assert_eq!(
            &img[img.len() - (w * ftr * 4) as usize..],
            &tall[tall.len() - (w * ftr * 4) as usize..]
        );
    }

    /// Scrolling upwards past the seed prepends the newly revealed rows
    /// instead of discarding them.
    #[test]
    fn prepends_when_scrolled_above_the_seed() {
        let (w, h) = (80u32, 200u32);
        let tall = page(w, 900);
        let mut s = Stitcher::new(&frame(&tall, w, 160, h), w, h).expect("stitcher");
        let step = s.push(&frame(&tall, w, 120, h), w, h);
        assert_eq!(
            step,
            Step::Appended(Appended {
                offset: -40,
                added: 40,
                total_h: 240,
            })
        );
        let (img, _, ih) = s.image();
        assert_eq!(ih, 240);
        assert_eq!(img, tall[(w as usize) * 120 * 4..(w as usize) * 360 * 4]);
    }

    /// Down → up → down again must not append the same rows twice.
    #[test]
    fn scrolling_back_and_forth_does_not_duplicate() {
        let (w, h) = (80u32, 200u32);
        let tall = page(w, 900);
        let mut s = Stitcher::new(&frame(&tall, w, 100, h), w, h).expect("stitcher");
        assert_eq!(
            s.push(&frame(&tall, w, 180, h), w, h),
            Step::Appended(Appended {
                offset: 80,
                added: 80,
                total_h: 280,
            })
        );
        assert_eq!(s.push(&frame(&tall, w, 140, h), w, h), Step::Still);
        assert_eq!(s.push(&frame(&tall, w, 180, h), w, h), Step::Still);
        assert_eq!(s.height(), 280);
        assert_eq!(
            s.push(&frame(&tall, w, 260, h), w, h),
            Step::Appended(Appended {
                offset: 80,
                added: 80,
                total_h: 360,
            })
        );
        let (img, _, ih) = s.image();
        assert_eq!(ih, 360);
        assert_eq!(img, tall[(w as usize) * 100 * 4..(w as usize) * 460 * 4]);
    }

    /// Re-rendering between two captures changes every pixel by a few levels;
    /// tolerant signatures must keep stitching instead of failing the frame.
    #[test]
    fn tolerates_brightness_drift() {
        let (w, h) = (80u32, 200u32);
        let tall = page(w, 900);
        let mut s = seed(&tall, w, h);
        let mut shifted = frame(&tall, w, 80, h);
        for px in shifted.as_chunks_mut::<4>().0 {
            px[0] = px[0].saturating_add(5);
            px[1] = px[1].saturating_add(5);
            px[2] = px[2].saturating_add(5);
        }
        assert_eq!(
            s.push(&shifted, w, h),
            Step::Appended(Appended {
                offset: 80,
                added: 80,
                total_h: 280,
            })
        );
    }

    /// A small animated region (video, ad, lazy image) must not abort the
    /// stitch as long as the rest of the viewport proves the offset.
    #[test]
    fn ignores_an_animated_band() {
        let (w, h) = (80u32, 200u32);
        let tall = page(w, 900);
        let mut s = seed(&tall, w, h);
        let mut shifted = frame(&tall, w, 80, h);
        // A small animated band (video, ad, lazy image) changes between grabs.
        for y in 90..98 {
            for x in 0..w {
                let o = ((y * w + x) * 4) as usize;
                shifted[o] = (x * 7) as u8;
                shifted[o + 1] = (y * 13) as u8;
                shifted[o + 2] = 60;
            }
        }
        assert_eq!(
            s.push(&shifted, w, h),
            Step::Appended(Appended {
                offset: 80,
                added: 80,
                total_h: 280,
            })
        );
    }

    /// A half-pixel scroll position (smooth scrolling at 125 % DPI) blends
    /// neighbouring source rows. It must still be located and appended.
    #[test]
    fn matches_half_pixel_scroll() {
        let (w, h) = (80u32, 200u32);
        let tall = page(w, 900);
        let mut s = seed(&tall, w, h);
        let shifted = blend_frame(&tall, w, 80, h);
        let step = s.push(&shifted, w, h);
        match step {
            Step::Appended(a) => {
                assert!(a.offset == 80 || a.offset == 81, "offset {}", a.offset);
                assert_eq!(a.added, a.offset as u32);
                assert_eq!(s.height(), h + a.added);
            }
            other => panic!("expected append, got {other:?}"),
        }
    }

    fn blend_frame(tall: &[u8], w: u32, y: u32, h: u32) -> Vec<u8> {
        let stride = (w as usize) * 4;
        let mut out = vec![0u8; stride * h as usize];
        for row in 0..h as usize {
            let a = (y as usize + row) * stride;
            let b = a + stride;
            for x in 0..stride {
                out[row * stride + x] = ((tall[a + x] as u16 + tall[b + x] as u16) / 2) as u8;
            }
        }
        out
    }

    /// The first two frames are identical: the seed is shown but not consumed,
    /// and a later real movement still grows the canvas.
    #[test]
    fn unchanged_frame_is_still() {
        let (w, h) = (80u32, 60u32);
        let tall = page(w, 300);
        let f = frame(&tall, w, 0, h);
        let mut s = Stitcher::new(&f, w, h).expect("stitcher");
        assert_eq!(s.push(&f, w, h), Step::Still);
        assert_eq!(s.height(), h, "first push establishes the seed content");
        assert!(s.unchanged(&f));
        assert_eq!(
            s.push(&frame(&tall, w, 20, h), w, h),
            Step::Appended(Appended {
                offset: 20,
                added: 20,
                total_h: 80,
            })
        );
    }

    #[test]
    fn settle_seeds_a_single_frame() {
        let (w, h) = (80u32, 60u32);
        let tall = page(w, 300);
        let f = frame(&tall, w, 0, h);
        let mut s = Stitcher::new(&f, w, h).expect("stitcher");
        assert_eq!(s.height(), 0, "seed is buffered until a second frame");
        s.settle();
        assert_eq!(s.height(), h);
        let (img, _, _) = s.image();
        assert_eq!(img, f);
    }

    #[test]
    fn caret_blink_is_still() {
        let (w, h) = (320u32, 160u32);
        let tall = text_page(w, 600);
        let f = frame(&tall, w, 0, h);
        let mut s = Stitcher::new(&f, w, h).expect("stitcher");
        s.push(&f, w, h);
        let mut caret = f.clone();
        for y in 40..52 {
            let o = ((y * w + 12) * 4) as usize;
            caret[o] = 0;
            caret[o + 1] = 0;
            caret[o + 2] = 0;
        }
        assert_eq!(s.push(&caret, w, h), Step::Still);
        assert_eq!(s.height(), h);
    }

    #[test]
    fn undo_then_rescroll_restores_the_same_image() {
        let (w, h) = (80u32, 200u32);
        let tall = page(w, 700);
        let mut s = seed(&tall, w, h);
        s.push(&frame(&tall, w, 80, h), w, h);
        s.push(&frame(&tall, w, 160, h), w, h);
        assert_eq!(s.height(), 360);
        assert!(s.undo_last());
        assert_eq!(s.height(), 280);
        assert_eq!(
            s.push(&frame(&tall, w, 160, h), w, h),
            Step::Appended(Appended {
                offset: 80,
                added: 80,
                total_h: 360,
            })
        );
        let (img, _, _) = s.image();
        assert_eq!(img, &tall[..(w as usize) * 360 * 4]);
    }

    #[test]
    fn undo_prepend_restores_the_previous_top() {
        let (w, h) = (80u32, 200u32);
        let tall = page(w, 900);
        let mut s = Stitcher::new(&frame(&tall, w, 160, h), w, h).expect("stitcher");
        assert_eq!(
            s.push(&frame(&tall, w, 120, h), w, h),
            Step::Appended(Appended {
                offset: -40,
                added: 40,
                total_h: 240,
            })
        );
        assert!(s.undo_last());
        assert_eq!(s.height(), h);
        let (img, _, _) = s.image();
        assert_eq!(img, frame(&tall, w, 160, h));
    }

    /// A real text page scrolled up, then down past the captured range: both
    /// directions must remain one contiguous block.
    #[test]
    fn reversed_scroll_on_a_text_page_stays_contiguous() {
        let (w, h) = (320u32, 200u32);
        let tall = text_page(w, 1200);
        let mut s = Stitcher::new(&frame(&tall, w, 300, h), w, h).expect("stitcher");
        assert_eq!(
            s.push(&frame(&tall, w, 240, h), w, h),
            Step::Appended(Appended {
                offset: -60,
                added: 60,
                total_h: 260,
            })
        );
        assert_eq!(
            s.push(&frame(&tall, w, 180, h), w, h),
            Step::Appended(Appended {
                offset: -60,
                added: 60,
                total_h: 320,
            })
        );
        assert_eq!(s.push(&frame(&tall, w, 260, h), w, h), Step::Still);
        assert_eq!(
            s.push(&frame(&tall, w, 400, h), w, h),
            Step::Appended(Appended {
                offset: 140,
                added: 100,
                total_h: 420,
            })
        );
        let (img, _, ih) = s.image();
        assert_eq!(ih, 420);
        assert_eq!(img, tall[(w as usize) * 180 * 4..(w as usize) * 600 * 4]);
    }

    #[test]
    fn rejects_a_gap_with_no_overlap() {
        let (w, h) = (80u32, 200u32);
        let tall = page(w, 900);
        let mut s = seed(&tall, w, h);
        let step = s.push(&frame(&tall, w, 300, h), w, h);
        assert_eq!(
            step,
            Step::Failed,
            "a jump beyond the viewport cannot overlap"
        );
    }

    #[test]
    fn keeps_stitching_after_a_bad_frame() {
        let (w, h) = (80u32, 200u32);
        let tall = page(w, 900);
        let mut s = seed(&tall, w, h);
        s.push(&frame(&tall, w, 80, h), w, h);
        let garbage = vec![17u8; (w * h * 4) as usize];
        assert_eq!(s.push(&garbage, w, h), Step::Failed);
        assert_eq!(
            s.push(&frame(&tall, w, 140, h), w, h),
            Step::Appended(Appended {
                offset: 60,
                added: 60,
                total_h: 340,
            }),
            "the last located frame remains the anchor"
        );
    }

    #[test]
    fn thumbnail_keeps_aspect_and_bounds() {
        let (w, h) = (400u32, 300u32);
        let tall = page(w, 900);
        let mut s = seed(&tall, w, h);
        s.push(&frame(&tall, w, 80, h), w, h);
        assert_eq!(s.height(), 380);
        let (thumb, tw, th) = s.thumbnail(120, 200).expect("thumb");
        assert!(tw <= 120 && th <= 200);
        assert_eq!(thumb.len(), (tw as usize) * (th as usize) * 4);
        assert_eq!((tw, th), (120, 114));
    }

    #[test]
    fn max_height_is_sane() {
        const { assert!(MAX_HEIGHT > 1000) };
        const { assert!(MAX_PIXELS > 1_000_000) };
    }
}

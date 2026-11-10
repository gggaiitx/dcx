//! Scrolling stitch engine.
//!
//! The model follows the one AirSnip settled on after removing automatic
//! scrolling: hash every row, then find the **smallest** scroll offset whose
//! overlap stays 90 % row-identical. Full-overlap verification is what makes
//! repetitive text work — a single row strip matches at many offsets, the whole
//! overlap matches at one. Sticky page headers/footers are detected when the
//! first real scroll frame arrives and are kept out of the content area, so a
//! fixed toolbar is not duplicated on every append.
//!
//! Pure data in/out: no Win32, no UI. Scrolling up is deliberately ignored
//! (the stitched image already contains that content; prepending it would
//! duplicate rows).

pub const MAX_HEIGHT: u32 = 30_000;
/// Four BGRA buffers exist at the clipboard stage: 24 MP ≈ 96 MB each.
pub const MAX_PIXELS: u64 = 24_000_000;
/// Smallest useful selection height (`MIN_CONTENT` + a header and footer).
pub const MIN_SEL_H: u32 = 32;

const MIN_CONTENT: u32 = 8;
/// A static band shorter than this is page background, not a fixed header.
const MIN_FIXED: u32 = 8;
/// Fraction of the overlap that must be row-identical.
const MATCH_RATIO: f32 = 0.9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Appended {
    /// Pixels the page moved: positive = down (new rows appended).
    pub offset: i32,
    pub added: u32,
    pub total_h: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Appended(Appended),
    /// No vertical movement (page bottom, user paused, caret blink).
    Still,
    /// No trustworthy overlap: the page jumped too far or changed completely.
    Failed,
    /// Height or pixel cap reached.
    Limit,
}

struct Segment {
    rows: u32,
}

pub struct Stitcher {
    w: u32,
    frame_h: u32,
    /// Seed held until the second frame, which is when fixed bands become
    /// measurable. Until then the canvas is empty by design.
    first: Option<Vec<u8>>,
    prev: Vec<u8>,
    header: Vec<u8>,
    footer: Vec<u8>,
    header_h: u32,
    footer_h: u32,
    /// Content rows between the fixed bands.
    content: std::collections::VecDeque<Vec<u8>>,
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
        Some(Stitcher {
            w,
            frame_h: h,
            first: Some(frame.to_vec()),
            prev: Vec::new(),
            header: Vec::new(),
            footer: Vec::new(),
            header_h: 0,
            footer_h: 0,
            content: std::collections::VecDeque::new(),
            segments: Vec::new(),
        })
    }

    pub fn height(&self) -> u32 {
        self.header_h + self.content_height() + self.footer_h
    }

    fn content_height(&self) -> u32 {
        self.content.len() as u32
    }

    pub fn unchanged(&self, frame: &[u8]) -> bool {
        self.first.is_none() && !self.prev.is_empty() && frame == self.prev.as_slice()
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
        let y = y - self.content_height();
        if y < self.footer_h {
            let o = y as usize * stride;
            return self.footer.get(o..o + stride);
        }
        None
    }

    /// Finish a capture that never received a second frame (the user pressed
    /// Enter right after starting): the seed becomes the whole image.
    pub fn settle(&mut self) {
        if let Some(first) = self.first.take() {
            self.adopt_first(&first, &first);
        }
    }

    /// Feed one live frame. `frame` must match the seed size.
    pub fn push(&mut self, frame: &[u8], w: u32, h: u32) -> Step {
        if w != self.w || h != self.frame_h || frame.len() != (w as usize) * (h as usize) * 4 {
            return Step::Failed;
        }
        if let Some(first) = self.first.take() {
            self.adopt_first(&first, frame);
        }
        if self.prev.len() != frame.len() {
            self.prev.clear();
            self.prev.extend_from_slice(frame);
            return Step::Failed;
        }

        let located = locate(
            &self.prev,
            frame,
            self.w,
            self.frame_h,
            self.header_h,
            self.footer_h,
        );
        let m = match located {
            Some(m) => m,
            None => {
                self.prev.clear();
                self.prev.extend_from_slice(frame);
                return Step::Failed;
            }
        };
        if m.offset == 0 {
            self.prev.clear();
            self.prev.extend_from_slice(frame);
            return Step::Still;
        }
        if m.offset < 0 {
            // The user scrolled back into rows already in the canvas. Adding
            // them again would duplicate content; treat it as a pause.
            self.prev.clear();
            self.prev.extend_from_slice(frame);
            return Step::Still;
        }
        let rows = m.end.saturating_sub(m.start);
        if rows == 0 {
            self.prev.clear();
            self.prev.extend_from_slice(frame);
            return Step::Still;
        }
        let total_h = self.height() + rows;
        if total_h > MAX_HEIGHT || (self.w as u64) * (total_h as u64) > MAX_PIXELS {
            self.prev.clear();
            self.prev.extend_from_slice(frame);
            return Step::Limit;
        }

        let stride = self.w as usize * 4;
        for y in m.start..m.end {
            let o = y as usize * stride;
            self.content.push_back(frame[o..o + stride].to_vec());
        }
        self.segments.push(Segment { rows });
        self.prev.clear();
        self.prev.extend_from_slice(frame);
        Step::Appended(Appended {
            offset: m.offset,
            added: rows,
            total_h,
        })
    }

    /// First real frame: measure the bands that stay put between the seed and
    /// this frame, keep them out of the content stream, then put the seed's
    /// content into the canvas.
    fn adopt_first(&mut self, first: &[u8], second: &[u8]) {
        let (mut top, mut bottom) = fixed_regions(first, second, self.w, self.frame_h);
        if top >= self.frame_h {
            // The page did not move at all (or moved less than a pixel).
            top = 0;
            bottom = 0;
        }
        if top < MIN_FIXED {
            top = 0;
        }
        if bottom < MIN_FIXED {
            bottom = 0;
        }
        if top + bottom > self.frame_h {
            bottom = self.frame_h - top;
        }
        let stride = self.w as usize * 4;
        self.header_h = top;
        self.footer_h = bottom;
        self.header = first[..top as usize * stride].to_vec();
        self.footer = first[(self.frame_h - bottom) as usize * stride..].to_vec();
        for y in top..self.frame_h - bottom {
            let o = y as usize * stride;
            self.content.push_back(first[o..o + stride].to_vec());
        }
        self.prev = first.to_vec();
    }

    /// Drop the last appended segment and rebuild `prev` from the canvas top
    /// so a re-scroll of the same rows stitches cleanly.
    pub fn undo_last(&mut self) -> bool {
        let Some(seg) = self.segments.pop() else {
            return false;
        };
        for _ in 0..seg.rows {
            self.content.pop_back();
        }
        self.rebuild_prev();
        true
    }

    fn rebuild_prev(&mut self) {
        let stride = self.w as usize * 4;
        let visible = (self.frame_h - self.header_h - self.footer_h) as usize;
        if self.content.len() < visible {
            self.prev.clear();
            return;
        }
        let mut frame = Vec::with_capacity(stride * self.frame_h as usize);
        frame.extend_from_slice(&self.header);
        let start = self.content.len() - visible;
        for row in self.content.iter().skip(start) {
            frame.extend_from_slice(row);
        }
        frame.extend_from_slice(&self.footer);
        self.prev = frame;
    }
}

struct Located {
    offset: i32,
    start: u32,
    end: u32,
}

fn row_hash(row: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    let mut chunks = row.chunks_exact(8);
    for c in &mut chunks {
        let v = u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]);
        h = (h ^ v).wrapping_mul(PRIME);
    }
    for &b in chunks.remainder() {
        h = (h ^ b as u64).wrapping_mul(PRIME);
    }
    h
}

/// Row hashes ignore a band at the right edge: app scrollbars live there and
/// their thumb does not move with the content, so hashing it would make the
/// true overlap look different even when every text row matches.
fn row_hashes(frame: &[u8], w: u32, h: u32) -> Vec<u64> {
    let stride = w as usize * 4;
    let margin = (w / 16).max(24) as usize;
    let keep = (w as usize).saturating_sub(margin).max(1) * 4;
    (0..h as usize)
        .map(|y| row_hash(&frame[y * stride..y * stride + keep]))
        .collect()
}

/// Bands at the top/bottom that are identical between the seed and the first
/// scrolled frame: sticky site headers, toolbars, chat input boxes.
fn fixed_regions(prev: &[u8], new: &[u8], w: u32, h: u32) -> (u32, u32) {
    let stride = w as usize * 4;
    let same = |y: u32| prev.get(y as usize * stride..(y as usize + 1) * stride)
        == new.get(y as usize * stride..(y as usize + 1) * stride);
    let mut top = 0;
    while top < h && same(top) {
        top += 1;
    }
    let mut bottom = 0;
    while bottom < h && same(h - 1 - bottom) {
        bottom += 1;
    }
    if top + bottom > h {
        bottom = h - top;
    }
    (top, bottom)
}

/// Smallest shift in `1..=content_h/2` whose overlap is at least 90% row
/// identical. Smallest first is the conservative choice: it never invents
/// content that the user has not actually revealed.
fn locate(
    prev: &[u8],
    new: &[u8],
    w: u32,
    h: u32,
    fixed_top: u32,
    fixed_bottom: u32,
) -> Option<Located> {
    let content_h = h.saturating_sub(fixed_top + fixed_bottom);
    if content_h < MIN_CONTENT {
        return None;
    }
    let ph = row_hashes(prev, w, h);
    let nh = row_hashes(new, w, h);
    let ct = fixed_top;

    let same = (0..content_h)
        .filter(|i| ph[(ct + i) as usize] == nh[(ct + i) as usize])
        .count() as u32;
    if same as f32 / content_h as f32 >= MATCH_RATIO {
        return Some(Located {
            offset: 0,
            start: h,
            end: h,
        });
    }

    // Half a viewport was needlessly conservative: a fast flick leaves less
    // than that, and rejecting it makes the preview freeze mid-scroll. Keep
    // at least an eighth of the content (min 24 rows) as proof of overlap.
    let min_overlap = (content_h / 8).max(24).min(content_h - 1);
    let max_shift = content_h - min_overlap;
    let down = min_shift(&ph, &nh, ct, content_h, max_shift, false);
    let up = min_shift(&ph, &nh, ct, content_h, max_shift, true);
    match (down, up) {
        (Some(sd), Some(su)) => Some(if sd <= su {
            down_at(ct, content_h, sd)
        } else {
            up_at(ct, su)
        }),
        (Some(sd), None) => Some(down_at(ct, content_h, sd)),
        (None, Some(su)) => Some(up_at(ct, su)),
        (None, None) => None,
    }
}

fn min_shift(
    ph: &[u64],
    nh: &[u64],
    ct: u32,
    content_h: u32,
    max_shift: u32,
    up: bool,
) -> Option<u32> {
    for s in 1..=max_shift {
        let overlap = content_h - s;
        if overlap == 0 {
            continue;
        }
        let mut same = 0u32;
        for i in 0..overlap {
            let (a, b) = if up {
                (ph[(ct + i) as usize], nh[(ct + i + s) as usize])
            } else {
                (ph[(ct + i + s) as usize], nh[(ct + i) as usize])
            };
            if a == b {
                same += 1;
            }
        }
        if same as f32 / overlap as f32 >= MATCH_RATIO {
            return Some(s);
        }
    }
    None
}

fn down_at(ct: u32, content_h: u32, s: u32) -> Located {
    Located {
        offset: s as i32,
        start: ct + content_h - s,
        end: ct + content_h,
    }
}

fn up_at(ct: u32, s: u32) -> Located {
    Located {
        offset: -(s as i32),
        start: ct,
        end: ct + s,
    }
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
                let hash = (x as u32)
                    .wrapping_mul(73_856_093)
                    ^ (y as u32).wrapping_mul(19_349_663);
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
        // The first push establishes the seed content; each later push adds 80.
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

    /// The page scrollbar moves with the page; 90 % row identity still finds
    /// the true offset.
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
        // Header appears once at the very top.
        assert_eq!(&img[..(w * hdr * 4) as usize], &tall[..(w * hdr * 4) as usize]);
        // Footer appears once at the very bottom.
        let foot = (w * ftr * 4) as usize;
        let img_bottom = &img[img.len() - foot..];
        let page_bottom = &tall[tall.len() - foot..];
        assert_eq!(img_bottom, page_bottom);
    }

    #[test]
    fn ignores_scrolling_up() {
        let (w, h) = (80u32, 200u32);
        let tall = page(w, 900);
        let mut s = seed(&tall, w, h);
        s.push(&frame(&tall, w, 80, h), w, h);
        s.push(&frame(&tall, w, 160, h), w, h);
        let before = s.height();
        let step = s.push(&frame(&tall, w, 80, h), w, h);
        assert_eq!(step, Step::Still, "up-scroll is a pause, not new content");
        assert_eq!(s.height(), before);
    }

    #[test]
    fn rejects_a_gap_with_no_overlap() {
        let (w, h) = (80u32, 200u32);
        let tall = page(w, 900);
        let mut s = seed(&tall, w, h);
        let step = s.push(&frame(&tall, w, 300, h), w, h);
        assert_eq!(step, Step::Failed, "a jump beyond the viewport cannot overlap");
    }

    #[test]
    fn unchanged_frame_is_still() {
        let (w, h) = (80u32, 60u32);
        let tall = page(w, 300);
        let f = frame(&tall, w, 0, h);
        let mut s = Stitcher::new(&f, w, h).expect("stitcher");
        assert_eq!(s.push(&f, w, h), Step::Still);
        assert_eq!(s.height(), h, "first push establishes the seed content");
        assert!(s.unchanged(&f));
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
        let before = s.height();
        assert_eq!(before, 360);
        assert!(s.undo_last());
        assert_eq!(s.height(), 280);
        // Re-scroll to the same position; the rebuilt `prev` must match it.
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
    fn thumbnail_keeps_aspect_and_bounds() {
        let (w, h) = (400u32, 300u32);
        let tall = page(w, 900);
        let mut s = seed(&tall, w, h);
        s.push(&frame(&tall, w, 80, h), w, h);
        assert_eq!(s.height(), 380);
        let (thumb, tw, th) = s.thumbnail(120, 200).expect("thumb");
        assert!(tw <= 120 && th <= 200);
        assert_eq!(thumb.len(), (tw as usize) * (th as usize) * 4);
        // 400x380 等比缩进 120x200 → 120x114
        assert_eq!((tw, th), (120, 114));
    }

    #[test]
    fn max_height_is_sane() {
        assert!(MAX_HEIGHT > 1000);
        assert!(MAX_PIXELS > 1_000_000);
    }
}

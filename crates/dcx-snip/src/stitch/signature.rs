//! Per-row fingerprints for the scrolling matcher.
//!
//! A signature is 32 luminance samples spread across the row, with extra
//! density at the left edge where bullets, line numbers and list markers live.
//! It is deliberately tolerant: subpixel scrolling repaints every pixel, but a
//! sample moves by only a few levels, so a tolerant comparison can still locate
//! the overlap. The previous engine hashed whole rows and demanded exact
//! equality; at 125 % DPI a mid-animation frame then looked like a complete
//! mismatch, which is why manual scrolling kept failing.
//!
//! Averaging into eight wide bands was tried first and rejected: a repeated
//! text body made wrong line phases score as perfect matches because the one
//! distinguishing band (the line number) was averaged away.

/// Sample points per row. Eight of them cover the left 32 px; the rest span
/// the usable width. 32 bytes per row keeps a 30k-row canvas under 1.2 MB.
pub const SAMPLES: usize = 32;
const LEFT_SAMPLES: usize = 8;
pub type RowSig = [u8; SAMPLES];

/// One frame's signatures. `tex_prefix` counts rows that differ from the row
/// above; matching refuses accidental overlaps of blank paper.
#[derive(Clone)]
pub struct Sigs {
    pub rows: Vec<RowSig>,
    /// Prefix sums of the textured-row mask: count in `[start, end)` is
    /// `prefix[end] - prefix[start]`.
    pub tex_prefix: Vec<u32>,
}

/// Mean absolute sample difference (0..=255).
pub fn row_diff(a: &RowSig, b: &RowSig) -> u32 {
    let mut sum = 0u32;
    for i in 0..SAMPLES {
        sum += (a[i] as i32 - b[i] as i32).unsigned_abs();
    }
    sum / SAMPLES as u32
}

/// A row pair still counts as "same" below this. Brightness drift and
/// anti-aliasing stay well under it; unrelated rows are far above.
pub const ROW_TOL: u32 = 16;

/// A row that differs at all from the row above counts as textured. The
/// requirement only exists to reject runs of pixel-identical blank rows; a
/// smooth gradient is still real content and must remain stitchable.
const TEXTURE_DELTA: u32 = 0;

pub fn compute(frame: &[u8], w: u32, h: u32) -> Sigs {
    let stride = w as usize * 4;
    let width = w as usize;
    // Scrollbars live at the right edge; keep a few pixels on the left because
    // list markers and line numbers start there.
    let right = (width / 32).max(24).min(width / 3);
    let start = 0usize;
    let end = width.saturating_sub(right).max(start + 1).min(width);
    let usable = end - start;

    let mut xs = [0usize; SAMPLES];
    let edge = usable.min(32);
    for (i, x) in xs.iter_mut().take(LEFT_SAMPLES).enumerate() {
        *x = start + edge * i / LEFT_SAMPLES;
    }
    let rest = SAMPLES - LEFT_SAMPLES;
    for (i, x) in xs.iter_mut().skip(LEFT_SAMPLES).enumerate() {
        *x = start + usable * (i + 1) / (rest + 1);
    }
    for x in xs.iter_mut() {
        *x = (*x).min(width - 1);
    }

    let mut rows = Vec::with_capacity(h as usize);
    for y in 0..h as usize {
        let base = y * stride;
        let mut sig = [0u8; SAMPLES];
        for (slot, &x) in sig.iter_mut().zip(xs.iter()) {
            let o = base + x * 4;
            let b = frame[o] as u32;
            let g = frame[o + 1] as u32;
            let r = frame[o + 2] as u32;
            *slot = ((77 * r + 150 * g + 29 * b) >> 8) as u8;
        }
        rows.push(sig);
    }

    let mut tex_prefix = Vec::with_capacity(rows.len() + 1);
    tex_prefix.push(0);
    for y in 0..rows.len() {
        let textured = y > 0 && row_diff(&rows[y], &rows[y - 1]) > TEXTURE_DELTA;
        let prev = *tex_prefix.last().expect("prefix seeded");
        tex_prefix.push(prev + textured as u32);
    }
    Sigs { rows, tex_prefix }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, v: u8) -> Vec<u8> {
        vec![v; (w as usize) * (h as usize) * 4]
    }

    #[test]
    fn identical_rows_have_zero_diff() {
        let f = solid(80, 20, 120);
        let s = compute(&f, 80, 20);
        assert_eq!(row_diff(&s.rows[0], &s.rows[10]), 0);
        assert_eq!(s.tex_prefix[20], 0);
    }

    #[test]
    fn random_rows_are_far_apart() {
        let mut a = [0u8; SAMPLES];
        let mut b = [0u8; SAMPLES];
        for i in 0..SAMPLES {
            a[i] = (i * 29) as u8;
            b[i] = (i * 29 + 90) as u8;
        }
        assert!(row_diff(&a, &b) > ROW_TOL);
    }

    #[test]
    fn small_brightness_shift_stays_within_tolerance() {
        let mut a = [0u8; SAMPLES];
        let mut b = [0u8; SAMPLES];
        for i in 0..SAMPLES {
            a[i] = 100 + i as u8;
            b[i] = a[i].saturating_add(5);
        }
        assert!(row_diff(&a, &b) <= ROW_TOL);
    }

    #[test]
    fn left_edge_samples_catch_small_markers() {
        let (w, h) = (200u32, 10u32);
        let a = solid(w, h, 255);
        let mut b = solid(w, h, 255);
        // A 24 px wide marker at x = 6..30, the size of a line number.
        for y in 0..h {
            for x in 6..30 {
                let o = ((y * w + x) * 4) as usize;
                b[o] = 0;
                b[o + 1] = 0;
                b[o + 2] = 0;
            }
        }
        let sa = compute(&a, w, h);
        let sb = compute(&b, w, h);
        assert!(
            row_diff(&sa.rows[0], &sb.rows[0]) > ROW_TOL,
            "a small left marker must change the signature"
        );
    }

    #[test]
    fn texture_prefix_counts_marked_rows() {
        let (w, h) = (80u32, 30u32);
        let mut f = solid(w, h, 200);
        for y in (2..h).step_by(5) {
            for x in 0..w {
                let o = ((y * w + x) * 4) as usize;
                f[o] = 10;
                f[o + 1] = 10;
                f[o + 2] = 10;
            }
        }
        let s = compute(&f, w, h);
        assert!(s.tex_prefix[h as usize] > 0);
        assert_eq!(s.tex_prefix[2], 0, "first rows are flat");
    }
}

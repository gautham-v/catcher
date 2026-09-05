//! The decode animation catcher opens with: every drawn cell starts as a dim
//! punctuation glyph and settles into its real character, in a random order,
//! flashing the accent for a beat as it lands. Nothing here keeps per-cell
//! state — a cell's settle time and its noise are pure functions of where it
//! is and the run's seed — so the draw pass simply asks, for each cell, what
//! it looks like at this many milliseconds in.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use std::time::Duration;

/// How long the last cell takes to settle.
pub const DURATION: Duration = Duration::from_millis(600);
/// How often an unsettled cell changes its glyph.
const FLICKER: Duration = Duration::from_millis(45);
/// How long a cell wears the accent after it settles.
const FLASH: Duration = Duration::from_millis(70);
/// Settle times are spread over this share of `DURATION`, so the last flash
/// has faded by the time the whole thing is over.
const SPREAD: f64 = 0.85;

const POOL: &[char] = &['·', ':', ';', '+', '*', '#', '%', '@', '&', '$', '=', '~'];

/// When the animation is over and the page needs no more redrawing.
pub fn total() -> Duration {
    DURATION.mul_f64(SPREAD) + FLASH
}

/// A stable pseudo-random number in `[0, 1)` for a cell.
fn hash(x: u16, y: u16, seed: u64, salt: u64) -> f64 {
    let mut h = seed ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= (x as u64) << 32 | y as u64;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    h = h.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    h ^= h >> 33;
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// When the cell at (x, y) settles.
fn settles_at(x: u16, y: u16, seed: u64) -> Duration {
    DURATION.mul_f64(SPREAD * hash(x, y, seed, 1))
}

#[derive(Debug, PartialEq)]
pub enum Phase {
    /// Still noise: draw this glyph, dimmed.
    Noise(char),
    /// Just settled: the real glyph in the accent colour.
    Flash,
    /// Settled for good.
    Done,
}

/// What the cell at (x, y) looks like `elapsed` into the run.
pub fn phase(x: u16, y: u16, seed: u64, elapsed: Duration) -> Phase {
    let at = settles_at(x, y, seed);
    if elapsed < at {
        let bucket = (elapsed.as_millis() / FLICKER.as_millis()) as u64;
        let i = (hash(x, y, seed, 2 + bucket) * POOL.len() as f64) as usize;
        Phase::Noise(POOL[i.min(POOL.len() - 1)])
    } else if elapsed < at + FLASH {
        Phase::Flash
    } else {
        Phase::Done
    }
}

/// Rewrite the drawn page in `area` to how it looks `elapsed` into the run.
/// Blank cells are left alone: it is the text that decodes, not the page.
pub fn apply(buf: &mut Buffer, area: Rect, seed: u64, elapsed: Duration) {
    let p = crate::theme::palette();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let Some(cell) = buf.cell_mut((x, y)) else {
                continue;
            };
            if cell.symbol().trim().is_empty() {
                continue;
            }
            match phase(x, y, seed, elapsed) {
                Phase::Noise(c) => {
                    cell.set_char(c);
                    cell.set_fg(p.grey);
                    cell.modifier = ratatui::style::Modifier::empty();
                }
                Phase::Flash => {
                    cell.set_fg(p.accent);
                }
                Phase::Done => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cell_goes_noise_then_flash_then_done_and_stays_put() {
        let at = settles_at(3, 4, 7);
        assert!(matches!(phase(3, 4, 7, Duration::ZERO), Phase::Noise(_)));
        assert_eq!(phase(3, 4, 7, at), Phase::Flash);
        assert_eq!(phase(3, 4, 7, at + FLASH), Phase::Done);
        assert_eq!(phase(3, 4, 7, total()), Phase::Done);
        // the same cell, the same run: the same answer every frame
        assert_eq!(phase(3, 4, 7, at), phase(3, 4, 7, at));
    }

    #[test]
    fn every_cell_is_settled_by_the_end() {
        for y in 0..40 {
            for x in 0..120 {
                assert_eq!(phase(x, y, 99, total()), Phase::Done);
            }
        }
    }

    #[test]
    fn cells_settle_in_a_scattered_order_not_a_sweep() {
        let row: Vec<Duration> = (0..30).map(|x| settles_at(x, 0, 1)).collect();
        assert!(row.windows(2).any(|w| w[0] > w[1]));
        assert!(row.windows(2).any(|w| w[0] < w[1]));
    }

    #[test]
    fn noise_changes_between_flicker_ticks_and_holds_within_one() {
        let x = 5;
        let y = 5;
        let seed = 3;
        let at = settles_at(x, y, seed);
        let t = Duration::ZERO;
        assert_eq!(phase(x, y, seed, t), phase(x, y, seed, t + FLICKER / 3));
        if at > FLICKER * 6 {
            let glyphs: std::collections::HashSet<_> = (0..6)
                .map(|i| match phase(x, y, seed, FLICKER * i) {
                    Phase::Noise(c) => c,
                    other => panic!("{other:?}"),
                })
                .collect();
            assert!(glyphs.len() > 1);
        }
    }

    #[test]
    fn apply_touches_text_and_leaves_blanks_alone() {
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        buf.set_string(0, 0, "ab   ", ratatui::style::Style::default());
        apply(&mut buf, area, 5, Duration::ZERO);
        assert!(POOL.contains(&buf[(0, 0)].symbol().chars().next().unwrap()));
        assert_eq!(buf[(2, 0)].symbol(), " ");
        let mut done = Buffer::empty(area);
        done.set_string(0, 0, "ab   ", ratatui::style::Style::default());
        apply(&mut done, area, 5, total());
        assert_eq!(done[(0, 0)].symbol(), "a");
    }
}

//! Folding sections under headings in the edit view.
//!
//! A fold hides every line under a heading down to the next heading of the
//! same or a higher level. The buffer never changes — the draw spends no rows
//! on a hidden line, the way it already spends none on hidden front matter —
//! so everything that walks the note by line goes through [`Visible`], which
//! maps buffer lines to the rows that are actually on screen and back.
//!
//! Folds are session state, kept per note and never written to disk: which
//! sections are closed is where you are in a note, not something about it.

use crate::md::{self, Block};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// The level of an ATX heading — `# ` through `###### `, with up to three
/// spaces of indent the way CommonMark allows — or `None` for any other line.
pub fn heading_level(line: &str) -> Option<usize> {
    let rest = line
        .strip_prefix("   ")
        .or_else(|| line.strip_prefix("  "))
        .or_else(|| line.strip_prefix(' '))
        .unwrap_or(line);
    let hashes = rest.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    rest[hashes..].starts_with(' ').then_some(hashes)
}

/// The level of the heading on line `row`, unless the line sits inside a
/// block — a `#` in a code fence or a table is not a heading.
pub fn heading_at(lines: &[String], blocks: &[Block], row: usize) -> Option<usize> {
    if md::block_at(blocks, row).is_some() {
        return None;
    }
    heading_level(lines.get(row)?)
}

/// The last line of the section a heading opens: the line before the next
/// heading of the same or a higher level, or the end of the buffer.
pub fn section_end(lines: &[String], blocks: &[Block], heading: usize) -> usize {
    let level = heading_at(lines, blocks, heading).unwrap_or(usize::MAX);
    (heading + 1..lines.len())
        .find(|&r| heading_at(lines, blocks, r).is_some_and(|l| l <= level))
        .unwrap_or(lines.len())
        - 1
}

/// The lines a fold on `heading` would hide, or `None` for a line that is not
/// a heading or has nothing under it.
fn hidden_span(lines: &[String], blocks: &[Block], heading: usize) -> Option<(usize, usize)> {
    heading_at(lines, blocks, heading)?;
    let end = section_end(lines, blocks, heading);
    (end > heading).then_some((heading + 1, end))
}

/// The folded headings of every note this session has folded anything in.
///
/// A fold remembers the text of the heading line it was made on, not only its
/// index: an edit that shifts the lines above it can then move the fold to
/// where the heading went, and an edit to the heading itself — which changes
/// the text — drops it, without the buffer having to report what changed.
#[derive(Default, Debug)]
pub struct Folds {
    by_note: HashMap<PathBuf, BTreeMap<usize, String>>,
}

impl Folds {
    /// The folded heading lines of `path`, in order.
    pub fn of(&self, path: &Path) -> Vec<usize> {
        self.by_note
            .get(path)
            .map(|m| m.keys().copied().collect())
            .unwrap_or_default()
    }

    pub fn is_folded(&self, path: &Path, row: usize) -> bool {
        self.by_note.get(path).is_some_and(|m| m.contains_key(&row))
    }

    /// Fold the section under `row`. How many lines it hid, or `None` when
    /// there was nothing to fold.
    pub fn fold(
        &mut self,
        path: &Path,
        lines: &[String],
        blocks: &[Block],
        row: usize,
    ) -> Option<usize> {
        let (from, to) = hidden_span(lines, blocks, row)?;
        self.by_note
            .entry(path.to_path_buf())
            .or_default()
            .insert(row, lines[row].clone());
        Some(to + 1 - from)
    }

    /// Open the fold on `row`. Whether there was one.
    pub fn unfold(&mut self, path: &Path, row: usize) -> bool {
        let Some(m) = self.by_note.get_mut(path) else {
            return false;
        };
        let had = m.remove(&row).is_some();
        if m.is_empty() {
            self.by_note.remove(path);
        }
        had
    }

    /// Fold every heading with a section under it. How many were folded.
    pub fn fold_all(&mut self, path: &Path, lines: &[String], blocks: &[Block]) -> usize {
        (0..lines.len())
            .filter(|&r| self.fold(path, lines, blocks, r).is_some())
            .count()
    }

    /// Open every fold in `path`. How many there were.
    pub fn unfold_all(&mut self, path: &Path) -> usize {
        self.by_note.remove(path).map_or(0, |m| m.len())
    }

    /// The note at `old` is now the file at `new`: its folds come along. A
    /// save that follows the title, a rename and a move all change the path
    /// and none of them change the text, so nothing is re-settled here.
    pub fn relocate(&mut self, old: &Path, new: &Path) {
        if old == new {
            return;
        }
        if let Some(m) = self.by_note.remove(old) {
            self.by_note.insert(new.to_path_buf(), m);
        }
    }

    /// Open every fold that hides `row`. Whether any did.
    pub fn reveal(&mut self, path: &Path, lines: &[String], blocks: &[Block], row: usize) -> bool {
        let covering: Vec<usize> = self
            .of(path)
            .into_iter()
            .filter(|&h| hidden_span(lines, blocks, h).is_some_and(|(a, b)| (a..=b).contains(&row)))
            .collect();
        for h in &covering {
            self.unfold(path, *h);
        }
        !covering.is_empty()
    }

    /// Bring the folds of `path` back in line with its buffer after an edit:
    /// a fold whose heading still reads the same stays, one whose heading
    /// moved follows it when there is exactly one line it can be, and the
    /// rest — an edited heading, a heading that is gone, one with nothing
    /// left under it — are dropped rather than left pointing at prose.
    pub fn settle(&mut self, path: &Path, lines: &[String], blocks: &[Block]) {
        let Some(m) = self.by_note.get_mut(path) else {
            return;
        };
        let old = std::mem::take(m);
        for (row, text) in old {
            let at = if lines.get(row) == Some(&text) {
                Some(row)
            } else {
                let mut hits = (0..lines.len()).filter(|&r| lines[r] == text);
                hits.next().filter(|_| hits.next().is_none())
            };
            if let Some(at) = at.filter(|&r| hidden_span(lines, blocks, r).is_some()) {
                m.insert(at, text);
            }
        }
        if m.is_empty() {
            self.by_note.remove(path);
        }
    }
}

/// Which buffer lines are on screen, and where. Built once per change and
/// then answered from, so a draw that asks about every line pays for one pass.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Visible {
    hidden: Vec<bool>,
    /// Row of each line; a hidden line takes the row of the last line drawn
    /// before it (its heading), so scrolling to it lands somewhere sensible.
    row_of: Vec<usize>,
    /// The line each row shows.
    line_of: Vec<usize>,
}

impl Visible {
    pub fn new(lines: &[String], blocks: &[Block], folded: &[usize]) -> Visible {
        let mut hidden = vec![false; lines.len()];
        for &h in folded {
            if let Some((from, to)) = hidden_span(lines, blocks, h) {
                hidden[from..=to].iter_mut().for_each(|x| *x = true);
            }
        }
        let mut row_of = Vec::with_capacity(lines.len());
        let mut line_of = Vec::new();
        for (i, &hid) in hidden.iter().enumerate() {
            if !hid {
                line_of.push(i);
            }
            row_of.push(line_of.len().saturating_sub(1));
        }
        Visible {
            hidden,
            row_of,
            line_of,
        }
    }

    /// True when nothing is folded, so callers can skip the mapping.
    pub fn is_plain(&self) -> bool {
        self.line_of.len() == self.hidden.len()
    }

    pub fn is_hidden(&self, line: usize) -> bool {
        self.hidden.get(line).copied().unwrap_or(false)
    }

    /// How many rows there are: one per line on screen.
    pub fn rows(&self) -> usize {
        self.line_of.len()
    }

    pub fn line_to_row(&self, line: usize) -> usize {
        self.row_of
            .get(line)
            .copied()
            .unwrap_or_else(|| self.rows().saturating_sub(1))
    }

    /// The line on `row`; past the end is the last line on screen.
    pub fn row_to_line(&self, row: usize) -> usize {
        self.line_of
            .get(row)
            .or(self.line_of.last())
            .copied()
            .unwrap_or(0)
    }

    /// The nearest line on screen at or after `line`.
    pub fn next_visible(&self, line: usize) -> Option<usize> {
        (line..self.hidden.len()).find(|&l| !self.hidden[l])
    }

    /// The nearest line on screen at or before `line`.
    pub fn prev_visible(&self, line: usize) -> Option<usize> {
        (0..=line.min(self.hidden.len().saturating_sub(1)))
            .rev()
            .find(|&l| !self.hidden[l])
    }

    /// How many lines the fold on `heading` hides: the hidden run right
    /// after it.
    pub fn hidden_under(&self, heading: usize) -> usize {
        self.hidden
            .get(heading + 1..)
            .map_or(0, |rest| rest.iter().take_while(|&&h| h).count())
    }

    /// Where the scroll must sit for the cursor's line to be on a page of
    /// `height` rows — the same rule the buffer applies, in rows on screen
    /// rather than buffer lines, so a fold between them does not count.
    pub fn scroll_for(&self, scroll: usize, cursor: usize, height: usize) -> usize {
        let height = height.max(1);
        let crow = self.line_to_row(cursor);
        let srow = self.line_to_row(scroll);
        if crow < srow {
            cursor
        } else if crow >= srow + height {
            self.row_to_line(crow + 1 - height)
        } else {
            scroll
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(String::from).collect()
    }

    const NOTE: &str = "# Title\nintro\n## One\na\nb\n### Deep\nc\n## Two\nd\n";

    fn note() -> (Vec<String>, Vec<Block>) {
        let l = lines(NOTE);
        let b = md::blocks(&l);
        (l, b)
    }

    #[test]
    fn a_heading_is_hashes_then_a_space_and_nothing_else_is() {
        assert_eq!(heading_level("# a"), Some(1));
        assert_eq!(heading_level("### a"), Some(3));
        assert_eq!(heading_level("   ## a"), Some(2));
        assert_eq!(heading_level("####### a"), None);
        assert_eq!(heading_level("#tag"), None);
        assert_eq!(heading_level("#"), None);
        assert_eq!(heading_level("plain"), None);
        assert_eq!(heading_level("    # code"), None);
    }

    #[test]
    fn a_hash_inside_a_fence_is_not_a_heading() {
        let l = lines("# a\n```\n# not\n```\n# b");
        let b = md::blocks(&l);
        assert_eq!(heading_at(&l, &b, 0), Some(1));
        assert_eq!(heading_at(&l, &b, 2), None);
        // so the fence is inside a's section, and b ends it
        assert_eq!(section_end(&l, &b, 0), 3);
    }

    #[test]
    fn a_section_runs_to_the_next_heading_of_the_same_or_a_higher_level() {
        let (l, b) = note();
        assert_eq!(section_end(&l, &b, 0), 8); // h1: everything
        assert_eq!(section_end(&l, &b, 2), 6); // h2 One: through Deep
        assert_eq!(section_end(&l, &b, 5), 6); // h3 Deep: c
        assert_eq!(section_end(&l, &b, 7), 8); // h2 Two: to the end
    }

    #[test]
    fn folding_hides_the_section_and_keeps_the_heading() {
        let (l, b) = note();
        let mut f = Folds::default();
        let p = Path::new("n.md");
        assert_eq!(f.fold(p, &l, &b, 2), Some(4));
        let v = Visible::new(&l, &b, &f.of(p));
        assert!(!v.is_hidden(2));
        assert!((3..=6).all(|r| v.is_hidden(r)));
        assert!(!v.is_hidden(7));
        assert_eq!(v.hidden_under(2), 4);
        assert_eq!(v.rows(), 5);
        // prose, and a heading with nothing under it, cannot fold
        assert_eq!(f.fold(p, &l, &b, 1), None);
        let l2 = lines("# a\n# b");
        let b2 = md::blocks(&l2);
        assert_eq!(f.fold(p, &l2, &b2, 0), None);
    }

    #[test]
    fn lines_and_rows_map_both_ways_around_a_fold() {
        let (l, b) = note();
        let v = Visible::new(&l, &b, &[2]);
        // lines 0 1 2 [3 4 5 6] 7 8 → rows 0 1 2 3 4
        assert_eq!(v.line_to_row(2), 2);
        assert_eq!(v.line_to_row(7), 3);
        assert_eq!(v.line_to_row(8), 4);
        // a hidden line sits on its heading's row
        assert_eq!(v.line_to_row(5), 2);
        assert_eq!(v.row_to_line(3), 7);
        assert_eq!(v.row_to_line(99), 8);
        assert_eq!(v.next_visible(3), Some(7));
        assert_eq!(v.prev_visible(6), Some(2));
        assert_eq!(v.next_visible(7), Some(7));
        assert!(!v.is_plain());
        assert!(Visible::new(&l, &b, &[]).is_plain());
    }

    #[test]
    fn nested_folds_hide_the_union_and_open_independently() {
        let (l, b) = note();
        let mut f = Folds::default();
        let p = Path::new("n.md");
        f.fold(p, &l, &b, 2);
        f.fold(p, &l, &b, 5);
        assert!(f.unfold(p, 2));
        let v = Visible::new(&l, &b, &f.of(p));
        assert!(!v.is_hidden(3));
        assert!(v.is_hidden(6));
        assert!(!f.unfold(p, 2));
        assert_eq!(f.unfold_all(p), 1);
        assert!(f.of(p).is_empty());
    }

    #[test]
    fn revealing_a_hidden_line_opens_every_fold_over_it() {
        let (l, b) = note();
        let mut f = Folds::default();
        let p = Path::new("n.md");
        f.fold(p, &l, &b, 2);
        f.fold(p, &l, &b, 5);
        f.fold(p, &l, &b, 7);
        // c (line 6) sits under both One and Deep; Two is left alone
        assert!(f.reveal(p, &l, &b, 6));
        assert_eq!(f.of(p), vec![7]);
        assert!(!f.reveal(p, &l, &b, 7));
    }

    #[test]
    fn fold_all_takes_every_heading_that_has_a_section() {
        let (l, b) = note();
        let mut f = Folds::default();
        let p = Path::new("n.md");
        assert_eq!(f.fold_all(p, &l, &b), 4);
        let v = Visible::new(&l, &b, &f.of(p));
        assert_eq!(v.rows(), 1);
    }

    #[test]
    fn folds_are_kept_per_note() {
        let (l, b) = note();
        let mut f = Folds::default();
        f.fold(Path::new("a.md"), &l, &b, 2);
        assert!(f.is_folded(Path::new("a.md"), 2));
        assert!(!f.is_folded(Path::new("b.md"), 2));
        assert!(f.of(Path::new("b.md")).is_empty());
    }

    #[test]
    fn a_fold_follows_its_heading_when_lines_are_added_above() {
        let (l, b) = note();
        let mut f = Folds::default();
        let p = Path::new("n.md");
        f.fold(p, &l, &b, 7);
        let mut l2 = l.clone();
        l2.insert(1, "more intro".into());
        f.settle(p, &l2, &md::blocks(&l2));
        assert_eq!(f.of(p), vec![8]);
    }

    #[test]
    fn editing_the_heading_line_unfolds_it() {
        let (l, b) = note();
        let mut f = Folds::default();
        let p = Path::new("n.md");
        f.fold(p, &l, &b, 7);
        let mut l2 = l.clone();
        l2[7] = "## Two more".into();
        f.settle(p, &l2, &md::blocks(&l2));
        assert!(f.of(p).is_empty());
    }

    #[test]
    fn a_fold_whose_section_is_gone_or_ambiguous_is_dropped() {
        let (l, b) = note();
        let mut f = Folds::default();
        let p = Path::new("n.md");
        f.fold(p, &l, &b, 7);
        // the last line deleted: nothing left under Two
        let l2 = l[..8].to_vec();
        f.settle(p, &l2, &md::blocks(&l2));
        assert!(f.of(p).is_empty());
        // moved, and the same heading text twice: nowhere certain to go
        f.fold(p, &l, &b, 7);
        let mut l3 = l.clone();
        l3.insert(0, "## Two".into());
        l3.insert(1, "x".into());
        f.settle(p, &l3, &md::blocks(&l3));
        assert!(f.of(p).is_empty());
    }

    #[test]
    fn the_scroll_counts_rows_on_screen_not_buffer_lines() {
        let (l, b) = note();
        let v = Visible::new(&l, &b, &[2]);
        // rows 0..5; a page of 3 rows starting at line 0 holds Two (row 3)?
        // no: rows 0 1 2 fit, so the cursor on line 7 needs the top on row 1
        assert_eq!(v.scroll_for(0, 7, 3), 1);
        assert_eq!(v.scroll_for(0, 2, 3), 0);
        // above the top: the cursor's own line
        assert_eq!(v.scroll_for(7, 0, 3), 0);
        // the buffer's rule would have counted the four hidden lines
        let plain = Visible::new(&l, &b, &[]);
        assert_eq!(plain.scroll_for(0, 7, 3), 5);
    }

    #[test]
    fn a_renamed_note_keeps_its_folds_under_its_new_path() {
        let (l, b) = note();
        let mut f = Folds::default();
        let old = Path::new("/v/one.md");
        let new = Path::new("/v/two.md");
        assert!(f.fold(old, &l, &b, 2).is_some());
        f.relocate(old, new);
        assert!(f.is_folded(new, 2));
        assert!(!f.is_folded(old, 2));
        // the same path is a no-op, and nothing is lost to a note with no folds
        f.relocate(new, new);
        assert!(f.is_folded(new, 2));
        f.relocate(Path::new("/v/none.md"), Path::new("/v/x.md"));
        assert!(f.is_folded(new, 2));
    }
}

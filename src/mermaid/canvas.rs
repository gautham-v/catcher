//! The surface both diagram builders draw on: a grid of characters, each with
//! the [`Role`] it will be styled by.
//!
//! Two things make it worth having rather than pushing strings around. First,
//! junctions: a diagram is lines crossing lines, and a `─` written across a
//! `│` has to come out `┼` rather than clobbering it, or every crossing in the
//! picture is a hole. [`Canvas::put`] merges box-drawing characters by their
//! edges, so a builder can draw each line in ignorance of the others and the
//! joins appear by themselves. Second, width: a label can hold CJK or an emoji,
//! which are two columns wide in a terminal but one `char` in a string, so the
//! grid is columns and a wide character claims the cell beside it.
//!
//! The canvas grows to fit whatever is written to it. A builder that has
//! miscounted its layout by a column gets a wider picture, never a panic and
//! never a silently dropped edge.

use super::{Role, Row, Run};
use crate::md::{char_width, str_width};

/// The second column of a two-column character. It is never drawn; it exists
/// so the grid's columns and the terminal's columns are the same thing.
const CONT: char = '\u{0}';

/// One cell: what to draw, what it counts as, and — for a line — which
/// directions it reaches in, so the next line to arrive can join it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Ink {
    ch: char,
    role: Role,
    /// Edges, as [`U`]/[`R`]/[`D`]/[`L`] bits. Zero for anything that is not a
    /// join: a space, a letter, an arrowhead, a slash.
    bits: u8,
}

const BLANK: Ink = Ink {
    ch: ' ',
    role: Role::Line,
    bits: 0,
};

/// Which side of a box, or which way an arrow points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Up,
    Right,
    Down,
    Left,
}

/// The shapes mermaid's node brackets ask for, as far as a terminal can honour
/// them: `[]` is a rectangle, `()` a rounded one, `{}` a decision, `(())` a
/// circle. Anything else mermaid can spell is drawn as the nearest of these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Rect,
    Round,
    Diamond,
    Circle,
}

/// The arrowhead that points `side`.
pub fn arrow_char(side: Side) -> char {
    match side {
        Side::Up => '▲',
        Side::Right => '▶',
        Side::Down => '▼',
        Side::Left => '◀',
    }
}

/// Where an edge meets a box of `w`×`h` at `(x, y)`: the middle of the side it
/// arrives on. Both builders attach here, so a line always lands on a border
/// rather than beside it.
pub fn attach(x: usize, y: usize, w: usize, h: usize, side: Side) -> (usize, usize) {
    match side {
        Side::Left => (x, y + h / 2),
        Side::Right => (x + w.saturating_sub(1), y + h / 2),
        Side::Up => (x + w / 2, y),
        Side::Down => (x + w / 2, y + h.saturating_sub(1)),
    }
}

// Edges of a box-drawing character, as bits. A character is the set of
// directions it reaches in, which is all merging needs to know.
const U: u8 = 1;
const R: u8 = 2;
const D: u8 = 4;
const L: u8 = 8;

/// The edges a character reaches in, or `None` for one that is not a join —
/// text, a diamond's slash, an arrowhead. Those overwrite and are overwritten
/// whole, which is what you want: an arrowhead half-merged into a line is not
/// an arrowhead.
fn is_arrow(ch: char) -> bool {
    matches!(ch, '▶' | '◀' | '▲' | '▼')
}

fn mask(ch: char) -> Option<u8> {
    Some(match ch {
        '─' => R | L,
        '│' => U | D,
        '┌' | '╭' => R | D,
        '┐' | '╮' => D | L,
        '└' | '╰' => U | R,
        '┘' | '╯' => U | L,
        '├' => U | R | D,
        '┤' => U | D | L,
        '┬' => R | D | L,
        '┴' => U | R | L,
        '┼' => U | R | D | L,
        _ => return None,
    })
}

/// The character for a set of edges. Corners come back rounded, matching the
/// callout card and the panel borders; junctions are sharp, because a rounded
/// tee does not exist and a corner is the only place the softer shape reads.
///
/// A single edge is a stub — the first cell of a run, before anything has
/// joined it — and draws as the run it is the start of.
fn glyph(mask: u8) -> char {
    match mask {
        m if m == R || m == L || m == R | L => '─',
        m if m == U || m == D || m == U | D => '│',
        m if m == R | D => '╭',
        m if m == D | L => '╮',
        m if m == U | R => '╰',
        m if m == U | L => '╯',
        m if m == U | R | D => '├',
        m if m == U | D | L => '┤',
        m if m == R | D | L => '┬',
        m if m == U | R | L => '┴',
        _ => '┼',
    }
}

/// The edges cell `i` of a run of `len` reaches in: `on` towards the rest of
/// the run, `back` towards the part already drawn.
fn ends(i: usize, len: usize, on: u8, back: u8) -> u8 {
    let mut bits = 0;
    if i + 1 < len {
        bits |= on;
    }
    if i > 0 {
        bits |= back;
    }
    // a run one cell long is a dash, not a stub pointing nowhere
    if bits == 0 {
        on | back
    } else {
        bits
    }
}

/// A grid of characters, drawn on by both builders.
#[derive(Clone, Debug, Default)]
pub struct Canvas {
    w: usize,
    h: usize,
    cells: Vec<Ink>,
}

impl Canvas {
    pub fn new(w: usize, h: usize) -> Canvas {
        Canvas {
            w,
            h,
            cells: vec![BLANK; w * h],
        }
    }

    pub fn width(&self) -> usize {
        self.w
    }

    pub fn height(&self) -> usize {
        self.h
    }

    /// Make room for at least `w`×`h`. Growing sideways has to move every row,
    /// so the grid is rebuilt rather than patched.
    pub fn ensure(&mut self, w: usize, h: usize) {
        if w <= self.w && h <= self.h {
            return;
        }
        let (nw, nh) = (self.w.max(w), self.h.max(h));
        let mut cells = vec![BLANK; nw * nh];
        for y in 0..self.h {
            for x in 0..self.w {
                cells[y * nw + x] = self.cells[y * self.w + x];
            }
        }
        self.w = nw;
        self.h = nh;
        self.cells = cells;
    }

    /// What is drawn at `(x, y)`; a space for anything off the grid, and for
    /// the hidden second column of a wide character.
    pub fn get(&self, x: usize, y: usize) -> char {
        if x >= self.w || y >= self.h {
            return ' ';
        }
        match self.cells[y * self.w + x].ch {
            CONT => ' ',
            ch => ch,
        }
    }

    /// Draw `ch` at `(x, y)`, replacing whatever was there.
    pub fn set(&mut self, x: usize, y: usize, ch: char, role: Role) {
        self.write(x, y, ch, role, mask(ch).unwrap_or(0));
    }

    /// Draw `ch` at `(x, y)`, joining it to what is already there when both are
    /// box-drawing characters. This is the one every line should use: it is
    /// what turns two lines crossing into a `┼` and a line meeting a box edge
    /// into a `├`.
    pub fn put(&mut self, x: usize, y: usize, ch: char, role: Role) {
        match mask(ch) {
            Some(bits) => self.put_bits(x, y, bits, role),
            None => self.set(x, y, ch, role),
        }
    }

    /// Add `bits` worth of edges to `(x, y)`. Everything a line draws goes
    /// through here, which is why a route drawn as three separate runs grows
    /// its own corners: the run that stops contributes the edge it came from,
    /// the run that starts contributes the edge it leaves by, and the cell
    /// they share adds up to `╮`.
    fn put_bits(&mut self, x: usize, y: usize, bits: u8, role: Role) {
        // an arrowhead is the end of an edge, and a line drawn over it later
        // — a back edge leaving the box it points at — must not erase it
        if x < self.w && y < self.h && is_arrow(self.cells[y * self.w + x].ch) {
            return;
        }
        let old = if x < self.w && y < self.h {
            self.cells[y * self.w + x].bits
        } else {
            0
        };
        let bits = bits | old;
        self.write(x, y, glyph(bits), role, bits);
    }

    fn write(&mut self, x: usize, y: usize, ch: char, role: Role, bits: u8) {
        self.ensure(x + char_width(ch), y + 1);
        self.clear_cell(x, y);
        self.cells[y * self.w + x] = Ink { ch, role, bits };
        if char_width(ch) > 1 {
            self.clear_cell(x + 1, y);
            self.cells[y * self.w + x + 1] = Ink {
                ch: CONT,
                role,
                bits: 0,
            };
        }
    }

    /// Write `s` starting at `(x, y)`, one cell per column. Text overwrites
    /// whatever it lands on — a label inside a box is meant to be read, not
    /// joined to the box. Returns the columns it took.
    pub fn text(&mut self, x: usize, y: usize, s: &str, role: Role) -> usize {
        let mut col = x;
        for ch in s.chars() {
            self.set(col, y, ch, role);
            col += char_width(ch);
        }
        col - x
    }

    /// A horizontal run of `len` columns starting at `(x, y)`, joined to
    /// whatever it crosses.
    ///
    /// The ends of a run carry only the edge they reach in by, so a run that
    /// stops at a box's side draws `├` rather than `┼` — the box has no line
    /// beyond it, and a cross there would claim it did.
    pub fn hline(&mut self, x: usize, y: usize, len: usize, role: Role) {
        for i in 0..len {
            self.put_bits(x + i, y, ends(i, len, R, L), role);
        }
    }

    /// A vertical run of `len` rows starting at `(x, y)`, joined to whatever it
    /// crosses.
    pub fn vline(&mut self, x: usize, y: usize, len: usize, role: Role) {
        for i in 0..len {
            self.put_bits(x, y + i, ends(i, len, D, U), role);
        }
    }

    /// An arrowhead pointing `side`. Never merged: a head that had picked up a
    /// line's edges would stop reading as a head.
    pub fn arrow(&mut self, x: usize, y: usize, side: Side, role: Role) {
        self.set(x, y, arrow_char(side), role);
    }

    /// The first cell of a run that leaves a line it is joined to: only the
    /// outward edge, so a dash leaving a lifeline reads `├` and not `┼`.
    pub fn stub(&mut self, x: usize, y: usize, side: Side, role: Role) {
        let bits = match side {
            Side::Right => R,
            Side::Left => L,
            Side::Up => U,
            Side::Down => D,
        };
        self.put_bits(x, y, bits, role);
    }

    /// The room a node needs for `label` (one string per line), as `(w, h)`.
    /// A box is its text plus a column of padding and a border on each side;
    /// a decision gives the slanted sides a column more, so the text does not
    /// crowd the point.
    pub fn node_size(shape: Shape, label: &[String]) -> (usize, usize) {
        let text = label.iter().map(|l| str_width(l)).max().unwrap_or(0);
        let pad = if shape == Shape::Diamond { 6 } else { 4 };
        (text + pad, label.len().max(1) + 2)
    }

    /// Draw a labelled node with its top-left at `(x, y)`, `w` columns wide and
    /// `h` rows tall — the size [`Canvas::node_size`] asked for, or a larger
    /// one when the caller is lining a whole layer up. The label is centred.
    pub fn node(&mut self, x: usize, y: usize, w: usize, h: usize, shape: Shape, label: &[String]) {
        // a decision needs four columns before its slanted corners have a side
        // to sit between; below that it is drawn square, which is honest — a
        // box two columns wide cannot say "decision" whatever it is drawn with
        let shape = if shape == Shape::Diamond && w < 4 {
            Shape::Rect
        } else {
            shape
        };
        if w < 2 || h < 2 {
            return;
        }
        let inner = w - 2;
        let (tl, tr, bl, br, side) = match shape {
            Shape::Rect => ('╭', '╮', '╰', '╯', '│'),
            Shape::Round => ('╭', '╮', '╰', '╯', '│'),
            Shape::Circle => ('╭', '╮', '╰', '╯', '('),
            Shape::Diamond => ('╱', '╲', '╲', '╱', '│'),
        };
        // the decision's corners are slashes, which sit one column in so the
        // shape reads as a point rather than a clipped corner
        let edge = usize::from(shape == Shape::Diamond);
        self.put(x + edge, y, tl, Role::Line);
        self.put(x + w - 1 - edge, y, tr, Role::Line);
        self.put(x + edge, y + h - 1, bl, Role::Line);
        self.put(x + w - 1 - edge, y + h - 1, br, Role::Line);
        self.hline(x + 1 + edge, y, inner - 2 * edge, Role::Line);
        self.hline(x + 1 + edge, y + h - 1, inner - 2 * edge, Role::Line);
        for row in 1..h - 1 {
            let right = if shape == Shape::Circle { ')' } else { side };
            self.put(x, y + row, side, Role::Line);
            self.put(x + w - 1, y + row, right, Role::Line);
        }
        for (i, line) in label.iter().enumerate() {
            if i + 1 >= h - 1 {
                break;
            }
            let pad = inner.saturating_sub(str_width(line)) / 2;
            self.text(x + 1 + pad, y + 1 + i, line, Role::Node);
        }
    }

    /// The drawing as rows of styled runs: adjacent cells of one role merged,
    /// and the blank tail of every row cut off, so nothing is styled that the
    /// reader cannot see.
    pub fn rows(&self) -> Vec<Row> {
        (0..self.h)
            .map(|y| {
                let row = &self.cells[y * self.w..(y + 1) * self.w];
                let end = row
                    .iter()
                    .rposition(|c| c.ch != ' ' && c.ch != CONT)
                    .map_or(0, |i| i + 1);
                let mut runs: Vec<Run> = Vec::new();
                for cell in &row[..end] {
                    if cell.ch == CONT {
                        continue;
                    }
                    match runs.last_mut() {
                        Some(last) if last.role == cell.role => last.text.push(cell.ch),
                        _ => runs.push(Run::new(cell.ch.to_string(), cell.role)),
                    }
                }
                runs
            })
            .collect()
    }

    /// Blank a cell, and the other half of any wide character it is part of.
    /// Half a wide character left on the grid would draw as a stray column and
    /// throw every cell after it out by one.
    fn clear_cell(&mut self, x: usize, y: usize) {
        if x >= self.w || y >= self.h {
            return;
        }
        let i = y * self.w + x;
        if self.cells[i].ch == CONT && x > 0 {
            self.cells[i - 1] = BLANK;
        }
        if char_width(self.cells[i].ch) > 1 && x + 1 < self.w {
            self.cells[i + 1] = BLANK;
        }
        self.cells[i] = BLANK;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canvas as plain text, one string per row.
    fn text(c: &Canvas) -> Vec<String> {
        c.rows()
            .iter()
            .map(|r| r.iter().map(|run| run.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn a_box_is_drawn_with_its_label_centred() {
        let label = vec!["ok".to_string()];
        let (w, h) = Canvas::node_size(Shape::Round, &label);
        let mut c = Canvas::new(w, h);
        c.node(0, 0, w, h, Shape::Round, &label);
        assert_eq!(text(&c), vec!["╭────╮", "│ ok │", "╰────╯"]);
    }

    #[test]
    fn a_decision_is_drawn_with_slanted_corners() {
        let label = vec!["ok?".to_string()];
        let (w, h) = Canvas::node_size(Shape::Diamond, &label);
        let mut c = Canvas::new(w, h);
        c.node(0, 0, w, h, Shape::Diamond, &label);
        assert_eq!(text(&c), vec![" ╱─────╲", "│  ok?  │", " ╲─────╱"]);
    }

    #[test]
    fn a_decision_too_narrow_for_its_corners_is_drawn_square() {
        // a layer lining its boxes up can hand a node less room than
        // `node_size` asked for; it must come back a box, not a panic
        let mut c = Canvas::new(3, 3);
        c.node(0, 0, 3, 3, Shape::Diamond, &[]);
        assert_eq!(text(&c), vec!["╭─╮", "│ │", "╰─╯"]);
    }

    #[test]
    fn a_two_line_label_makes_a_taller_box() {
        let label = vec!["read".to_string(), "the file".to_string()];
        let (w, h) = Canvas::node_size(Shape::Rect, &label);
        assert_eq!((w, h), (12, 4));
        let mut c = Canvas::new(w, h);
        c.node(0, 0, w, h, Shape::Rect, &label);
        assert_eq!(
            text(&c),
            vec![
                "╭──────────╮",
                "│   read   │",
                "│ the file │",
                "╰──────────╯"
            ]
        );
    }

    #[test]
    fn crossing_lines_merge_into_a_junction() {
        let mut c = Canvas::new(5, 3);
        c.hline(0, 1, 5, Role::Line);
        c.vline(2, 0, 3, Role::Line);
        assert_eq!(text(&c), vec!["  │", "──┼──", "  │"]);
    }

    #[test]
    fn a_line_meeting_a_box_becomes_a_tee() {
        let label = vec!["a".to_string()];
        let (w, h) = Canvas::node_size(Shape::Round, &label);
        let mut c = Canvas::new(w + 3, h);
        c.node(0, 0, w, h, Shape::Round, &label);
        let (ax, ay) = attach(0, 0, w, h, Side::Right);
        c.hline(ax, ay, 3, Role::Line);
        assert_eq!(text(&c), vec!["╭───╮", "│ a ├──", "╰───╯"]);
        // and a corner that grows a third edge is a tee too
        let mut c = Canvas::new(3, 2);
        c.put(0, 0, '╭', Role::Line);
        c.put(0, 0, '─', Role::Line);
        assert_eq!(c.get(0, 0), '┬');
    }

    #[test]
    fn text_overwrites_the_line_under_it() {
        let mut c = Canvas::new(6, 1);
        c.hline(0, 0, 6, Role::Line);
        c.text(1, 0, "no", Role::Label);
        assert_eq!(text(&c), vec!["─no───"]);
        let row = &c.rows()[0];
        assert_eq!(row[0].role, Role::Line);
        assert_eq!(row[1], Run::new("no", Role::Label));
        assert_eq!(row[2].role, Role::Line);
    }

    #[test]
    fn an_arrowhead_never_merges_with_the_line_it_ends() {
        let mut c = Canvas::new(4, 1);
        c.hline(0, 0, 4, Role::Line);
        c.arrow(3, 0, Side::Right, Role::Line);
        assert_eq!(text(&c), vec!["───▶"]);
    }

    #[test]
    fn a_line_drawn_over_an_arrowhead_leaves_it_be() {
        let mut c = Canvas::new(4, 1);
        c.arrow(3, 0, Side::Right, Role::Line);
        c.hline(0, 0, 4, Role::Line);
        assert_eq!(text(&c), vec!["───▶"]);
    }

    #[test]
    fn a_stub_leaving_a_line_is_a_tee_not_a_cross() {
        let mut c = Canvas::new(3, 3);
        c.vline(0, 0, 3, Role::Line);
        c.stub(0, 1, Side::Right, Role::Line);
        c.put(1, 1, '─', Role::Line);
        assert_eq!(text(&c), vec!["│", "├─", "│"]);
    }

    #[test]
    fn a_wide_character_takes_two_columns() {
        let mut c = Canvas::new(8, 1);
        c.text(0, 0, "図", Role::Node);
        c.text(2, 0, "x", Role::Node);
        assert_eq!(text(&c), vec!["図x"]);
        assert_eq!(str_width(&text(&c)[0]), 3);
        // and a box sized for it is wide enough to hold it
        let label = vec!["図".to_string()];
        assert_eq!(Canvas::node_size(Shape::Round, &label).0, 6);
    }

    #[test]
    fn overwriting_half_a_wide_character_leaves_no_stray_column() {
        let mut c = Canvas::new(4, 1);
        c.text(0, 0, "図", Role::Node);
        c.set(1, 0, 'y', Role::Node);
        assert_eq!(text(&c), vec![" y"]);
        let mut c = Canvas::new(4, 1);
        c.text(0, 0, "図", Role::Node);
        c.set(0, 0, 'y', Role::Node);
        assert_eq!(text(&c), vec!["y"]);
    }

    #[test]
    fn rows_are_trimmed_at_the_right() {
        let mut c = Canvas::new(10, 2);
        c.text(0, 0, "hi", Role::Node);
        assert_eq!(text(&c), vec!["hi", ""]);
        assert_eq!(c.rows()[1].len(), 0);
    }

    #[test]
    fn the_canvas_grows_to_fit_a_write_past_its_edge() {
        let mut c = Canvas::new(2, 1);
        c.text(1, 2, "abc", Role::Node);
        assert_eq!((c.width(), c.height()), (4, 3));
        assert_eq!(text(&c), vec!["", "", " abc"]);
    }
}

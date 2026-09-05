//! Editing a markdown table as a grid: the rows of a table block parsed into
//! a matrix of cell texts, the operations the palette offers on rows and
//! columns, and the padded pipe source written back. Every operation is a
//! pure function on the matrix so it can be tested without an editor; the
//! app swaps the block's lines for what [`Table::emit`] returns.

use crate::md::{self, Align};

/// A table with its separator row taken out: `rows` are the header rows
/// (the `head` of them) followed by the body rows, `aligns` one per column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    pub rows: Vec<Vec<String>>,
    pub head: usize,
    pub aligns: Vec<Align>,
}

/// One of the row or column operations, as the palette names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    RowAbove,
    RowBelow,
    RowDelete,
    RowDuplicate,
    ColLeft,
    ColRight,
    ColDelete,
    ColMoveLeft,
    ColMoveRight,
    ColDuplicate,
    AlignLeft,
    AlignCenter,
    AlignRight,
}

impl Table {
    /// The lines of a table block, or `None` when they do not hold one
    /// (no separator row).
    pub fn parse(lines: &[String]) -> Option<Table> {
        let head = lines.iter().position(|l| md::is_table_rule(l))?;
        let (rule, _) = md::split_row(&lines[head]);
        let mut rows: Vec<Vec<String>> = lines
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != head)
            .map(|(_, l)| md::split_row(l).0.into_iter().map(|c| c.text).collect())
            .collect();
        let cols = rows
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(0)
            .max(rule.len())
            .max(1);
        for r in &mut rows {
            r.resize(cols, String::new());
        }
        let mut aligns: Vec<Align> = rule.iter().map(|c| md::align_of(&c.text)).collect();
        aligns.resize(cols, Align::Left);
        Some(Table { rows, head, aligns })
    }

    /// An empty table of `rows` body rows and `cols` columns, one header row.
    pub fn blank(rows: usize, cols: usize) -> Table {
        Table {
            rows: vec![vec![String::new(); cols]; rows + 1],
            head: 1,
            aligns: vec![Align::Left; cols],
        }
    }

    pub fn cols(&self) -> usize {
        self.aligns.len()
    }

    /// The row of the matrix that source line `src_row` of the block is,
    /// or `None` for the separator.
    pub fn row_of(&self, src_row: usize) -> Option<usize> {
        match src_row.cmp(&self.head) {
            std::cmp::Ordering::Less => Some(src_row),
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some(src_row - 1),
        }
    }

    /// The source line of the block that matrix row `row` is written to.
    pub fn src_row(&self, row: usize) -> usize {
        if row < self.head {
            row
        } else {
            row + 1
        }
    }

    /// The block's lines, cells padded to their columns so the source reads
    /// as a grid too.
    pub fn emit(&self) -> Vec<String> {
        let widths: Vec<usize> = (0..self.cols())
            .map(|c| {
                self.rows
                    .iter()
                    .map(|r| md::str_width(&r[c]))
                    .max()
                    .unwrap_or(0)
                    .max(3)
            })
            .collect();
        let line = |cells: Vec<String>| {
            let mut s = String::from("|");
            for (c, text) in cells.iter().enumerate() {
                let (left, right) = md::pad_for(md::str_width(text), widths[c], self.aligns[c]);
                s.push(' ');
                s.push_str(&" ".repeat(left));
                s.push_str(text);
                s.push_str(&" ".repeat(right));
                s.push_str(" |");
            }
            s
        };
        let rule: Vec<String> = widths
            .iter()
            .zip(&self.aligns)
            .map(|(w, a)| match a {
                Align::Left => "-".repeat(*w),
                Align::Center => format!(":{}:", "-".repeat(w - 2)),
                Align::Right => format!("{}:", "-".repeat(w - 1)),
            })
            .collect();
        let mut out = Vec::with_capacity(self.rows.len() + 1);
        for (i, r) in self.rows.iter().enumerate() {
            if i == self.head {
                out.push(line(rule.clone()));
            }
            out.push(line(r.clone()));
        }
        if self.head >= self.rows.len() {
            out.push(line(rule));
        }
        out
    }

    /// Apply `op` at cell (`row`, `col`). Returns the cell the cursor should
    /// land in afterwards, or `None` when the operation cannot be done here
    /// (the last column cannot be deleted, a header row cannot be moved).
    pub fn apply(&mut self, op: Op, row: usize, col: usize) -> Option<(usize, usize)> {
        let cols = self.cols();
        let blank = vec![String::new(); cols];
        match op {
            Op::RowAbove => {
                self.rows.insert(row, blank);
                if row < self.head {
                    self.head += 1;
                }
                Some((row, col))
            }
            Op::RowBelow => {
                self.rows.insert(row + 1, blank);
                if row + 1 < self.head {
                    self.head += 1;
                }
                Some((row + 1, col))
            }
            Op::RowDelete => {
                // a header row is the one thing a table cannot do without
                if row < self.head && self.head == 1 {
                    return None;
                }
                self.rows.remove(row);
                if row < self.head {
                    self.head -= 1;
                }
                if self.rows.is_empty() {
                    return None;
                }
                Some((row.min(self.rows.len() - 1), col))
            }
            Op::RowDuplicate => {
                let dup = self.rows[row].clone();
                self.rows.insert(row + 1, dup);
                if row + 1 < self.head {
                    self.head += 1;
                }
                Some((row + 1, col))
            }
            Op::ColLeft | Op::ColRight => {
                let at = if op == Op::ColLeft { col } else { col + 1 };
                for r in &mut self.rows {
                    r.insert(at, String::new());
                }
                self.aligns.insert(at, Align::Left);
                Some((row, at))
            }
            Op::ColDelete => {
                if cols == 1 {
                    return None;
                }
                for r in &mut self.rows {
                    r.remove(col);
                }
                self.aligns.remove(col);
                Some((row, col.min(cols - 2)))
            }
            Op::ColMoveLeft | Op::ColMoveRight => {
                let to = if op == Op::ColMoveLeft {
                    col.checked_sub(1)?
                } else {
                    (col + 1 < cols).then_some(col + 1)?
                };
                for r in &mut self.rows {
                    r.swap(col, to);
                }
                self.aligns.swap(col, to);
                Some((row, to))
            }
            Op::ColDuplicate => {
                for r in &mut self.rows {
                    let dup = r[col].clone();
                    r.insert(col + 1, dup);
                }
                self.aligns.insert(col + 1, self.aligns[col]);
                Some((row, col + 1))
            }
            Op::AlignLeft | Op::AlignCenter | Op::AlignRight => {
                self.aligns[col] = match op {
                    Op::AlignLeft => Align::Left,
                    Op::AlignCenter => Align::Center,
                    _ => Align::Right,
                };
                Some((row, col))
            }
        }
    }
}

/// A rectangle of cells: rows `r0..=r1`, columns `c0..=c1`, in matrix
/// coordinates, both ends inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub r0: usize,
    pub c0: usize,
    pub r1: usize,
    pub c1: usize,
}

impl Rect {
    /// The rectangle spanned by two corners, in any order.
    pub fn between((ar, ac): (usize, usize), (br, bc): (usize, usize)) -> Rect {
        Rect {
            r0: ar.min(br),
            c0: ac.min(bc),
            r1: ar.max(br),
            c1: ac.max(bc),
        }
    }

    /// Clipped to a table of `rows` × `cols`.
    pub fn clip(self, rows: usize, cols: usize) -> Rect {
        Rect {
            r0: self.r0.min(rows.saturating_sub(1)),
            c0: self.c0.min(cols.saturating_sub(1)),
            r1: self.r1.min(rows.saturating_sub(1)),
            c1: self.c1.min(cols.saturating_sub(1)),
        }
    }
}

impl Table {
    /// Empty every cell in `rect`.
    pub fn clear(&mut self, rect: Rect) {
        for r in rect.r0..=rect.r1.min(self.rows.len().saturating_sub(1)) {
            for c in rect.c0..=rect.c1.min(self.cols().saturating_sub(1)) {
                self.rows[r][c].clear();
            }
        }
    }

    /// The cells of `rect` as tab-separated rows: what the clipboard gets,
    /// and what a spreadsheet reads.
    pub fn tsv(&self, rect: Rect) -> String {
        let rect = rect.clip(self.rows.len(), self.cols());
        (rect.r0..=rect.r1)
            .map(|r| {
                (rect.c0..=rect.c1)
                    .map(|c| self.rows[r][c].replace('\t', " "))
                    .collect::<Vec<_>>()
                    .join("\t")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Write `block` into the table with its top-left at (`r`, `c`), adding
    /// rows and columns as needed. Returns the rectangle written.
    pub fn paste(&mut self, r: usize, c: usize, block: &[Vec<String>]) -> Rect {
        let height = block.len().max(1);
        let width = block.iter().map(Vec::len).max().unwrap_or(1).max(1);
        while self.cols() < c + width {
            let at = self.cols();
            for row in &mut self.rows {
                row.insert(at, String::new());
            }
            self.aligns.push(Align::Left);
        }
        while self.rows.len() < r + height {
            self.rows.push(vec![String::new(); self.cols()]);
        }
        for (i, row) in block.iter().enumerate() {
            for (j, text) in row.iter().enumerate() {
                self.rows[r + i][c + j] = text.clone();
            }
        }
        Rect {
            r0: r,
            c0: c,
            r1: r + height - 1,
            c1: c + width - 1,
        }
    }

    /// Remove rows `r0..=r1`. Refused when that would take the last header
    /// row or every row.
    pub fn delete_rows(&mut self, r0: usize, r1: usize) -> bool {
        let r1 = r1.min(self.rows.len().saturating_sub(1));
        if r0 > r1 || r1 + 1 - r0 >= self.rows.len() {
            return false;
        }
        let heads = (r0..=r1).filter(|r| *r < self.head).count();
        if heads >= self.head {
            return false;
        }
        self.rows.drain(r0..=r1);
        self.head -= heads;
        true
    }

    /// Remove columns `c0..=c1`, keeping at least one.
    pub fn delete_cols(&mut self, c0: usize, c1: usize) -> bool {
        let c1 = c1.min(self.cols().saturating_sub(1));
        if c0 > c1 || c1 + 1 - c0 >= self.cols() {
            return false;
        }
        for row in &mut self.rows {
            row.drain(c0..=c1);
        }
        self.aligns.drain(c0..=c1);
        true
    }

    /// Slide rows `r0..=r1` one step down or up, within the header or the
    /// body. Returns the rows' new range.
    pub fn move_rows(&mut self, r0: usize, r1: usize, down: bool) -> Option<(usize, usize)> {
        let other = if down { r1 + 1 } else { r0.checked_sub(1)? };
        if other >= self.rows.len() || (other < self.head) != (r0 < self.head) {
            return None;
        }
        let row = self.rows.remove(other);
        if down {
            self.rows.insert(r0, row);
            Some((r0 + 1, r1 + 1))
        } else {
            self.rows.insert(r1, row);
            Some((r0 - 1, r1 - 1))
        }
    }

    /// Slide columns `c0..=c1` one step right or left. Returns their new range.
    pub fn move_cols(&mut self, c0: usize, c1: usize, right: bool) -> Option<(usize, usize)> {
        let other = if right { c1 + 1 } else { c0.checked_sub(1)? };
        if other >= self.cols() {
            return None;
        }
        for row in &mut self.rows {
            let cell = row.remove(other);
            row.insert(if right { c0 } else { c1 }, cell);
        }
        let a = self.aligns.remove(other);
        self.aligns.insert(if right { c0 } else { c1 }, a);
        Some(if right { (c0 + 1, c1 + 1) } else { (c0 - 1, c1 - 1) })
    }
}

/// Clipboard text as a block of cells: tabs split columns, newlines rows.
/// One line with no tab is a single cell.
pub fn parse_tsv(text: &str) -> Vec<Vec<String>> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    text.split('\n')
        .map(|l| {
            l.trim_end_matches('\r')
                .split('\t')
                .map(str::to_string)
                .collect()
        })
        .collect()
}

/// Which cell of a source row column `col` is in, or would be: the index of
/// the cell whose text spans `col`, else the nearest cell in the direction
/// `forward`. `None` for a line with no cells.
pub fn cell_at(line: &str, col: usize, forward: bool) -> Option<usize> {
    let (cells, _) = md::split_row(line);
    if cells.is_empty() {
        return None;
    }
    if let Some(i) = cells.iter().position(|c| col >= c.start && col <= c.end()) {
        return Some(i);
    }
    let next = cells.iter().position(|c| c.start > col);
    let prev = cells.iter().rposition(|c| c.end() < col);
    if forward {
        next.or(prev)
    } else {
        prev.or(next)
    }
}

/// The source columns `(start, end)` of cell `i`'s text on `line`.
pub fn cell_span(line: &str, i: usize) -> Option<(usize, usize)> {
    let (cells, _) = md::split_row(line);
    cells.get(i).map(|c| (c.start, c.end()))
}

/// The source column at the end of cell `c`'s text on `line`, where the
/// cursor lands when it moves into the cell; 0 when there is no such cell.
pub fn cell_end(line: &str, c: usize) -> usize {
    cell_span(line, c).map_or(0, |(_, e)| e)
}

/// Where the cursor settles on a grid row: inside the cell it is in, or at
/// the edge of the nearest one when it has drifted into a pipe or padding.
/// `forward` says which neighbour wins from a gap.
pub fn settle(line: &str, col: usize, forward: bool) -> usize {
    let (cells, _) = md::split_row(line);
    let Some(i) = cell_at(line, col, forward) else {
        return col;
    };
    let c = &cells[i];
    if col < c.start {
        c.start
    } else if col > c.end() {
        c.end()
    } else {
        col
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(String::from).collect()
    }

    #[test]
    fn parse_and_emit_round_trip_a_padded_grid() {
        let t = Table::parse(&lines("| a | bb |\n|---|:--:|\n| c | d |")).unwrap();
        assert_eq!(t.head, 1);
        assert_eq!(t.aligns, vec![Align::Left, Align::Center]);
        assert_eq!(
            t.emit(),
            lines("| a   | bb  |\n| --- | :-: |\n| c   |  d  |")
        );
        assert_eq!(t.row_of(0), Some(0));
        assert_eq!(t.row_of(1), None);
        assert_eq!(t.row_of(2), Some(1));
        assert_eq!(t.src_row(1), 2);
    }

    #[test]
    fn ragged_rows_are_squared_off() {
        let t = Table::parse(&lines("| a |\n|---|\n| c | d |")).unwrap();
        assert_eq!(t.cols(), 2);
        assert_eq!(t.rows[0], vec!["a", ""]);
    }

    #[test]
    fn a_blank_table_is_two_by_two() {
        assert_eq!(
            Table::blank(1, 2).emit(),
            lines("|     |     |\n| --- | --- |\n|     |     |")
        );
    }

    #[test]
    fn row_operations() {
        let base = Table::parse(&lines("| h |\n|---|\n| a |\n| b |")).unwrap();
        let mut t = base.clone();
        assert_eq!(t.apply(Op::RowBelow, 1, 0), Some((2, 0)));
        assert_eq!(t.rows.len(), 4);
        assert_eq!(t.rows[2], vec![""]);
        let mut t = base.clone();
        assert_eq!(t.apply(Op::RowAbove, 0, 0), Some((0, 0)));
        assert_eq!(t.head, 2);
        let mut t = base.clone();
        assert_eq!(t.apply(Op::RowDelete, 0, 0), None);
        assert_eq!(t.apply(Op::RowDelete, 2, 0), Some((1, 0)));
        assert_eq!(t.rows.len(), 2);
        let mut t = base.clone();
        assert_eq!(t.apply(Op::RowDuplicate, 1, 0), Some((2, 0)));
        assert_eq!(t.rows[2], vec!["a"]);
    }

    #[test]
    fn column_operations() {
        let base = Table::parse(&lines("| a | b |\n|---|--:|\n| 1 | 2 |")).unwrap();
        let mut t = base.clone();
        assert_eq!(t.apply(Op::ColRight, 0, 0), Some((0, 1)));
        assert_eq!(t.rows[1], vec!["1", "", "2"]);
        assert_eq!(t.aligns, vec![Align::Left, Align::Left, Align::Right]);
        let mut t = base.clone();
        assert_eq!(t.apply(Op::ColLeft, 0, 0), Some((0, 0)));
        assert_eq!(t.rows[0], vec!["", "a", "b"]);
        let mut t = base.clone();
        assert_eq!(t.apply(Op::ColDelete, 0, 1), Some((0, 0)));
        assert_eq!(t.rows[0], vec!["a"]);
        assert_eq!(t.apply(Op::ColDelete, 0, 0), None);
        let mut t = base.clone();
        assert_eq!(t.apply(Op::ColMoveRight, 0, 0), Some((0, 1)));
        assert_eq!(t.rows[0], vec!["b", "a"]);
        assert_eq!(t.aligns, vec![Align::Right, Align::Left]);
        assert_eq!(t.apply(Op::ColMoveRight, 0, 1), None);
        let mut t = base.clone();
        assert_eq!(t.apply(Op::ColDuplicate, 0, 0), Some((0, 1)));
        assert_eq!(t.rows[1], vec!["1", "1", "2"]);
        let mut t = base.clone();
        t.apply(Op::AlignCenter, 0, 0);
        assert_eq!(t.emit()[1], "| :-: | --: |");
    }

    #[test]
    fn selections_clear_copy_paste_delete_and_move() {
        let base = Table::parse(&lines("| a | b | c |\n|---|---|---|\n| 1 | 2 | 3 |\n| 4 | 5 | 6 |")).unwrap();
        let rect = Rect::between((2, 2), (1, 1));
        assert_eq!(rect, Rect { r0: 1, c0: 1, r1: 2, c1: 2 });
        assert_eq!(base.tsv(rect), "2\t3\n5\t6");
        let mut t = base.clone();
        t.clear(rect);
        assert_eq!(t.rows[2], vec!["4", "", ""]);
        let mut t = base.clone();
        let block = parse_tsv("x\ty\tz\nq\n");
        assert_eq!(block, vec![vec!["x", "y", "z"], vec!["q"]]);
        let wrote = t.paste(2, 2, &block);
        assert_eq!(wrote, Rect { r0: 2, c0: 2, r1: 3, c1: 4 });
        assert_eq!(t.cols(), 5);
        assert_eq!(t.rows.len(), 4);
        assert_eq!(t.rows[2], vec!["4", "5", "x", "y", "z"]);
        assert_eq!(t.rows[3], vec!["", "", "q", "", ""]);
        let mut t = base.clone();
        assert!(!t.delete_rows(0, 0));
        assert!(t.delete_rows(1, 2));
        assert_eq!(t.rows.len(), 1);
        let mut t = base.clone();
        assert!(t.delete_cols(0, 1));
        assert_eq!(t.rows[0], vec!["c"]);
        assert!(!t.delete_cols(0, 0));
        let mut t = base.clone();
        assert_eq!(t.move_rows(1, 1, true), Some((2, 2)));
        assert_eq!(t.rows[1], vec!["4", "5", "6"]);
        assert_eq!(t.move_rows(0, 0, true), None);
        assert_eq!(t.move_cols(0, 1, true), Some((1, 2)));
        assert_eq!(t.rows[0], vec!["c", "a", "b"]);
        assert_eq!(t.move_cols(1, 2, true), None);
    }

    #[test]
    fn cursor_settles_into_cells() {
        let l = "| ab |  cd | ";
        assert_eq!(cell_span(l, 0), Some((2, 4)));
        assert_eq!(cell_span(l, 1), Some((8, 10)));
        assert_eq!(cell_end(l, 0), 4);
        assert_eq!(cell_end(l, 1), 10);
        assert_eq!(cell_end(l, 2), 0);
        assert_eq!(settle(l, 3, true), 3);
        assert_eq!(settle(l, 0, true), 2);
        assert_eq!(settle(l, 6, true), 8);
        assert_eq!(settle(l, 6, false), 4);
        assert_eq!(settle(l, 12, true), 10);
        assert_eq!(cell_at(l, 9, true), Some(1));
    }
}

//! Table editing in the editor: the hover handles at a table's edges, cell
//! selection, the keys that step and reshape a table, and the cell-level
//! mapping between the screen and the source.

use super::*;

/// The handles drawn at the edges of a hovered table: the add handles, and
/// the grips that select a row (by source line) or a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableHandle {
    AddColumn,
    AddRow,
    SelectRow(usize),
    SelectCol(usize),
}

/// Which edge of a table the pointer is at: right of its last column, under
/// its last row, in the gutter beside one row (by source line), or above it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableEdge {
    Right,
    Bottom,
    Left(usize),
    Top,
}

/// What a table selection is: a dragged block of cells, whole rows, or
/// whole columns. Rows and columns keep their other axis full however the
/// table changes shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelKind {
    Cells,
    Rows,
    Cols,
}

/// Selected cells of one table: the block (by its first line), the corner
/// the selection started from and the one it reaches, in the table's matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellSel {
    pub start: usize,
    pub anchor: (usize, usize),
    pub head: (usize, usize),
    pub kind: SelKind,
}

impl CellSel {
    /// The cells covered, for a table of `rows` × `cols`.
    pub fn rect(&self, rows: usize, cols: usize) -> crate::table::Rect {
        let mut r = crate::table::Rect::between(self.anchor, self.head);
        match self.kind {
            SelKind::Rows => {
                r.c0 = 0;
                r.c1 = cols.saturating_sub(1);
            }
            SelKind::Cols => {
                r.r0 = 0;
                r.r1 = rows.saturating_sub(1);
            }
            SelKind::Cells => {}
        }
        r.clip(rows, cols)
    }
}

impl App {
    /// The table edge the pointer at (x, y) is at, if any: on a handle it
    /// already shows, a few columns right of a table row, on the first row
    /// of the line under a table, or in the space under a table that ends
    /// the note.
    pub(super) fn table_edge_at(&self, x: u16, y: u16) -> Option<(usize, TableEdge)> {
        let at = ratatui::layout::Position { x, y };
        if self.table_handles.iter().any(|(r, _)| r.contains(at)) {
            return self.table_hover;
        }
        let blocks = self.blocks();
        let table = |line: usize| {
            md::block_at(&blocks, line)
                .filter(|b| b.kind == md::BlockKind::Table)
                .copied()
        };
        let hit = self
            .edit_rows
            .iter()
            .find(|r| y >= r.rect.y && y < r.rect.y + r.rect.height);
        let Some(hit) = hit else {
            // under everything drawn: the bottom of a table that ends the note
            let n = self.editor.lines().len();
            let last_drawn = self.edit_rows.last()?;
            if y >= last_drawn.rect.y + last_drawn.rect.height && last_drawn.line + 1 == n {
                return table(n - 1).map(|b| (b.start, TableEdge::Bottom));
            }
            return None;
        };
        let dx = x.saturating_sub(self.editor_area.x) as usize;
        if let Some(b) = table(hit.line) {
            if self.table_source == Some(b.start) {
                return None;
            }
            if dx < TABLE_GUTTER {
                // beside a row: its grip (the separator has none)
                return (!md::is_table_rule(&self.editor.lines()[hit.line]))
                    .then_some((b.start, TableEdge::Left(hit.line)));
            }
            let width = self.editor_area.width.max(1) as usize;
            let segs = self.wrapped(hit.line, &blocks, width);
            let grid: usize = segs
                .first()
                .map(|s| s.cells.iter().map(|c| c.ch).collect::<String>())
                .map(|t| md::str_width(&t))
                .unwrap_or(0);
            return (dx >= grid && dx < grid + 4).then_some((b.start, TableEdge::Right));
        }
        // the first row of the line right under a table
        if hit.seg == 0 && hit.line > 0 {
            if let Some(b) = table(hit.line - 1).filter(|b| b.end + 1 == hit.line) {
                return Some((b.start, TableEdge::Bottom));
            }
        }
        // the last row of the line right above a table
        if let Some(b) = table(hit.line + 1).filter(|b| b.start == hit.line + 1) {
            let last = self
                .edit_rows
                .iter()
                .filter(|r| r.line == hit.line)
                .map(|r| r.seg)
                .max()
                .unwrap_or(0);
            if hit.seg == last && hit.seg != usize::MAX {
                return Some((b.start, TableEdge::Top));
            }
        }
        None
    }

    /// Is the pointer at `edge` of the table `row` is in? `Top` only holds
    /// on the table's first line (the column grips are drawn over it) and
    /// `Bottom` only on its last; `Right` on every row of it (the
    /// add-column handle).
    pub fn hovered_table_edge(&self, blocks: &[md::Block], row: usize, edge: TableEdge) -> bool {
        self.view == View::Edit
            && md::block_at(blocks, row).is_some_and(|b| {
                b.kind == md::BlockKind::Table
                    && match edge {
                        TableEdge::Top => b.start == row,
                        TableEdge::Bottom => b.end == row,
                        _ => true,
                    }
                    && self.table_hover == Some((b.start, edge))
            })
    }

    /// The table selection's rectangle, with the table it is in.
    pub(super) fn selection_rect(
        &self,
    ) -> Option<(md::Block, crate::table::Table, crate::table::Rect)> {
        let sel = self.cell_sel?;
        let blocks = self.blocks();
        let block = *md::block_at(&blocks, sel.start).filter(|b| b.start == sel.start)?;
        let table = crate::table::Table::parse(&self.editor.lines()[block.start..=block.end])?;
        let rect = sel.rect(table.rows.len(), table.cols());
        Some((block, table, rect))
    }

    /// Put the cursor at the end of cell (`r`, `c`) of the table at `block`.
    pub(super) fn goto_cell(
        &mut self,
        block: md::Block,
        table: &crate::table::Table,
        r: usize,
        c: usize,
    ) {
        let src = block.start + table.src_row(r);
        let col = self
            .editor
            .lines()
            .get(src)
            .map_or(0, |l| crate::table::cell_end(l, c));
        self.editor.clear_selection();
        self.editor.set_cursor((src, col));
    }

    /// Grow (or start) a cell selection from the cursor's cell to the cell
    /// `dr`, `dc` away, and follow it with the cursor.
    pub(super) fn extend_cell_sel(&mut self, dr: isize, dc: isize) {
        let Some((block, table, r, c)) = self.table_cell() else {
            return;
        };
        let anchor = match self.cell_sel {
            Some(sel) if sel.start == block.start && sel.kind == SelKind::Cells => sel.anchor,
            _ => (r, c),
        };
        let head = (
            r.saturating_add_signed(dr).min(table.rows.len() - 1),
            c.saturating_add_signed(dc).min(table.cols() - 1),
        );
        self.cell_sel = Some(CellSel {
            start: block.start,
            anchor,
            head,
            kind: SelKind::Cells,
        });
        self.goto_cell(block, &table, head.0, head.1);
    }

    /// Empty the selected cells, keeping the selection.
    pub(super) fn clear_selected_cells(&mut self) {
        let Some((block, mut table, rect)) = self.selection_rect() else {
            return;
        };
        table.clear(rect);
        let (r, c) = self.cell_sel.map(|s| s.head).unwrap_or((rect.r0, rect.c0));
        let keep = self.cell_sel;
        self.write_table(
            block,
            &table,
            (r.min(table.rows.len() - 1), c.min(table.cols() - 1)),
        );
        self.cell_sel = keep;
    }

    /// ⌘C over a table selection: tab-separated rows to the clipboard.
    pub(super) fn copy_cells(&mut self) -> bool {
        let Some((_, table, rect)) = self.selection_rect() else {
            return false;
        };
        let text = table.tsv(rect);
        let n = (rect.r1 + 1 - rect.r0) * (rect.c1 + 1 - rect.c0);
        if crate::clipboard::copy(&text) {
            self.flash(format!("copied {n} cells"));
        } else {
            self.flash("copy failed".to_string());
        }
        true
    }

    /// ⌘X over a table selection: copy, then clear the cells — or take the
    /// rows or columns out altogether when whole ones are selected.
    pub(super) fn cut_cells(&mut self) -> bool {
        let Some((block, mut table, rect)) = self.selection_rect() else {
            return false;
        };
        let kind = self.cell_sel.map(|s| s.kind).unwrap_or(SelKind::Cells);
        if !crate::clipboard::copy(&table.tsv(rect)) {
            self.flash("copy failed — nothing cut".to_string());
            return true;
        }
        let removed = match kind {
            SelKind::Rows => table.delete_rows(rect.r0, rect.r1),
            SelKind::Cols => table.delete_cols(rect.c0, rect.c1),
            SelKind::Cells => false,
        };
        if !removed {
            table.clear(rect);
        }
        self.cell_sel = None;
        let to = (
            rect.r0.min(table.rows.len() - 1),
            rect.c0.min(table.cols() - 1),
        );
        self.write_table(block, &table, to);
        self.flash(if removed { "cut" } else { "cut cells" }.to_string());
        true
    }

    /// ⌘V in a grid: a block of cells (tabs and newlines) goes in cell by
    /// cell from the cursor's cell — or the selection's corner — growing the
    /// table to fit; plain text goes into the cell like typing. Returns
    /// false when the cursor is not in a grid.
    pub(super) fn paste_cells(&mut self, text: &str) -> bool {
        let Some((block, mut table, r, c)) = self.table_cell() else {
            return false;
        };
        let (r, c) = match self.selection_rect() {
            Some((b, _, rect)) if b.start == block.start => (rect.r0, rect.c0),
            _ => (r, c),
        };
        let block_cells = crate::table::parse_tsv(text);
        let single = block_cells.len() == 1 && block_cells[0].len() == 1;
        if single {
            // one cell's worth: type it in, newlines and all flattened
            self.cell_sel = None;
            let flat = text.replace(['\n', '\r'], " ");
            self.editor.insert_str(&flat);
            self.sync_editor_to_note();
            return true;
        }
        let wrote = table.paste(r, c, &block_cells);
        self.write_table(block, &table, (wrote.r1, wrote.c1));
        self.cell_sel = Some(CellSel {
            start: block.start,
            anchor: (wrote.r0, wrote.c0),
            head: (wrote.r1, wrote.c1),
            kind: SelKind::Cells,
        });
        self.flash(format!(
            "pasted {} cells",
            (wrote.r1 + 1 - wrote.r0) * (wrote.c1 + 1 - wrote.c0)
        ));
        true
    }

    /// ⌥↑ ⌥↓: the selected rows, or the cursor's, one step. The selection
    /// moves with them.
    pub(super) fn move_table_rows(&mut self, down: bool) {
        let Some((block, mut table, r, c)) = self.table_cell() else {
            return;
        };
        let (r0, r1) = match self.selection_rect() {
            Some((b, _, rect)) if b.start == block.start => (rect.r0, rect.r1),
            _ => (r, r),
        };
        let Some((n0, _)) = table.move_rows(r0, r1, down) else {
            self.flash("nowhere to move".to_string());
            return;
        };
        let sel = self.cell_sel.map(|s| CellSel {
            anchor: (s.anchor.0 + n0 - r0, s.anchor.1),
            head: (s.head.0 + n0 - r0, s.head.1),
            ..s
        });
        self.write_table(block, &table, (r + n0 - r0, c));
        self.cell_sel = sel;
    }

    /// ⌥← ⌥→: the selected columns, or the cursor's, one step.
    pub(super) fn move_table_cols(&mut self, right: bool) {
        let Some((block, mut table, r, c)) = self.table_cell() else {
            return;
        };
        let (c0, c1) = match self.selection_rect() {
            Some((b, _, rect)) if b.start == block.start => (rect.c0, rect.c1),
            _ => (c, c),
        };
        let Some((n0, _)) = table.move_cols(c0, c1, right) else {
            self.flash("nowhere to move".to_string());
            return;
        };
        let sel = self.cell_sel.map(|s| CellSel {
            anchor: (s.anchor.0, s.anchor.1 + n0 - c0),
            head: (s.head.0, s.head.1 + n0 - c0),
            ..s
        });
        self.write_table(block, &table, (r, c + n0 - c0));
        self.cell_sel = sel;
    }

    /// The table block the cursor is in while it is drawn as a grid, with the
    /// cursor's row and column in the grid's matrix: what every table command
    /// acts on. `None` outside a table, on its separator, or while the
    /// table's source is showing.
    pub fn table_cell(&self) -> Option<(md::Block, crate::table::Table, usize, usize)> {
        self.table_cell_at(self.editor.cursor)
            .filter(|(block, ..)| self.table_source != Some(block.start))
    }

    /// The table block at `pos`, parsed, with `pos`'s row and column in its
    /// matrix. `None` outside a table or on its separator.
    pub(super) fn table_cell_at(
        &self,
        (row, col): (usize, usize),
    ) -> Option<(md::Block, crate::table::Table, usize, usize)> {
        let blocks = self.blocks();
        let block = *md::block_at(&blocks, row).filter(|b| b.kind == md::BlockKind::Table)?;
        let lines = self.editor.lines();
        let table = crate::table::Table::parse(&lines[block.start..=block.end])?;
        let r = table.row_of(row - block.start)?;
        let c = crate::table::cell_at(&lines[row], col, true)?;
        Some((block, table, r, c))
    }

    /// Does a rule sit under `row` in the editor: a table row with another
    /// row of the same table beneath it, the separator aside (it is drawn as
    /// the rule under the head already).
    pub fn table_rule_under(&self, blocks: &[md::Block], row: usize) -> bool {
        let lines = self.editor.lines();
        md::block_at(blocks, row).is_some_and(|b| {
            b.kind == md::BlockKind::Table
                && row < b.end
                && !md::is_table_rule(&lines[row])
                && !md::is_table_rule(&lines[row + 1])
        })
    }

    /// A table that ends the note has nothing under it to move to, so the
    /// step past its last row makes the line. Returns true when it did.
    pub(super) fn step_below_table(&mut self) -> bool {
        let n = self.editor.lines().len();
        let blocks = self.blocks();
        let ends_note =
            md::block_at(&blocks, n - 1).is_some_and(|b| b.kind == md::BlockKind::Table);
        if !ends_note {
            return false;
        }
        self.editor.insert_lines(n, vec![String::new()], (n, 0));
        self.sync_editor_to_note();
        true
    }

    /// Is the cursor in a table drawn as a grid?
    pub(super) fn in_table_grid(&self) -> bool {
        let row = self.editor.cursor.0;
        let blocks = self.blocks();
        md::block_at(&blocks, row)
            .is_some_and(|b| b.kind == md::BlockKind::Table && self.table_source != Some(b.start))
    }

    /// The keys that mean something else with the cursor in a grid: tab and
    /// enter walk and grow the table, esc shows its source, and a delete at a
    /// cell's edge is refused rather than allowed to eat a pipe. Returns true
    /// when the key was taken.
    pub(super) fn table_key(&mut self, key: KeyEvent) -> bool {
        let (row, col) = self.editor.cursor;
        // esc works both ways: grid to source, and source back to grid
        if key.code == KeyCode::Esc && self.editor.selection().is_none() {
            let blocks = self.blocks();
            if md::block_at(&blocks, row).is_some_and(|b| b.kind == md::BlockKind::Table) {
                self.toggle_table_source();
                return true;
            }
        }
        if !self.in_table_grid() {
            return false;
        }
        let modified = key
            .modifiers
            .intersects(KeyModifiers::SUPER | KeyModifiers::CONTROL | KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let alt_only = key.modifiers == KeyModifiers::ALT;
        let selected = self.cell_sel.is_some();
        // a selection first: the keys that act on it, and the ones that drop it
        match key.code {
            KeyCode::Esc if selected => {
                self.cell_sel = None;
                return true;
            }
            KeyCode::Backspace | KeyCode::Delete if selected => {
                self.clear_selected_cells();
                return true;
            }
            KeyCode::Char(_) if selected && !modified => {
                // typing replaces the selection: clear, then the key types
                // into the cursor's cell as usual
                self.clear_selected_cells();
                self.cell_sel = None;
                if let Some((block, table, r, c)) = self.table_cell() {
                    self.goto_cell(block, &table, r, c);
                }
                return false;
            }
            KeyCode::Up if alt_only => {
                self.move_table_rows(false);
                return true;
            }
            KeyCode::Down if alt_only => {
                self.move_table_rows(true);
                return true;
            }
            KeyCode::Left if alt_only => {
                self.move_table_cols(false);
                return true;
            }
            KeyCode::Right if alt_only => {
                self.move_table_cols(true);
                return true;
            }
            KeyCode::Up if shift && !modified => {
                self.extend_cell_sel(-1, 0);
                return true;
            }
            KeyCode::Down if shift && !modified => {
                self.extend_cell_sel(1, 0);
                return true;
            }
            KeyCode::Left | KeyCode::Right if shift && !modified => {
                // text within the cell until the edge; whole cells past it
                let line = &self.editor.lines()[row];
                let edge = crate::table::cell_at(line, col, true)
                    .and_then(|i| crate::table::cell_span(line, i))
                    .is_some_and(|(s, e)| {
                        if key.code == KeyCode::Left {
                            col <= s
                        } else {
                            col >= e
                        }
                    });
                if selected || edge {
                    self.editor.clear_selection();
                    self.extend_cell_sel(0, if key.code == KeyCode::Left { -1 } else { 1 });
                    return true;
                }
                return false;
            }
            _ => {}
        }
        if selected && matches!(key.code, KeyCode::Tab | KeyCode::BackTab | KeyCode::Enter) {
            self.cell_sel = None;
        }
        match key.code {
            KeyCode::Tab if !modified => {
                self.table_step(true);
                true
            }
            KeyCode::BackTab => {
                self.table_step(false);
                true
            }
            KeyCode::Enter if !modified => {
                self.table_op(crate::table::Op::RowBelow);
                true
            }
            KeyCode::Down
                if !modified
                    && !key.modifiers.contains(KeyModifiers::SHIFT)
                    && row + 1 == self.editor.lines().len() =>
            {
                self.step_below_table()
            }
            KeyCode::Backspace | KeyCode::Delete => {
                let line = &self.editor.lines()[row];
                let Some(i) = crate::table::cell_at(line, col, true) else {
                    return true;
                };
                let (start, end) = crate::table::cell_span(line, i).unwrap_or((col, col));
                let at_edge = if key.code == KeyCode::Backspace {
                    col <= start
                } else {
                    col >= end
                };
                // a modified delete reaches past the cell; a plain one at
                // the edge has nothing in the cell to take
                if modified || at_edge {
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    /// Tab / shift-tab in a grid: the next or previous cell, and past the
    /// last cell a new row.
    pub(super) fn table_step(&mut self, forward: bool) {
        let Some((block, mut table, r, c)) = self.table_cell() else {
            return;
        };
        let cols = table.cols();
        let (nr, nc) = if forward {
            if c + 1 < cols {
                (r, c + 1)
            } else if r + 1 < table.rows.len() {
                (r + 1, 0)
            } else {
                let Some(to) = table.apply(crate::table::Op::RowBelow, r, 0) else {
                    return;
                };
                self.write_table(block, &table, to);
                return;
            }
        } else if c > 0 {
            (r, c - 1)
        } else if r > 0 {
            (r - 1, cols - 1)
        } else {
            return;
        };
        let src = block.start + table.src_row(nr);
        let col = crate::table::cell_end(&self.editor.lines()[src], nc);
        self.editor.set_cursor((src, col));
    }

    /// One of the palette's row or column commands on the cursor's cell.
    pub(super) fn table_op(&mut self, op: crate::table::Op) {
        use crate::table::Op;
        self.enter_edit_view();
        let Some((block, mut table, r, c)) = self.table_cell() else {
            self.flash("not in a table".to_string());
            return;
        };
        // over a selection, the deleting and aligning commands take all of it
        if let Some((_, _, rect)) = self
            .selection_rect()
            .filter(|(b, _, _)| b.start == block.start)
        {
            let done = match op {
                Op::RowDelete => Some(table.delete_rows(rect.r0, rect.r1)),
                Op::ColDelete => Some(table.delete_cols(rect.c0, rect.c1)),
                Op::AlignLeft | Op::AlignCenter | Op::AlignRight => {
                    for col in rect.c0..=rect.c1 {
                        table.apply(op, r, col);
                    }
                    Some(true)
                }
                _ => None,
            };
            if let Some(ok) = done {
                if !ok {
                    self.flash("can't do that here".to_string());
                    return;
                }
                let keep = matches!(op, Op::AlignLeft | Op::AlignCenter | Op::AlignRight)
                    .then_some(self.cell_sel)
                    .flatten();
                let to = (r.min(table.rows.len() - 1), c.min(table.cols() - 1));
                self.write_table(block, &table, to);
                self.cell_sel = keep;
                return;
            }
        }
        self.cell_sel = None;
        match table.apply(op, r, c) {
            Some(to) => self.write_table(block, &table, to),
            None => self.flash("can't do that here".to_string()),
        }
    }

    /// Put `table` back over `block`'s lines and leave the cursor in cell
    /// `(r, c)` of it.
    pub(super) fn write_table(
        &mut self,
        block: md::Block,
        table: &crate::table::Table,
        (r, c): (usize, usize),
    ) {
        let lines = table.emit();
        let src = block.start + table.src_row(r);
        let col = lines
            .get(table.src_row(r))
            .map_or(0, |l| crate::table::cell_end(l, c));
        self.editor
            .replace_lines(block.start, block.end, lines, (src, col));
        self.sync_editor_to_note();
    }

    /// A 2×2 table at the cursor, on a paragraph of its own.
    pub(super) fn insert_table(&mut self) {
        let with = crate::table::Table::blank(1, 2).emit();
        let col = crate::table::cell_span(&with[0], 0)
            .map(|(_, e)| e)
            .unwrap_or(2);
        self.insert_block(with, 0, col);
    }

    /// Esc in a grid: show this table's pipes until the cursor leaves it, and
    /// esc again puts the grid back.
    pub(super) fn toggle_table_source(&mut self) {
        self.enter_edit_view();
        let row = self.editor.cursor.0;
        let blocks = self.blocks();
        let Some(block) = md::block_at(&blocks, row).filter(|b| b.kind == md::BlockKind::Table)
        else {
            self.flash("not in a table".to_string());
            return;
        };
        self.table_source = if self.table_source == Some(block.start) {
            None
        } else {
            Some(block.start)
        };
    }

    /// After any move: a cursor in a grid sits in a cell, never on a pipe,
    /// in the padding or on the separator row; and a table whose source was
    /// showing goes back to a grid once the cursor has left it. `before` is
    /// where the cursor was, which says which way it was going.
    pub(super) fn settle_table_cursor(&mut self, before: Pos) {
        let (row, col) = self.editor.cursor;
        let blocks = self.blocks();
        let block = md::block_at(&blocks, row)
            .filter(|b| b.kind == md::BlockKind::Table)
            .copied();
        if let Some(start) = self.table_source {
            if block.map(|b| b.start) != Some(start) {
                self.table_source = None;
            }
        }
        if let Some(sel) = self.cell_sel {
            if block.map(|b| b.start) != Some(sel.start) {
                self.cell_sel = None;
            }
        }
        let Some(block) = block else {
            return;
        };
        if self.table_source == Some(block.start) || self.editor.selection().is_some() {
            return;
        }
        let down = row > before.0 || (row == before.0 && col >= before.1);
        let mut row = row;
        if md::is_table_rule(&self.editor.lines()[row]) {
            // the separator is drawn, not edited: step over it
            let n = self.editor.lines().len();
            row = if down {
                (row + 1).min(n - 1)
            } else {
                row.saturating_sub(1)
            };
            if row == before.0 || !block.contains(row) {
                self.editor.set_cursor(before);
                return;
            }
        }
        let forward = row != before.0 || col > before.1;
        let col = crate::table::settle(&self.editor.lines()[row], col, forward);
        self.editor.set_cursor((row, col));
    }

    /// A drag that started in a grid cell: across other cells it selects the
    /// block between; within the cell it selects text as anywhere.
    pub(super) fn drag_cells(&mut self, x: u16, y: u16) {
        let Some((start, from, press)) = self.table_drag else {
            return;
        };
        let pos = self.pos_at(x, y);
        let here = self.table_cell_at(pos).filter(|(b, ..)| b.start == start);
        match here {
            Some((b, t, r, c)) if (r, c) != from => {
                self.cell_sel = Some(CellSel {
                    start,
                    anchor: from,
                    head: (r, c),
                    kind: SelKind::Cells,
                });
                self.goto_cell(b, &t, r, c);
            }
            Some(_) => {
                // back in the cell it started in: text selection within it
                self.cell_sel = None;
                let line = &self.editor.lines()[pos.0];
                let col = crate::table::settle(line, pos.1, pos.1 >= press.1);
                self.editor.anchor = Some(press);
                self.editor.set_cursor((pos.0, col));
            }
            None => {}
        }
    }

    /// A click on one of the handles beside a hovered table: a column on the
    /// right, or a row at the bottom.
    pub(super) fn table_handle(&mut self, handle: TableHandle) {
        let Some((start, _)) = self.table_hover else {
            return;
        };
        let blocks = self.blocks();
        let Some(block) = md::block_at(&blocks, start).copied() else {
            return;
        };
        let Some(mut table) =
            crate::table::Table::parse(&self.editor.lines()[block.start..=block.end])
        else {
            return;
        };
        let last = table.rows.len().saturating_sub(1);
        let to = match handle {
            TableHandle::AddColumn => table.apply(crate::table::Op::ColRight, 0, table.cols() - 1),
            TableHandle::AddRow => table.apply(crate::table::Op::RowBelow, last, 0),
            TableHandle::SelectRow(line) => {
                if let Some(r) = table.row_of(line - block.start) {
                    self.select_rows(block.start, r, r);
                    self.goto_cell(block, &table, r, 0);
                }
                return;
            }
            TableHandle::SelectCol(c) => {
                self.select_cols(block.start, c, c);
                self.goto_cell(block, &table, 0, c);
                return;
            }
        };
        if let Some(to) = to {
            self.table_source = None;
            self.write_table(block, &table, to);
        }
    }
}

/// The text of `cells` between two display columns, as drawn. Columns rather
/// than indices because that is what a pointer lands on, and a wide character
/// covers two of them. `offset` is the column the first cell stands for — the
/// pan, for a row of a scrolling table.
pub(super) fn slice_cells(
    cells: &[crate::render::PCell],
    offset: usize,
    from: usize,
    to: usize,
) -> String {
    let mut out = String::new();
    let mut col = offset;
    for c in cells {
        let w = md::char_width(c.ch);
        if col >= from && col < to {
            out.push(c.ch);
        }
        col += w;
    }
    out.trim_end().to_string()
}

/// Where a click on display column `dcol` of a rendered preview row lands in
/// the source. Rendered rows carry the source position of every character they
/// drew, so wrapped continuations, table cells, indented code and quote bars all
/// map back exactly; scaffolding the renderer invented has no source position,
/// so the nearest real character on the row wins.
pub(super) fn cell_source(cells: &[crate::render::PCell], dcol: usize) -> Option<Pos> {
    let mut hit = cells.len();
    let mut x = 0;
    for (i, c) in cells.iter().enumerate() {
        let w = md::char_width(c.ch);
        if dcol < x + w {
            hit = i;
            break;
        }
        x += w;
    }
    cells[hit.min(cells.len())..]
        .iter()
        .find_map(|c| c.src)
        .or_else(|| {
            cells[..hit.min(cells.len())]
                .iter()
                .rev()
                .find_map(|c| c.src.map(|(l, col)| (l, col + 1)))
        })
}

/// Screen cell → (buffer line, display column), undoing the centred column's
/// origin and the editor's scroll. Clicks left of/above the page clamp to it.
pub(super) fn screen_to_cell(area: Rect, scroll: usize, x: u16, y: u16) -> (usize, usize) {
    (
        scroll + y.saturating_sub(area.y) as usize,
        x.saturating_sub(area.x) as usize,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preview_selection_takes_the_text_it_covers_by_column() {
        let cells = pcells("date  │  company");
        // a selection that starts mid-word takes only what it covers
        assert_eq!(slice_cells(&cells, 0, 0, 4), "date");
        assert_eq!(slice_cells(&cells, 0, 9, usize::MAX), "company");
        // trailing padding a table put there is not worth copying
        assert_eq!(slice_cells(&cells, 0, 0, 8), "date  │");
        // a column range past the end of the row is simply the rest of it
        assert_eq!(slice_cells(&cells, 0, 99, usize::MAX), "");
        // a panned row's first cell stands for a later column
        assert_eq!(slice_cells(&cells, 6, 6, 10), "date");
    }

    fn pcells(text: &str) -> Vec<crate::render::PCell> {
        text.chars()
            .map(|ch| crate::render::PCell {
                ch,
                style: ratatui::style::Style::default(),
                link: None,
                src: None,
            })
            .collect()
    }

    #[test]
    fn preview_clicks_map_through_the_rendered_row() {
        // a code block: the preview indents by two, the source does not
        let r = crate::render::render("```\nlet x = 1;\n```\n");
        let row = r.lines.iter().find(|l| l.text().contains("let x")).unwrap();
        // display column 6 is "x" — source line 1, column 4
        assert_eq!(cell_source(&row.cells, 6), Some((1, 4)));

        // a bulleted item: the "• " is ours, the text is the file's
        let r = crate::render::render("- hello\n");
        let row = r.lines.iter().find(|l| l.text().contains("hello")).unwrap();
        assert_eq!(cell_source(&row.cells, 2), Some((0, 2)));
    }

    #[test]
    fn clicks_map_through_the_centred_column_and_scroll() {
        let area = Rect::new(20, 3, 40, 10);
        assert_eq!(screen_to_cell(area, 0, 20, 3), (0, 0));
        assert_eq!(screen_to_cell(area, 0, 25, 5), (2, 5));
        // scrolled: row 7 of the buffer sits on the top screen row
        assert_eq!(screen_to_cell(area, 7, 22, 3), (7, 2));
        // clicks in the left gutter clamp to the start of the line
        assert_eq!(screen_to_cell(area, 0, 0, 0), (0, 0));
    }

    #[test]
    fn clicks_land_on_source_columns_past_hidden_markers() {
        let area = Rect::new(10, 0, 40, 10);
        let line = md::style_line("- [x] done");
        let (_, dcol) = screen_to_cell(area, 0, 12, 0);
        // display "✓ done": column 2 is "d", source column 6
        assert_eq!(line.one_row().display_to_source(dcol), 6);
        // clicking well past the text lands at end of the source line
        let (_, far) = screen_to_cell(area, 0, 39, 0);
        assert_eq!(line.one_row().display_to_source(far), 10);
    }
}

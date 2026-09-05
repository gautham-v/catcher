//! A small line-based text buffer: the note being edited.
//!
//! Deliberately plain — no vim motions. Lines are `String`s and
//! positions are (line, char column); long lines are soft-wrapped by the view,
//! which owns the wrapping because it also owns the styling. Everything the
//! live-preview view needs is here.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub type Pos = (usize, usize);

/// What an edit was, for the purpose of grouping undo steps. Consecutive
/// edits of the same kind fold into one step, so undo goes back by word or by
/// run of deletions rather than by keystroke.
#[derive(Clone, Copy, PartialEq)]
enum EditKind {
    Type,
    Delete,
    /// Never coalesces: pastes, newlines, line kills, structural edits.
    Other,
}

/// The whole buffer before one edit. Notes are small; copying the lines is far
/// simpler than a diff, and cheap enough not to notice.
#[derive(Clone)]
struct Snapshot {
    lines: Vec<String>,
    cursor: Pos,
    anchor: Option<Pos>,
}

/// How many undo steps to keep. Well past any plausible reach-back.
const HISTORY_LIMIT: usize = 400;

#[derive(Default)]
pub struct Editor {
    lines: Vec<String>,
    pub cursor: Pos,
    /// Selection start; the cursor is the other end. `None` when nothing is selected.
    pub anchor: Option<Pos>,
    /// First visible source line.
    pub scroll: usize,
    /// Whether the view should chase the cursor on the next draw. Key handling
    /// sets it; the mouse wheel clears it, so a scrolled page stays put.
    follow_cursor: bool,
    /// The file ended with a newline, and should again after a save.
    trailing_newline: bool,
    /// The file used CRLF line endings, and should again after a save.
    crlf: bool,
    /// Pre-edit states, oldest first, and the states undone away from.
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// The kind of the last edit, or `None` when the next edit must start a
    /// fresh undo step (after a cursor move, an undo, or a group break).
    last_kind: Option<EditKind>,
    /// Non-zero while a compound edit is running, so its inner mutations don't
    /// each record a step of their own.
    batch: usize,
    /// Spaces one `tab` inserts. Set from the settings; never zero.
    pub tab_width: usize,
}

impl Editor {
    pub fn new(content: &str) -> Editor {
        let mut lines: Vec<String> = content.lines().map(String::from).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        Editor {
            lines,
            follow_cursor: true,
            trailing_newline: content.ends_with('\n'),
            crlf: content.contains("\r\n"),
            tab_width: 2,
            ..Default::default()
        }
    }

    /// Whether the view is still chasing the cursor. The draw needs to know:
    /// with images on screen a line is not always one row, so the vertical
    /// scroll gets a second, height-aware pass — but only when following.
    pub fn following(&self) -> bool {
        self.follow_cursor
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// The buffer as it should be written back: the file's own line ending and
    /// its trailing newline are preserved, so notes shared with git or another
    /// editor aren't rewritten by merely opening them.
    pub fn text(&self) -> String {
        let sep = if self.crlf { "\r\n" } else { "\n" };
        let mut out = self.lines.join(sep);
        if self.trailing_newline {
            out.push_str(sep);
        }
        out
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            lines: self.lines.clone(),
            cursor: self.cursor,
            anchor: self.anchor,
        }
    }

    /// Record the state *before* an edit. Same-kind edits in a row coalesce
    /// into the step already on the stack; any edit clears the redo branch.
    fn record(&mut self, kind: EditKind) {
        if self.batch > 0 {
            return;
        }
        self.redo.clear();
        let coalesce =
            kind != EditKind::Other && self.last_kind == Some(kind) && !self.undo.is_empty();
        self.last_kind = Some(kind);
        if coalesce {
            return;
        }
        self.undo.push(self.snapshot());
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
    }

    fn restore(&mut self, s: Snapshot) {
        self.lines = s.lines;
        self.anchor = s.anchor;
        self.cursor = self.clamp(s.cursor);
        self.follow_cursor = true;
        self.last_kind = None;
    }

    /// Step back one edit. Returns false when there is nothing left to undo.
    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else {
            return false;
        };
        let now = self.snapshot();
        self.redo.push(now);
        self.restore(prev);
        true
    }

    /// Step forward again after an undo.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        let now = self.snapshot();
        self.undo.push(now);
        self.restore(next);
        true
    }

    pub fn select_all(&mut self) {
        self.anchor = Some((0, 0));
        self.cursor = self.doc_end();
        self.follow_cursor = true;
        self.last_kind = None;
    }

    fn line_len(&self, row: usize) -> usize {
        self.lines.get(row).map_or(0, |l| l.chars().count())
    }

    /// Clamp an arbitrary (line, col) onto the buffer.
    pub fn clamp(&self, (row, col): Pos) -> Pos {
        let row = row.min(self.lines.len().saturating_sub(1));
        (row, col.min(self.line_len(row)))
    }

    fn byte_index(&self, (row, col): Pos) -> usize {
        self.lines[row]
            .char_indices()
            .nth(col)
            .map_or(self.lines[row].len(), |(b, _)| b)
    }

    /// Selection as an ordered (start, end) pair, if any text is selected.
    pub fn selection(&self) -> Option<(Pos, Pos)> {
        let a = self.anchor?;
        if a == self.cursor {
            return None;
        }
        Some(if a <= self.cursor {
            (a, self.cursor)
        } else {
            (self.cursor, a)
        })
    }

    /// Selected source columns on `row`, as a half-open range.
    pub fn selection_on(&self, row: usize) -> Option<(usize, usize)> {
        let ((sr, sc), (er, ec)) = self.selection()?;
        if row < sr || row > er {
            return None;
        }
        let start = if row == sr { sc } else { 0 };
        let end = if row == er {
            ec
        } else {
            self.line_len(row) + 1 // include the newline, so the row reads as full
        };
        Some((start, end))
    }

    pub fn selected_text(&self) -> Option<String> {
        let ((sr, sc), (er, ec)) = self.selection()?;
        if sr == er {
            let s: String = self.lines[sr].chars().skip(sc).take(ec - sc).collect();
            return Some(s);
        }
        let mut out: String = self.lines[sr].chars().skip(sc).collect();
        for line in &self.lines[sr + 1..er] {
            out.push('\n');
            out.push_str(line);
        }
        out.push('\n');
        out.extend(self.lines[er].chars().take(ec));
        Some(out)
    }

    /// Replace one line wholesale (the preview's checkbox toggle).
    pub fn set_line(&mut self, row: usize, text: String) {
        if row >= self.lines.len() {
            return;
        }
        self.record(EditKind::Other);
        if let Some(line) = self.lines.get_mut(row) {
            *line = text;
            self.cursor = self.clamp(self.cursor);
        }
    }

    /// Swap the whole buffer for `content` as a single undo step: what a
    /// reload after another program wrote the file does. The cursor stays on
    /// its line number, clamped to the new length, and the scroll is left
    /// where it was, so the page does not jump for an edit made elsewhere.
    pub fn replace_all(&mut self, content: &str) {
        self.record(EditKind::Other);
        // and the step after this must not fold into it
        self.last_kind = None;
        let fresh = Editor::new(content);
        self.lines = fresh.lines;
        self.trailing_newline = fresh.trailing_newline;
        self.crlf = fresh.crlf;
        self.anchor = None;
        self.cursor = self.clamp(self.cursor);
        self.scroll = self.scroll.min(self.lines.len().saturating_sub(1));
    }

    /// The rows a line-wise command acts on: the selection's rows, or the
    /// cursor's. A selection that ends at column 0 of a row stops short of
    /// that row — dragging down to the start of the next line has not
    /// selected it.
    pub fn selected_rows(&self) -> (usize, usize) {
        match self.selection() {
            Some(((sr, _), (er, ec))) if ec == 0 && er > sr => (sr, er - 1),
            Some(((sr, _), (er, _))) => (sr, er),
            None => (self.cursor.0, self.cursor.0),
        }
    }

    /// Rewrite every selected row (or the cursor row) with `f`, as one undo
    /// step. The cursor and anchor stay on their rows, clamped to whatever
    /// length the rows have afterwards.
    pub fn map_selected_lines(&mut self, f: impl Fn(&str) -> String) {
        let (from, to) = self.selected_rows();
        self.record(EditKind::Other);
        for row in from..=to {
            let new = f(&self.lines[row]);
            self.lines[row] = new;
        }
        self.cursor = self.clamp(self.cursor);
        self.anchor = self.anchor.map(|a| self.clamp(a));
        self.follow_cursor = true;
    }

    /// Swap the selected rows (or the cursor row) with the line beyond them,
    /// carrying the cursor and the selection along. False at either edge.
    pub fn move_selected_lines(&mut self, down: bool) -> bool {
        let (from, to) = self.selected_rows();
        let can = if down {
            to + 1 < self.lines.len()
        } else {
            from > 0
        };
        if !can {
            return false;
        }
        self.record(EditKind::Other);
        if down {
            let line = self.lines.remove(to + 1);
            self.lines.insert(from, line);
        } else {
            let line = self.lines.remove(from - 1);
            self.lines.insert(to, line);
        }
        let by = if down { 1 } else { -1 };
        self.cursor.0 = self.cursor.0.wrapping_add_signed(by);
        self.anchor = self.anchor.map(|(r, c)| (r.wrapping_add_signed(by), c));
        self.follow_cursor = true;
        true
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    pub fn set_cursor(&mut self, pos: Pos) {
        self.cursor = self.clamp(pos);
        self.follow_cursor = true;
    }

    /// Move the cursor, extending the selection when `select` is set. Public
    /// for the view's display-row Up/Down, which needs the wrapping to know
    /// where the next visual row is.
    pub fn move_cursor(&mut self, pos: Pos, select: bool) {
        self.follow_cursor = true;
        self.move_to(pos, select);
    }

    /// Move the cursor, extending the selection when `select` is set.
    fn move_to(&mut self, pos: Pos, select: bool) {
        if select {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = self.clamp(pos);
    }

    pub fn delete_selection(&mut self) -> bool {
        if self.selection().is_none() {
            return false;
        }
        self.record(EditKind::Other);
        self.remove_selection()
    }

    /// The selection removal itself, without touching the undo history: the
    /// callers below have already recorded their own step.
    fn remove_selection(&mut self) -> bool {
        let Some(((sr, sc), (er, ec))) = self.selection() else {
            return false;
        };
        let head: String = self.lines[sr].chars().take(sc).collect();
        let tail: String = self.lines[er].chars().skip(ec).collect();
        self.lines.splice(sr..=er, [head + &tail]);
        self.cursor = (sr, sc);
        self.anchor = None;
        true
    }

    pub fn insert_char(&mut self, c: char) {
        self.record(EditKind::Type);
        // a run of word characters is one undo step; anything else ends it
        if !c.is_alphanumeric() {
            self.last_kind = None;
        }
        self.remove_selection();
        let at = self.byte_index(self.cursor);
        self.lines[self.cursor.0].insert(at, c);
        self.cursor.1 += 1;
    }

    /// Insert arbitrary text at the cursor, newlines and all (a paste).
    pub fn insert_str(&mut self, text: &str) {
        self.record(EditKind::Other);
        self.batch += 1;
        self.remove_selection();
        for (i, part) in text.replace("\r\n", "\n").split('\n').enumerate() {
            if i > 0 {
                self.insert_newline();
            }
            for c in part.chars() {
                if c != '\r' {
                    self.insert_char(c);
                }
            }
        }
        self.batch -= 1;
    }

    pub fn insert_newline(&mut self) {
        self.record(EditKind::Other);
        self.batch += 1;
        self.remove_selection();
        self.batch -= 1;
        let (row, col) = self.cursor;
        let at = self.byte_index((row, col));
        let tail = self.lines[row].split_off(at);
        self.lines.insert(row + 1, tail);
        self.cursor = (row + 1, 0);
    }

    /// Enter in a list item: carry the marker onto the new line the way
    /// Obsidian does. `- a` continues as `- `, `3. a` as `4. `, `- [x] a`
    /// as `- [ ] `. Enter on an item with no text clears the marker instead,
    /// so a double Enter ends the list. Anywhere but the end of a line this
    /// is a plain newline.
    pub fn insert_newline_continuing_list(&mut self) {
        let (row, col) = self.cursor;
        if self.anchor.is_some() || col != self.line_len(row) {
            self.insert_newline();
            return;
        }
        let Some((prefix, body_empty)) = list_continuation(&self.lines[row]) else {
            self.insert_newline();
            return;
        };
        self.record(EditKind::Other);
        self.batch += 1;
        if body_empty {
            self.lines[row].clear();
            self.cursor.1 = 0;
        } else {
            self.insert_newline();
            for c in prefix.chars() {
                self.insert_char(c);
            }
        }
        self.batch -= 1;
        self.last_kind = None;
    }

    pub fn backspace(&mut self) {
        self.record(EditKind::Delete);
        if self.remove_selection() {
            return;
        }
        let (row, col) = self.cursor;
        if col > 0 {
            let at = self.byte_index((row, col - 1));
            self.lines[row].remove(at);
            self.cursor.1 = col - 1;
        } else if row > 0 {
            let line = self.lines.remove(row);
            let prev = self.line_len(row - 1);
            self.lines[row - 1].push_str(&line);
            self.cursor = (row - 1, prev);
        }
    }

    pub fn delete_forward(&mut self) {
        self.record(EditKind::Delete);
        if self.remove_selection() {
            return;
        }
        let (row, col) = self.cursor;
        if col < self.line_len(row) {
            let at = self.byte_index((row, col));
            self.lines[row].remove(at);
        } else if row + 1 < self.lines.len() {
            let next = self.lines.remove(row + 1);
            self.lines[row].push_str(&next);
        }
    }

    fn left(&self) -> Pos {
        let (row, col) = self.cursor;
        if col > 0 {
            (row, col - 1)
        } else if row > 0 {
            (row - 1, self.line_len(row - 1))
        } else {
            (0, 0)
        }
    }

    fn right(&self) -> Pos {
        let (row, col) = self.cursor;
        if col < self.line_len(row) {
            (row, col + 1)
        } else if row + 1 < self.lines.len() {
            (row + 1, 0)
        } else {
            self.cursor
        }
    }

    /// Delete from the cursor back to the start of the line (Cmd+Backspace).
    /// On an already-empty prefix this joins with the previous line, like a
    /// plain backspace, so the key is never a no-op mid-buffer.
    pub fn delete_to_line_start(&mut self) {
        self.record(EditKind::Other);
        self.batch += 1;
        self.kill_to_line_start();
        self.batch -= 1;
    }

    fn kill_to_line_start(&mut self) {
        if self.remove_selection() {
            return;
        }
        let (row, col) = self.cursor;
        if col == 0 {
            self.backspace();
            return;
        }
        let at = self.byte_index((row, col));
        let tail = self.lines[row][at..].to_string();
        self.lines[row] = tail;
        self.cursor = (row, 0);
    }

    /// Delete the word before the cursor (Option+Backspace).
    pub fn delete_prev_word(&mut self) {
        self.record(EditKind::Delete);
        self.batch += 1;
        self.kill_prev_word();
        self.batch -= 1;
    }

    fn kill_prev_word(&mut self) {
        if self.remove_selection() {
            return;
        }
        let (row, col) = self.cursor;
        if col == 0 {
            self.backspace();
            return;
        }
        let start = self.word_left(row, col);
        let from = self.byte_index((row, start));
        let to = self.byte_index((row, col));
        self.lines[row].replace_range(from..to, "");
        self.cursor = (row, start);
    }

    /// The last position in the buffer.
    fn doc_end(&self) -> Pos {
        let last = self.lines.len().saturating_sub(1);
        (last, self.line_len(last))
    }

    /// Handle one key. Returns true if the buffer changed.
    pub fn on_key(&mut self, key: KeyEvent) -> bool {
        self.follow_cursor = true;
        let m = key.modifiers;
        let select = m.contains(KeyModifiers::SHIFT);
        // Cmd, when the terminal reports it (kitty keyboard protocol).
        let line_wise = m.contains(KeyModifiers::SUPER);
        // Option/Alt — and Ctrl, which many terminals send for word motion.
        let word =
            !line_wise && (m.contains(KeyModifiers::ALT) || m.contains(KeyModifiers::CONTROL));
        let (row, col) = self.cursor;

        // The legacy bytes a terminal sends for the Mac editing keys when it
        // does *not* report Cmd/Option on the key itself. Ghostty's default
        // macOS keybinds rewrite them this way before the kitty keyboard
        // protocol ever gets a look in, so ⌘← arrives as a plain Ctrl-A and
        // this is the only path that sees it. They are the classic readline
        // chords besides, so binding them costs nothing.
        if let Some(changed) = self.legacy_chord(key, select) {
            if !changed {
                self.last_kind = None;
            }
            return changed;
        }

        match key.code {
            // ctrl-chords belong to the app, not the buffer
            KeyCode::Char(_) if key.modifiers.contains(KeyModifiers::CONTROL) => {}
            KeyCode::Char(c) => {
                self.insert_char(c);
                return true;
            }
            KeyCode::Enter => {
                self.insert_newline_continuing_list();
                return true;
            }
            KeyCode::Backspace => {
                if line_wise {
                    self.delete_to_line_start();
                } else if word {
                    self.delete_prev_word();
                } else {
                    self.backspace();
                }
                return true;
            }
            KeyCode::Delete => {
                self.delete_forward();
                return true;
            }
            KeyCode::Tab => {
                for _ in 0..self.tab_width.max(1) {
                    self.insert_char(' ');
                }
                return true;
            }
            KeyCode::Left => {
                let to = if line_wise {
                    (row, 0)
                } else if word && col > 0 {
                    (row, self.word_left(row, col))
                } else {
                    self.left()
                };
                self.move_to(to, select);
            }
            KeyCode::Right => {
                let to = if line_wise {
                    (row, self.line_len(row))
                } else if word && col < self.line_len(row) {
                    (row, self.word_right(row, col))
                } else {
                    self.right()
                };
                self.move_to(to, select);
            }
            KeyCode::Up if line_wise => self.move_to((0, 0), select),
            KeyCode::Down if line_wise => {
                let end = self.doc_end();
                self.move_to(end, select);
            }
            KeyCode::Up => self.move_to((row.saturating_sub(1), col), select),
            KeyCode::Down => self.move_to((row + 1, col), select),
            KeyCode::Home => self.move_to((row, 0), select),
            KeyCode::End => self.move_to((row, self.line_len(row)), select),
            KeyCode::PageUp => self.move_to((row.saturating_sub(10), col), select),
            KeyCode::PageDown => self.move_to((row + 10, col), select),
            KeyCode::Esc => self.clear_selection(),
            _ => {}
        }
        // nothing changed, so this was a movement: the next typed run is a
        // new undo step rather than a continuation of the one before it
        self.last_kind = None;
        false
    }

    /// The legacy Mac-editing chords, or `None` if this key isn't one.
    /// `Some(changed)` mirrors [`Editor::on_key`]'s own return.
    fn legacy_chord(&mut self, key: KeyEvent, select: bool) -> Option<bool> {
        let m = key.modifiers;
        let ctrl = m.contains(KeyModifiers::CONTROL);
        let alt = m.contains(KeyModifiers::ALT);
        let (row, col) = self.cursor;
        match key.code {
            // ⌘← / ⌘→ reach us as Ctrl-A / Ctrl-E
            KeyCode::Char('a') if ctrl => self.move_to((row, 0), select),
            KeyCode::Char('e') if ctrl => self.move_to((row, self.line_len(row)), select),
            // ⌘⌫ as Ctrl-U, ⌥⌫ as Ctrl-W
            KeyCode::Char('u') if ctrl => {
                self.delete_to_line_start();
                return Some(true);
            }
            KeyCode::Char('w') if ctrl => {
                self.delete_prev_word();
                return Some(true);
            }
            // ⌥← / ⌥→ as ESC b / ESC f
            KeyCode::Char('b') if alt && !ctrl => {
                let to = if col > 0 {
                    (row, self.word_left(row, col))
                } else {
                    self.left()
                };
                self.move_to(to, select);
            }
            KeyCode::Char('f') if alt && !ctrl => {
                let to = if col < self.line_len(row) {
                    (row, self.word_right(row, col))
                } else {
                    self.right()
                };
                self.move_to(to, select);
            }
            // ⌘↑ / ⌘↓ have no legacy encoding at all — Ghostty keeps them for
            // its own scrollback jumping — so Ctrl-Home / Ctrl-End stand in,
            // which is what most terminals send for them anyway.
            KeyCode::Home if ctrl => self.move_to((0, 0), select),
            KeyCode::End if ctrl => {
                let end = self.doc_end();
                self.move_to(end, select);
            }
            _ => return None,
        }
        Some(false)
    }

    fn word_left(&self, row: usize, col: usize) -> usize {
        let chars: Vec<char> = self.lines[row].chars().collect();
        let mut i = col;
        while i > 0 && !chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        while i > 0 && chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        i
    }

    fn word_right(&self, row: usize, col: usize) -> usize {
        let chars: Vec<char> = self.lines[row].chars().collect();
        let mut i = col;
        while i < chars.len() && !chars[i].is_alphanumeric() {
            i += 1;
        }
        while i < chars.len() && chars[i].is_alphanumeric() {
            i += 1;
        }
        i
    }

    /// Scroll the page without moving the cursor (the mouse wheel). The view
    /// stops chasing the cursor until the next keystroke.
    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.lines.len().saturating_sub(1);
        self.scroll = (self.scroll as isize + delta).clamp(0, max as isize) as usize;
        self.follow_cursor = false;
    }

    /// Keep the cursor's line on screen for a viewport of `height` rows. This
    /// is the coarse, one-row-per-line pass; the draw refines it in display
    /// rows once it knows how many rows each line wrapped to. Does nothing
    /// while the page is being scrolled by hand.
    pub fn scroll_into_view(&mut self, height: usize) {
        if !self.follow_cursor {
            return;
        }
        let height = height.max(1);
        if self.cursor.0 < self.scroll {
            self.scroll = self.cursor.0;
        } else if self.cursor.0 >= self.scroll + height {
            self.scroll = self.cursor.0 + 1 - height;
        }
    }
}

/// If `line` is a list item, the prefix its successor should start with and
/// whether the item has no text after its marker.
fn list_continuation(line: &str) -> Option<(String, bool)> {
    let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    let (indent, rest) = line.split_at(indent_len);
    let (marker, body) = if let Some(rest) = rest
        .strip_prefix("- ")
        .or_else(|| rest.strip_prefix("* "))
        .or_else(|| rest.strip_prefix("+ "))
    {
        let mark = &line[indent_len..indent_len + 2];
        if let Some(body) = rest
            .strip_prefix("[ ] ")
            .or_else(|| rest.strip_prefix("[x] "))
            .or_else(|| rest.strip_prefix("[X] "))
        {
            (format!("{mark}[ ] "), body)
        } else {
            (mark.to_string(), rest)
        }
    } else {
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 {
            return None;
        }
        let after = &rest[digits..];
        let punct = after.chars().next().filter(|c| matches!(c, '.' | ')'))?;
        let body = after[1..].strip_prefix(' ')?;
        let n: u64 = rest[..digits].parse().ok()?;
        (format!("{}{punct} ", n + 1), body)
    };
    Some((format!("{indent}{marker}"), body.trim().is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed() -> Editor {
        Editor::new("one\ntwo\nthree")
    }

    #[test]
    fn insert_and_backspace() {
        let mut e = Editor::new("ab");
        e.set_cursor((0, 1));
        e.insert_char('X');
        assert_eq!(e.text(), "aXb");
        e.backspace();
        assert_eq!(e.text(), "ab");
    }

    #[test]
    fn newline_splits_and_backspace_joins() {
        let mut e = Editor::new("hello");
        e.set_cursor((0, 2));
        e.insert_newline();
        assert_eq!(e.text(), "he\nllo");
        assert_eq!(e.cursor, (1, 0));
        e.backspace();
        assert_eq!(e.text(), "hello");
        assert_eq!(e.cursor, (0, 2));
    }

    #[test]
    fn enter_continues_lists() {
        let cases = [
            ("- a", "- a\n- "),
            ("  * a", "  * a\n  * "),
            ("3. a", "3. a\n4. "),
            ("9) a", "9) a\n10) "),
            ("- [x] a", "- [x] a\n- [ ] "),
            ("plain", "plain\n"),
            ("-a", "-a\n"),
        ];
        for (before, after) in cases {
            let mut e = Editor::new(before);
            e.set_cursor((0, before.chars().count()));
            e.insert_newline_continuing_list();
            assert_eq!(e.text(), after, "{before:?}");
            assert_eq!(e.cursor, (1, after.len() - before.len() - 1));
        }
    }

    #[test]
    fn enter_on_empty_item_ends_the_list() {
        let mut e = Editor::new("- a\n- ");
        e.set_cursor((1, 2));
        e.insert_newline_continuing_list();
        assert_eq!(e.text(), "- a\n");
        assert_eq!(e.cursor, (1, 0));
        // one undo restores the marker
        e.undo();
        assert_eq!(e.text(), "- a\n- ");
    }

    #[test]
    fn enter_mid_item_is_a_plain_newline() {
        let mut e = Editor::new("1. ab");
        e.set_cursor((0, 4));
        e.insert_newline_continuing_list();
        assert_eq!(e.text(), "1. a\nb");
        e.undo();
        assert_eq!(e.text(), "1. ab");
        e.set_cursor((0, 5));
        e.insert_newline_continuing_list();
        assert_eq!(e.text(), "1. ab\n2. ");
        e.undo();
        assert_eq!(e.text(), "1. ab");
    }

    #[test]
    fn multiline_selection_text_and_delete() {
        let mut e = ed();
        e.anchor = Some((0, 1));
        e.cursor = (2, 2);
        assert_eq!(e.selected_text().unwrap(), "ne\ntwo\nth");
        assert_eq!(e.selection_on(1), Some((0, 4)));
        e.delete_selection();
        assert_eq!(e.text(), "oree");
        assert_eq!(e.cursor, (0, 1));
    }

    #[test]
    fn paste_inserts_multiple_lines() {
        let mut e = Editor::new("ab");
        e.set_cursor((0, 1));
        e.insert_str("X\r\nY");
        assert_eq!(e.text(), "aX\nYb");
        assert_eq!(e.cursor, (1, 1));
    }

    #[test]
    fn clamp_keeps_positions_inside() {
        let e = ed();
        assert_eq!(e.clamp((9, 9)), (2, 5));
        assert_eq!(e.clamp((1, 99)), (1, 3));
    }

    #[test]
    fn saving_keeps_the_file_s_trailing_newline_and_line_endings() {
        assert_eq!(Editor::new("a\nb\n").text(), "a\nb\n");
        assert_eq!(Editor::new("a\nb").text(), "a\nb");
        assert_eq!(Editor::new("a\r\nb\r\n").text(), "a\r\nb\r\n");
    }

    #[test]
    fn wheel_scrolling_is_not_snapped_back_to_the_cursor() {
        let mut e = Editor::new(&"x\n".repeat(50));
        e.scroll_by(10);
        e.scroll_into_view(5);
        assert_eq!(
            e.scroll, 10,
            "the wheel's scroll must survive the next frame"
        );
        // a keystroke puts the view back on the cursor
        e.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        e.scroll_into_view(5);
        assert_eq!(e.scroll, e.cursor.0);
    }

    #[test]
    fn line_wise_commands_take_the_selection_s_rows_or_the_cursor_s() {
        let mut e = ed();
        e.set_cursor((1, 2));
        assert_eq!(e.selected_rows(), (1, 1));
        e.anchor = Some((0, 1));
        e.cursor = (2, 1);
        assert_eq!(e.selected_rows(), (0, 2));
        // a selection ending at the start of a row has not taken that row
        e.cursor = (2, 0);
        assert_eq!(e.selected_rows(), (0, 1));
    }

    #[test]
    fn mapping_lines_is_one_undo_step_and_keeps_the_selection() {
        let mut e = ed();
        e.anchor = Some((0, 0));
        e.cursor = (1, 3);
        e.map_selected_lines(|l| format!("- {l}"));
        assert_eq!(e.text(), "- one\n- two\nthree");
        assert_eq!(e.selection(), Some(((0, 0), (1, 3))));
        assert!(e.undo());
        assert_eq!(e.text(), "one\ntwo\nthree");
        // a line that shrinks pulls the cursor back onto it
        let mut e = Editor::new("abcdef");
        e.set_cursor((0, 6));
        e.map_selected_lines(|_| "ab".to_string());
        assert_eq!(e.cursor, (0, 2));
    }

    #[test]
    fn moving_lines_carries_the_cursor_and_stops_at_the_edges() {
        let mut e = ed();
        e.set_cursor((0, 2));
        assert!(!e.move_selected_lines(false));
        assert!(e.move_selected_lines(true));
        assert_eq!(e.text(), "two\none\nthree");
        assert_eq!(e.cursor, (1, 2));
        assert!(e.move_selected_lines(true));
        assert_eq!(e.text(), "two\nthree\none");
        assert!(!e.move_selected_lines(true));
        assert!(e.undo());
        assert_eq!(e.text(), "two\none\nthree");
        assert_eq!(e.cursor, (1, 2));
    }

    #[test]
    fn a_selection_moves_as_a_block() {
        let mut e = Editor::new("a\nb\nc\nd");
        e.anchor = Some((1, 0));
        e.cursor = (2, 1);
        assert!(e.move_selected_lines(false));
        assert_eq!(e.text(), "b\nc\na\nd");
        assert_eq!(e.selection(), Some(((0, 0), (1, 1))));
        assert!(e.move_selected_lines(true));
        assert!(e.move_selected_lines(true));
        assert_eq!(e.text(), "a\nd\nb\nc");
        assert_eq!(e.selection(), Some(((2, 0), (3, 1))));
    }

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    const SUPER: KeyModifiers = KeyModifiers::SUPER;
    const ALT: KeyModifiers = KeyModifiers::ALT;
    const SHIFT: KeyModifiers = KeyModifiers::SHIFT;

    #[test]
    fn cmd_arrows_go_to_line_and_document_ends() {
        let mut e = Editor::new("one two\nthree four\nfive");
        e.set_cursor((1, 4));
        e.on_key(key(KeyCode::Left, SUPER));
        assert_eq!(e.cursor, (1, 0));
        e.on_key(key(KeyCode::Right, SUPER));
        assert_eq!(e.cursor, (1, 10));
        e.on_key(key(KeyCode::Up, SUPER));
        assert_eq!(e.cursor, (0, 0));
        e.on_key(key(KeyCode::Down, SUPER));
        assert_eq!(e.cursor, (2, 4));
        // Home/End still work for terminals without the protocol
        e.on_key(key(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(e.cursor, (2, 0));
        e.on_key(key(KeyCode::End, KeyModifiers::NONE));
        assert_eq!(e.cursor, (2, 4));
    }

    #[test]
    fn option_arrows_move_by_word_and_cross_lines_at_the_edges() {
        let mut e = Editor::new("alpha beta\ngamma");
        e.set_cursor((0, 10));
        e.on_key(key(KeyCode::Left, ALT));
        assert_eq!(e.cursor, (0, 6));
        e.on_key(key(KeyCode::Left, ALT));
        assert_eq!(e.cursor, (0, 0));
        // at the very start of a line, word-left falls back to joining motion
        e.on_key(key(KeyCode::Right, ALT));
        assert_eq!(e.cursor, (0, 5));
        e.set_cursor((0, 10));
        e.on_key(key(KeyCode::Right, ALT));
        assert_eq!(e.cursor, (1, 0), "past the end of a line, wrap to the next");
    }

    const CTRL: KeyModifiers = KeyModifiers::CONTROL;

    /// Exactly what Ghostty's default macOS keybinds put on the wire for the
    /// Mac editing keys — the kitty protocol never sees them.
    #[test]
    fn the_legacy_mac_editing_chords_move_and_delete() {
        let mut e = Editor::new("alpha beta\ngamma delta\nlast");
        e.set_cursor((1, 6));
        e.on_key(key(KeyCode::Char('a'), CTRL)); // ⌘←
        assert_eq!(e.cursor, (1, 0));
        e.on_key(key(KeyCode::Char('e'), CTRL)); // ⌘→
        assert_eq!(e.cursor, (1, 11));
        e.on_key(key(KeyCode::Char('b'), ALT)); // ⌥←
        assert_eq!(e.cursor, (1, 6));
        e.on_key(key(KeyCode::Char('f'), ALT)); // ⌥→
        assert_eq!(e.cursor, (1, 11));
        // ⌘↑ / ⌘↓ stand-ins
        e.on_key(key(KeyCode::Home, CTRL));
        assert_eq!(e.cursor, (0, 0));
        e.on_key(key(KeyCode::End, CTRL));
        assert_eq!(e.cursor, (2, 4));

        // ⌘⌫ and ⌥⌫
        let mut e = Editor::new("alpha beta");
        e.set_cursor((0, 10));
        assert!(e.on_key(key(KeyCode::Char('w'), CTRL)));
        assert_eq!(e.text(), "alpha ");
        assert!(e.on_key(key(KeyCode::Char('u'), CTRL)));
        assert_eq!(e.text(), "");
        // ⌥⌫ also arrives as ESC DEL, which crossterm reports as alt-backspace
        let mut e = Editor::new("alpha beta");
        e.set_cursor((0, 10));
        assert!(e.on_key(key(KeyCode::Backspace, ALT)));
        assert_eq!(e.text(), "alpha ");
    }

    #[test]
    fn the_legacy_chords_extend_a_selection_and_leave_plain_letters_alone() {
        let mut e = Editor::new("alpha beta");
        e.set_cursor((0, 10));
        e.on_key(key(KeyCode::Char('a'), CTRL | SHIFT));
        assert_eq!(e.selected_text().unwrap(), "alpha beta");
        // an unmodified letter is still just typed, chord letters included
        let mut e = Editor::new("");
        for c in "abefuw".chars() {
            assert!(e.on_key(key(KeyCode::Char(c), KeyModifiers::NONE)));
        }
        assert_eq!(e.text(), "abefuw");
    }

    #[test]
    fn shift_extends_the_selection_for_the_new_movements() {
        let mut e = Editor::new("alpha beta\ngamma");
        e.set_cursor((0, 10));
        e.on_key(key(KeyCode::Left, SUPER | SHIFT));
        assert_eq!(e.selected_text().unwrap(), "alpha beta");
        e.set_cursor((0, 10));
        e.clear_selection();
        e.on_key(key(KeyCode::Left, ALT | SHIFT));
        assert_eq!(e.selected_text().unwrap(), "beta");
        e.set_cursor((0, 0));
        e.clear_selection();
        e.on_key(key(KeyCode::Down, SUPER | SHIFT));
        assert_eq!(e.selected_text().unwrap(), "alpha beta\ngamma");
    }

    #[test]
    fn cmd_backspace_kills_to_the_line_start() {
        let mut e = Editor::new("one\nhello there");
        e.set_cursor((1, 6));
        e.on_key(key(KeyCode::Backspace, SUPER));
        assert_eq!(e.text(), "one\nthere");
        assert_eq!(e.cursor, (1, 0));
        // already at column 0: joins with the line above, like a plain backspace
        e.on_key(key(KeyCode::Backspace, SUPER));
        assert_eq!(e.text(), "onethere");
        assert_eq!(e.cursor, (0, 3));
    }

    #[test]
    fn option_backspace_kills_the_previous_word() {
        let mut e = Editor::new("alpha beta gamma");
        e.set_cursor((0, 16));
        e.on_key(key(KeyCode::Backspace, ALT));
        assert_eq!(e.text(), "alpha beta ");
        e.on_key(key(KeyCode::Backspace, ALT));
        assert_eq!(e.text(), "alpha ");
        assert_eq!(e.cursor, (0, 6));
    }

    #[test]
    fn word_deletion_is_char_wise_over_unicode() {
        let mut e = Editor::new("café crème");
        e.set_cursor((0, 10));
        e.on_key(key(KeyCode::Backspace, ALT));
        assert_eq!(e.text(), "café ");
    }

    #[test]
    fn a_modified_backspace_deletes_the_selection_first() {
        let mut e = Editor::new("alpha beta");
        e.anchor = Some((0, 0));
        e.cursor = (0, 6);
        e.on_key(key(KeyCode::Backspace, ALT));
        assert_eq!(e.text(), "beta");
        assert_eq!(e.anchor, None);
    }

    #[test]
    fn undo_walks_back_word_runs_and_redo_walks_forward() {
        let mut e = Editor::new("");
        for c in "hello world".chars() {
            e.insert_char(c);
        }
        assert_eq!(e.text(), "hello world");
        // a run of word characters is one step, and the space that ends the
        // run belongs to the run before it
        assert!(e.undo());
        assert_eq!(e.text(), "hello ");
        assert!(e.undo());
        assert_eq!(e.text(), "");
        assert!(!e.undo(), "nothing left to undo");
        assert!(e.redo());
        assert_eq!(e.text(), "hello ");
        assert!(e.redo());
        assert_eq!(e.text(), "hello world");
        assert!(!e.redo());
    }

    #[test]
    fn replacing_the_buffer_is_one_undo_step_that_keeps_the_cursor_line() {
        let mut e = Editor::new("one\ntwo\nthree\n");
        e.set_cursor((2, 3));
        e.replace_all("uno\ndos\n");
        assert_eq!(e.lines(), ["uno", "dos"]);
        // past the end: clamped to the last line, and to its length
        assert_eq!(e.cursor, (1, 3));
        assert!(e.undo());
        assert_eq!(e.text(), "one\ntwo\nthree\n");
        assert!(e.redo());
        assert_eq!(e.text(), "uno\ndos\n");
    }

    #[test]
    fn typing_after_a_replace_does_not_fold_into_it() {
        let mut e = Editor::new("abc\n");
        e.insert_char('x');
        e.replace_all("new\n");
        e.insert_char('y');
        assert!(e.undo());
        assert_eq!(e.text(), "new\n");
        assert!(e.undo());
        assert_eq!(e.text(), "xabc\n");
    }

    #[test]
    fn a_paste_is_one_undo_step_and_restores_the_cursor() {
        let mut e = Editor::new("ab");
        e.set_cursor((0, 1));
        e.insert_str("X\nY\nZ");
        assert_eq!(e.text(), "aX\nY\nZb");
        assert!(e.undo());
        assert_eq!(e.text(), "ab");
        assert_eq!(e.cursor, (0, 1));
    }

    #[test]
    fn a_new_edit_after_an_undo_drops_the_redo_branch() {
        let mut e = Editor::new("");
        e.insert_str("one");
        e.insert_str("two");
        e.undo();
        assert_eq!(e.text(), "one");
        e.insert_str("three");
        assert!(!e.redo(), "the redo branch was abandoned by the new edit");
        assert_eq!(e.text(), "onethree");
    }

    #[test]
    fn moving_the_cursor_starts_a_fresh_undo_step() {
        let mut e = Editor::new("");
        for c in "abc".chars() {
            e.insert_char(c);
        }
        e.on_key(key(KeyCode::Left, KeyModifiers::NONE));
        e.insert_char('X');
        assert_eq!(e.text(), "abXc");
        e.undo();
        assert_eq!(
            e.text(),
            "abc",
            "the typing before the move is a separate step"
        );
    }

    #[test]
    fn deletions_coalesce_and_undo_restores_the_selection_they_ate() {
        let mut e = Editor::new("alpha beta");
        e.set_cursor((0, 10));
        for _ in 0..4 {
            e.backspace();
        }
        assert_eq!(e.text(), "alpha ");
        assert!(e.undo());
        assert_eq!(e.text(), "alpha beta");

        let mut e = Editor::new("alpha beta");
        e.anchor = Some((0, 0));
        e.cursor = (0, 6);
        e.insert_char('X');
        assert_eq!(e.text(), "Xbeta");
        e.undo();
        assert_eq!(e.text(), "alpha beta");
        assert_eq!(e.selected_text().unwrap(), "alpha ");
    }

    #[test]
    fn select_all_spans_the_whole_buffer() {
        let mut e = ed();
        e.select_all();
        assert_eq!(e.selected_text().unwrap(), "one\ntwo\nthree");
    }

    #[test]
    fn unicode_columns_are_chars_not_bytes() {
        let mut e = Editor::new("café");
        e.set_cursor((0, 4));
        e.backspace();
        assert_eq!(e.text(), "caf");
    }
}

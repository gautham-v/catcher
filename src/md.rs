//! Line-based markdown styling shared by the live-preview editor and the
//! full-page preview renderer.
//!
//! A source line becomes a [`RLine`]: one display cell per visible character,
//! each remembering which source column it came from. Keeping the mapping at
//! cell granularity makes both directions trivial — cursor placement from a
//! click, and selection highlighting from source columns — even when markers
//! like `## ` or `- [ ] ` are hidden or replaced.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;
use crate::theme;

/// How many terminal columns a character occupies. Zero-width characters are
/// still given a column so the cursor has somewhere to sit.
pub fn char_width(ch: char) -> usize {
    ch.width().unwrap_or(0).max(1)
}

/// The first character of a theme marker string.
fn first_char(s: &str) -> char {
    s.chars().next().unwrap_or(' ')
}

/// Display width of a string, in terminal columns.
pub fn str_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// How many leading items, each `widths` columns wide, fit in `width` with a
/// column left over for an ellipsis: the index of the first one that does not.
pub fn cut_at(widths: impl Iterator<Item = usize>, width: usize) -> usize {
    let room = width.saturating_sub(1);
    let mut used = 0;
    let mut n = 0;
    for cw in widths {
        if used + cw > room {
            break;
        }
        used += cw;
        n += 1;
    }
    n
}

/// Column alignment of a table column, shared by both views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// The `" │ "` between two table columns.
pub const COL_SEP: &str = " │ ";

/// Widest cell in each column, in display columns.
pub fn column_widths(rows: &[Vec<usize>], cols: usize) -> Vec<usize> {
    let mut widths = vec![0usize; cols];
    for row in rows {
        for (i, w) in row.iter().enumerate().take(cols) {
            widths[i] = widths[i].max(*w);
        }
    }
    widths
}

/// Padding to put on each side of a cell of `content` columns in a column of
/// `width`, for `align`.
pub fn pad_for(content: usize, width: usize, align: Align) -> (usize, usize) {
    let pad = width.saturating_sub(content);
    match align {
        Align::Right => (pad, 0),
        Align::Center => (pad / 2, pad - pad / 2),
        Align::Left => (0, pad),
    }
}

/// Narrowest a column is ever squeezed to: one character plus the ellipsis.
pub const MIN_COL: usize = 2;

/// Shrink `widths` until the whole table fits `total` display columns.
///
/// Columns are cut widest-first, so one runaway cell (a pasted URL) gives up
/// its space before the short columns beside it lose any.
pub fn fit_widths(widths: &[usize], total: usize) -> Vec<usize> {
    let mut w = widths.to_vec();
    if w.is_empty() {
        return w;
    }
    let seps = COL_SEP.chars().count() * (w.len() - 1);
    let budget = total.saturating_sub(seps);
    // below this even the minimum does not fit; nothing left to give
    let floor = MIN_COL * w.len();
    if budget <= floor {
        return vec![MIN_COL; w.len()];
    }
    while w.iter().sum::<usize>() > budget {
        let (i, _) = w
            .iter()
            .enumerate()
            .max_by_key(|(i, v)| (**v, std::cmp::Reverse(*i)))
            .unwrap();
        w[i] -= 1;
    }
    w
}

/// `text` cut to `width` display columns, with an ellipsis when it was cut.
pub fn truncate(text: &str, width: usize) -> String {
    if str_width(text) <= width {
        return text.to_string();
    }
    let n = cut_at(text.chars().map(char_width), width);
    let mut out: String = text.chars().take(n).collect();
    out.push('…');
    out
}

/// The rule drawn under a table's head, sized to its columns.
pub fn table_rule(widths: &[usize]) -> String {
    widths
        .iter()
        .map(|w| "─".repeat(*w))
        .collect::<Vec<_>>()
        .join("─┼─")
}

/// One rendered character plus the source column it maps back to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub ch: char,
    pub style: Style,
    /// Column (in source `char`s) this cell stands for.
    pub src: usize,
}

/// A styled source line: display cells plus the source length.
#[derive(Clone, Debug, Default)]
pub struct RLine {
    pub cells: Vec<Cell>,
    pub src_len: usize,
}

impl RLine {
    /// Raw, unstyled: what the cursor's own line shows (syntax revealed).
    pub fn raw(src: &str) -> RLine {
        let cells = src
            .chars()
            .enumerate()
            .map(|(i, ch)| Cell {
                ch,
                style: theme::PLAIN,
                src: i,
            })
            .collect();
        RLine {
            cells,
            src_len: src.chars().count(),
        }
    }

    /// The whole line as one unwrapped display row. Both column mappings live
    /// on [`Seg`], because on screen every line is wrapped into rows.
    pub fn one_row(&self) -> Seg {
        Seg {
            cells: self.cells.clone(),
            indent: 0,
            end_src: self.src_len,
        }
    }

    /// Merge adjacent cells of equal style into ratatui spans.
    /// `selection` is a source-column range rendered reversed.
    pub fn to_line(&self, selection: Option<(usize, usize)>) -> Line<'static> {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut text = String::new();
        let mut current: Option<Style> = None;
        for cell in &self.cells {
            let mut style = cell.style;
            if let Some((a, b)) = selection {
                if cell.src >= a && cell.src < b {
                    style = style.add_modifier(Modifier::REVERSED);
                }
            }
            if current != Some(style) {
                if let Some(s) = current {
                    spans.push(Span::styled(std::mem::take(&mut text), s));
                }
                current = Some(style);
            }
            text.push(cell.ch);
        }
        if let Some(s) = current {
            spans.push(Span::styled(text, s));
        }
        // an empty selected line still needs a visible sliver
        if spans.is_empty() {
            if let Some((a, b)) = selection {
                if a < b {
                    spans.push(Span::styled(
                        " ".to_string(),
                        Style::new().add_modifier(Modifier::REVERSED),
                    ));
                }
            }
        }
        Line::from(spans)
    }
}

/// One display row of a soft-wrapped line.
///
/// Continuation rows of a list item or a quote are drawn under the text rather
/// than under the marker, so `indent` display columns of blank sit in front of
/// `cells` — clicks and the cursor both go through here, so the hanging indent
/// costs the mapping nothing.
#[derive(Clone, Debug, Default)]
pub struct Seg {
    pub cells: Vec<Cell>,
    /// Blank display columns drawn before `cells` (0 on the first row).
    pub indent: usize,
    /// Source column just past this row: the next row's first source column,
    /// or the line's source length for the last row.
    pub end_src: usize,
}

impl Seg {
    /// Does this row reach past source column `col`? Rows are searched in
    /// order, so the first that says yes is the row `col` is drawn on — and a
    /// column past every row (the end of the line, or a space dropped at the
    /// last break) belongs to the last row.
    pub fn owns_src(&self, col: usize) -> bool {
        col < self.end_src
    }

    /// Display column (within this row) → source column.
    pub fn display_to_source(&self, col: usize) -> usize {
        let col = col.saturating_sub(self.indent);
        let mut x = 0;
        for c in &self.cells {
            let w = char_width(c.ch);
            if col < x + w {
                return c.src;
            }
            x += w;
        }
        self.end_src
    }

    /// The cell drawn at display column `col` of this row, if any.
    pub fn cell_at_display(&self, col: usize) -> Option<&Cell> {
        let col = col.checked_sub(self.indent)?;
        let mut x = 0;
        for c in &self.cells {
            let w = char_width(c.ch);
            if col < x + w {
                return Some(c);
            }
            x += w;
        }
        None
    }

    /// Source column → display column within this row.
    pub fn source_to_display(&self, col: usize) -> usize {
        let mut x = 0;
        for c in &self.cells {
            if c.src >= col {
                return self.indent + x;
            }
            x += char_width(c.ch);
        }
        self.indent + x
    }

    /// This row as a ratatui line, hanging indent and all.
    pub fn to_line(&self, selection: Option<(usize, usize)>) -> Line<'static> {
        let inner = RLine {
            cells: self.cells.clone(),
            src_len: self.end_src,
        }
        .to_line(selection);
        if self.indent == 0 {
            return inner;
        }
        let mut spans = vec![Span::raw(" ".repeat(self.indent))];
        spans.extend(inner.spans);
        Line::from(spans)
    }
}

/// Soft-wrap a styled line into display rows no wider than `width` columns.
/// Always returns at least one row, so an empty line still has somewhere to
/// put the cursor.
pub fn wrap_rline(line: &RLine, width: usize) -> Vec<Seg> {
    if width == 0 {
        return vec![line.one_row()];
    }
    // a hanging indent that ate half the page would be worse than none
    let indent = hanging_indent(&line.cells).min(width / 2);
    let chars: Vec<char> = line.cells.iter().map(|c| c.ch).collect();
    wrap_breaks(&chars, width, width - indent)
        .into_iter()
        .enumerate()
        .map(|(i, (s, e))| Seg {
            cells: line.cells[s..e].to_vec(),
            indent: if i == 0 { 0 } else { indent },
            end_src: line.cells.get(e).map_or(line.src_len, |c| c.src),
        })
        .collect()
}

/// The display width continuation rows are indented by: the line's own leading
/// whitespace, plus any quote bar and list marker, so wrapped text lines up
/// with the text above it rather than with the bullet.
fn hanging_indent(cells: &[Cell]) -> usize {
    let ch = |i: usize| cells.get(i).map(|c: &Cell| c.ch);
    let mut i = 0;
    let mut w = 0;
    let space = |i: &mut usize, w: &mut usize| {
        while matches!(ch(*i), Some(' ') | Some('\t')) {
            *w += 1;
            *i += 1;
        }
    };
    space(&mut i, &mut w);
    // quote bars, however deeply nested
    let bar = first_char(theme::QUOTE_BAR);
    while ch(i) == Some(bar) {
        w += char_width(bar);
        i += 1;
        space(&mut i, &mut w);
    }
    // one list marker: a bullet/checkbox we drew, a raw -/*/+, or "12."
    let markers = [
        first_char(theme::BULLET),
        first_char(theme::CHECKED),
        first_char(theme::UNCHECKED),
    ];
    let mut j = i;
    match ch(j) {
        Some(c) if markers.contains(&c) => j += 1,
        Some('-') | Some('*') | Some('+') => j += 1,
        Some(c) if c.is_ascii_digit() => {
            while matches!(ch(j), Some(c) if c.is_ascii_digit()) {
                j += 1;
            }
            if !matches!(ch(j), Some('.') | Some(')')) {
                return w;
            }
            j += 1;
        }
        _ => return w,
    }
    if ch(j) != Some(' ') {
        return w;
    }
    let marker: usize = cells[i..j].iter().map(|c| char_width(c.ch)).sum();
    w + marker + 1
}

/// Break `chars` into display rows: the first `first` columns wide, the rest
/// `rest` wide. Rows are half-open index ranges; a space at a break point is
/// dropped. Widths are terminal columns, so CJK and emoji break where they
/// actually reach the edge.
pub fn wrap_breaks(chars: &[char], first: usize, rest: usize) -> Vec<(usize, usize)> {
    if chars.is_empty() {
        return vec![(0, 0)];
    }
    let mut out = Vec::new();
    let mut start = 0;
    let mut avail = first.max(1);
    while start < chars.len() {
        // the first character that would not fit in `avail` columns
        let mut x = 0;
        let mut fit = start;
        while fit < chars.len() {
            let w = char_width(chars[fit]);
            if x + w > avail {
                break;
            }
            x += w;
            fit += 1;
        }
        if fit >= chars.len() {
            out.push((start, chars.len()));
            break;
        }
        let brk = chars[start..=fit]
            .iter()
            .rposition(|c| *c == ' ')
            .map(|p| start + p)
            .filter(|p| *p > start);
        let (end, next) = match brk {
            Some(p) => (p, p + 1), // the space itself is dropped
            None => {
                let e = fit.max(start + 1); // a word longer than the page is cut
                (e, e)
            }
        };
        out.push((start, end));
        start = next;
        avail = rest.max(1);
    }
    if out.is_empty() {
        out.push((0, chars.len()));
    }
    out
}

struct Builder<'a> {
    src: &'a [char],
    cells: Vec<Cell>,
}

impl<'a> Builder<'a> {
    fn keep(&mut self, i: usize, style: Style) {
        self.cells.push(Cell {
            ch: self.src[i],
            style,
            src: i,
        });
    }

    /// Substitute display text for a source column (markers we replace or hide).
    fn sub(&mut self, text: &str, style: Style, src: usize) {
        for ch in text.chars() {
            self.cells.push(Cell { ch, style, src });
        }
    }
}

/// Only the inline pass — emphasis, code, links — over a run of markdown that
/// is already known to be prose rather than a fence, a rule or a table row.
/// The linked-mentions footer styles its excerpts with this, so `**bold**`
/// reads as bold there and a `[[link]]` as its label, the same as in the
/// editor. Each cell keeps the source column it came from.
pub fn style_inline(src: &str) -> Vec<Cell> {
    let chars: Vec<char> = src.chars().collect();
    let mut b = Builder {
        src: &chars,
        cells: Vec::with_capacity(chars.len()),
    };
    inline(&mut b, 0, theme::PLAIN);
    b.cells
}

/// Style one markdown source line for display.
pub fn style_line(src: &str) -> RLine {
    let chars: Vec<char> = src.chars().collect();
    let src_len = chars.len();
    let mut b = Builder {
        src: &chars,
        cells: Vec::with_capacity(src_len),
    };
    let mut i = 0;

    // fenced code lines: shown verbatim, dimmed
    if is_fence(src) {
        for (idx, _) in chars.iter().enumerate() {
            b.keep(idx, theme::code());
        }
        return RLine {
            cells: b.cells,
            src_len,
        };
    }

    // horizontal rule
    let trimmed = src.trim();
    if trimmed.len() >= 3 && trimmed.chars().all(|c| c == '-') {
        for (idx, _) in chars.iter().enumerate() {
            b.sub("─", theme::marker(), idx);
        }
        return RLine {
            cells: b.cells,
            src_len,
        };
    }

    // table row: keep every character, but dim the scaffolding
    if trimmed.starts_with('|') && trimmed.len() > 1 {
        let rule = trimmed
            .chars()
            .all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'));
        for (idx, ch) in chars.iter().enumerate() {
            let style = if rule || *ch == '|' {
                theme::marker()
            } else {
                theme::PLAIN
            };
            b.keep(idx, style);
        }
        return RLine {
            cells: b.cells,
            src_len,
        };
    }

    let mut base = theme::PLAIN;

    // leading blockquote bars (possibly nested), each "> " → "▌ "
    loop {
        let mut j = i;
        while j < chars.len() && (chars[j] == ' ' || chars[j] == '\t') {
            j += 1;
        }
        if j < chars.len() && chars[j] == '>' {
            for k in i..j {
                b.keep(k, theme::marker());
            }
            b.sub(theme::QUOTE_BAR, theme::marker(), j);
            i = j + 1;
            if i < chars.len() && chars[i] == ' ' {
                b.keep(i, theme::marker());
                i += 1;
            }
            base = theme::quote();
        } else {
            break;
        }
    }

    // indentation before a list marker or heading
    while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
        b.keep(i, base);
        i += 1;
    }

    // heading: hide the "### " marker, style the rest
    if i < chars.len() && chars[i] == '#' {
        let mut h = i;
        while h < chars.len() && chars[h] == '#' && h - i < 6 {
            h += 1;
        }
        if h < chars.len() && chars[h] == ' ' {
            base = theme::heading(h - i);
            for k in i..=h {
                b.sub("", base, k); // hidden marker
            }
            i = h + 1;
            inline(&mut b, i, base);
            return RLine {
                cells: b.cells,
                src_len,
            };
        }
    }

    // task list / bullet
    if let Some((marker, style, width)) = list_marker(&chars, i) {
        b.sub(marker, style, i);
        b.sub(" ", style, i + 1);
        for k in i..i + width {
            if k >= i + 2 {
                b.sub("", style, k);
            }
        }
        i += width;
        if marker == theme::CHECKED {
            base = base.patch(theme::done_text());
        }
    }

    inline(&mut b, i, base);
    RLine {
        cells: b.cells,
        src_len,
    }
}

/// The source columns `start..end` of the `- [ ] ` prefix of a task line,
/// indent excluded from `start`. `None` for any other line.
pub fn task_prefix(src: &str) -> Option<(usize, usize)> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    while matches!(chars.get(i), Some(' ') | Some('\t')) {
        i += 1;
    }
    match list_marker(&chars, i) {
        Some((_, _, 6)) => Some((i, i + 6)),
        _ => None,
    }
}

/// The cursor's own line, raw except for its checkbox: the `- [ ] ` stays a
/// box unless the cursor is inside those six columns, which is when the
/// syntax is what is being edited. Obsidian's rule.
pub fn raw_with_task(src: &str, cursor_col: usize) -> RLine {
    let Some((start, end)) = task_prefix(src) else {
        return RLine::raw(src);
    };
    if cursor_col < end {
        return RLine::raw(src);
    }
    let chars: Vec<char> = src.chars().collect();
    let src_len = chars.len();
    let mut b = Builder {
        src: &chars,
        cells: Vec::with_capacity(src_len),
    };
    for k in 0..start {
        b.keep(k, theme::PLAIN);
    }
    let (marker, style, _) = list_marker(&chars, start).expect("task_prefix said so");
    b.sub(marker, style, start);
    b.sub(" ", style, start + 1);
    for k in start + 2..end {
        b.sub("", style, k);
    }
    for k in end..src_len {
        b.keep(k, theme::PLAIN);
    }
    RLine {
        cells: b.cells,
        src_len,
    }
}

/// Recognise `- [ ] `, `- [x] `, `- `, `* `, `+ ` at `i`.
/// Returns (display marker, style, consumed source width).
fn list_marker(chars: &[char], i: usize) -> Option<(&'static str, Style, usize)> {
    let at = |k: usize| chars.get(k).copied();
    let bullet = matches!(at(i), Some('-') | Some('*') | Some('+'));
    if !bullet || at(i + 1) != Some(' ') {
        return None;
    }
    if at(i + 2) == Some('[') && at(i + 4) == Some(']') && at(i + 5) == Some(' ') {
        return match at(i + 3) {
            Some(' ') => Some((theme::UNCHECKED, theme::marker(), 6)),
            Some('x') | Some('X') => Some((theme::CHECKED, theme::done(), 6)),
            _ => Some((theme::BULLET, theme::marker(), 2)),
        };
    }
    Some((theme::BULLET, theme::marker(), 2))
}

/// Inline emphasis, code, links and highlights from source column `i` on.
fn inline(b: &mut Builder, mut i: usize, base: Style) {
    while i < b.src.len() {
        i = span_at(b, i, base).unwrap_or_else(|| {
            b.keep(i, base);
            i + 1
        });
    }
}

/// Try to consume one inline construct at `i`; returns the next source column.
fn span_at(b: &mut Builder, i: usize, base: Style) -> Option<usize> {
    let c = b.src[i];

    // [[wikilink]] — checked before `[text](url)`, which falls straight
    // through on a double bracket and would leave it as literal text
    if c == '[' && b.src.get(i + 1) == Some(&'[') && links::enabled() {
        if let Some(w) = wikilink_at(b.src, i) {
            let style = wiki_style(base, &w.target);
            return Some(delimited(
                b,
                w.start,
                w.label_start,
                w.label_end,
                w.end,
                style,
            ));
        }
    }

    // `code`
    if c == '`' {
        let end = find(b.src, i + 1, '`')?;
        return Some(delimited(b, i, i + 1, end, end + 1, theme::inline_code()));
    }

    // [text](url) — show the text, hide the target
    if c == '[' {
        if let Some(close) = find(b.src, i + 1, ']') {
            if b.src.get(close + 1) == Some(&'(') {
                if let Some(paren) = find(b.src, close + 2, ')') {
                    let style = base.patch(theme::link());
                    b.sub("", style, i);
                    for k in i + 1..close {
                        b.keep(k, style);
                    }
                    for k in close..=paren {
                        b.sub("", style, k);
                    }
                    return Some(paren + 1);
                }
            }
        }
    }

    // bare URL
    if let Some(end) = url_at(b.src, i) {
        let style = base.patch(theme::link());
        for k in i..end {
            b.keep(k, style);
        }
        return Some(end);
    }

    // #tag — after the URL check, so a fragment is never mistaken for one
    if c == '#' && tags::enabled() {
        if let Some(end) = tag_at(b.src, i) {
            let style = base.patch(theme::tag());
            for k in i..end {
                b.keep(k, style);
            }
            return Some(end);
        }
    }

    // paired two-character markers
    for (m, style) in [
        ('*', base.add_modifier(Modifier::BOLD)),
        ('~', base.add_modifier(Modifier::CROSSED_OUT)),
        ('=', theme::highlight()),
    ] {
        if c == m && b.src.get(i + 1) == Some(&m) {
            if let Some(end) = find_pair(b.src, i + 2, m) {
                return Some(delimited(b, i, i + 2, end, end + 2, style));
            }
        }
    }

    // *italic* / _italic_
    if (c == '*' || c == '_') && b.src.get(i + 1) != Some(&c) {
        let end = find(b.src, i + 1, c)?;
        return Some(delimited(
            b,
            i,
            i + 1,
            end,
            end + 1,
            base.add_modifier(Modifier::ITALIC),
        ));
    }

    None
}

/// Hide `open..body_start` and `body_end..close_end`, style the body between.
fn delimited(
    b: &mut Builder,
    open: usize,
    body_start: usize,
    body_end: usize,
    close_end: usize,
    style: Style,
) -> usize {
    for k in open..body_start {
        b.sub("", style, k);
    }
    for k in body_start..body_end {
        b.keep(k, style);
    }
    for k in body_end..close_end {
        b.sub("", style, k);
    }
    close_end
}

/// The link covering source column `col` of `line`, if any. Used by
/// modifier-click and by ⌥⏎ in the editor: the whole span counts — target,
/// pipe and brackets included — so clicking anywhere on it follows the link.
///
/// A [`LinkTarget::Url`] is for the desktop to open; a [`LinkTarget::Wiki`] is
/// a note in the vault, and its string is the raw target, still to be resolved
/// by `index::resolve`. The caller has to tell them apart, which is why this
/// returns an enum rather than the string it used to.
pub fn link_at(line: &str, col: usize) -> Option<LinkTarget> {
    let src: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < src.len() {
        // a code span is literal, the way the styling already draws it
        if src[i] == '`' {
            if let Some(end) = find(&src, i + 1, '`') {
                i = end + 1;
                continue;
            }
        }
        // [[wikilink]] first: the `[text](url)` scan below does not recognise
        // one, and would walk into the middle of it looking for a `(`
        if src[i] == '[' && src.get(i + 1) == Some(&'[') && links::enabled() {
            if let Some(w) = wikilink_at(&src, i) {
                if (w.start..w.end).contains(&col) {
                    return Some(LinkTarget::Wiki(w.target));
                }
                i = w.end;
                continue;
            }
        }
        // [text](url)
        // an image (`![alt](path)`) is not something to open in a browser
        if src[i] == '[' && (i == 0 || src[i - 1] != '!') {
            if let Some(close) = find(&src, i + 1, ']') {
                if src.get(close + 1) == Some(&'(') {
                    if let Some(paren) = find(&src, close + 2, ')') {
                        if (i..=paren).contains(&col) {
                            let url: String = src[close + 2..paren].iter().collect();
                            return (!url.trim().is_empty())
                                .then(|| LinkTarget::Url(url.trim().to_string()));
                        }
                        i = paren + 1;
                        continue;
                    }
                }
            }
        }
        if let Some(end) = url_at(&src, i) {
            if (i..end).contains(&col) {
                return Some(LinkTarget::Url(src[i..end].iter().collect()));
            }
            i = end.max(i + 1);
            continue;
        }
        if src[i] == '#' && tags::enabled() {
            if let Some(end) = tag_at(&src, i) {
                if (i..end).contains(&col) {
                    return Some(LinkTarget::Tag(src[i + 1..end].iter().collect()));
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    None
}

/// Does a bare `http(s)://` URL start at `i`? If so, where it ends: the run
/// up to the next whitespace, less any trailing punctuation that is more
/// likely the sentence's than the URL's. Shared with the reading view, so the
/// editor and the renderer agree on what a URL is.
pub fn url_at(src: &[char], i: usize) -> Option<usize> {
    let rest: String = src[i..].iter().take(8).collect();
    let starts = (rest.starts_with("http://") || rest.starts_with("https://"))
        && (i == 0 || !src[i - 1].is_alphanumeric());
    if !starts {
        return None;
    }
    let mut end = i;
    while end < src.len() && !src[end].is_whitespace() {
        end += 1;
    }
    while end > i && matches!(src[end - 1], '.' | ',' | ')' | ']' | '!' | '?') {
        end -= 1;
    }
    Some(end)
}

fn find(src: &[char], from: usize, ch: char) -> Option<usize> {
    (from..src.len()).find(|&k| src[k] == ch)
}

pub(crate) fn find_pair(src: &[char], from: usize, ch: char) -> Option<usize> {
    (from..src.len().saturating_sub(1)).find(|&k| src[k] == ch && src[k + 1] == ch)
}

// ---------------------------------------------------------------------------
// Tags
//
// `#tag` is Obsidian's other way of joining notes up: not a link to one note
// but a name several notes share. The grammar lives here with the rest of
// the inline syntax; which notes carry a tag is index.rs's business.

/// Where the `#tag` starting at column `i` ends, or `None` when `#` is not
/// opening one there. A tag is a `#` on a word boundary, then a letter, then
/// letters, digits, `-`, `_` or `/`. The boundary is what keeps `a#b` and a
/// URL fragment out; the letter is what keeps `#1`, `##` and a heading's
/// `# ` out.
pub fn tag_at(src: &[char], i: usize) -> Option<usize> {
    if src.get(i) != Some(&'#') || !tag_boundary(i.checked_sub(1).map(|k| src[k])) {
        return None;
    }
    // `[[#heading]]` is a link to a place in this note, which `wikilink_at`
    // leaves as typed — and typed, its `#` sits after a `[` the way `(#tag)`
    // does. It is still not a tag
    if i >= 2 && src[i - 1] == '[' && src[i - 2] == '[' {
        return None;
    }
    if !src.get(i + 1).is_some_and(|c| c.is_alphabetic()) {
        return None;
    }
    let mut end = i + 2;
    while end < src.len() && is_tag_char(src[end]) {
        end += 1;
    }
    Some(end)
}

/// Can a tag start after `prev`? Start of line, whitespace, or an opener —
/// `(#tag)` and `"#tag"` are how tags turn up in prose.
pub fn tag_boundary(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => c.is_whitespace() || matches!(c, '(' | '[' | '{' | '"' | '\''),
    }
}

fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | '/')
}

/// Every `#tag` on a source line, as (start, end) columns, `#` included.
/// Code spans, `[text](url)` and bare URLs are stepped over, the same things
/// the styling steps over, so what is drawn as a tag and what the index
/// counts as one are the same set.
pub fn tags_in(line: &str) -> Vec<(usize, usize)> {
    let src: Vec<char> = line.chars().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i < src.len() {
        match src[i] {
            '`' => {
                if let Some(end) = find(&src, i + 1, '`') {
                    i = end + 1;
                    continue;
                }
            }
            '[' => {
                // a [[wikilink]] is one thing, the way `link_at` and the
                // styling take it: `[[#heading]]` names a heading, not a tag
                if src.get(i + 1) == Some(&'[') && links::enabled() {
                    if let Some(w) = wikilink_at(&src, i) {
                        i = w.end;
                        continue;
                    }
                }
                if let Some(close) = find(&src, i + 1, ']') {
                    if src.get(close + 1) == Some(&'(') {
                        if let Some(paren) = find(&src, close + 2, ')') {
                            i = paren + 1;
                            continue;
                        }
                    }
                }
            }
            '#' => {
                if let Some(end) = tag_at(&src, i) {
                    found.push((i, end));
                    i = end;
                    continue;
                }
            }
            _ if url_at(&src, i).is_some() => {
                while i < src.len() && !src[i].is_whitespace() {
                    i += 1;
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    found
}

/// The form a tag is matched in: lower-case, no `#`. `#Work` and `#work`
/// are one tag, as they are in Obsidian.
pub fn tag_key(tag: &str) -> String {
    tag.trim().trim_start_matches('#').to_ascii_lowercase()
}

/// The `tags` setting. Process-wide for the reason [`links`] is.
pub mod tags {
    use std::sync::RwLock;

    static ON: RwLock<bool> = RwLock::new(true);

    pub fn set_enabled(on: bool) {
        if let Ok(mut w) = ON.write() {
            *w = on;
        }
    }

    pub fn enabled() -> bool {
        ON.read().map(|b| *b).unwrap_or(true)
    }
}

// ---------------------------------------------------------------------------
// Wikilinks
//
// `[[note]]` is how an Obsidian vault spells a link from one of its own notes
// to another, and a vault migrated into catcher is full of them. The syntax
// lives here, next to the rest of the inline grammar, because md.rs is the
// leaf: it owns what the characters mean, index.rs owns which file they name,
// and app.rs owns what happens when you press enter on one. Keeping the
// normalisation in one function ([`link_key`]) is what stops the colour a link
// is drawn in from disagreeing with the note a click on it opens.

/// A `[[wikilink]]` found in a source line. `start`/`end` and the label range
/// are source *columns* in chars, with `end` exclusive — one past the final
/// `]`.
///
/// The label is a range and never a synthesised string, so the display cells
/// it becomes keep honest source columns exactly the way `[text](url)` does.
/// Hiding characters is only safe while every character that survives still
/// knows which column of the file it came from; that mapping is what turns a
/// click back into a cursor position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wikilink {
    pub start: usize,
    pub end: usize,
    /// What the link names: trimmed, with any `#heading` suffix dropped. The
    /// heading is a place *inside* a note and the note is what opens, so it is
    /// shown but never resolved against.
    pub target: String,
    pub label_start: usize,
    pub label_end: usize,
}

/// The one rule set for what counts as a wikilink at source column `i`, called
/// from styling, from `link_at` and from the full-page renderer, so all three
/// answer the same way.
pub fn wikilink_at(src: &[char], i: usize) -> Option<Wikilink> {
    if src.get(i) != Some(&'[') || src.get(i + 1) != Some(&'[') {
        return None;
    }
    // `\[[x]]` is someone showing the syntax rather than using it, and
    // `![[x.png]]` is an Obsidian embed — a picture, not somewhere to go — so
    // neither of them becomes a link
    if i > 0 && matches!(src[i - 1], '\\' | '!') {
        return None;
    }
    // a wikilink never spans a line, and a stray bracket inside one means the
    // pair was never a pair: `[[a] b]]` and `[[unclosed` stay literal text
    let body_start = i + 2;
    let mut k = body_start;
    let close = loop {
        match src.get(k) {
            Some(']') if src.get(k + 1) == Some(&']') => break k,
            Some('[') | Some(']') | Some('\n') | None => return None,
            Some(_) => k += 1,
        }
    };
    if src[body_start..close].iter().all(|c| c.is_whitespace()) {
        return None;
    }
    // `[[#heading]]` names a place in the note you are already reading: there
    // is nothing to resolve and nothing to create, so leave it as it was typed
    if src[body_start] == '#' {
        return None;
    }
    // the FIRST pipe splits target from label, so a label may contain one
    let pipe = (body_start..close).find(|&k| src[k] == '|');
    let (target_end, label) = match pipe {
        // `[[note|]]` has no label to show, so the target is what is drawn —
        // up to the pipe, and not the pipe itself or the blank after it
        Some(p) if src[p + 1..close].iter().all(|c| c.is_whitespace()) => (p, (body_start, p)),
        Some(p) => (p, (p + 1, close)),
        None => (close, (body_start, close)),
    };
    let raw: String = src[body_start..target_end].iter().collect();
    let target = raw.split('#').next().unwrap_or("").trim().to_string();
    if target.is_empty() {
        return None;
    }
    Some(Wikilink {
        start: i,
        end: close + 2,
        target,
        label_start: label.0,
        label_end: label.1,
    })
}

/// Every wikilink on one source line, left to right.
///
/// [`wikilink_at`] answers about a single column, because that is the question
/// styling and `link_at` ask. The linked-mentions scan asks about a whole line
/// instead, and it must get the same answer: a mention is exactly a link the
/// reader could have clicked, so both questions go through the one rule set
/// rather than a second scanner that would drift away from it.
pub fn wikilinks(line: &str) -> Vec<Wikilink> {
    let src: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < src.len() {
        match wikilink_at(&src, i) {
            // past the whole span, so `[[a]] [[b]]` is two links and the
            // brackets of the first can never start a third
            Some(w) => {
                i = w.end;
                out.push(w);
            }
            None => i += 1,
        }
    }
    out
}

/// The one normalisation a link target and a note's own names are both put
/// through before they are compared: trimmed, `#heading` dropped, backslashes
/// turned into slashes, a trailing `.md` removed, lowercased.
///
/// Styling asks "does this resolve?" and resolution asks "which note is it?".
/// They are answered in different modules and must never disagree, which is
/// why both of them come through here rather than each rolling its own.
pub fn link_key(target: &str) -> String {
    let t = target.split('#').next().unwrap_or("").trim().to_lowercase();
    let t = t.replace('\\', "/");
    t.strip_suffix(".md").unwrap_or(&t).trim().to_string()
}

/// How a link travels through the parts of the app that only speak in strings.
pub const WIKI_SCHEME: &str = "wikilink:";

/// The same trick for a link the app built itself and already knows the file
/// behind — the rows of the linked-mentions footer. A name would have to be
/// resolved all over again, and two notes called `spec` would send the click
/// to whichever one the resolver prefers rather than the one whose row was
/// clicked.
pub const NOTE_SCHEME: &str = "note:";

/// The escape that keeps [`NOTE_SCHEME`] the app's own. A note body can write
/// `[report](note:/etc/passwd)` as easily as the footer can name a file it
/// found, and the two must not arrive as the same string: one opens a file the
/// app already had in hand, the other is a stranger's text. A body href that
/// would claim either scheme is prefixed on its way in and unwrapped on its
/// way out, so it reaches the desktop opener spelled exactly as it was typed.
pub const URL_SCHEME: &str = "url:";

/// A `#tag`, on its way from a drawn row to the picker it opens.
pub const TAG_SCHEME: &str = "tag:";

/// What a click or ⌥click landed on: a URL for the desktop, or a wikilink for
/// the vault. The distinction has to survive, because handing `wikilink:spec`
/// to `open`/`xdg-open` would be nonsense.
///
/// This enum is what [`link_at`] returns in the editor. The reading view cannot
/// carry it — `render::Rendered::urls` is a `Vec<String>` and
/// `App::preview_links` a `Vec<(Rect, String)>`, and typing those would mean a
/// far wider refactor for one bit of information — so the same distinction
/// travels through them as the [`WIKI_SCHEME`] prefix on the front of the
/// string. A hand-written `[x](wikilink:y)` therefore opens a note by name;
/// that is the sane reading of what someone typing it meant, and it can only
/// name a note the way any other `[[link]]` does. [`NOTE_SCHEME`] is not like
/// that — it names a file by path, with nothing left to check — so a href out
/// of a note body is never allowed to claim it; see [`URL_SCHEME`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    Url(String),
    Wiki(String),
    /// An exact file, by path. Nothing a reader types produces one of these —
    /// only the app, for a row it drew from a file it had already found.
    Note(String),
    /// A `#tag`, without its `#`: not a note but a list of them.
    Tag(String),
}

impl LinkTarget {
    pub fn href(&self) -> String {
        match self {
            // a URL that reads as one of the app's own schemes is wrapped in
            // `url:` so it comes back out of `parse` as the URL it is, rather
            // than as an instruction to open a file by path
            LinkTarget::Url(u)
                if u.starts_with(NOTE_SCHEME)
                    || u.starts_with(URL_SCHEME)
                    || u.starts_with(TAG_SCHEME) =>
            {
                format!("{URL_SCHEME}{u}")
            }
            LinkTarget::Url(u) => u.clone(),
            LinkTarget::Wiki(t) => format!("{WIKI_SCHEME}{t}"),
            LinkTarget::Note(p) => format!("{NOTE_SCHEME}{p}"),
            LinkTarget::Tag(t) => format!("{TAG_SCHEME}{t}"),
        }
    }

    pub fn parse(href: &str) -> LinkTarget {
        // first, and without looking at what is left: one `url:` was put there
        // by `href` and unwrapping it is the whole of the job
        if let Some(u) = href.strip_prefix(URL_SCHEME) {
            return LinkTarget::Url(u.to_string());
        }
        if let Some(t) = href.strip_prefix(WIKI_SCHEME) {
            return LinkTarget::Wiki(t.to_string());
        }
        if let Some(t) = href.strip_prefix(TAG_SCHEME) {
            return LinkTarget::Tag(t.to_string());
        }
        match href.strip_prefix(NOTE_SCHEME) {
            Some(p) => LinkTarget::Note(p.to_string()),
            None => LinkTarget::Url(href.to_string()),
        }
    }
}

/// Which wikilink targets this vault actually has, and whether wikilinks are
/// on at all.
///
/// Process-wide state, the sibling of [`theme`] and here for the same reason:
/// styling happens deep inside line layout, and threading a vault index down
/// through `style_line`, `style_block_line`, `view_line` and every one of
/// their call sites would put a parameter on functions that have no other
/// reason to know a vault exists at all.
pub mod links {
    use std::collections::HashSet;
    use std::sync::RwLock;

    /// Every name a note in this vault answers to, or `None` when nothing has
    /// been walked yet. That distinction is the whole point of the `Option`: a
    /// session that has not scanned must draw links in link colour, not open
    /// on a page of red.
    static KNOWN: RwLock<Option<HashSet<String>>> = RwLock::new(None);
    /// The `wikilinks` setting. Off leaves `[[x]]` as the literal text a
    /// reader without Obsidian sees.
    static ON: RwLock<bool> = RwLock::new(true);

    /// Install the set of names the vault answers to. Called after each index
    /// walk, never per frame.
    pub fn set_known(keys: HashSet<String>) {
        if let Ok(mut w) = KNOWN.write() {
            *w = Some(keys);
        }
    }

    pub fn set_enabled(on: bool) {
        if let Ok(mut w) = ON.write() {
            *w = on;
        }
    }

    pub fn enabled() -> bool {
        ON.read().map(|b| *b).unwrap_or(true)
    }

    /// Does `target` name a note we know about? True when nothing has been
    /// scanned yet, so an un-walked vault is not one long broken link.
    pub fn resolves(target: &str) -> bool {
        let key = super::link_key(target);
        match KNOWN.read() {
            Ok(k) => match &*k {
                Some(set) => set.contains(&key),
                None => true,
            },
            Err(_) => true,
        }
    }

    /// Put the state back to "nothing walked yet". Only the tests want this —
    /// the app scans and rescans, it never unscans.
    #[cfg(test)]
    pub fn forget() {
        if let Ok(mut w) = KNOWN.write() {
            *w = None;
        }
    }
}

/// How a wikilink is drawn: like any other link when it names a note that
/// exists, and grey when it does not.
///
/// Grey, not danger: a link to a note not written yet is an invitation, not a
/// fault — following it makes the note — and a vault carried over from
/// somewhere else is full of them. Both views call this, so they can never
/// disagree about what resolves.
///
/// An unresolved link is still a link, so it keeps the underline
/// `theme::link()` carries and only its colour changes.
pub fn wiki_style(base: Style, target: &str) -> Style {
    let base = base.patch(theme::link());
    if links::resolves(target) {
        base
    } else {
        base.patch(theme::grey())
    }
}

/// What a run of a mermaid diagram is drawn in. The diagram module deals in
/// roles and never in colour, so the mapping lives here, next to the palette,
/// and both views ask for it rather than each deciding for itself what a box
/// edge looks like.
///
/// No accent anywhere in it: a diagram is chrome the note draws around the
/// words the author typed, and the note spends its one hue on headings.
pub fn mermaid_style(role: crate::mermaid::Role) -> Style {
    use crate::mermaid::Role;
    match role {
        Role::Line => theme::marker(),
        Role::Node => theme::PLAIN,
        Role::Label => theme::grey(),
        Role::Bright => theme::bright(),
    }
}

// ---------------------------------------------------------------------------
// Block awareness
//
// The live preview is line-based, but some markdown only means anything across
// several lines. Spans are computed over the whole buffer once per frame; the
// block the cursor (or a selection end) sits in shows its raw source, every
// other block is drawn. One source line stays one display line throughout, the
// single exception being an image, which gets the rows its picture needs.
// ---------------------------------------------------------------------------

/// A multi-line markdown construct the live preview draws as a whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    /// A ```-fenced code block, fences included.
    Fence,
    /// A fence whose info string names mermaid: the same block, drawn as a
    /// picture rather than as code when there is room for one.
    Mermaid,
    /// `---` / `***` / `___` alone on a line.
    Rule,
    /// A pipe table with its separator row.
    Table,
    /// A line holding nothing but `![alt](url)`.
    Image,
    /// The leading `---` … `---` block, fences included. A block rather than a
    /// run of lines so the whole thing reveals together the way a fence does,
    /// and so nothing inside it is ever read as markdown.
    FrontMatter,
}

/// One block, as an inclusive range of source lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    pub start: usize,
    pub end: usize,
}

impl Block {
    pub fn contains(&self, row: usize) -> bool {
        row >= self.start && row <= self.end
    }
}

/// A fence line: three backticks or three tildes at the start, as CommonMark
/// and Obsidian have it. The one place the answer lives, so the block scan,
/// the line styler and the vault scanners never disagree about what a fence is.
pub(crate) fn is_fence(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}

/// A fence line's info string: whatever follows its run of backticks or
/// tildes. `` ```mermaid {theme: dark} `` gives `mermaid {theme: dark}`.
fn fence_info(line: &str) -> &str {
    line.trim_start().trim_start_matches(['`', '~']).trim()
}

/// `---`, `***` or `___` alone on a line.
pub fn is_rule(line: &str) -> bool {
    let t = line.trim();
    t.chars().count() >= 3
        && (t.chars().all(|c| c == '-')
            || t.chars().all(|c| c == '*')
            || t.chars().all(|c| c == '_'))
}

/// A `| --- | :-: |` table separator row.
pub(crate) fn is_table_rule(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|')
        && t.contains('-')
        && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

fn is_table_row(line: &str) -> bool {
    line.trim().starts_with('|') && line.trim().chars().count() > 1
}

/// `![alt](url)` or an Obsidian `![[url]]` embed, and nothing else on the
/// line — split into (alt, url).
pub fn image_line(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    if let Some(found) = embed_line(t) {
        return Some(found);
    }
    let rest = t.strip_prefix("![")?;
    let close = rest.find("](")?;
    let alt = &rest[..close];
    let url = rest[close + 2..].strip_suffix(')')?;
    if alt.contains(']') || url.contains(')') || url.is_empty() {
        return None;
    }
    Some((alt.to_string(), url.to_string()))
}

/// An Obsidian embed alone on a line: `![[picture.png]]` or
/// `![[picture.png|alt]]`, split into (alt, url). Only pictures: an embed of
/// another note (`![[plan]]`) is not one, and stays the text it was typed as.
/// Obsidian also reads a bare number after the pipe as a width; here it is
/// dropped rather than shown as alt text.
pub fn embed_line(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    let body = t.strip_prefix("![[")?.strip_suffix("]]")?;
    if body.contains('[') || body.contains(']') || body.contains('\n') {
        return None;
    }
    let (url, alt) = match body.split_once('|') {
        Some((u, a)) => (u.trim(), a.trim()),
        None => (body.trim(), ""),
    };
    if url.is_empty() || !is_image_path(url) {
        return None;
    }
    let alt = if alt.chars().all(|c| c.is_ascii_digit()) {
        ""
    } else {
        alt
    };
    Some((alt.to_string(), url.to_string()))
}

/// Whether a path names a picture the preview could draw, by extension.
pub fn is_image_path(path: &str) -> bool {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    matches!(
        ext.as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff")
    )
}

/// Every block in the buffer, in order and never overlapping.
pub fn blocks(lines: &[String]) -> Vec<Block> {
    blocks_from(lines, 0)
}

/// The same scan, started at line `from`. Front matter is the one construct
/// markdown itself knows nothing about, so the caller that recognises it hands
/// us the first line *below* it — which is what stops its closing `---` being
/// read as a rule and its `tags:` picking up emphasis. Filtering afterwards
/// would not do: a stray ``` inside the block would still swallow the note.
/// Line numbers stay absolute, since the loop indexes `lines` directly.
pub fn blocks_from(lines: &[String], from: usize) -> Vec<Block> {
    let mut out = Vec::new();
    let mut i = from;
    while i < lines.len() {
        // a fence swallows everything up to its close, so a `---` or a table
        // drawn inside a code sample is never mistaken for one
        if is_fence(&lines[i]) {
            let mut j = i + 1;
            while j < lines.len() && !is_fence(&lines[j]) {
                j += 1;
            }
            let end = j.min(lines.len() - 1);
            // naming mermaid changes only how the block is drawn: it still
            // swallows every line to its close, so a table or a rule inside a
            // diagram is part of the diagram and not markdown
            let kind = if crate::mermaid::is_mermaid(fence_info(&lines[i])) {
                BlockKind::Mermaid
            } else {
                BlockKind::Fence
            };
            out.push(Block {
                kind,
                start: i,
                end,
            });
            i = end + 1;
            continue;
        }
        if is_table_row(&lines[i]) && lines.get(i + 1).is_some_and(|l| is_table_rule(l)) {
            let mut j = i;
            while j < lines.len() && is_table_row(&lines[j]) {
                j += 1;
            }
            out.push(Block {
                kind: BlockKind::Table,
                start: i,
                end: j - 1,
            });
            i = j;
            continue;
        }
        if is_rule(&lines[i]) {
            out.push(Block {
                kind: BlockKind::Rule,
                start: i,
                end: i,
            });
        } else if image_line(&lines[i]).is_some() {
            out.push(Block {
                kind: BlockKind::Image,
                start: i,
                end: i,
            });
        }
        i += 1;
    }
    out
}

/// The block covering `row`, if any.
pub fn block_at(blocks: &[Block], row: usize) -> Option<&Block> {
    blocks.iter().find(|b| b.contains(row))
}

/// Draw one line of `block`. `width` is the page width, used to size a rule.
pub fn style_block_line(lines: &[String], block: &Block, row: usize, width: usize) -> RLine {
    let src = lines.get(row).map(String::as_str).unwrap_or("");
    match block.kind {
        BlockKind::Fence => fence_line(src, row == block.start || row == block.end),
        BlockKind::Mermaid => {
            mermaid_line(&lines[block.start..=block.end], row - block.start, width)
        }
        BlockKind::Rule => rule_line(src, width),
        BlockKind::Image => image_fallback_line(src),
        BlockKind::FrontMatter => front_matter_line(src),
        BlockKind::Table => table_line(&lines[block.start..=block.end], row - block.start, width),
    }
}

/// Cells that all map back to the same source column.
fn at(text: &str, style: Style, src: usize) -> Vec<Cell> {
    text.chars().map(|ch| Cell { ch, style, src }).collect()
}

fn done(cells: Vec<Cell>, src: &str) -> RLine {
    RLine {
        cells,
        src_len: src.chars().count(),
    }
}

/// A code fence with the cursor elsewhere: the body in the code colour, and the
/// caps quiet — the backticks are dropped altogether, leaving the dim language
/// name on the opening line and a blank line for the close (and for an opening
/// fence that names no language). It is still one display line per source line,
/// so a click on one lands back on the fence and reveals the block.
fn fence_line(src: &str, cap: bool) -> RLine {
    if !cap {
        let cells = src
            .chars()
            .enumerate()
            .map(|(i, ch)| Cell {
                ch,
                style: theme::code(),
                src: i,
            })
            .collect();
        return done(cells, src);
    }
    // the fence's own run of backticks/tildes is hidden; whatever follows it on
    // the line is the info string, kept at its real source columns
    let cells = src
        .chars()
        .enumerate()
        .skip_while(|(_, ch)| *ch == '`' || *ch == '~' || ch.is_whitespace())
        .map(|(i, ch)| Cell {
            ch,
            style: theme::marker(),
            src: i,
        })
        .collect();
    done(cells, src)
}

/// A ```mermaid fence with the cursor elsewhere: the picture when it fits the
/// fence that holds it, and the fence's own source when it does not.
///
/// The editor draws exactly one display line per source line — `app::view_line`
/// and the table path both lean on that for clicks, selection and scrolling —
/// and a diagram is nearly always taller than the handful of lines that
/// describe it. So a diagram is only drawn here when it is short enough to sit
/// inside its own fence; a taller one stays the code it was, and is read as a
/// picture in the full page, which is one **^P** away.
fn mermaid_line(rows: &[String], row: usize, width: usize) -> RLine {
    let src = rows.get(row).map(String::as_str).unwrap_or("");
    if rows.len() > 2 {
        // the caps are the fence, not the diagram
        let body = rows[1..rows.len() - 1].join("\n");
        if let Some(line) =
            rendered_memo(&body, width).and_then(|d| diagram_line(&d, rows.len(), row, src))
        {
            return line;
        }
    }
    fence_line(src, row == 0 || row + 1 == rows.len())
}

/// One remembered layout: the fence body, the width, and what it rendered to.
type MermaidMemo = (String, usize, Option<std::rc::Rc<crate::mermaid::Rendered>>);

thread_local! {
    /// The last diagram laid out for the editor.
    static MERMAID_MEMO: std::cell::RefCell<Option<MermaidMemo>> =
        const { std::cell::RefCell::new(None) };
}

/// `mermaid::render(body, width)`, remembered for the most recent `(body,
/// width)` so a fence is laid out once rather than once per row per pass.
///
/// Every row of a fence asks for the same diagram, and the rows of one fence
/// are visited back to back in the plan loop, the draw loop and the
/// cursor-follow loop, so a single entry is enough; a failed layout is
/// remembered too, so an unparseable fence does not retry on every row.
fn rendered_memo(body: &str, width: usize) -> Option<std::rc::Rc<crate::mermaid::Rendered>> {
    MERMAID_MEMO.with(|memo| {
        let mut memo = memo.borrow_mut();
        if let Some((b, w, d)) = memo.as_ref() {
            if b == body && *w == width {
                return d.clone();
            }
        }
        let d = crate::mermaid::render(body, width).map(std::rc::Rc::new);
        *memo = Some((body.to_string(), width, d.clone()));
        d
    })
}

/// Row `row` of a fence `rows` source lines tall, drawn as one row of `d`
/// centred in it — or `None` when the diagram is taller than the fence and
/// there is nowhere to put the rest of it.
///
/// Every cell maps back to source column 0, so a click anywhere on the picture
/// puts the cursor at the start of the source line it was drawn on; the block
/// then reveals itself and the caret is already in the text that made the
/// picture. Click the diagram, edit the diagram.
fn diagram_line(d: &crate::mermaid::Rendered, rows: usize, row: usize, src: &str) -> Option<RLine> {
    if d.height() > rows {
        return None;
    }
    let top = (rows - d.height()) / 2;
    let drawn = row.checked_sub(top).and_then(|i| d.rows.get(i));
    let cells = drawn
        .into_iter()
        .flatten()
        .flat_map(|run| {
            let style = mermaid_style(run.role);
            run.text.chars().map(move |ch| Cell { ch, style, src: 0 })
        })
        .collect();
    Some(done(cells, src))
}

/// A line of front matter: exactly what was typed, only quiet. Deliberately
/// not what `rule_line` does — the `---` that opens the block is a fence and
/// not a thematic break, and stretching it across the page would announce the
/// metadata rather than get it out of the way. Every char keeps its own source
/// column, so the block stays as clickable and as editable as any prose.
fn front_matter_line(src: &str) -> RLine {
    let cells = src
        .chars()
        .enumerate()
        .map(|(i, ch)| Cell {
            ch,
            style: theme::marker(),
            src: i,
        })
        .collect();
    done(cells, src)
}

/// A thematic break, drawn across the page. Columns past the source clamp to
/// the end of the line, so a click anywhere on the rule lands on it.
fn rule_line(src: &str, width: usize) -> RLine {
    let len = src.chars().count();
    let n = width.max(len).max(1);
    let cells = (0..n)
        .map(|i| Cell {
            ch: '─',
            style: theme::marker(),
            src: i.min(len),
        })
        .collect();
    done(cells, src)
}

/// What an image line shows when the terminal can't draw pictures.
fn image_fallback_line(src: &str) -> RLine {
    let len = src.chars().count();
    let (alt, url) = image_line(src).unwrap_or_default();
    let label = if alt.is_empty() {
        format!("🖼 {url}")
    } else {
        format!("🖼 {alt} ({url})")
    };
    let cells = label
        .chars()
        .enumerate()
        .map(|(i, ch)| Cell {
            ch,
            style: theme::marker(),
            src: i.min(len),
        })
        .collect();
    done(cells, src)
}

/// One source cell of a table row: its trimmed text and where that text starts.
pub(crate) struct TCell {
    pub(crate) start: usize,
    pub(crate) text: String,
}

impl TCell {
    /// Source column just past the cell's text.
    pub(crate) fn end(&self) -> usize {
        self.start + self.text.chars().count()
    }
}

/// Split `| a | b |` into its cells and the source columns of its pipes.
pub(crate) fn split_row(src: &str) -> (Vec<TCell>, Vec<usize>) {
    let chars: Vec<char> = src.chars().collect();
    let pipes: Vec<usize> = chars
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == '|')
        .map(|(i, _)| i)
        .collect();
    let mut cells = Vec::new();
    for w in pipes.windows(2) {
        let (a, b) = (w[0] + 1, w[1]);
        let mut start = a;
        while start < b && chars[start].is_whitespace() {
            start += 1;
        }
        let mut end = b;
        while end > start && chars[end - 1].is_whitespace() {
            end -= 1;
        }
        cells.push(TCell {
            start,
            text: chars[start..end].iter().collect(),
        });
    }
    (cells, pipes)
}

fn align_of(spec: &str) -> Align {
    let t = spec.trim();
    match (t.starts_with(':'), t.ends_with(':')) {
        (true, true) => Align::Center,
        (false, true) => Align::Right,
        _ => Align::Left,
    }
}

/// One table cell's text with its inline markup styled — code, emphasis,
/// links — the way any other line gets it. Source columns are relative to
/// the cell text; the caller offsets them to the row.
fn styled_cell(text: &str, base: Style) -> Vec<Cell> {
    let chars: Vec<char> = text.chars().collect();
    let mut b = Builder {
        src: &chars,
        cells: Vec::with_capacity(chars.len()),
    };
    inline(&mut b, 0, base);
    b.cells
}

fn cells_width(cells: &[Cell]) -> usize {
    cells.iter().map(|c| char_width(c.ch)).sum()
}

/// `cells` cut to `width` display columns, with an ellipsis when cut.
fn truncate_cells(mut cells: Vec<Cell>, width: usize) -> Vec<Cell> {
    if cells_width(&cells) <= width {
        return cells;
    }
    // the ellipsis takes the style of the first cell that did not fit
    let n = cut_at(cells.iter().map(|c| char_width(c.ch)), width);
    let cut = Cell {
        ch: '…',
        ..cells[n]
    };
    cells.truncate(n);
    cells.push(cut);
    cells
}

/// A table's layout: its rows split into cells, which row is the separator,
/// the column alignments it declares and the column widths fitted to the page.
struct TableLayout {
    parsed: Vec<(Vec<TCell>, Vec<usize>)>,
    rule_row: Option<usize>,
    aligns: Vec<Align>,
    widths: Vec<usize>,
}

/// Lay a table's rows out in aligned columns. `raw_row` is the row whose
/// cells are shown as typed, markup and all — the cursor's row while the
/// grid is being edited — so its columns are measured on the raw text.
fn table_layout(rows: &[String], width: usize, raw_row: Option<usize>) -> TableLayout {
    let parsed: Vec<(Vec<TCell>, Vec<usize>)> = rows.iter().map(|r| split_row(r)).collect();
    let rule_row = rows.iter().position(|r| is_table_rule(r));
    let aligns: Vec<Align> = rule_row
        .map(|i| parsed[i].0.iter().map(|c| align_of(&c.text)).collect())
        .unwrap_or_default();
    let cols = parsed.iter().map(|(c, _)| c.len()).max().unwrap_or(0);
    let measured: Vec<Vec<usize>> = parsed
        .iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != rule_row)
        .map(|(i, (c, _))| {
            c.iter()
                .map(|c| {
                    if Some(i) == raw_row {
                        str_width(&c.text)
                    } else {
                        cells_width(&styled_cell(&c.text, theme::PLAIN))
                    }
                })
                .collect()
        })
        .collect();
    let widths = fit_widths(&column_widths(&measured, cols), width);
    TableLayout {
        parsed,
        rule_row,
        aligns,
        widths,
    }
}

/// One remembered table layout: the rows, the width, and what they laid out to.
type TableMemo = (Vec<String>, usize, Option<usize>, std::rc::Rc<TableLayout>);

thread_local! {
    /// The last table laid out for the editor.
    static TABLE_MEMO: std::cell::RefCell<Option<TableMemo>> =
        const { std::cell::RefCell::new(None) };
}

/// `table_layout(rows, width)`, remembered for the most recent `(rows, width)`
/// so a table is measured once rather than once per row per pass — the same
/// single slot `rendered_memo` keeps for a diagram, and for the same reason:
/// the rows of one table are visited back to back, and an edit to any of them
/// changes the key.
fn layout_memo(rows: &[String], width: usize, raw_row: Option<usize>) -> std::rc::Rc<TableLayout> {
    TABLE_MEMO.with(|memo| {
        let mut memo = memo.borrow_mut();
        if let Some((r, w, rr, l)) = memo.as_ref() {
            if r == rows && *w == width && *rr == raw_row {
                return l.clone();
            }
        }
        let l = std::rc::Rc::new(table_layout(rows, width, raw_row));
        *memo = Some((rows.to_vec(), width, raw_row, l.clone()));
        l
    })
}

/// Draw row `row` of a table. Every source row is exactly one display row,
/// separator included.
fn table_line(rows: &[String], row: usize, width: usize) -> RLine {
    table_row(&layout_memo(rows, width, None), rows, row, false)
}

/// A table row drawn while the cursor is in the grid: `row` of the block
/// `lines[block.start..=block.end]`, with the cursor's own row (`raw_row`,
/// block-relative) shown as typed so every source column has a place on
/// screen for the cursor to sit.
pub fn table_line_editing(lines: &[String], block: &Block, row: usize, width: usize, raw_row: usize) -> RLine {
    let rows = &lines[block.start..=block.end];
    let r = row - block.start;
    table_row(&layout_memo(rows, width, Some(raw_row)), rows, r, r == raw_row)
}

/// The rule drawn between two body rows of a table in the editor, to the
/// same column widths as its rows. `raw_row` is the cursor's row, as for
/// [`table_line_editing`], so the layout is the one the rows use.
pub fn table_rule_editing(lines: &[String], block: &Block, width: usize, raw_row: Option<usize>) -> RLine {
    let rows = &lines[block.start..=block.end];
    let l = layout_memo(rows, width, raw_row);
    // each cell says which column it is under, so a selection can tint the
    // rule beneath its cells; a joint belongs to no column
    let mut cells = Vec::new();
    for (i, w) in l.widths.iter().enumerate() {
        if i > 0 {
            cells.extend("─┼─".chars().map(|ch| Cell {
                ch,
                style: theme::marker(),
                src: usize::MAX,
            }));
        }
        cells.extend(std::iter::repeat_n('─', *w).map(|ch| Cell {
            ch,
            style: theme::marker(),
            src: i,
        }));
    }
    RLine { cells, src_len: 0 }
}

/// The display column each table column starts at, and its width, for the
/// grid drawn from `lines[block]` — the grips above a table go by these.
pub fn table_column_spans(lines: &[String], block: &Block, width: usize, raw_row: Option<usize>) -> Vec<(usize, usize)> {
    let rows = &lines[block.start..=block.end];
    let l = layout_memo(rows, width, raw_row);
    let mut x = 0;
    l.widths
        .iter()
        .map(|w| {
            let at = x;
            x += w + COL_SEP.chars().count();
            (at, *w)
        })
        .collect()
}

/// Tint the cells of a drawn table row that `selected(column)` says are
/// selected, and the separator between two selected neighbours, with
/// `style`. `src` is the row's source, whose pipes say where columns are.
pub fn tint_table_cells(line: &mut RLine, src: &str, selected: &dyn Fn(usize) -> bool, style: Style) {
    let (_, pipes) = split_row(src);
    if pipes.len() < 2 {
        return;
    }
    for cell in &mut line.cells {
        let s = cell.src;
        // which column a source position is in, or the separator it is
        let hit = if s < pipes[0] {
            None
        } else if let Some(i) = pipes.iter().position(|p| *p == s) {
            // a pipe: between column i-1 and i
            (i > 0 && i + 1 < pipes.len() && selected(i - 1) && selected(i)).then_some(true)
        } else {
            let i = pipes.iter().filter(|p| **p < s).count() - 1;
            let i = i.min(pipes.len().saturating_sub(2));
            Some(selected(i))
        };
        if hit == Some(true) {
            cell.style = cell.style.patch(style);
        }
    }
}

/// Row `row` of `rows`, drawn to the layout `l`. A `raw` row keeps every
/// character of its cells rather than styling their markup.
fn table_row(l: &TableLayout, rows: &[String], row: usize, raw: bool) -> RLine {
    let TableLayout {
        parsed,
        rule_row,
        aligns,
        widths,
    } = l;
    let rule_row = *rule_row;
    let src = rows.get(row).map(String::as_str).unwrap_or("");
    // the separator row becomes the rule under the head
    if Some(row) == rule_row {
        let len = src.chars().count();
        let cells = table_rule(widths)
            .chars()
            .enumerate()
            .map(|(i, ch)| Cell {
                ch,
                style: theme::marker(),
                src: i.min(len),
            })
            .collect();
        return done(cells, src);
    }

    let head = rule_row.is_some_and(|r| row < r);
    let body = if head {
        theme::PLAIN.add_modifier(Modifier::BOLD)
    } else {
        theme::PLAIN
    };
    let (row_cells, pipes) = &parsed[row];
    let mut cells: Vec<Cell> = Vec::new();
    for (ci, w) in widths.iter().enumerate() {
        if ci > 0 {
            let pipe = pipes.get(ci).copied().unwrap_or(0);
            cells.extend(at(COL_SEP, theme::marker(), pipe));
        }
        let empty = TCell {
            start: pipes.last().copied().unwrap_or(0),
            text: String::new(),
        };
        let cell = row_cells.get(ci).unwrap_or(&empty);
        let align = aligns.get(ci).copied().unwrap_or(Align::Left);
        let styled = if raw {
            cell.text
                .chars()
                .enumerate()
                .map(|(i, ch)| Cell { ch, style: body, src: i })
                .collect()
        } else {
            styled_cell(&cell.text, body)
        };
        let styled = truncate_cells(styled, *w);
        let (left, right) = pad_for(cells_width(&styled), *w, align);
        cells.extend(at(&" ".repeat(left), body, cell.start));
        cells.extend(styled.into_iter().map(|c| Cell {
            src: cell.start + c.src,
            ..c
        }));
        let after = cell.start + cell.text.chars().count();
        cells.extend(at(&" ".repeat(right), body, after));
    }
    done(cells, src)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(l: &RLine) -> String {
        l.cells.iter().map(|c| c.ch).collect()
    }

    #[test]
    fn heading_marker_is_hidden_and_styled() {
        let l = style_line("## Title");
        assert_eq!(text(&l), "Title");
        assert!(l.cells[0].style.add_modifier.contains(Modifier::BOLD));
        // clicking the first visible cell lands on "T" in the source
        assert_eq!(l.one_row().display_to_source(0), 3);
        assert_eq!(l.one_row().display_to_source(4), 7);
        // past the end clamps to end of source
        assert_eq!(l.one_row().display_to_source(99), 8);
        assert_eq!(l.one_row().source_to_display(3), 0);
    }

    #[test]
    fn checkboxes_and_bullets() {
        let done = style_line("- [x] ship it");
        assert_eq!(text(&done), "✓ ship it");
        assert_eq!(done.one_row().display_to_source(2), 6);
        let todo = style_line("- [ ] later");
        assert_eq!(text(&todo), "☐ later");
        let bullet = style_line("- plain");
        assert_eq!(text(&bullet), "• plain");
        assert_eq!(bullet.one_row().display_to_source(2), 2);
    }

    #[test]
    fn inline_markers_are_hidden() {
        let l = style_line("a **b** c *d* `e` ==f== ~~g~~");
        assert_eq!(text(&l), "a b c d e f g");
        assert_eq!(l.one_row().display_to_source(2), 4); // "b"
    }

    #[test]
    fn links_show_only_the_text() {
        let l = style_line("see [docs](http://x.y) now");
        assert_eq!(text(&l), "see docs now");
        assert_eq!(l.one_row().display_to_source(4), 5);
        assert!(l.cells[4].style.fg == theme::link().fg);
    }

    #[test]
    fn table_rows_keep_their_characters_and_dim_the_pipes() {
        let l = style_line("| a | b |");
        assert_eq!(text(&l), "| a | b |");
        assert_eq!(l.cells[0].style, theme::marker());
        assert_eq!(l.cells[2].style, theme::PLAIN);
        assert_eq!(l.one_row().display_to_source(4), 4);
        let sep = style_line("| --- | ---: |");
        assert!(sep.cells.iter().all(|c| c.style == theme::marker()));
    }

    #[test]
    fn inline_code_in_a_table_cell_is_styled_and_kept() {
        let rows: Vec<String> = ["| a | `foo` |", "| --- | --- |", "| 1 | x `bar` y |"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let head = table_line(&rows, 0, 80);
        let t = text(&head);
        assert!(t.contains("foo"), "{t}");
        assert!(!t.contains('`'), "{t}");
        let f = head.cells.iter().find(|c| c.ch == 'f').unwrap();
        assert_eq!(f.style.fg, theme::inline_code().fg);
        // the code text maps back to its own source columns
        assert_eq!(f.src, rows[0].find('f').unwrap());
        let body = table_line(&rows, 2, 80);
        let t = text(&body);
        assert!(t.contains("x bar y"), "{t}");
        let x = body.cells.iter().find(|c| c.ch == 'x').unwrap();
        assert_eq!(x.style, theme::PLAIN);
        let b = body.cells.iter().find(|c| c.ch == 'b').unwrap();
        assert_eq!(b.style.fg, theme::inline_code().fg);
        // and the same construct outside a table still works
        let l = style_line("say `foo` now");
        assert_eq!(text(&l), "say foo now");
        assert_eq!(l.cells[4].style.fg, theme::inline_code().fg);
        assert_eq!(l.cells[4].style.bg, None);
    }

    #[test]
    fn highlight_body_is_reversed_out_of_the_page() {
        let l = style_line("a ==wow== b");
        assert_eq!(text(&l), "a wow b");
        let w = l.cells.iter().find(|c| c.ch == 'w').unwrap();
        assert_eq!(w.style.bg, theme::highlight().bg);
    }

    #[test]
    fn bare_urls_are_styled_as_links() {
        let l = style_line("see https://x.y/z. ok");
        assert_eq!(text(&l), "see https://x.y/z. ok");
        let c = l.cells[4];
        assert_eq!(c.style.fg, theme::link().fg);
        // the trailing full stop is not part of the link
        assert_eq!(l.cells[17].style.fg, None);
    }

    #[test]
    fn link_at_finds_the_url_under_a_source_column() {
        // "see [docs](http://x.y) now" — the whole span, target included
        let url = |u: &str| Some(LinkTarget::Url(u.to_string()));
        let line = "see [docs](http://x.y) now";
        assert_eq!(link_at(line, 3), None);
        assert_eq!(link_at(line, 4), url("http://x.y"));
        assert_eq!(link_at(line, 6), url("http://x.y"));
        assert_eq!(link_at(line, 21), url("http://x.y"));
        assert_eq!(link_at(line, 22), None);

        // a bare URL, without its trailing punctuation
        let bare = "see https://x.y/z. ok";
        assert_eq!(link_at(bare, 4), url("https://x.y/z"));
        assert_eq!(link_at(bare, 16), url("https://x.y/z"));
        assert_eq!(link_at(bare, 17), None);

        // images are not links to open
        assert_eq!(link_at("![alt](attachments/a.png)", 8), None);
        // the second of two links on a line
        assert_eq!(
            link_at("[a](http://a) and [b](http://b)", 20),
            url("http://b")
        );
        assert_eq!(link_at("plain text", 2), None);
        assert_eq!(link_at("[empty]()", 2), None);
    }

    #[test]
    fn done_tasks_are_struck_through() {
        let l = style_line("- [x] ship it");
        let s = l.cells.iter().find(|c| c.ch == 's').unwrap().style;
        assert!(s.add_modifier.contains(Modifier::CROSSED_OUT));
    }

    #[test]
    fn quotes_get_a_bar() {
        let l = style_line("> hi");
        assert_eq!(text(&l), "▌ hi");
        assert_eq!(l.one_row().display_to_source(0), 0);
        assert_eq!(l.one_row().display_to_source(2), 2);
    }

    #[test]
    fn cursor_line_keeps_its_checkbox_unless_the_cursor_is_in_it() {
        let text = |l: &RLine| l.cells.iter().map(|c| c.ch).collect::<String>();
        // cursor past the marker: drawn as a box, body raw
        let l = raw_with_task("- [ ] **hi**", 6);
        assert_eq!(text(&l), format!("{} **hi**", theme::UNCHECKED));
        assert_eq!(l.src_len, 12);
        // cursor inside the marker: the syntax comes back
        assert_eq!(text(&raw_with_task("- [ ] hi", 5)), "- [ ] hi");
        assert_eq!(text(&raw_with_task("- [ ] hi", 0)), "- [ ] hi");
        // indent is kept, a checked box is checked
        assert_eq!(
            text(&raw_with_task("  - [x] hi", 10)),
            format!("  {} hi", theme::CHECKED)
        );
        // not a task: plain raw
        assert_eq!(text(&raw_with_task("- [ ]", 5)), "- [ ]");
        assert_eq!(text(&raw_with_task("hi", 2)), "hi");
        assert_eq!(task_prefix("  - [ ] a"), Some((2, 8)));
        assert_eq!(task_prefix("- a"), None);
    }

    #[test]
    fn a_selection_tints_its_cells_and_the_separator_between_them() {
        let lines: Vec<String> = "| a | b | c |\n|---|---|---|".lines().map(String::from).collect();
        let block = Block { kind: BlockKind::Table, start: 0, end: 1 };
        let mut l = table_line_editing(&lines, &block, 0, 40, 0);
        let tint = Style::new().bg(ratatui::style::Color::Red);
        tint_table_cells(&mut l, &lines[0], &|c| c >= 1, tint);
        let text: String = l.cells.iter().map(|c| c.ch).collect();
        assert_eq!(text, "a │ b │ c");
        let tinted: Vec<bool> = l.cells.iter().map(|c| c.style.bg == Some(ratatui::style::Color::Red)).collect();
        // "a │ " untinted, then "b │ c" tinted, separator included
        assert_eq!(tinted, vec![false, false, false, false, true, true, true, true, true]);
    }

    #[test]
    fn raw_line_maps_one_to_one() {
        let l = RLine::raw("## Title");
        assert_eq!(text(&l), "## Title");
        for i in 0..8 {
            assert_eq!(l.one_row().display_to_source(i), i);
            assert_eq!(l.one_row().source_to_display(i), i);
        }
    }

    #[test]
    fn wide_characters_take_two_display_columns() {
        let l = style_line("**漢字** x");
        assert_eq!(text(&l), "漢字 x");
        // "漢" occupies columns 0-1, "字" columns 2-3
        assert_eq!(l.one_row().display_to_source(0), 2);
        assert_eq!(l.one_row().display_to_source(1), 2);
        assert_eq!(l.one_row().display_to_source(2), 3);
        assert_eq!(l.one_row().source_to_display(3), 2);
        // the space and the "x" sit past four columns, not two
        assert_eq!(l.one_row().display_to_source(5), 7);
    }

    fn buf(s: &str) -> Vec<String> {
        s.lines().map(String::from).collect()
    }

    #[test]
    fn an_obsidian_embed_alone_on_a_line_is_an_image() {
        assert_eq!(
            embed_line("![[attachments/hero.jpg]]"),
            Some((String::new(), "attachments/hero.jpg".into()))
        );
        assert_eq!(
            image_line("  ![[a.png|a cat]]  "),
            Some(("a cat".into(), "a.png".into()))
        );
        // a bare number after the pipe is Obsidian's width, not alt text
        assert_eq!(
            image_line("![[a.png|300]]"),
            Some((String::new(), "a.png".into()))
        );
        // a note embed is not a picture, and neither is anything malformed
        assert_eq!(embed_line("![[plan]]"), None);
        assert_eq!(embed_line("![[a.png]] tail"), None);
        assert_eq!(embed_line("![[]]"), None);
        assert_eq!(embed_line("[[a.png]]"), None);
        assert!(blocks(&["![[a.png]]".to_string()])
            .iter()
            .any(|b| b.kind == BlockKind::Image));
    }

    #[test]
    fn block_spans_cover_fences_tables_rules_and_images() {
        let lines = buf("intro\n```rust\nlet x = 1;\n```\n\n---\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\n![cat](cat.png)\n");
        let bs = blocks(&lines);
        assert_eq!(
            bs,
            vec![
                Block {
                    kind: BlockKind::Fence,
                    start: 1,
                    end: 3
                },
                Block {
                    kind: BlockKind::Rule,
                    start: 5,
                    end: 5
                },
                Block {
                    kind: BlockKind::Table,
                    start: 7,
                    end: 9
                },
                Block {
                    kind: BlockKind::Image,
                    start: 11,
                    end: 11
                },
            ]
        );
        assert_eq!(block_at(&bs, 2).unwrap().kind, BlockKind::Fence);
        assert!(block_at(&bs, 0).is_none());
        assert!(block_at(&bs, 4).is_none());
    }

    #[test]
    fn a_fence_swallows_what_looks_like_other_blocks() {
        // a rule and a table drawn inside a code sample are just code
        let lines = buf("```\n---\n| a | b |\n| --- | --- |\n```\n");
        let bs = blocks(&lines);
        assert_eq!(bs.len(), 1);
        assert_eq!(bs[0].kind, BlockKind::Fence);
        assert_eq!((bs[0].start, bs[0].end), (0, 4));
    }

    #[test]
    fn an_unclosed_fence_runs_to_the_end_of_the_buffer() {
        let lines = buf("```\nlet x = 1;\nmore\n");
        let bs = blocks(&lines);
        assert_eq!((bs[0].start, bs[0].end), (0, 2));
    }

    #[test]
    fn pipes_without_a_separator_row_are_not_a_table() {
        assert!(blocks(&buf("| a | b |\ntext\n")).is_empty());
        // and a lone image reference mid-sentence is not an image block
        assert!(blocks(&buf("see ![cat](cat.png) here\n")).is_empty());
        assert_eq!(
            image_line("  ![a cat](x/cat.png)  "),
            Some(("a cat".into(), "x/cat.png".into()))
        );
        assert_eq!(
            image_line("![](p.png)"),
            Some((String::new(), "p.png".into()))
        );
    }

    #[test]
    fn front_matter_is_one_block_and_the_markdown_scan_starts_below_it() {
        // the caller that recognised the block hands us the line under it, so
        // the closing `---` never gets a chance to be a rule
        let lines = buf("---\ntags: a\n---\n\n# Title\n\n---\n");
        let bs = blocks_from(&lines, 3);
        assert_eq!(
            bs,
            vec![Block {
                kind: BlockKind::Rule,
                start: 6,
                end: 6,
            }]
        );
    }

    #[test]
    fn blocks_from_reports_absolute_line_numbers() {
        let lines = buf("---\na: b\n---\n```\ncode\n```\n");
        let bs = blocks_from(&lines, 3);
        assert_eq!(bs.len(), 1);
        assert_eq!((bs[0].start, bs[0].end), (3, 5));
        // and starting at zero is exactly what `blocks` does
        assert_eq!(blocks_from(&lines, 0), blocks(&lines));
    }

    #[test]
    fn a_front_matter_fence_is_drawn_as_typed_not_as_a_horizontal_rule() {
        let lines = buf("---\ntags: work\n---\n");
        let block = Block {
            kind: BlockKind::FrontMatter,
            start: 0,
            end: 2,
        };
        // the fence keeps its three dashes rather than being stretched over
        // the page the way `rule_line` would stretch a thematic break
        let l = style_block_line(&lines, &block, 0, 80);
        assert_eq!(text(&l), "---");
        // and `tags:` is shown verbatim, never picked up as markdown
        let l = style_block_line(&lines, &block, 1, 80);
        assert_eq!(text(&l), "tags: work");
        assert_eq!(l.one_row().display_to_source(5), 5);
        assert!(l.cells.iter().all(|c| c.style == theme::marker()));
    }

    /// A diagram built by hand. The flow and sequence builders are their own
    /// piece of work; what the editor owns is the fitting, the styling and the
    /// click mapping, and none of the three care what drew the rows.
    fn drawn(rows: &[&str]) -> crate::mermaid::Rendered {
        use crate::mermaid::{Role, Run};
        crate::mermaid::Rendered::new(
            rows.iter()
                .map(|r| vec![Run::new(*r, Role::Node)])
                .collect(),
        )
    }

    #[test]
    fn a_mermaid_fence_is_its_own_block_kind() {
        let lines = buf("```mermaid\nflowchart LR\n  A --> B\n```\n");
        assert_eq!(
            blocks(&lines),
            vec![Block {
                kind: BlockKind::Mermaid,
                start: 0,
                end: 3,
            }]
        );
        // the info string is read however the fence spells it
        assert_eq!(
            blocks(&buf("```Mermaid {theme: dark}\nx\n```\n"))[0].kind,
            BlockKind::Mermaid
        );
        // and a fence that only looks like one is still code
        assert_eq!(
            blocks(&buf("```mermaidjs\nx\n```\n"))[0].kind,
            BlockKind::Fence
        );
    }

    #[test]
    fn the_editor_draws_a_diagram_that_fits_its_fence() {
        // five source lines, three drawn rows: centred, blank above and below
        let d = drawn(&["╭───╮", "│ A │", "╰───╯"]);
        let row = |r| text(&diagram_line(&d, 5, r, "  A --> B").unwrap());
        assert_eq!(row(0), "");
        assert_eq!(row(1), "╭───╮");
        assert_eq!(row(2), "│ A │");
        assert_eq!(row(3), "╰───╯");
        assert_eq!(row(4), "");
    }

    #[test]
    fn a_diagram_taller_than_its_fence_falls_back_to_the_fence() {
        // one display line per source line is the rule the editor lives by, so
        // a picture with nowhere to put its extra rows is not drawn at all
        let d = drawn(&["a", "b", "c", "d"]);
        assert!(diagram_line(&d, 3, 0, "```mermaid").is_none());
        // and a kind catcher does not draw is the code it always was
        let lines = buf("```mermaid\ngantt\n  title Ship it\n```\n");
        let block = Block {
            kind: BlockKind::Mermaid,
            start: 0,
            end: 3,
        };
        assert_eq!(text(&style_block_line(&lines, &block, 1, 80)), "gantt");
        assert_eq!(
            text(&style_block_line(&lines, &block, 2, 80)),
            "  title Ship it"
        );
    }

    #[test]
    fn every_source_line_of_a_mermaid_block_is_exactly_one_display_line() {
        let lines = buf("```mermaid\nflowchart LR\n  A --> B\n  B --> C\n```\n");
        let bs = blocks(&lines);
        let block = &bs[0];
        for row in block.start..=block.end {
            // whatever is drawn on it, the row still stands for its own source
            // line and for the whole of it — that is what a click maps through
            let l = style_block_line(&lines, block, row, 60);
            assert_eq!(l.src_len, lines[row].chars().count());
        }
    }

    #[test]
    fn a_click_on_a_drawn_diagram_lands_on_its_own_source_line() {
        let d = drawn(&["│ A │"]);
        let l = diagram_line(&d, 1, 0, "  A --> B").unwrap();
        // every cell of the picture maps to the start of the line it was drawn
        // on, so the click reveals the fence with the caret in the text
        assert!(l.cells.iter().all(|c| c.src == 0));
        assert_eq!(l.one_row().display_to_source(3), 0);
        assert_eq!(l.src_len, "  A --> B".chars().count());
    }

    #[test]
    fn rules_take_any_of_the_three_markers() {
        assert!(is_rule("---"));
        assert!(is_rule("  ***  "));
        assert!(is_rule("___"));
        assert!(!is_rule("--"));
        assert!(!is_rule("- item"));
    }

    #[test]
    fn a_rule_is_drawn_across_the_page_and_clicks_land_on_it() {
        let l = rule_line("---", 10);
        assert_eq!(text(&l), "──────────");
        assert_eq!(l.one_row().display_to_source(0), 0);
        // past the source's own three characters, clicks clamp to its end
        assert_eq!(l.one_row().display_to_source(9), 3);
    }

    #[test]
    fn a_fence_hides_its_backticks_and_colours_its_body() {
        // the opening cap keeps only the language, dimmed — no backticks
        let open = fence_line("```rust", true);
        assert_eq!(text(&open), "rust");
        assert_eq!(open.cells[0].style, theme::marker());
        // and the language still maps back to where it sits in the source
        assert_eq!(open.cells[0].src, 3);
        assert_eq!(open.one_row().display_to_source(0), 3);
        // a bare fence, and the close, are blank lines
        assert_eq!(text(&fence_line("```", true)), "");
        assert_eq!(text(&fence_line("~~~", true)), "");
        // a click on a blank cap still lands on that line, at its end
        assert_eq!(fence_line("```", true).one_row().display_to_source(0), 3);
        // the body is unchanged: one source line, one display line
        assert_eq!(
            fence_line("let x = 1;", false).cells[0].style,
            theme::code()
        );
        assert_eq!(text(&fence_line("let x = 1;", false)), "let x = 1;");
    }

    #[test]
    fn tables_are_laid_out_in_aligned_columns() {
        let rows = buf("| a | bbbb |\n| --- | ---: |\n| 1 | 2 |");
        assert_eq!(text(&table_line(&rows, 0, 80)), "a │ bbbb");
        assert_eq!(text(&table_line(&rows, 1, 80)), "──┼─────");
        assert_eq!(text(&table_line(&rows, 2, 80)), "1 │    2"); // right aligned
                                                                 // every row is the same width, and the head is bold
        assert!(table_line(&rows, 0, 80).cells[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD));
        assert!(!table_line(&rows, 2, 80).cells[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD));
        // it matches what the full preview draws for the same table
        let r = crate::render::render("| a | bbbb |\n| --- | ---: |\n| 1 | 2 |\n");
        let drawn: Vec<String> = r
            .lines
            .iter()
            .map(|l| l.cells.iter().map(|c| c.ch).collect::<String>())
            .filter(|t| !t.trim().is_empty())
            .collect();
        assert_eq!(drawn, vec!["a │ bbbb", "──┼─────", "1 │    2"]);
    }

    #[test]
    fn a_wide_table_is_squeezed_into_the_page_width() {
        let rows = buf("| a | bbbbbbbbbbbbbbbbbbbb |\n| --- | --- |\n| 1 | 2 |");
        for r in 0..3 {
            let l = table_line(&rows, r, 16);
            assert!(str_width(&text(&l)) <= 16, "{:?}", text(&l));
        }
        // the runaway column gave up the space, not the short one beside it
        assert_eq!(text(&table_line(&rows, 0, 16)), "a │ bbbbbbbbbbb…");
        assert_eq!(text(&table_line(&rows, 2, 16)), "1 │ 2           ");
    }

    #[test]
    fn clicking_a_laid_out_table_maps_back_into_the_source_row() {
        let rows = buf("| a | bbbb |\n| --- | ---: |\n| 1 | 2 |");
        let l = table_line(&rows, 0, 80);
        // "a" is source column 2; the separator maps to the pipe at column 4
        assert_eq!(l.one_row().display_to_source(0), 2);
        assert_eq!(l.one_row().display_to_source(2), 4);
        // "bbbb" starts at source column 6
        assert_eq!(l.one_row().display_to_source(4), 6);
        // the padding of a right-aligned cell clamps to its content
        let body = table_line(&rows, 2, 80);
        assert_eq!(body.one_row().display_to_source(4), 6);
    }

    #[test]
    fn an_image_line_falls_back_to_a_labelled_row() {
        let l = image_fallback_line("![a cat](cat.png)");
        assert_eq!(text(&l), "🖼 a cat (cat.png)");
        assert!(l.one_row().display_to_source(99) <= 17);
    }

    #[test]
    fn selection_reverses_only_the_selected_cells() {
        let l = style_line("hello");
        let line = l.to_line(Some((1, 3)));
        let rev: String = line
            .spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(rev, "el");
    }

    /// The known-target set is process-wide and `cargo test` runs its tests in
    /// parallel, so the ones that care what colour a wikilink is drawn in take
    /// turns here. Without it, the test that installs a set of known names
    /// races the ones that assume nothing has been scanned, and a plain link
    /// comes out in the broken colour for whichever of them lost.
    fn colours() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        // a failed assertion poisons the lock; the next test still wants its turn
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn a_wikilink_shows_its_target_as_the_text() {
        let _turn = colours();
        // "see [[note]] now" — the brackets go, the target stays, and every
        // drawn character still knows the column it came from
        let l = style_line("see [[note]] now");
        assert_eq!(text(&l), "see note now");
        let row = l.one_row();
        assert_eq!(row.display_to_source(4), 6); // the "n" of note
        assert_eq!(l.cells[4].style.fg, theme::link().fg);
    }

    #[test]
    fn a_piped_wikilink_shows_only_its_label() {
        let src = "[[stories/story-matrix|the matrix]]";
        let l = style_line(src);
        assert_eq!(text(&l), "the matrix");
        // the first drawn cell is the label's own column, not column 0: a
        // click there has to land inside the label, not on the target
        assert_eq!(l.one_row().display_to_source(0), src.find("the").unwrap());
        let w = wikilink_at(&src.chars().collect::<Vec<_>>(), 0).unwrap();
        assert_eq!(w.target, "stories/story-matrix");
    }

    #[test]
    fn a_pipe_with_no_label_after_it_draws_the_target_and_not_the_pipe() {
        // `[[note|]]` is a label someone deleted, or one they have not typed
        // yet; the target is what is drawn, and a trailing `|` is punctuation
        // from the syntax rather than something the note says
        assert_eq!(text(&style_line("[[note|]]")), "note");
        assert_eq!(text(&style_line("[[note| ]]")), "note");
        let chars: Vec<char> = "[[note|]]".chars().collect();
        let w = wikilink_at(&chars, 0).unwrap();
        assert_eq!((w.label_start, w.label_end), (2, 6));
    }

    #[test]
    fn a_heading_suffix_is_shown_but_is_not_part_of_the_target() {
        let l = style_line("[[note#Method]]");
        // Obsidian shows the whole thing; only the target drops the heading
        assert_eq!(text(&l), "note#Method");
        let chars: Vec<char> = "[[note#Method]]".chars().collect();
        assert_eq!(wikilink_at(&chars, 0).unwrap().target, "note");
    }

    #[test]
    fn unmatched_or_escaped_brackets_stay_literal_text() {
        for src in [
            "[[unclosed",
            "[[a] b]]",
            "\\[[escaped]]",
            "![[embed.png]]",
            "[[ ]]",
            "[[#heading]]",
            "[x]",
        ] {
            assert_eq!(text(&style_line(src)), src, "{src}");
        }
    }

    #[test]
    fn two_wikilinks_on_one_line_are_both_links() {
        let _turn = colours();
        let l = style_line("[[a]] and [[b|bee]]");
        assert_eq!(text(&l), "a and bee");
        assert_eq!(l.cells[0].style.fg, theme::link().fg);
        assert_eq!(l.cells[6].style.fg, theme::link().fg);
        // the space between them is plain
        assert_eq!(l.cells[1].style.fg, None);
    }

    #[test]
    fn an_unresolved_wikilink_is_grey_and_still_underlined() {
        // both halves live in one test on purpose: the known-target set is
        // process-wide, and `cargo test` runs these in parallel, so splitting
        // the assertions would let one of them race the other's set-up
        let _turn = colours();
        let mut known = std::collections::HashSet::new();
        known.insert("real".to_string());
        links::set_known(known);

        let ok = style_line("[[real]]");
        assert_eq!(ok.cells[0].style.fg, theme::link().fg);
        let broken = style_line("[[gone]]");
        assert_eq!(broken.cells[0].style.fg, theme::grey().fg);
        assert!(broken.cells[0]
            .style
            .add_modifier
            .contains(Modifier::UNDERLINED));
        // a name is matched without its case or its extension
        assert_eq!(
            style_line("[[Real.md]]").cells[0].style.fg,
            theme::link().fg
        );

        links::forget();
        // nothing scanned means nothing is broken yet
        assert_eq!(style_line("[[gone]]").cells[0].style.fg, theme::link().fg);
    }

    #[test]
    fn link_at_tells_a_wikilink_from_a_url() {
        let line = "see [[note|label]] and [d](http://x.y)";
        let wiki = Some(LinkTarget::Wiki("note".to_string()));
        assert_eq!(link_at(line, 3), None);
        assert_eq!(link_at(line, 4), wiki); // the opening bracket
        assert_eq!(link_at(line, 12), wiki); // inside the label
        assert_eq!(link_at(line, 17), wiki); // the last `]`
        assert_eq!(link_at(line, 18), None);
        assert_eq!(
            link_at(line, 24),
            Some(LinkTarget::Url("http://x.y".to_string()))
        );
        assert_eq!(link_at(line, 99), None);
    }

    #[test]
    fn a_tag_is_a_hash_on_a_word_boundary_then_a_letter() {
        let ends = |line: &str| -> Vec<String> {
            let chars: Vec<char> = line.chars().collect();
            tags_in(line)
                .into_iter()
                .map(|(s, e)| chars[s..e].iter().collect())
                .collect()
        };
        assert_eq!(ends("a #work note"), vec!["#work"]);
        assert_eq!(ends("#top of line"), vec!["#top"]);
        assert_eq!(ends("(#paren) \"#quoted\""), vec!["#paren", "#quoted"]);
        assert_eq!(ends("#a-b_c/d9 tail"), vec!["#a-b_c/d9"]);
        // a tag ends at the first character that cannot be in one
        assert_eq!(ends("#done."), vec!["#done"]);
        // a heading marker, a number and a bare hash are not tags
        assert!(ends("# Heading").is_empty());
        assert!(ends("## Heading").is_empty());
        assert!(ends("#1 and # and #").is_empty());
        // nor is anything glued to the word before it
        assert!(ends("a#b c&#d").is_empty());
    }

    #[test]
    fn a_tag_inside_code_a_link_or_a_url_is_not_one() {
        assert!(tags_in("`#code` [t](http://x.y/#frag)").is_empty());
        assert!(tags_in("https://x.y/p#frag").is_empty());
        assert!(tags_in("see `a #b` c").is_empty());
        // but one beside them still is
        assert_eq!(tags_in("`x` #tag https://a.b#c"), vec![(4, 8)]);
    }

    #[test]
    fn a_hash_inside_a_wikilink_is_a_heading_and_not_a_tag() {
        // what the index counts must be what the styling draws, and the
        // styling takes the whole [[…]] before it ever looks for a tag
        assert!(tags_in("[[#heading]] [[note#part|alias]]").is_empty());
        assert_eq!(tags_in("[[note]] #tag"), vec![(9, 13)]);
        // and neither the cursor nor the styling finds a tag there either
        assert_eq!(link_at("[[#heading]]", 3), None);
        let l = style_line("[[#heading]]");
        assert!(l.cells.iter().all(|c| c.style.fg != theme::tag().fg));
    }

    #[test]
    fn a_tag_is_drawn_in_the_accent_and_kept_whole() {
        tags::set_enabled(true);
        let l = style_line("note #work here");
        assert_eq!(text(&l), "note #work here");
        assert_eq!(l.cells[5].style.fg, theme::tag().fg);
        assert_eq!(l.cells[9].style.fg, theme::tag().fg);
        assert_eq!(l.cells[10].style, theme::PLAIN);
        // a heading's marker is a marker, and a tag in the heading is a tag
        let h = style_line("# Title #work");
        assert_eq!(text(&h), "Title #work");
        assert_eq!(h.cells[6].style.fg, theme::tag().fg);
        // a fragment is part of its link
        let u = style_line("https://x.y/#frag");
        assert_eq!(u.cells[12].style.fg, theme::link().fg);
    }

    #[test]
    fn link_at_finds_a_tag_and_steps_over_code() {
        tags::set_enabled(true);
        let line = "see `#no` and #yes now";
        assert_eq!(link_at(line, 5), None);
        assert_eq!(link_at(line, 14), Some(LinkTarget::Tag("yes".to_string())));
        assert_eq!(link_at(line, 17), Some(LinkTarget::Tag("yes".to_string())));
        assert_eq!(link_at(line, 18), None);
    }

    #[test]
    fn tag_key_drops_the_hash_and_the_case() {
        assert_eq!(tag_key("#Work/Q3"), "work/q3");
        assert_eq!(tag_key(" work "), "work");
    }

    #[test]
    fn a_tag_href_round_trips_through_the_scheme() {
        let t = LinkTarget::Tag("work".to_string());
        assert_eq!(t.href(), "tag:work");
        assert_eq!(LinkTarget::parse(&t.href()), t);
        let url = LinkTarget::Url("tag:x".to_string());
        assert_eq!(LinkTarget::parse(&url.href()), url);
    }

    #[test]
    fn every_wikilink_on_a_line_is_found_once_and_in_order() {
        let found = wikilinks("see [[a]] and [[b|bee]] and [[unclosed");
        let targets: Vec<&str> = found.iter().map(|w| w.target.as_str()).collect();
        assert_eq!(targets, vec!["a", "b"]);
        // and the spans are the whole `[[…]]`, so nothing is scanned twice
        assert_eq!(
            &"see [[a]] and [[b|bee]] and [[unclosed"[found[0].start..found[0].end],
            "[[a]]"
        );
        assert!(wikilinks("nothing here at all").is_empty());
    }

    #[test]
    fn a_wikilink_href_round_trips_through_the_scheme() {
        let w = LinkTarget::Wiki("a/b".to_string());
        assert_eq!(w.href(), "wikilink:a/b");
        assert_eq!(LinkTarget::parse(&w.href()), w);
        // and a row the app drew itself names its file outright, so nothing
        // has to be resolved a second time when it is clicked
        let n = LinkTarget::Note("/vault/meta.md".to_string());
        assert_eq!(n.href(), "note:/vault/meta.md");
        assert_eq!(LinkTarget::parse(&n.href()), n);
        assert_eq!(
            LinkTarget::parse("https://x.y"),
            LinkTarget::Url("https://x.y".to_string())
        );
    }

    #[test]
    fn a_url_that_spells_out_the_apps_own_scheme_stays_a_url() {
        // `[report](note:/etc/passwd)` is a note body, not the app naming a
        // file it found: it must come back out as the URL it is, spelled
        // exactly as it was typed, and go to the desktop opener like any other
        for u in ["note:/etc/passwd", "url:note:/etc/passwd", "url:x"] {
            let url = LinkTarget::Url(u.to_string());
            assert_eq!(LinkTarget::parse(&url.href()), url, "{u}");
        }
        // and the app's own row is still a file, by path
        assert_eq!(
            LinkTarget::parse("note:/vault/meta.md"),
            LinkTarget::Note("/vault/meta.md".to_string())
        );
    }

    #[test]
    fn link_key_drops_the_heading_the_extension_and_the_case() {
        assert_eq!(
            link_key("Stories/Story-Matrix.md#Method"),
            link_key("stories/story-matrix")
        );
        assert_eq!(link_key(" A\\B "), "a/b");
    }
}

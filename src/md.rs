//! Line-based markdown styling shared by the live-preview editor and the
//! full-page preview renderer.
//!
//! A source line becomes a [`RLine`]: one display cell per visible character,
//! each remembering which source column it came from. Keeping the mapping at
//! cell granularity makes both directions trivial — cursor placement from a
//! click, and selection highlighting from source columns — even when markers
//! like `## ` or `- [ ] ` are hidden or replaced.

use crate::theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// How many terminal columns a character occupies. Zero-width characters are
/// still given a column so the cursor has somewhere to sit.
pub fn char_width(ch: char) -> usize {
    ch.width().unwrap_or(0).max(1)
}

/// The first character of a theme marker string.
fn first_char(s: &str) -> char {
    s.chars().next().unwrap_or(' ')
}

/// Whether `g` is a grapheme cluster the editor treats as one unit: a ZWJ
/// sequence (a family emoji) or a regional-indicator pair (a flag). Other
/// clusters, combining marks included, stay one column per char so the
/// cursor can sit on each.
pub fn is_joined_cluster(g: &str) -> bool {
    if g.contains('\u{200D}') {
        return true;
    }
    let mut it = g.chars();
    matches!((it.next(), it.next(), it.next()), (Some(a), Some(b), None) if is_ri(a) && is_ri(b))
}

fn is_ri(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

/// The joined cluster (see `is_joined_cluster`) containing the char at
/// `col`, as a `(start, end)` char range, or `None` when that char stands
/// alone.
pub fn joined_cluster_at(s: &str, col: usize) -> Option<(usize, usize)> {
    let mut start = 0;
    for g in s.graphemes(true) {
        let n = g.chars().count();
        if col < start + n {
            return (n > 1 && is_joined_cluster(g)).then_some((start, start + n));
        }
        start += n;
    }
    None
}

/// Display width of a string, in terminal columns.
pub fn str_width(s: &str) -> usize {
    s.graphemes(true)
        .map(|g| {
            if is_joined_cluster(g) {
                g.width().max(1)
            } else {
                g.chars().map(char_width).sum()
            }
        })
        .sum()
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

/// A cell of ours rather than the source's: a code band's side padding, its
/// line numbers, and the ground painted out to the page edge. The mapping
/// treats it as no column at all — a click on one lands on the code beside it,
/// the cursor never sits on one, and a selection never picks the numbers up.
pub const PAD: usize = usize::MAX;

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
        self.to_line_marked(selection, &[])
    }

    /// `to_line`, with the source-column ranges in `marks` lit the way a
    /// search hit is: what the find overlay shows on the page.
    pub fn to_line_marked(
        &self,
        selection: Option<(usize, usize)>,
        marks: &[(usize, usize)],
    ) -> Line<'static> {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut text = String::new();
        let mut current: Option<Style> = None;
        for cell in &self.cells {
            let mut style = cell.style;
            if marks.iter().any(|&(a, b)| cell.src >= a && cell.src < b) {
                style = style
                    .patch(crate::theme::state())
                    .add_modifier(Modifier::UNDERLINED);
            }
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
        for (i, c) in self.cells.iter().enumerate() {
            let w = char_width(c.ch);
            if col < x + w {
                if c.src != PAD {
                    return c.src;
                }
                // a band's gutter stands for the code beside it; padding past
                // the code stands for the end of the row
                return self.cells[i..]
                    .iter()
                    .map(|c| c.src)
                    .find(|s| *s != PAD)
                    .unwrap_or(self.end_src);
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
        // the band's fill past the code is nobody's column either: the end of
        // the line is just after its last real cell, not the page edge
        let last = self.cells.iter().rposition(|c| c.src != PAD);
        let mut x = 0;
        for (i, c) in self.cells.iter().enumerate() {
            // padding is nobody's column, so the cursor walks past the gutter
            // rather than landing on it
            if c.src != PAD && c.src >= col {
                return self.indent + x;
            }
            if last.is_some_and(|l| i > l) {
                return self.indent + x;
            }
            x += char_width(c.ch);
        }
        self.indent + x
    }

    /// This row as a ratatui line, hanging indent and all: [`RLine::to_line`]
    /// with `marks` (source-column ranges) lit as search hits.
    pub fn to_line_marked(
        &self,
        selection: Option<(usize, usize)>,
        marks: &[(usize, usize)],
    ) -> Line<'static> {
        let inner = RLine {
            cells: self.cells.clone(),
            src_len: self.end_src,
        }
        .to_line_marked(selection, marks);
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
    // a code band's gutter is ours, not the line's: it comes off the front,
    // the code is wrapped in what is left, and it goes back — blank — on every
    // continuation row, so the band's left edge stays one straight line
    let gutter = line.cells.iter().take_while(|c| c.src == PAD).count();
    let (head, rest) = line.cells.split_at(gutter);
    let head_w: usize = head.iter().map(|c| char_width(c.ch)).sum();
    // a hanging indent that ate half the page would be worse than none
    let indent = if gutter == 0 {
        hanging_indent(rest).min(width / 2)
    } else {
        0
    };
    let avail = width.saturating_sub(head_w).max(1);
    let blank: Vec<Cell> = head.iter().map(|c| Cell { ch: ' ', ..*c }).collect();
    let chars: Vec<char> = rest.iter().map(|c| c.ch).collect();
    wrap_breaks(&chars, avail, avail - indent)
        .into_iter()
        .enumerate()
        .map(|(i, (s, e))| {
            let mut cells = if i == 0 { head.to_vec() } else { blank.clone() };
            cells.extend_from_slice(&rest[s..e]);
            // the band is painted to the page edge, so a short line of code is
            // still a row of ground rather than a stripe
            if gutter > 0 {
                let used: usize = cells.iter().map(|c| char_width(c.ch)).sum();
                let style = theme::code();
                cells.extend(std::iter::repeat_n(
                    Cell {
                        ch: ' ',
                        style,
                        src: PAD,
                    },
                    width.saturating_sub(used),
                ));
            }
            Seg {
                cells,
                indent: if i == 0 { 0 } else { indent },
                end_src: rest.get(e).map_or(line.src_len, |c| c.src),
            }
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
    let mut markers = vec![
        first_char(theme::BULLET),
        first_char(theme::BULLET_2),
        first_char(theme::BULLET_3),
    ];
    markers.extend(theme::TASK_GLYPHS.iter().map(|g| first_char(g)));
    // a drawn box has a colour; a `?` that opens a plain sentence has none
    let drawn = |k: usize| {
        cells
            .get(k)
            .is_some_and(|c| c.ch != '?' || c.style.fg.is_some())
    };
    let mut j = i;
    match ch(j) {
        Some(c) if markers.contains(&c) && drawn(j) => j += 1,
        Some('-') | Some('*') | Some('+') => j += 1,
        Some(c) if c.is_ascii_digit() => {
            while matches!(ch(j), Some(c) if c.is_ascii_digit()) {
                j += 1;
            }
            if !matches!(ch(j), Some('.') | Some(')')) {
                return w;
            }
            j += 1;
            // a numbered task: `1. ☐ text` hangs under the text
            if ch(j) == Some(' ')
                && matches!(ch(j + 1), Some(c) if markers.contains(&c) && drawn(j + 1))
                && ch(j + 2) == Some(' ')
            {
                j += 2;
            }
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
    /// The ordinal the next footnote reference on this line gets — an inline
    /// `^[note]` is drawn as that number, so a line has to know how many
    /// references came before it in the note.
    note: usize,
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

/// The inline pass over `text` with `base` as its style: cells whose
/// source columns are relative to `text`.
fn styled_inline(text: &str, base: Style) -> Vec<Cell> {
    styled_cell(text, base)
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
        note: 1,
    };
    inline(&mut b, 0, theme::PLAIN);
    b.cells
}

/// Style one markdown source line for display.
pub fn style_line(src: &str) -> RLine {
    style_line_from(src, 1)
}

/// The same, for a line whose first footnote reference is number `note` in
/// the note: an inline `^[text]` is drawn as that ordinal. `style_line`
/// starts every line at 1; `style_line_in` counts the lines above.
pub fn style_line_from(src: &str, note: usize) -> RLine {
    let mut line = style_line_inner(src, note);
    // a trailing ` ^blockid` is an address, not prose: kept, but dimmed to
    // the weight of a marker, whatever the line around it was styled as
    if let Some((col, _)) = block_id_at(src) {
        for cell in line.cells.iter_mut().filter(|c| c.src >= col) {
            cell.style = theme::marker();
        }
    }
    line
}

fn style_line_inner(src: &str, note: usize) -> RLine {
    let chars: Vec<char> = src.chars().collect();
    let src_len = chars.len();
    let mut b = Builder {
        src: &chars,
        cells: Vec::with_capacity(src_len),
        note,
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
    let indent_start = i;
    while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
        b.keep(i, base);
        i += 1;
    }
    let depth = list_depth(&chars[indent_start..i]);

    // a callout's title line: `[!type] Title` becomes the glyph, the type
    // and the title, all in the accent
    if base == theme::quote() && i > 0 {
        if let Some((kind, after, title)) = callout_title(&chars, i) {
            let style = theme::callout(&kind).add_modifier(Modifier::BOLD);
            let mut head = String::new();
            if let Some(g) = callout_glyph(&kind) {
                head.push(g);
                head.push(' ');
            }
            head.push_str(&kind);
            b.sub(&head, style, i);
            for k in i + 1..after {
                b.sub("", style, k);
            }
            if title < chars.len() {
                b.sub(" · ", style, after);
            }
            for k in after..title {
                b.sub("", style, k);
            }
            inline(&mut b, title, style);
            return RLine {
                cells: b.cells,
                src_len,
            };
        }
    }

    // a footnote's definition: `[^1]: text` becomes `¹ text`
    if chars.get(i) == Some(&'[') && chars.get(i + 1) == Some(&'^') {
        if let Some(close) = find(&chars, i + 2, ']') {
            if chars.get(close + 1) == Some(&':') {
                let label: String = chars[i + 2..close].iter().collect();
                b.sub(&superscript(&label), theme::state(), i);
                for k in i + 1..=close + 1 {
                    b.sub("", theme::state(), k);
                }
                let mut t = close + 2;
                if chars.get(t) == Some(&' ') {
                    b.sub(" ", theme::PLAIN, t);
                    t += 1;
                }
                inline(&mut b, t, base);
                return RLine {
                    cells: b.cells,
                    src_len,
                };
            }
        }
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
    if let Some((marker, style, width)) = list_marker(&chars, i, depth) {
        b.sub(marker, style, i);
        b.sub(" ", style, i + 1);
        for k in i..i + width {
            if k >= i + 2 {
                b.sub("", style, k);
            }
        }
        i += width;
        if struck(marker) {
            base = base.patch(theme::done_text());
        }
    } else if let Some((at, marker, style, end)) = ordered_task(&chars, i) {
        // `1. [ ] text`: the number stays, the box becomes a glyph
        for k in i..at {
            b.keep(k, base);
        }
        b.sub(marker, style, at);
        b.sub(" ", style, at + 1);
        for k in at + 2..end {
            b.sub("", style, k);
        }
        i = end;
        if struck(marker) {
            base = base.patch(theme::done_text());
        }
    }

    inline(&mut b, i, base);

    // a hard line break — two trailing spaces or a trailing backslash — shows
    // as ↵ in place of its last column; the other trailing spaces stay blank
    if let Some(col) = hard_break_col(&chars) {
        if col >= i {
            while b.cells.last().is_some_and(|c| c.src == col) {
                b.cells.pop();
            }
            b.sub(theme::HARD_BREAK, theme::marker(), col);
        }
    }
    RLine {
        cells: b.cells,
        src_len,
    }
}

/// The column of the hard line break ending `chars`: the last of two or more
/// trailing spaces, or a trailing backslash that is not itself escaped.
/// `None` when the line is blank or ends in neither.
fn hard_break_col(chars: &[char]) -> Option<usize> {
    let last = chars.len().checked_sub(1)?;
    if chars[last] == ' ' {
        let spaces = chars.iter().rev().take_while(|c| **c == ' ').count();
        return (spaces >= 2 && spaces < chars.len()).then_some(last);
    }
    if chars[last] == '\\' {
        let slashes = chars.iter().rev().take_while(|c| **c == '\\').count();
        return (slashes % 2 == 1).then_some(last);
    }
    None
}

/// The source columns `start..end` of a task line's box: from the list
/// marker of `- [ ] ` (indent excluded), or from the `[` of a numbered
/// `1. [ ] ` — in both cases `start` is the column the glyph is drawn at.
/// `None` for any other line.
pub fn task_prefix(src: &str) -> Option<(usize, usize)> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    while matches!(chars.get(i), Some(' ') | Some('\t')) {
        i += 1;
    }
    task_at(&chars, i).map(|(at, _, _, end)| (at, end))
}

/// The glyph, style and end column of the task box at the list marker at
/// `i`, bullet or numbered, with the column the glyph stands at.
fn task_at(chars: &[char], i: usize) -> Option<(usize, &'static str, Style, usize)> {
    match list_marker(chars, i, 1) {
        Some((marker, style, 6)) => Some((i, marker, style, i + 6)),
        Some(_) => None,
        None => ordered_task(chars, i),
    }
}

/// The task box after an ordered marker at `i`: `12. [ ] ` or `12) [x] `.
/// Returns (column of the `[`, glyph, style, end column).
fn ordered_task(chars: &[char], i: usize) -> Option<(usize, &'static str, Style, usize)> {
    let at = |k: usize| chars.get(k).copied();
    let mut j = i;
    while matches!(at(j), Some(c) if c.is_ascii_digit()) {
        j += 1;
    }
    if j == i || !matches!(at(j), Some('.') | Some(')')) || at(j + 1) != Some(' ') {
        return None;
    }
    let b = j + 2;
    if at(b) != Some('[') || at(b + 2) != Some(']') || at(b + 3) != Some(' ') {
        return None;
    }
    let (glyph, style) = task_state(at(b + 1)?)?;
    Some((b, glyph, style, b + 4))
}

/// The glyph and style a task box holding `c` is drawn as: the two Markdown
/// states, and the Obsidian Tasks ones — `/` in progress, `-` cancelled,
/// `>` forwarded, `?` a question. Any other character is not a box.
pub fn task_state(c: char) -> Option<(&'static str, Style)> {
    Some(match c {
        ' ' => (theme::UNCHECKED, theme::marker()),
        'x' | 'X' => (theme::CHECKED, theme::done()),
        '/' => (theme::IN_PROGRESS, theme::done()),
        '-' => (theme::CANCELLED, theme::marker()),
        '>' => (theme::FORWARDED, theme::forwarded()),
        '?' => (theme::QUESTION, theme::done()),
        _ => return None,
    })
}

/// Whether the text of a task drawn with `glyph` reads as struck through:
/// done and cancelled, the two states that are over.
pub fn struck(glyph: &str) -> bool {
    glyph == theme::CHECKED || glyph == theme::CANCELLED
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
        note: 1,
    };
    for k in 0..start {
        b.keep(k, theme::PLAIN);
    }
    let (marker, style) = task_glyph_at(&chars, start).expect("task_prefix said so");
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

/// How deeply a list item with this leading whitespace is nested, 1 being
/// top level. Obsidian's rule, simplified: every tab is one level and every
/// two spaces are one level, so `- a` is depth 1, `  - b` and `\t- b` are
/// depth 2, `    - c` is depth 3. An odd space is ignored.
fn list_depth(indent: &[char]) -> usize {
    let tabs = indent.iter().filter(|c| **c == '\t').count();
    let spaces = indent.len() - tabs;
    1 + tabs + spaces / 2
}

/// Recognise `- [ ] `, `- [x] `, `- `, `* `, `+ ` at `i`; a plain bullet's
/// glyph follows the item's nesting `depth` (see `theme::bullet`).
/// Returns (display marker, style, consumed source width).
fn list_marker(chars: &[char], i: usize, depth: usize) -> Option<(&'static str, Style, usize)> {
    let at = |k: usize| chars.get(k).copied();
    let bullet = matches!(at(i), Some('-') | Some('*') | Some('+'));
    if !bullet || at(i + 1) != Some(' ') {
        return None;
    }
    if at(i + 2) == Some('[') && at(i + 4) == Some(']') && at(i + 5) == Some(' ') {
        return match at(i + 3).and_then(task_state) {
            Some((glyph, style)) => Some((glyph, style, 6)),
            None => Some((theme::bullet(depth), theme::marker(), 2)),
        };
    }
    Some((theme::bullet(depth), theme::marker(), 2))
}

/// The glyph and style of the box whose `[` — or whose bullet — sits at
/// column `at`, as `task_prefix` reports it.
fn task_glyph_at(chars: &[char], at: usize) -> Option<(&'static str, Style)> {
    match list_marker(chars, at, 1) {
        Some((glyph, style, 6)) => Some((glyph, style)),
        _ if chars.get(at) == Some(&'[') => task_state(*chars.get(at + 1)?),
        _ => None,
    }
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

    // \* — a backslash escape: the backslash dimmed, the punctuation after it
    // literal, so `\*not\*` is not italic and `\#2026` is not a tag. First,
    // ahead of every construct the escaped character could otherwise open.
    if c == '\\' {
        if let Some(next) = b.src.get(i + 1) {
            if next.is_ascii_punctuation() {
                b.keep(i, theme::marker());
                b.keep(i + 1, base);
                return Some(i + 2);
            }
        }
    }

    // ![[note]] in a sentence — an embedded note, drawn as the link it also
    // is: the `!` goes with the brackets, and a `#heading` reads as ` › `
    if c == '!'
        && b.src.get(i + 1) == Some(&'[')
        && b.src.get(i + 2) == Some(&'[')
        && links::enabled()
    {
        if let Some(w) = wikilink_at(b.src, i + 1) {
            let style = wiki_style(base, &w.target);
            b.sub("", style, i);
            embed_label(b, &w, style);
            return Some(w.end);
        }
    }

    // [[wikilink]] — checked before `[text](url)`, which falls straight
    // through on a double bracket and would leave it as literal text
    if c == '[' && b.src.get(i + 1) == Some(&'[') && links::enabled() {
        if let Some(w) = wikilink_at(b.src, i) {
            let style = wiki_style(base, &w.target);
            // `[[note#Heading]]` reads as `note › Heading`: the `#` cell is
            // drawn as the chevron, so every column still has its one cell
            if let Some(h) = w.shown_hash(b.src) {
                delimited(b, w.start, w.label_start, h, h, style);
                let sep = if h == w.label_start { "› " } else { " › " };
                b.sub(sep, style, h);
                return Some(delimited(b, h + 1, h + 1, w.label_end, w.end, style));
            }
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

    // %% comment %% — kept as typed, delimiters and all, only quiet; nothing
    // inside it is markdown, so the whole span is swallowed here
    if c == '%' {
        if let Some(end) = inline_comment_end(b.src, i) {
            for k in i..end {
                b.keep(k, theme::marker());
            }
            return Some(end);
        }
    }

    // [^1] — a footnote reference, as a superscript
    if c == '[' && b.src.get(i + 1) == Some(&'^') {
        if let Some(close) = find(b.src, i + 2, ']') {
            let label: String = b.src[i + 2..close].iter().collect();
            if !label.is_empty() && !label.contains(' ') {
                b.sub(&superscript(&label), theme::state(), i);
                for k in i + 1..=close {
                    b.sub("", theme::state(), k);
                }
                b.note += 1;
                return Some(close + 1);
            }
        }
    }

    // ^[note] — an inline footnote: its number as a superscript, the text
    // kept but quiet, the brackets gone
    if c == '^' && b.src.get(i + 1) == Some(&'[') {
        if let Some(close) = find(b.src, i + 2, ']') {
            if close > i + 2 {
                b.sub(&superscript(&b.note.to_string()), theme::state(), i);
                b.sub("", theme::state(), i + 1);
                for k in i + 2..close {
                    b.keep(k, theme::marker());
                }
                b.sub("", theme::state(), close);
                b.note += 1;
                return Some(close + 1);
            }
        }
    }

    // $x$ or $$x$$ — maths, in italics with the dollars hidden
    if c == '$' {
        let n = if b.src.get(i + 1) == Some(&'$') { 2 } else { 1 };
        let start = i + n;
        if b.src.get(start).is_some_and(|ch| !ch.is_whitespace()) {
            let mut k = start;
            while k < b.src.len() {
                if b.src[k] == '$' && (n == 1 || b.src.get(k + 1) == Some(&'$')) {
                    if k > start && !b.src[k - 1].is_whitespace() {
                        let style = base.patch(theme::math());
                        return Some(delimited(b, i, start, k, k + n, style));
                    }
                    break;
                }
                k += 1;
            }
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

    // <tag>, </tag>, <br> and <!-- comments --> — the little HTML Obsidian
    // users write. Tags stay on the page, dimmed; a known pair styles its body
    if c == '<' {
        if let Some(end) = html_comment_end(b.src, i) {
            for k in i..end {
                b.keep(k, theme::marker());
            }
            return Some(end);
        }
        if let Some(tag) = html_tag_at(b.src, i) {
            let dim = theme::marker();
            if tag.name == "br" {
                b.sub("↵", dim, i);
                for k in i + 1..tag.end {
                    b.sub("", dim, k);
                }
                return Some(tag.end);
            }
            if tag.opens() {
                if let Some(style) = html_style(&tag.name, base) {
                    if let Some((close, close_end)) = html_close_at(b.src, tag.end, &tag.name) {
                        for k in i..tag.end {
                            b.keep(k, dim);
                        }
                        for k in tag.end..close {
                            let ch = b.src[k];
                            let shown = match tag.name.as_str() {
                                "sub" => sub_char(ch),
                                "sup" => sup_char(ch),
                                _ => None,
                            };
                            match shown {
                                Some(s) => b.sub(&s.to_string(), style, k),
                                None => b.keep(k, style),
                            }
                        }
                        for k in close..close_end {
                            b.keep(k, dim);
                        }
                        return Some(close_end);
                    }
                }
            }
            for k in i..tag.end {
                b.keep(k, dim);
            }
            return Some(tag.end);
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

/// The end (exclusive) of the `%% … %%` comment opening at `i`, if one does:
/// a `%%` with a closing `%%` further along the line. An unclosed `%%` is
/// literal text, so it gets `None`.
pub(crate) fn inline_comment_end(src: &[char], i: usize) -> Option<usize> {
    if src.get(i) != Some(&'%') || src.get(i + 1) != Some(&'%') {
        return None;
    }
    let mut k = i + 2;
    while k + 1 < src.len() {
        if src[k] == '%' && src[k + 1] == '%' {
            return Some(k + 2);
        }
        k += 1;
    }
    None
}

/// A `%%` alone on a line: the fence of a block comment, when it has a
/// partner further down.
pub(crate) fn is_comment_fence(line: &str) -> bool {
    line.trim() == "%%"
}

/// The byte ranges of `markdown` that are Obsidian comments — block comments
/// fenced by `%%` lines, and inline `%% … %%` spans — so the reading view can
/// leave them out. Ascending and disjoint. A fenced code block is code, not
/// prose, so a `%%` inside one is kept; so is one inside a `` `code` `` span.
/// A line that holds nothing but comments goes altogether, newline included,
/// so `a\n%% x %%\nb` still reads as one paragraph.
pub fn comment_cuts(markdown: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    // (byte start of the line, the line without its newline, byte end incl. newline)
    let lines: Vec<(usize, &str, usize)> = {
        let mut v = Vec::new();
        let mut start = 0;
        for l in markdown.split_inclusive('\n') {
            let end = start + l.len();
            v.push((start, l.trim_end_matches(['\n', '\r']), end));
            start = end;
        }
        v
    };
    let mut in_fence = false;
    let mut i = 0;
    while i < lines.len() {
        let (start, line, end) = lines[i];
        if is_fence(line) {
            in_fence = !in_fence;
            i += 1;
            continue;
        }
        if in_fence {
            i += 1;
            continue;
        }
        if is_comment_fence(line) {
            if let Some(j) = (i + 1..lines.len()).find(|j| is_comment_fence(lines[*j].1)) {
                out.push(start..lines[j].2);
                i = j + 1;
                continue;
            }
        }
        let chars: Vec<char> = line.chars().collect();
        let mut spans: Vec<std::ops::Range<usize>> = Vec::new();
        let mut byte = start;
        let mut k = 0;
        while k < chars.len() {
            // `\%` is a literal per cent sign, not the start of a comment
            if chars[k] == '\\' && chars.get(k + 1).is_some_and(|c| c.is_ascii_punctuation()) {
                byte += chars[k].len_utf8() + chars[k + 1].len_utf8();
                k += 2;
                continue;
            }
            if chars[k] == '`' {
                if let Some(close) = find(&chars, k + 1, '`') {
                    byte += chars[k..=close].iter().map(|c| c.len_utf8()).sum::<usize>();
                    k = close + 1;
                    continue;
                }
            }
            if let Some(mut e) = inline_comment_end(&chars, k) {
                // `a %% x %% b` reads `a b`, not `a  b`: a comment sitting
                // between two spaces takes one of them with it
                if k > 0 && chars[k - 1] == ' ' && chars.get(e) == Some(&' ') {
                    e += 1;
                }
                let len: usize = chars[k..e].iter().map(|c| c.len_utf8()).sum();
                spans.push(byte..byte + len);
                byte += len;
                k = e;
                continue;
            }
            byte += chars[k].len_utf8();
            k += 1;
        }
        if !spans.is_empty() {
            let mut rest = String::new();
            let mut at = start;
            for s in &spans {
                rest.push_str(&markdown[at..s.start]);
                at = s.end;
            }
            rest.push_str(&markdown[at..start + line.len()]);
            if rest.trim().is_empty() {
                out.push(start..end);
            } else {
                out.extend(spans);
            }
        }
        i += 1;
    }
    out
}

/// `markdown` with its comments cut out, and where the cuts were: each entry
/// is the byte offset in the stripped text at which some bytes went missing,
/// and how many. The reading view renders the stripped text and maps every
/// offset back through the cuts, so a click still lands where it should.
pub fn strip_comments(markdown: &str) -> (String, Vec<(usize, usize)>) {
    let cuts = comment_cuts(markdown);
    if cuts.is_empty() {
        return (markdown.to_string(), Vec::new());
    }
    let mut out = String::with_capacity(markdown.len());
    let mut map = Vec::with_capacity(cuts.len());
    let mut at = 0;
    for c in cuts {
        out.push_str(&markdown[at..c.start]);
        map.push((out.len(), c.end - c.start));
        at = c.end;
    }
    out.push_str(&markdown[at..]);
    (out, map)
}

/// The label of an embedded note link, brackets hidden. A label typed after
/// a pipe is kept as typed; a bare `Note#Heading` shows as `Note › Heading`,
/// the `#` standing for the three characters so every column still maps.
fn embed_label(b: &mut Builder, w: &Wikilink, style: Style) {
    for k in w.start..w.label_start {
        b.sub("", style, k);
    }
    let aliased = w.label_start != w.start + 2;
    let mut seen_hash = false;
    for k in w.label_start..w.label_end {
        if !aliased && !seen_hash && b.src[k] == '#' {
            seen_hash = true;
            b.sub(" › ", style, k);
        } else {
            b.keep(k, style);
        }
    }
    for k in w.label_end..w.end {
        b.sub("", style, k);
    }
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
                // an embedded note's `!` is part of the span too
                let start = if i > 0 && src[i - 1] == '!' {
                    i - 1
                } else {
                    w.start
                };
                if (start..w.end).contains(&col) {
                    return Some(LinkTarget::wiki(w.full_target()));
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
                                .then(|| LinkTarget::from_href(url.trim()));
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

/// A `[text](href)` on a source line, as char columns: the whole span, for a
/// click or an excerpt, and the href inside the parens, for a rewrite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MdLink {
    pub start: usize,
    pub end: usize,
    pub href_start: usize,
    pub href_end: usize,
}

impl MdLink {
    pub fn href(&self, src: &[char]) -> String {
        src[self.href_start..self.href_end].iter().collect()
    }
}

/// Every `[text](href)` on one source line, left to right, by the same rule
/// [`link_at`] follows: code spans are stepped over, a `[[wikilink]]` is not
/// one, and an image (`![alt](src)`) is not one either.
pub fn md_links(line: &str) -> Vec<MdLink> {
    let src: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < src.len() {
        if let Some(next) = skip_inert(&src, i) {
            i = next;
            continue;
        }
        if src[i] == '[' && src.get(i + 1) == Some(&'[') {
            if let Some(w) = wikilink_at(&src, i) {
                i = w.end;
                continue;
            }
        }
        if src[i] == '[' && (i == 0 || src[i - 1] != '!') {
            if let Some(close) = find(&src, i + 1, ']') {
                if src.get(close + 1) == Some(&'(') {
                    if let Some(paren) = find(&src, close + 2, ')') {
                        out.push(MdLink {
                            start: i,
                            end: paren + 1,
                            href_start: close + 2,
                            href_end: paren,
                        });
                        i = paren + 1;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    out
}

/// `%20` and friends turned back into the bytes they stand for; anything that
/// is not a well-formed escape is kept as typed.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Spaces as `%20`, so a rewritten href stays inside its parens as far as
/// CommonMark is concerned. Nothing else is touched: the rest of what was
/// typed was already a working href.
pub fn percent_encode_spaces(s: &str) -> String {
    s.replace(' ', "%20")
}

/// Does `href` start with a URL scheme — `https:`, `mailto:`, `file:`?
fn has_scheme(href: &str) -> bool {
    let Some(colon) = href.find(':') else {
        return false;
    };
    let scheme = &href[..colon];
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// The note a `[text](href)` names, when it names one: a href with no scheme
/// whose path — percent-decoded, before any `#fragment` — ends in `.md`. The
/// result is a wikilink target (`folder/name.md`, or `name.md#Heading`) for
/// the resolver, with any leading `./` and `../` dropped: the resolver
/// matches a path by its `/`-boundary suffixes, so the folder that is left is
/// what places the note. Anything else — a URL, a `.pdf`, a bare word — is
/// `None`, and stays a href for the desktop.
pub fn note_href(href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() || has_scheme(href) || href.starts_with('/') {
        return None;
    }
    let (path, fragment) = match href.find('#') {
        Some(i) => (&href[..i], Some(&href[i + 1..])),
        None => (href, None),
    };
    let mut path = percent_decode(path);
    loop {
        if let Some(rest) = path.strip_prefix("./") {
            path = rest.to_string();
        } else if let Some(rest) = path.strip_prefix("../") {
            path = rest.to_string();
        } else {
            break;
        }
    }
    if !path.to_lowercase().ends_with(".md") || path.len() == 3 {
        return None;
    }
    Some(match fragment {
        Some(f) => format!("{path}#{}", percent_decode(f)),
        None => path,
    })
}

pub(crate) fn find_pair(src: &[char], from: usize, ch: char) -> Option<usize> {
    (from..src.len().saturating_sub(1)).find(|&k| src[k] == ch && src[k + 1] == ch)
}

/// If column `i` opens a span nothing inline is read from — a closed
/// `` `code` `` span, a backslash escape, a closed `%% comment %%` — the
/// column just past it. The line scanners for tags, wikilinks and footnotes
/// all step over the same set, so none of them finds a thing the others (or
/// the reading view) would not.
fn skip_inert(src: &[char], i: usize) -> Option<usize> {
    match src[i] {
        '`' => find(src, i + 1, '`').map(|end| end + 1),
        '\\' => Some(i + 2),
        '%' if src.get(i + 1) == Some(&'%') => find_pair(src, i + 2, '%').map(|end| end + 2),
        _ => None,
    }
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

pub fn is_tag_char(c: char) -> bool {
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
        if let Some(next) = skip_inert(&src, i) {
            i = next;
            continue;
        }
        match src[i] {
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
// Front matter
//
// The YAML between the leading `---` fences, read by hand and only as far as
// notes actually write it: `key: value`, `key: [a, b]`, and `key:` over a
// block of `  - item` lines. There is no YAML crate behind this on purpose —
// front matter is a handful of properties, and a parser that understood
// anchors and multi-line scalars would be most of a dependency for none of
// the notes anyone keeps. The reading view draws these as a properties box,
// and the index reads `tags` and `aliases` out of them.
// ---------------------------------------------------------------------------

/// One top-level key of a note's front matter and the values under it. A
/// scalar is one value; an inline `[a, b]` or a block list is several.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Property {
    pub key: String,
    pub values: Vec<String>,
    /// The file line the key sits on, so a click on the drawn row can land
    /// the cursor on it.
    pub line: usize,
}

/// Every top-level property of the front matter that opens `content`, in
/// file order; empty when the note has none. An indented line belongs to the
/// key above it (a list item, or a nested map drawn as its raw `k: v` text)
/// and a comment line is nobody's.
pub fn front_matter_properties(content: &str) -> Vec<Property> {
    let lines: Vec<&str> = content.lines().collect();
    let Some(end) = crate::notes::front_matter_end(lines.iter().copied()) else {
        return Vec::new();
    };
    let mut out: Vec<Property> = Vec::new();
    let mut i = 1;
    while i < end {
        let line = lines[i];
        let key_line = i;
        i += 1;
        if line.starts_with([' ', '\t', '-', '#']) {
            continue;
        }
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let rest = rest.trim();
        let mut values = Vec::new();
        if let Some(inner) = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            values.extend(inner.split(',').map(unquote).filter(|v| !v.is_empty()));
        } else if !rest.is_empty() {
            let v = unquote(rest);
            if !v.is_empty() {
                values.push(v);
            }
        } else {
            // a block: `- item` rows, indented or not, until the next key
            while i < end {
                let l = lines[i];
                match l.trim_start().strip_prefix('-') {
                    Some(item) => {
                        let v = unquote(item);
                        if !v.is_empty() {
                            values.push(v);
                        }
                    }
                    None if l.starts_with([' ', '\t']) => {
                        let v = l.trim();
                        if !v.is_empty() && !v.starts_with('#') {
                            values.push(v.to_string());
                        }
                    }
                    None => break,
                }
                i += 1;
            }
        }
        out.push(Property {
            key: key.to_string(),
            values,
            line: key_line,
        });
    }
    out
}

/// The values under `key` in the front matter that opens `content` — every
/// item of a list, or the one scalar — with quotes shed; empty for a note
/// without the key.
pub fn front_matter_values(content: &str, key: &str) -> Vec<String> {
    front_matter_properties(content)
        .into_iter()
        .find(|p| p.key == key)
        .map(|p| p.values)
        .unwrap_or_default()
}

/// The other names a note answers to: its front matter `aliases`, each in the
/// form [`link_key`] compares on. `aliases: a, b` written without brackets is
/// read as the list it was meant to be. The singular `alias:` counts too.
pub fn front_matter_aliases(content: &str) -> Vec<String> {
    ["aliases", "alias"]
        .iter()
        .flat_map(|key| front_matter_values(content, key))
        .collect::<Vec<_>>()
        .iter()
        .flat_map(|v| v.split(','))
        .map(link_key)
        .filter(|a| !a.is_empty())
        .collect()
}

/// A YAML scalar as a person wrote it: trimmed, and the quotes taken off.
fn unquote(text: &str) -> String {
    let t = text.trim();
    let t = t
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| t.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(t);
    t.trim().to_string()
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
    /// The place inside the note, if the link named one: the `Heading` of
    /// `[[note#Heading]]` or the `^id` of `[[note#^id]]`, trimmed. Empty
    /// `target` with a fragment is `[[#Heading]]`, a place in this note.
    pub fragment: Option<String>,
    pub label_start: usize,
    pub label_end: usize,
}

impl Wikilink {
    /// The target with its fragment put back — `note#Heading` — which is how
    /// a link travels to the follower, so it can open the note and then find
    /// the place. `link_key` and the resolver drop the fragment themselves.
    pub fn full_target(&self) -> String {
        match &self.fragment {
            Some(f) => format!("{}#{f}", self.target),
            None => self.target.clone(),
        }
    }

    /// The source column of the `#` shown in the label, when the label is the
    /// target itself rather than an alias: that is the column drawn as ` › `.
    pub fn shown_hash(&self, src: &[char]) -> Option<usize> {
        if self.fragment.is_none() || self.label_start != self.start + 2 {
            return None;
        }
        (self.label_start..self.label_end).find(|&k| src[k] == '#')
    }
}

/// A link target split at its first `#`: the note's name and the place inside
/// it, both trimmed, the fragment `None` when there is none or it is blank.
/// `[[#Heading]]` gives an empty name: a place in the note you are reading.
pub fn split_fragment(target: &str) -> (&str, Option<&str>) {
    match target.split_once('#') {
        Some((name, frag)) => {
            let frag = frag.trim();
            (name.trim(), (!frag.is_empty()).then_some(frag))
        }
        None => (target.trim(), None),
    }
}

/// The source column where a trailing ` ^blockid` begins — the space before
/// the caret, or column 0 for a line that is nothing but `^blockid` — and the
/// id itself. Obsidian's block reference: letters, digits and dashes after a
/// caret, ending the line. Anything else, `None`.
pub fn block_id_at(line: &str) -> Option<(usize, String)> {
    let chars: Vec<char> = line.chars().collect();
    let mut end = chars.len();
    while end > 0 && chars[end - 1].is_whitespace() {
        end -= 1;
    }
    let mut k = end;
    while k > 0 && (chars[k - 1].is_ascii_alphanumeric() || chars[k - 1] == '-') {
        k -= 1;
    }
    if k == end || k == 0 || chars[k - 1] != '^' {
        return None;
    }
    let caret = k - 1;
    let id: String = chars[k..end].iter().collect();
    if caret == 0 {
        return Some((0, id));
    }
    (chars[caret - 1] == ' ').then(|| (caret - 1, id))
}

/// The one rule set for what counts as a wikilink at source column `i`, called
/// from styling, from `link_at` and from the full-page renderer, so all three
/// answer the same way.
pub fn wikilink_at(src: &[char], i: usize) -> Option<Wikilink> {
    if src.get(i) != Some(&'[') || src.get(i + 1) != Some(&'[') {
        return None;
    }
    // `\[[x]]` is someone showing the syntax rather than using it, so it
    // never becomes a link; `![[x]]` is an Obsidian embed, and is judged
    // below once the target is known — a picture is not somewhere to go, but
    // an embedded note is still the note it names
    if i > 0 && src[i - 1] == '\\' {
        return None;
    }
    let embedded = i > 0 && src[i - 1] == '!';
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
    let (target, fragment) = split_fragment(&raw);
    // `[[#heading]]` names a place in the note you are already reading — a
    // link with nothing to resolve, but a link; `[[#]]` names nothing at all
    if target.is_empty() && fragment.is_none() {
        return None;
    }
    // `![[picture.png]]` is a picture, drawn by the image path; only an
    // embedded *note* is a link
    if embedded && is_image_path(target) {
        return None;
    }
    Some(Wikilink {
        start: i,
        end: close + 2,
        target: target.to_string(),
        fragment: fragment.map(str::to_string),
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
        if let Some(next) = skip_inert(&src, i) {
            i = next;
            continue;
        }
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

/// An attachment named by a `[[link]]`, on its way to the desktop opener.
pub const FILE_SCHEME: &str = "attachment:";

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
    /// An attachment — `[[report.pdf]]`, `[[board.canvas]]` — by the name it
    /// was linked as. Found the way a picture is and opened by the desktop;
    /// never something a note is made for.
    File(String),
}

impl LinkTarget {
    /// What a `[[target]]` opens: the attachment it names when the name has
    /// an attachment's extension (see [`is_attachment_path`]), the note
    /// otherwise.
    pub fn wiki(target: String) -> LinkTarget {
        if is_attachment_path(split_fragment(&target).0) {
            LinkTarget::File(split_fragment(&target).0.to_string())
        } else {
            LinkTarget::Wiki(target)
        }
    }

    /// What a `[text](href)` written in a note body opens: a note, when the
    /// href is a relative path to a `.md` file (see [`note_href`]), and the
    /// desktop otherwise. Never a [`LinkTarget::Note`] — that names a file by
    /// path with nothing left to check, and a body href gets no such trust.
    pub fn from_href(href: &str) -> LinkTarget {
        match note_href(href) {
            Some(t) => LinkTarget::Wiki(t),
            None => LinkTarget::Url(href.to_string()),
        }
    }

    pub fn href(&self) -> String {
        match self {
            // a URL that reads as one of the app's own schemes is wrapped in
            // `url:` so it comes back out of `parse` as the URL it is, rather
            // than as an instruction to open a file by path
            LinkTarget::Url(u)
                if u.starts_with(NOTE_SCHEME)
                    || u.starts_with(URL_SCHEME)
                    || u.starts_with(TAG_SCHEME)
                    || u.starts_with(FILE_SCHEME) =>
            {
                format!("{URL_SCHEME}{u}")
            }
            LinkTarget::Url(u) => u.clone(),
            LinkTarget::Wiki(t) => format!("{WIKI_SCHEME}{t}"),
            LinkTarget::Note(p) => format!("{NOTE_SCHEME}{p}"),
            LinkTarget::Tag(t) => format!("{TAG_SCHEME}{t}"),
            LinkTarget::File(f) => format!("{FILE_SCHEME}{f}"),
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
        if let Some(f) = href.strip_prefix(FILE_SCHEME) {
            return LinkTarget::File(f.to_string());
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
    let name = split_fragment(target).0;
    // an attachment is found beside the note, not in the index
    if is_attachment_path(name) {
        return if embeds::file(name).is_some() {
            base
        } else {
            base.patch(theme::grey())
        };
    }
    // an empty name is `[[#heading]]`: the note on screen, which exists
    if name.is_empty() || links::resolves(target) {
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
    /// A `$$` … `$$` maths block, fences included — or `$$x$$` on one line.
    Math,
    /// A blockquote opening with `> [!type]`: drawn as a card, the way the
    /// reading view draws it, with the title line as the card's top edge.
    Callout,
    /// The leading `---` … `---` block, fences included. A block rather than a
    /// run of lines so the whole thing reveals together the way a fence does,
    /// and so nothing inside it is ever read as markdown.
    FrontMatter,
    /// A `%%` … `%%` block comment, fences included: drawn quiet and as
    /// typed, and nothing inside it is markdown. An unclosed `%%` is not one.
    Comment,
    /// A line holding nothing but `![[note]]` — an embedded note, drawn as a
    /// card: its title on the source line, and the first lines of its body on
    /// rows hung under it the way a callout hangs its bottom edge.
    Embed,
    /// A setext heading: a line of text with a line of `===` (H1) or `---`
    /// (H2) under it. Two lines, so the underline is never read as a rule.
    Setext,
    /// A run of lines indented four spaces or a tab, set off by a blank line
    /// above: code the way CommonMark first spelt it, before fences.
    IndentedCode,
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

/// The last line of the maths block opening on line `i`, if one does: a
/// line starting with `$$`, closed by the first line ending with `$$` —
/// which may be the same line (`$$x$$`). An unclosed opener is not a block.
fn math_block_end(lines: &[String], i: usize) -> Option<usize> {
    let t = lines[i].trim();
    if !t.starts_with("$$") {
        return None;
    }
    if t.len() > 4 && t.ends_with("$$") {
        return Some(i);
    }
    (i + 1..lines.len()).find(|j| lines[*j].trim().ends_with("$$"))
}

/// Does `line` open a callout: `> [!type]`, with any indent?
fn is_callout_start(line: &str) -> bool {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while chars.get(i).is_some_and(|c| c.is_whitespace()) {
        i += 1;
    }
    if chars.get(i) != Some(&'>') {
        return false;
    }
    i += 1;
    while chars.get(i).is_some_and(|c| c.is_whitespace()) {
        i += 1;
    }
    callout_title(&chars, i).is_some()
}

/// One card in a callout block: the callout itself, at depth 1, and every
/// callout nested inside it — `> > [!tip] Inner` — one level deeper for
/// each `>` in front of its title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub start: usize,
    pub end: usize,
    /// How many `>` open the card's lines: 1 for the block's own callout.
    pub depth: usize,
    pub kind: String,
    /// The fold marker after `[!kind]`: `-` starts folded, `+` open.
    pub marker: Option<char>,
}

impl Card {
    pub fn contains(&self, row: usize) -> bool {
        row >= self.start && row <= self.end
    }

    /// Is there anything under the title to fold away?
    pub fn has_body(&self) -> bool {
        self.end > self.start
    }
}

/// How many `>` open `chars`, each after optional whitespace.
fn quote_depth(chars: &[char]) -> usize {
    let mut i = 0;
    let mut n = 0;
    loop {
        while chars.get(i).is_some_and(|c| c.is_whitespace()) {
            i += 1;
        }
        if chars.get(i) != Some(&'>') {
            return n;
        }
        n += 1;
        i += 1;
    }
}

/// The char index of the `k`-th `>` (from 1) of a quote line — where the
/// decoration standing for it maps a click back to — or the line's end.
fn quote_marker_col(chars: &[char], k: usize) -> usize {
    let mut i = 0;
    let mut n = 0;
    loop {
        while chars.get(i).is_some_and(|c| c.is_whitespace()) {
            i += 1;
        }
        if chars.get(i) != Some(&'>') {
            return i.min(chars.len());
        }
        n += 1;
        if n == k {
            return i;
        }
        i += 1;
    }
}

/// Where a quote line's own text starts once `n` of its `>` are stripped:
/// past the markers and one space after the last.
fn quote_body_start_n(chars: &[char], n: usize) -> usize {
    let mut i = 0;
    for _ in 0..n {
        while chars.get(i).is_some_and(|c| c.is_whitespace()) {
            i += 1;
        }
        if chars.get(i) != Some(&'>') {
            break;
        }
        i += 1;
    }
    if chars.get(i) == Some(&' ') {
        i += 1;
    }
    i
}

/// Every card in a callout block, in order of their title lines: the block's
/// own callout first, then each one nested in it. A nested card opens where
/// a deeper run of `>` begins with `[!kind]` — the same place the reading
/// view's parser opens a new blockquote — and runs as long as that depth does.
pub fn callout_cards(lines: &[String], block: &Block) -> Vec<Card> {
    let mut cards: Vec<Card> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    let mut prev_depth = 0;
    let last = block.end.min(lines.len().saturating_sub(1));
    for (row, line) in lines.iter().enumerate().take(last + 1).skip(block.start) {
        let chars: Vec<char> = line.chars().collect();
        let q = quote_depth(&chars).max(1);
        while open.last().is_some_and(|&c| cards[c].depth > q) {
            let c = open.pop().expect("an open card");
            cards[c].end = row - 1;
        }
        if prev_depth < q {
            if let Some((kind, after, _)) = callout_title(&chars, quote_body_start_n(&chars, q)) {
                let marker = chars.get(after).copied().filter(|c| matches!(c, '-' | '+'));
                open.push(cards.len());
                cards.push(Card {
                    start: row,
                    end: block.end,
                    depth: q,
                    kind,
                    marker,
                });
            }
        }
        prev_depth = q;
    }
    cards
}

/// The card whose title is on `row`, when the line opens one.
pub fn callout_card_at(lines: &[String], blocks: &[Block], row: usize) -> Option<Card> {
    let block = block_at(blocks, row).filter(|b| b.kind == BlockKind::Callout)?;
    callout_cards(lines, block)
        .into_iter()
        .find(|c| c.start == row)
}

/// The width a callout card is drawn at on a page `width` wide. A width no
/// page has means the caller did not care, so the card takes a comfortable
/// default.
fn card_page_width(width: usize) -> usize {
    if width == usize::MAX {
        60
    } else {
        width.max(8)
    }
}

/// The decoration to the left of a row inside a callout block, for the
/// levels before `depth` — a `│ ` for each enclosing card, a `▌ ` for a
/// plain quote between them — with the right edges those cards close on and
/// how wide the row's own level may be.
struct Frame {
    cells: Vec<Cell>,
    /// Outermost first: the style of each card's edge and the display
    /// column its ` │` ends at.
    closes: Vec<(Style, usize)>,
    avail: usize,
}

fn card_frame(cards: &[Card], chars: &[char], row: usize, depth: usize, w: usize) -> Frame {
    let mut cells = Vec::new();
    let mut closes = Vec::new();
    let mut avail = w;
    let mut x = 0;
    for k in 1..depth {
        let col = quote_marker_col(chars, k);
        match cards.iter().find(|c| c.depth == k && c.contains(row)) {
            Some(c) => {
                let style = theme::callout(&c.kind);
                cells.extend(at("│ ", style, col));
                closes.push((style, x + avail));
                avail = avail.saturating_sub(4);
            }
            None => {
                cells.extend(at(&format!("{} ", theme::QUOTE_BAR), theme::marker(), col));
                avail = avail.saturating_sub(2);
            }
        }
        x += 2;
    }
    Frame {
        cells,
        closes,
        avail,
    }
}

/// Close the enclosing cards' right edges, innermost first, each padded out
/// to where its card ends; a row already past an edge leaves it and the
/// ones outside it off.
fn close_edges(cells: &mut Vec<Cell>, closes: &[(Style, usize)], src: usize) {
    for (style, end) in closes.iter().rev() {
        let used = cells_width(cells);
        if used + 2 > *end {
            return;
        }
        cells.extend(at(&" ".repeat(end - used - 2), theme::PLAIN, src));
        cells.extend(at(" │", *style, src));
    }
}

/// One line of a callout card, as wide as `width`. The title line is the
/// card's top edge — glyph, type and title in the callout's colour — and
/// every other line sits between two rails. On the cursor's line
/// (`raw`) the text is shown as typed, title syntax included, so it can be
/// edited; the card stays. A callout nested in the card is a card of its
/// own, drawn inside the outer one's rails.
pub fn callout_line(lines: &[String], block: &Block, row: usize, width: usize, raw: bool) -> RLine {
    callout_line_folded(lines, block, row, width, raw, None)
}

/// The same line with its fold shown: `hidden` is how many lines the fold
/// on this title hides, `None` for an open one. A folded title carries `▸ `
/// before its glyph and the count at its right; an open title that can fold
/// (`[!kind]-` or `[!kind]+`) carries `▾ `.
pub fn callout_line_folded(
    lines: &[String],
    block: &Block,
    row: usize,
    width: usize,
    raw: bool,
    hidden: Option<usize>,
) -> RLine {
    let src = lines.get(row).map(String::as_str).unwrap_or("");
    let chars: Vec<char> = src.chars().collect();
    let src_len = chars.len();
    let cards = callout_cards(lines, block);
    let w = card_page_width(width);
    let q = quote_depth(&chars).max(1);
    let body = quote_body_start_n(&chars, q);
    let title = cards.iter().find(|c| c.depth == q && c.start == row);
    // a body row sits inside every level up to its own; a title row opens
    // its level, so its frame stops one short
    let Frame {
        mut cells,
        closes,
        avail,
    } = card_frame(&cards, &chars, row, q + usize::from(title.is_none()), w);
    let Some(card) = title else {
        let text: String = chars[body..].iter().collect();
        let inner = if raw {
            RLine::raw(&text).cells
        } else {
            style_line_from(&text, first_footnote(lines, row, &text)).cells
        };
        cells.extend(inner.into_iter().map(|c| Cell {
            src: body + c.src,
            ..c
        }));
        close_edges(&mut cells, &closes, src_len);
        return done(cells, src);
    };
    let style = theme::callout(&card.kind);
    let col = quote_marker_col(&chars, q);
    let x = cells_width(&cells);
    cells.extend(at("╭─ ", style, col));
    match hidden {
        Some(_) => cells.extend(at(theme::FOLDED, theme::fold(), col)),
        None if card.marker.is_some() => cells.extend(at(theme::UNFOLDED, style, col)),
        None => {}
    }
    if raw {
        for (k, ch) in chars.iter().enumerate().skip(body) {
            cells.push(Cell {
                ch: *ch,
                style: theme::PLAIN,
                src: k,
            });
        }
        cells.push(Cell {
            ch: ' ',
            style,
            src: src_len,
        });
    } else if let Some((kind, after, title)) = callout_title(&chars, body) {
        let bold = style.add_modifier(Modifier::BOLD);
        let mut head = String::new();
        if let Some(g) = callout_glyph(&kind) {
            head.push(g);
            head.push(' ');
        }
        head.push_str(&kind);
        cells.extend(at(&head, style, body));
        if title < src_len {
            cells.extend(at(" · ", style, after));
            let t: String = chars[title..].iter().collect();
            cells.extend(styled_inline(&t, bold).into_iter().map(|c| Cell {
                src: title + c.src,
                ..c
            }));
        }
        cells.push(Cell {
            ch: ' ',
            style,
            src: src_len,
        });
    }
    let used = cells_width(&cells) - x;
    let mut dashes = avail.saturating_sub(used + 1);
    // the count sits in the top edge, with a dash or more before it; a
    // title that leaves it no room keeps its text, and the marker in front
    // already says the card is folded
    let tail = hidden.map(fold_count).filter(|l| dashes > str_width(l) + 3);
    if let Some(l) = &tail {
        dashes -= str_width(l) + 3;
    }
    cells.extend(at(&"─".repeat(dashes), style, src_len));
    if let Some(l) = &tail {
        cells.extend(at(" ", style, src_len));
        cells.extend(at(l, theme::marker(), src_len));
        cells.extend(at(" ─", style, src_len));
    }
    cells.extend(at("╮", style, src_len));
    close_edges(&mut cells, &closes, src_len);
    done(cells, src)
}

/// What a folded callout says about what it hides.
pub fn fold_count(hidden: usize) -> String {
    match hidden {
        1 => "1 line".to_string(),
        n => format!("{n} lines"),
    }
}

/// The bottom edges drawn under `row` of a callout block: one for every
/// card whose last line on screen it is — a card that closes there, or one
/// folded down to its title — innermost first, each inside the rails of
/// the cards around it. `hidden` says which lines a fold has taken off the
/// screen.
pub fn callout_closes(
    lines: &[String],
    block: &Block,
    row: usize,
    width: usize,
    hidden: &dyn Fn(usize) -> bool,
) -> Vec<RLine> {
    let cards = callout_cards(lines, block);
    let w = card_page_width(width);
    let mut out = Vec::new();
    for card in cards.iter().rev() {
        let last = (card.start..=card.end).rev().find(|&l| !hidden(l));
        if last != Some(row) {
            continue;
        }
        let chars: Vec<char> = lines[card.start].chars().collect();
        let Frame {
            mut cells,
            closes,
            avail,
        } = card_frame(&cards, &chars, card.start, card.depth, w);
        let style = theme::callout(&card.kind);
        cells.extend(at(
            &format!("╰{}╯", "─".repeat(avail.saturating_sub(2))),
            style,
            0,
        ));
        close_edges(&mut cells, &closes, 0);
        out.push(RLine { cells, src_len: 0 });
    }
    out
}

/// The bottom edge of a callout card `width` wide.
#[cfg(test)]
pub fn callout_close(kind: &str, width: usize) -> RLine {
    let w = card_page_width(width);
    let cells = format!("╰{}╯", "─".repeat(w - 2))
        .chars()
        .map(|ch| Cell {
            ch,
            style: theme::callout(kind),
            src: 0,
        })
        .collect();
    RLine { cells, src_len: 0 }
}

/// A footnote label as a superscript: digits in superscript, anything else
/// as `^name`.
pub fn superscript(label: &str) -> String {
    if !label.is_empty() && label.chars().all(|c| c.is_ascii_digit()) {
        label.chars().filter_map(sup_char).collect()
    } else {
        format!("^{label}")
    }
}

/// The superscript form of `c`, for the characters Unicode has one for.
pub fn sup_char(c: char) -> Option<char> {
    Some(match c {
        '0' => '⁰',
        '1' => '¹',
        '2' => '²',
        '3' => '³',
        '4' => '⁴',
        '5' => '⁵',
        '6' => '⁶',
        '7' => '⁷',
        '8' => '⁸',
        '9' => '⁹',
        '+' => '⁺',
        '-' => '⁻',
        '=' => '⁼',
        '(' => '⁽',
        ')' => '⁾',
        _ => return None,
    })
}

/// The subscript form of `c`, for the characters Unicode has one for.
pub fn sub_char(c: char) -> Option<char> {
    Some(match c {
        '0' => '₀',
        '1' => '₁',
        '2' => '₂',
        '3' => '₃',
        '4' => '₄',
        '5' => '₅',
        '6' => '₆',
        '7' => '₇',
        '8' => '₈',
        '9' => '₉',
        '+' => '₊',
        '-' => '₋',
        '=' => '₌',
        '(' => '₍',
        ')' => '₎',
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Inline HTML
//
// Obsidian lets a note fall back to HTML for the few things markdown has no
// spelling for — a key cap, a subscript, an underline, a hard break. Only the
// tag grammar lives here; what each tag looks like is decided by the two
// views, which share `html_style` for the pairs they both know.

/// One HTML tag as written: `<name attrs>`, `</name>` or `<name/>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HtmlTag {
    /// The tag name, lower-cased.
    pub name: String,
    /// `</name>`.
    pub closing: bool,
    /// `<name/>`.
    pub self_closing: bool,
    /// One past the closing `>`, in chars from the start of the source.
    pub end: usize,
}

impl HtmlTag {
    /// Does this tag open a span that a later `</name>` closes?
    pub fn opens(&self) -> bool {
        !self.closing && !self.self_closing
    }
}

/// The HTML tag starting at column `i`, or `None` when the `<` there is just
/// a less-than sign: a tag name has to follow it (after an optional `/`),
/// letters first, and the tag has to close with `>` on this line.
pub(crate) fn html_tag_at(src: &[char], i: usize) -> Option<HtmlTag> {
    if src.get(i) != Some(&'<') {
        return None;
    }
    let mut k = i + 1;
    let closing = src.get(k) == Some(&'/');
    if closing {
        k += 1;
    }
    if !src.get(k).is_some_and(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let name_start = k;
    while src
        .get(k)
        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '-')
    {
        k += 1;
    }
    let name: String = src[name_start..k].iter().collect::<String>().to_lowercase();
    // what follows the name: `>`, `/>`, or whitespace and then attributes
    let mut self_closing = false;
    match src.get(k) {
        Some('>') => {}
        Some('/') if src.get(k + 1) == Some(&'>') => {
            self_closing = true;
            k += 1;
        }
        Some(c) if c.is_whitespace() && !closing => {
            let mut quote: Option<char> = None;
            loop {
                let c = *src.get(k)?;
                match quote {
                    Some(q) if c == q => quote = None,
                    Some(_) => {}
                    None if c == '"' || c == '\'' => quote = Some(c),
                    None if c == '>' => break,
                    None if c == '/' && src.get(k + 1) == Some(&'>') => {
                        self_closing = true;
                        k += 1;
                        break;
                    }
                    None if c == '<' => return None,
                    None => {}
                }
                k += 1;
            }
        }
        Some(c) if c.is_whitespace() => {
            while src.get(k).is_some_and(|c| c.is_whitespace()) {
                k += 1;
            }
            if src.get(k) != Some(&'>') {
                return None;
            }
        }
        _ => return None,
    }
    Some(HtmlTag {
        name,
        closing,
        self_closing,
        end: k + 1,
    })
}

/// One past the `-->` of the comment opening at column `i`, when `<!--`
/// starts there and the comment closes on this line.
pub(crate) fn html_comment_end(src: &[char], i: usize) -> Option<usize> {
    if !starts_with(src, i, "<!--") {
        return None;
    }
    let mut k = i + 4;
    while k + 3 <= src.len() {
        if starts_with(src, k, "-->") {
            return Some(k + 3);
        }
        k += 1;
    }
    None
}

/// Does a `-->` start at column `k`? For a comment opened on an earlier line.
pub(crate) fn html_comment_close_at(src: &[char], k: usize) -> bool {
    starts_with(src, k, "-->")
}

fn starts_with(src: &[char], i: usize, s: &str) -> bool {
    s.chars()
        .enumerate()
        .all(|(n, c)| src.get(i + n) == Some(&c))
}

/// Where `</name>` next appears from column `from`: the column of its `<`
/// and one past its `>`.
pub(crate) fn html_close_at(src: &[char], from: usize, name: &str) -> Option<(usize, usize)> {
    (from..src.len()).find_map(|k| {
        html_tag_at(src, k)
            .filter(|t| t.closing && t.name == name)
            .map(|t| (k, t.end))
    })
}

/// The style the body of a `<name>…</name>` pair is drawn in, for the tags
/// both views know how to draw. Sub- and superscripts change their glyphs,
/// not their style, so they come back plain.
pub fn html_style(name: &str, base: Style) -> Option<Style> {
    Some(match name {
        "kbd" => base.patch(theme::inline_code()),
        "u" => base.add_modifier(Modifier::UNDERLINED),
        "mark" => base.patch(theme::highlight()),
        "sub" | "sup" => base,
        _ => return None,
    })
}

/// One footnote reference on a line: `[^label]` or an inline `^[text]`, as
/// the char columns `start..end` of the whole span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FootnoteRef {
    pub start: usize,
    pub end: usize,
    /// `^[text]` rather than `[^label]`; `body` is then the text's columns.
    pub inline: bool,
    pub body: (usize, usize),
}

/// Every footnote reference on one line, in source order — the things that
/// take a number. A `[^1]: definition` at the head of the line is not one,
/// and nothing inside a code span is.
pub fn footnote_refs(line: &str) -> Vec<FootnoteRef> {
    let src: Vec<char> = line.chars().collect();
    let head = src.iter().position(|c| !c.is_whitespace()).unwrap_or(0);
    let mut out = Vec::new();
    let mut i = 0;
    while i < src.len() {
        // an escaped `\[^1]`, a code span or a `%% comment %%` is text, not
        // a footnote
        if let Some(next) = skip_inert(&src, i) {
            i = next;
            continue;
        }
        let c = src[i];
        if c == '[' && src.get(i + 1) == Some(&'^') {
            if let Some(close) = find(&src, i + 2, ']') {
                let label = &src[i + 2..close];
                let definition = i == head && src.get(close + 1) == Some(&':');
                if !label.is_empty() && !label.contains(&' ') && !definition {
                    out.push(FootnoteRef {
                        start: i,
                        end: close + 1,
                        inline: false,
                        body: (i + 2, close),
                    });
                    i = close + 1;
                    continue;
                }
            }
        }
        if c == '^' && src.get(i + 1) == Some(&'[') {
            if let Some(close) = find(&src, i + 2, ']') {
                if close > i + 2 {
                    out.push(FootnoteRef {
                        start: i,
                        end: close + 1,
                        inline: true,
                        body: (i + 2, close),
                    });
                    i = close + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// The number the first footnote reference on each line would get: one more
/// than the references on the lines above it, with fenced code and `%%`
/// block comments left out. One entry per line, plus a last one for the
/// count after the final line. Both the editor and the reading view number
/// from this, so a footnote is never two different numbers.
pub fn footnote_counts<S: AsRef<str>>(lines: &[S]) -> Vec<usize> {
    let mut out = Vec::with_capacity(lines.len() + 1);
    let mut n = 1;
    let mut fenced = false;
    let mut commented = false;
    for (i, line) in lines.iter().enumerate() {
        let line = line.as_ref();
        out.push(n);
        if is_fence(line) {
            fenced = !fenced;
            continue;
        }
        // a `%%` block comment, like the reading view leaves it out —
        // an unclosed one is text, so only a `%%` with a partner opens one
        if !fenced
            && line.trim() == "%%"
            && (commented || lines[i + 1..].iter().any(|l| l.as_ref().trim() == "%%"))
        {
            commented = !commented;
            continue;
        }
        if !fenced && !commented && (line.contains("[^") || line.contains("^[")) {
            n += footnote_refs(line).len();
        }
    }
    out.push(n);
    out
}

/// The number the first footnote reference on line `row` gets: one more than
/// the references on the lines above it, fenced code left out.
pub fn footnote_ordinal(lines: &[String], row: usize) -> usize {
    let counts = footnote_counts(lines);
    counts[row.min(counts.len() - 1)]
}

/// `footnote_ordinal`, but only paid for when `src` — the text about to be
/// styled — has an inline footnote to number. A `[^1]` shows its own label,
/// so most lines never need the count.
fn first_footnote(lines: &[String], row: usize, src: &str) -> usize {
    if src.contains("^[") {
        footnote_ordinal(lines, row)
    } else {
        1
    }
}

/// `style_line` for line `row` of a note, with its inline footnotes numbered
/// in document order rather than from 1.
pub fn style_line_in(lines: &[String], row: usize) -> RLine {
    let src = lines.get(row).map(String::as_str).unwrap_or("");
    if src.contains("^[") {
        style_line_from(src, footnote_ordinal(lines, row))
    } else {
        style_line(src)
    }
}

/// The glyph a callout type is drawn with in its title row.
pub fn callout_glyph(kind: &str) -> Option<char> {
    match kind {
        "summary" | "abstract" | "tldr" => Some('≡'),
        "note" | "info" | "todo" => Some('i'),
        "tip" | "hint" | "success" | "check" | "done" => Some('✓'),
        "warning" | "caution" | "attention" => Some('!'),
        "danger" | "error" | "bug" | "failure" | "fail" | "missing" => Some('✗'),
        "question" | "help" | "faq" => Some('?'),
        "example" => Some('▸'),
        "quote" | "cite" => Some('❝'),
        _ => None,
    }
}

/// `[!type] Title` at the start of `rest` (a quote line's body, bars
/// stripped): the type, the title, and the char index the title starts at.
pub(crate) fn callout_title(chars: &[char], i: usize) -> Option<(String, usize, usize)> {
    if chars.get(i) != Some(&'[') || chars.get(i + 1) != Some(&'!') {
        return None;
    }
    let close = (i + 2..chars.len()).find(|k| chars[*k] == ']')?;
    let kind: String = chars[i + 2..close].iter().collect();
    if kind.is_empty() || kind.chars().any(char::is_whitespace) {
        return None;
    }
    let mut t = close + 1;
    if matches!(chars.get(t), Some('-') | Some('+')) {
        t += 1;
    }
    while chars.get(t).is_some_and(|c| c.is_whitespace()) {
        t += 1;
    }
    Some((kind.to_lowercase(), close + 1, t))
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

/// The level of the setext heading whose text is on line `i`: 1 under a
/// line of three or more `=`, 2 under one of three or more `-`. Only a line
/// of plain text takes an underline — a `---` under a list item, a heading,
/// a quote or a blank line is a rule, as before.
pub fn setext_level(lines: &[String], i: usize) -> Option<usize> {
    let text = lines.get(i)?;
    let under = lines.get(i + 1)?.trim();
    if !is_setext_text(text) || under.chars().count() < 3 {
        return None;
    }
    if under.chars().all(|c| c == '=') {
        Some(1)
    } else if under.chars().all(|c| c == '-') {
        Some(2)
    } else {
        None
    }
}

/// Could `line` be the text of a setext heading: a paragraph line, and not
/// the start of some other construct?
fn is_setext_text(line: &str) -> bool {
    let t = line.trim_start();
    if t.is_empty() || is_indented(line) || is_fence(line) || is_rule(line) || is_table_row(line) {
        return false;
    }
    if t.starts_with(['#', '>', '$']) {
        return false;
    }
    let chars: Vec<char> = t.chars().collect();
    !is_list_item(&chars)
}

/// `- `, `* `, `+ `, `1. ` or `1) ` at the start of `chars`.
pub(crate) fn is_list_item(chars: &[char]) -> bool {
    if list_marker(chars, 0, 1).is_some() {
        return true;
    }
    let digits = chars.iter().take_while(|c| c.is_ascii_digit()).count();
    digits > 0
        && matches!(chars.get(digits), Some('.') | Some(')'))
        && matches!(chars.get(digits + 1), Some(' ') | None)
}

/// Four spaces or a tab at the start of the line.
fn is_indented(line: &str) -> bool {
    line.starts_with("    ") || line.starts_with('\t')
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

/// Does an indented code block open on line `i`? The line is indented and
/// not blank, and sits at the top of the scan or under a blank line — one
/// that does not itself follow a list item or an indented continuation, so a
/// paragraph inside a list keeps being a paragraph.
fn opens_indented_code(lines: &[String], from: usize, i: usize) -> bool {
    if !is_indented(&lines[i]) || is_blank(&lines[i]) {
        return false;
    }
    if i == from {
        return true;
    }
    if !is_blank(&lines[i - 1]) {
        return false;
    }
    match i
        .checked_sub(2)
        .filter(|&k| k >= from)
        .map(|k| lines[k].as_str())
    {
        Some(prev) => {
            let chars: Vec<char> = prev.trim_start().chars().collect();
            !(is_list_item(&chars) || prev.starts_with([' ', '\t']))
        }
        None => true,
    }
}

/// The last line of the indented code block opening on line `i`: it runs
/// through blank lines as long as the next non-blank line is still indented,
/// and never ends on a blank one.
fn indented_code_end(lines: &[String], i: usize) -> usize {
    let mut end = i;
    let mut j = i + 1;
    while j < lines.len() {
        if is_blank(&lines[j]) {
            j += 1;
            continue;
        }
        if !is_indented(&lines[j]) {
            break;
        }
        end = j;
        j += 1;
    }
    end
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
pub fn image_line(line: &str) -> Option<(String, String, Option<u32>)> {
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
    Some((alt.to_string(), url.to_string(), None))
}

/// An Obsidian embed alone on a line: `![[picture.png]]` or
/// `![[picture.png|alt]]`, split into (alt, url, width). Only pictures: an
/// embed of another note (`![[plan]]`) is not one, and stays the text it was
/// typed as. Obsidian reads a bare number after the pipe as a width in
/// pixels (`|300`, or `|300x200` for a width and a height): that is the width
/// the picture is drawn at, never alt text.
pub fn embed_line(line: &str) -> Option<(String, String, Option<u32>)> {
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
    match embed_width(alt) {
        Some(w) => Some((String::new(), url.to_string(), Some(w))),
        None => Some((alt.to_string(), url.to_string(), None)),
    }
}

/// Obsidian's size after the pipe: `300` or `300x200`, as the width in
/// pixels. Anything else is alt text.
fn embed_width(alt: &str) -> Option<u32> {
    let (w, h) = match alt.split_once('x') {
        Some((w, h)) => (w, Some(h)),
        None => (alt, None),
    };
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    if !digits(w) || h.is_some_and(|h| !digits(h)) {
        return None;
    }
    w.parse().ok().filter(|&w| w > 0)
}

/// An attachment embedded alone on a line, `![[report.pdf]]` or
/// `![[report.pdf|label]]`: a file the app cannot draw (see
/// [`is_attachment_path`]; a picture is [`embed_line`]'s), as (name, label).
pub fn attachment_embed_line(line: &str) -> Option<(String, Option<String>)> {
    let t = line.trim();
    let body = t.strip_prefix("![[")?.strip_suffix("]]")?;
    if body.contains('[') || body.contains(']') || body.contains('\n') {
        return None;
    }
    let (name, label) = match body.split_once('|') {
        Some((u, l)) => (u.trim(), Some(l.trim()).filter(|l| !l.is_empty())),
        None => (body.trim(), None),
    };
    if name.is_empty() || is_image_path(name) || !is_attachment_path(name) {
        return None;
    }
    Some((name.to_string(), label.map(str::to_string)))
}

/// An Obsidian note embed: what `![[Note#Heading|label]]` names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteEmbed {
    /// The note, trimmed, without its `#heading`.
    pub target: String,
    /// The heading inside it, when one was named.
    pub heading: Option<String>,
    /// The text after the pipe, when there was one.
    pub label: Option<String>,
}

/// `![[Note]]`, `![[Note#Heading]]` or `![[Note|label]]` alone on a line,
/// where the target is a note rather than a picture — the picture case is
/// [`embed_line`]'s. Brackets inside the body mean the pair was never a pair.
pub fn note_embed_line(line: &str) -> Option<NoteEmbed> {
    let t = line.trim();
    let body = t.strip_prefix("![[")?.strip_suffix("]]")?;
    if body.contains('[') || body.contains(']') || body.contains('\n') {
        return None;
    }
    let (target, label) = match body.split_once('|') {
        Some((u, l)) => (u.trim(), Some(l.trim()).filter(|l| !l.is_empty())),
        None => (body.trim(), None),
    };
    let (note, heading) = match target.split_once('#') {
        Some((n, h)) => (n.trim(), Some(h.trim()).filter(|h| !h.is_empty())),
        None => (target, None),
    };
    if note.is_empty() || is_attachment_path(note) {
        return None;
    }
    Some(NoteEmbed {
        target: note.to_string(),
        heading: heading.map(str::to_string),
        label: label.map(str::to_string),
    })
}

/// Whether a path names a picture the preview could draw, by extension.
pub fn is_image_path(path: &str) -> bool {
    matches!(
        extension_of(path).as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff")
    )
}

/// Whether a path names an attachment rather than a note: a picture, or a
/// file Obsidian keeps in a vault and opens outside it — a PDF, a recording,
/// a video, a canvas. A `[[link]]` to one is handed to the desktop, and
/// following it never makes a `.md`.
pub fn is_attachment_path(path: &str) -> bool {
    is_image_path(path)
        || matches!(
            extension_of(path).as_deref(),
            Some("pdf" | "mp3" | "m4a" | "wav" | "mp4" | "mov" | "canvas")
        )
}

/// The lower-cased extension of `path`, when it has one.
fn extension_of(path: &str) -> Option<String> {
    std::path::Path::new(path.trim())
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
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
        // indented code first: a fence, a rule or a table row indented four
        // spaces under a blank line is a code sample of one, not the thing
        if opens_indented_code(lines, from, i) {
            let end = indented_code_end(lines, i);
            out.push(Block {
                kind: BlockKind::IndentedCode,
                start: i,
                end,
            });
            i = end + 1;
            continue;
        }
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
        // a block comment swallows everything to its closing `%%` too; one
        // that never closes is literal text and falls through
        if is_comment_fence(&lines[i]) {
            if let Some(end) = (i + 1..lines.len()).find(|j| is_comment_fence(&lines[*j])) {
                out.push(Block {
                    kind: BlockKind::Comment,
                    start: i,
                    end,
                });
                i = end + 1;
                continue;
            }
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
        if is_callout_start(&lines[i]) {
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim_start().starts_with('>') {
                j += 1;
            }
            out.push(Block {
                kind: BlockKind::Callout,
                start: i,
                end: j - 1,
            });
            i = j;
            continue;
        }
        if let Some(end) = math_block_end(lines, i) {
            out.push(Block {
                kind: BlockKind::Math,
                start: i,
                end,
            });
            i = end + 1;
            continue;
        }
        // a setext heading takes its underline with it, so the `---` under
        // a line of text is never a rule
        if setext_level(lines, i).is_some() {
            out.push(Block {
                kind: BlockKind::Setext,
                start: i,
                end: i + 1,
            });
            i += 2;
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
        } else if links::enabled()
            && (note_embed_line(&lines[i]).is_some() || attachment_embed_line(&lines[i]).is_some())
        {
            out.push(Block {
                kind: BlockKind::Embed,
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
        BlockKind::Fence => code_fence_line(&lines[block.start..=block.end], row - block.start),
        BlockKind::Mermaid => {
            mermaid_line(&lines[block.start..=block.end], row - block.start, width)
        }
        BlockKind::Rule => rule_line(src, width),
        BlockKind::Image => image_fallback_line(src),
        BlockKind::FrontMatter => front_matter_line(src),
        // the same bargain as front matter: as typed, only quiet
        BlockKind::Comment => front_matter_line(src),
        BlockKind::Math => math_line(src, row == block.start, row == block.end, width),
        BlockKind::Callout => callout_line(lines, block, row, width, false),
        BlockKind::Table => table_line(&lines[block.start..=block.end], row - block.start, width),
        BlockKind::Embed => embed_title_line(src, width),
        BlockKind::Setext => {
            let level = setext_level(lines, block.start).unwrap_or(2);
            setext_line(src, level, row == block.end)
        }
        BlockKind::IndentedCode => fence_line(src, false),
    }
}

/// One line of a setext heading: the text in the heading's colour with its
/// inline spans styled, the way an ATX heading is drawn; the underline kept
/// as typed, only quiet.
fn setext_line(src: &str, level: usize, underline: bool) -> RLine {
    let chars: Vec<char> = src.chars().collect();
    let src_len = chars.len();
    let mut b = Builder {
        src: &chars,
        cells: Vec::with_capacity(src_len),
        note: 1,
    };
    if underline {
        for k in 0..src_len {
            b.keep(k, theme::marker());
        }
    } else {
        let base = theme::heading(level);
        let mut i = 0;
        while i < src_len && (chars[i] == ' ' || chars[i] == '\t') {
            b.keep(i, base);
            i += 1;
        }
        inline(&mut b, i, base);
    }
    RLine {
        cells: b.cells,
        src_len,
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

/// One line of a ``` fence, `i` counted from the fence's opening line: a row
/// of the band the reading view draws, gutter and indent rules and all. A body
/// line takes its words' colours when the fence names a language syntect knows
/// and `code_colors` is on.
///
/// The whole fence is highlighted at once even though the editor draws a line
/// at a time — a comment or a string opened three lines up decides what this
/// one is — and the answer is remembered by language and body, so walking the
/// rows of a fence parses it once and an edit inside it simply misses.
fn code_fence_line(rows: &[String], i: usize) -> RLine {
    let src = rows.get(i).map(String::as_str).unwrap_or("");
    let cap = i == 0 || i + 1 == rows.len();
    let mut line = if cap {
        fence_cap_line(src)
    } else {
        let mut line = match fence_runs(rows).as_ref().and_then(|r| r.get(i - 1)) {
            Some(runs) => code_line(src, runs),
            None => fence_line(src, false),
        };
        line.cells = code_guides(line.cells);
        line
    };
    // the ``` rows are the band's top and bottom — an editor row is a file
    // row, so there is nowhere to put a padding row of its own — and they
    // carry a blank gutter so the left edge lines up with the code above it
    let number = (!cap).then_some(i);
    line.cells
        .splice(0..0, band_gutter(gutter_width(rows), number));
    line
}

/// How wide a fence's gutter is: the digits its last line number needs, plus
/// the two columns of air between the numbers and the code. Zero when
/// `code_numbers` is off, which leaves the band and nothing else.
fn gutter_width(rows: &[String]) -> usize {
    if !crate::highlight::numbers() {
        return 0;
    }
    // the caps are the fence, not the code
    let body = rows.len().saturating_sub(2).max(1);
    body.to_string().len() + 2
}

/// The band's left edge for one row: two columns of side padding, then the
/// gutter with `number` right-aligned in it — blank on a cap and on a wrapped
/// continuation row, both of which are rows the source never numbered.
///
/// None of it is the source's, so every cell is [`PAD`]: a click lands on the
/// code beside it, the cursor steps over it, and a selection leaves it be.
fn band_gutter(width: usize, number: Option<usize>) -> Vec<Cell> {
    let mut cells = at("  ", theme::code(), PAD);
    if width > 0 {
        let label = number.map(|n| n.to_string()).unwrap_or_default();
        cells.extend(at(
            &format!("{label:>w$}  ", w = width - 2),
            theme::code_gutter(),
            PAD,
        ));
    }
    cells
}

/// A code row's leading whitespace ruled every fourth column, so depth is
/// countable at a glance instead of measurable with a finger — the page's
/// indent guides, cell for cell. A tab counts four columns wide but stays one
/// cell, because the editor's mapping rests on one cell per source character.
///
/// Only the leading run: a rule struck through the middle of a string would be
/// reading the text, not ruling it.
fn code_guides(mut cells: Vec<Cell>) -> Vec<Cell> {
    if !crate::highlight::numbers() {
        return cells;
    }
    let mut col: usize = 0;
    for c in cells.iter_mut() {
        let width = match c.ch {
            '\t' => 4,
            ' ' => 1,
            _ => break,
        };
        if c.ch == ' ' && col.is_multiple_of(4) {
            c.ch = theme::CODE_GUIDE;
            c.style = theme::code_guide();
        }
        col += width;
    }
    cells
}

/// A fence's own ``` row on the band: kept as typed, language word and all,
/// only dim — the band's edge is what says "code" now, so there is nothing
/// left for a hidden marker to say.
fn fence_cap_line(src: &str) -> RLine {
    let cells = src
        .chars()
        .enumerate()
        .map(|(i, ch)| Cell {
            ch,
            style: theme::code_gutter(),
            src: i,
        })
        .collect();
    done(cells, src)
}

/// The highlighter's runs for a whole fence, one `Vec` per body line: `None`
/// when the fence names no language catcher can colour.
fn fence_runs(rows: &[String]) -> Option<std::rc::Rc<Vec<Vec<crate::highlight::Run>>>> {
    let lang = crate::highlight::language(fence_info(rows.first()?))?;
    // the caps are the fence, not the code; a fence left unclosed at the end
    // of the buffer has its last line read as one all the same
    let body: String = rows[1..rows.len().saturating_sub(1)]
        .iter()
        .map(|l| format!("{l}\n"))
        .collect();
    crate::highlight::runs(lang, &body)
}

/// A highlighted code line: one cell per source char, as `fence_line` draws
/// it, with each run in its role's colour. Anything the highlighter did not
/// reach keeps the plain code colour.
fn code_line(src: &str, runs: &[crate::highlight::Run]) -> RLine {
    let mut cells = Vec::with_capacity(src.len());
    let mut at = 0;
    let mut col = 0;
    let mut push = |text: &str, style: Style, col: &mut usize| {
        for ch in text.chars() {
            cells.push(Cell {
                ch,
                style,
                src: *col,
            });
            *col += 1;
        }
    };
    for run in runs {
        let Some(text) = src.get(at..at + run.len) else {
            break;
        };
        push(text, crate::highlight::style(run.role), &mut col);
        at += run.len;
    }
    if let Some(rest) = src.get(at..) {
        push(rest, theme::code(), &mut col);
    }
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

/// A line of a maths block with the cursor elsewhere: the `$$` fences are
/// dropped and what is left is set in the middle of the page, in italics,
/// the way a displayed formula is. A cap line with nothing else on it
/// becomes a blank row, so the block takes the rows the file gives it.
fn math_line(src: &str, first: bool, last: bool, width: usize) -> RLine {
    let chars: Vec<char> = src.chars().collect();
    let mut a = 0;
    let mut b = chars.len();
    while a < b && chars[a].is_whitespace() {
        a += 1;
    }
    while b > a && chars[b - 1].is_whitespace() {
        b -= 1;
    }
    if first && chars[a..b].starts_with(&['$', '$']) {
        a += 2;
    }
    if last && chars[a..b].ends_with(&['$', '$']) {
        b -= 2;
    }
    while a < b && chars[a].is_whitespace() {
        a += 1;
    }
    while b > a && chars[b - 1].is_whitespace() {
        b -= 1;
    }
    let mut cells: Vec<Cell> = Vec::new();
    if a < b {
        let w: usize = chars[a..b].iter().map(|c| char_width(*c)).sum();
        let pad = if width == usize::MAX {
            0
        } else {
            width.saturating_sub(w) / 2
        };
        cells.extend(at(&" ".repeat(pad), theme::PLAIN, a));
        cells.extend(chars[a..b].iter().enumerate().map(|(k, ch)| Cell {
            ch: *ch,
            style: theme::math(),
            src: a + k,
        }));
    }
    done(cells, src)
}

/// What an image line shows when the terminal can't draw pictures.
fn image_fallback_line(src: &str) -> RLine {
    let len = src.chars().count();
    let (alt, url, _) = image_line(src).unwrap_or_default();
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

// ---------------------------------------------------------------------------
// Note embeds
//
// `![[note]]` on a line of its own pulls another note into this one. Both
// views draw it as a card: the note's title, then the first few lines of its
// body. The file behind it is found the way a wikilink is followed — through
// the vault index — and read from disk, so the card is always what the note
// says now rather than what it said when it was linked.
// ---------------------------------------------------------------------------

/// Which file a `![[note]]` embed reads, and whether the vault has been walked
/// at all yet.
///
/// Process-wide, the sibling of [`links`] and for the same reason: the card is
/// drawn deep inside line styling, where no vault index is to hand. The app
/// installs a resolver after every index walk, so an embed resolves exactly
/// the way following the same link would.
pub mod embeds {
    use std::path::PathBuf;
    use std::sync::RwLock;

    /// A link target → the file it names, by the index's own rules.
    pub type Resolver = Box<dyn Fn(&str) -> Option<PathBuf> + Send + Sync>;

    static RESOLVER: RwLock<Option<Resolver>> = RwLock::new(None);

    /// What resolving a target came to.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Found {
        /// The note, by path.
        Note(PathBuf),
        /// The vault has no such note.
        Missing,
        /// Nothing has been walked yet, so nobody can say.
        Unknown,
    }

    /// Install the resolver. Called after each index walk, never per frame.
    pub fn set_resolver(r: Resolver) {
        if let Ok(mut w) = RESOLVER.write() {
            *w = Some(r);
        }
    }

    /// The file `target` names.
    pub fn resolve(target: &str) -> Found {
        match RESOLVER.read() {
            Ok(r) => match &*r {
                Some(f) => f(target).map_or(Found::Missing, Found::Note),
                None => Found::Unknown,
            },
            Err(_) => Found::Unknown,
        }
    }

    /// An attachment name → the file beside the note, in its attachments
    /// folder or the configured one: the app installs this for the note on
    /// screen, since it is the note that says where "beside" is.
    static FILES: RwLock<Option<Resolver>> = RwLock::new(None);

    /// Install the attachment resolver. Called when the note on screen
    /// changes, never per frame.
    pub fn set_file_resolver(r: Resolver) {
        if let Ok(mut w) = FILES.write() {
            *w = Some(r);
        }
    }

    /// The attachment `name` names, when it is there to be found.
    pub fn file(name: &str) -> Option<PathBuf> {
        FILES.read().ok()?.as_ref()?(name)
    }

    /// Back to "nothing walked yet". Only the tests want this.
    #[cfg(test)]
    pub fn forget() {
        if let Ok(mut w) = RESOLVER.write() {
            *w = None;
        }
    }

    /// The resolver is process-wide and `cargo test` runs in parallel, so the
    /// tests that install one — in this module and in the renderer's — take
    /// turns here.
    #[cfg(test)]
    pub fn turn() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A resolver over one folder: `name` is `<dir>/<name>.md` when that file
    /// exists. Enough of the index's rules for a test vault.
    #[cfg(test)]
    pub fn install_dir(dir: &std::path::Path) {
        let dir = dir.to_path_buf();
        set_resolver(Box::new(move |target| {
            let path = dir.join(format!("{}.md", super::link_key(target)));
            path.exists().then_some(path)
        }));
    }
}

/// The most of an embedded note that is read: a card wants its first lines,
/// not a megabyte of someone's log.
const EMBED_READ_CAP: u64 = 64 * 1024;

/// What a `![[note]]` card shows, worked out once and drawn by either view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbedCard {
    /// The note's title — or the target as typed, when there is no note.
    pub title: String,
    /// The heading the embed points into, when it names one.
    pub heading: Option<String>,
    /// The embedded lines: the whole body, or the whole section the embed
    /// names — as they stand in the file, blanks and indents kept, so either
    /// view can lay them out as markdown.
    pub lines: Vec<String>,
    /// Whether the note was found at all.
    pub found: embeds::Found,
}

impl EmbedCard {
    /// The title row's text: `Title`, or `Title › Heading`.
    pub fn head(&self) -> String {
        match &self.heading {
            Some(h) => format!("{} › {h}", self.title),
            None => self.title.clone(),
        }
    }
}

/// The card for `embed`: the target resolved, the file read, the section
/// under the heading (or the body) cut to its first lines.
pub fn embed_card(embed: &NoteEmbed) -> EmbedCard {
    let found = embeds::resolve(&embed.target);
    let mut card = EmbedCard {
        title: embed.target.clone(),
        heading: embed.heading.clone(),
        lines: Vec::new(),
        found: found.clone(),
    };
    let embeds::Found::Note(path) = found else {
        return card;
    };
    let Some(content) = read_embed(&path) else {
        card.found = embeds::Found::Missing;
        return card;
    };
    card.title = crate::notes::title_of(&content);
    let body = crate::notes::body_after_front_matter(&content);
    let lines: Vec<&str> = body.lines().collect();
    let section: Vec<&str> = match &embed.heading {
        Some(h) if h.starts_with('^') => match block_under(&lines, h) {
            Some(s) => s,
            None => {
                card.lines.push("(no such block)".to_string());
                return card;
            }
        },
        Some(h) => match section_under(&lines, h) {
            Some(s) => s,
            None => {
                card.lines.push("(no such heading)".to_string());
                return card;
            }
        },
        None => {
            // the title line is already the card's first row
            let first = lines.iter().position(|l| !l.trim().is_empty());
            match first {
                Some(i) if heading_text(lines[i]).is_some_and(|t| t == card.title) => {
                    lines[i + 1..].to_vec()
                }
                _ => lines,
            }
        }
    };
    // the blank lines at either end are the seam with the embed, not content
    let start = section.iter().position(|l| !l.trim().is_empty());
    card.lines = match start {
        None => Vec::new(),
        Some(a) => {
            let b = section
                .iter()
                .rposition(|l| !l.trim().is_empty())
                .unwrap_or(a);
            section[a..=b].iter().map(|l| l.to_string()).collect()
        }
    };
    card
}

/// The line without its trailing ` ^blockid` — an address, not prose, and
/// nothing a card's preview of the line should show.
pub fn strip_block_id(line: &str) -> &str {
    match block_id_at(line) {
        Some((col, _)) => {
            let byte = line.char_indices().nth(col).map_or(line.len(), |(b, _)| b);
            &line[..byte]
        }
        None => line,
    }
}

/// The text of an ATX heading line — up to three spaces of indent and the
/// opening hashes gone, a closing run of hashes dropped when it stands on its
/// own (`# Title #` is `Title`, `# C#` is still `C#`), and a trailing
/// ` ^blockid` dropped too — trimmed. `None` for any other line.
pub fn heading_text(line: &str) -> Option<&str> {
    let level = crate::fold::heading_level(line)?;
    let body = line.trim_start()[level..].trim();
    let closing = body.trim_end_matches('#');
    let body = if closing.len() < body.len()
        && (closing.is_empty() || closing.ends_with(char::is_whitespace))
    {
        closing.trim_end()
    } else {
        body
    };
    Some(strip_block_id(body).trim())
}

/// The lines under the heading called `name` (matched without case), up to
/// the next heading of the same or a higher level. Fenced code is stepped
/// over, as a `# comment` in a shell snippet is not a heading. `None` when
/// no heading of that name exists.
fn section_under<'a>(lines: &[&'a str], name: &str) -> Option<Vec<&'a str>> {
    let want = name.trim().to_lowercase();
    let mut fenced = false;
    let at = lines.iter().position(|l| {
        if is_fence(l) {
            fenced = !fenced;
            return false;
        }
        !fenced && heading_text(l).is_some_and(|t| t.to_lowercase() == want)
    })?;
    let level = crate::fold::heading_level(lines[at]).unwrap_or(usize::MAX);
    let mut fenced = false;
    let end = (at + 1..lines.len())
        .find(|&i| {
            if is_fence(lines[i]) {
                fenced = !fenced;
                return false;
            }
            !fenced && crate::fold::heading_level(lines[i]).is_some_and(|l| l <= level)
        })
        .unwrap_or(lines.len());
    Some(lines[at + 1..end].to_vec())
}

/// The block a `^id` names: the list item carrying the id, or the paragraph
/// — the run of non-blank lines around the line that carries it, a heading
/// or fence bounding it. `None` when no line ends in ` ^id`.
fn block_under<'a>(lines: &[&'a str], fragment: &str) -> Option<Vec<&'a str>> {
    let owned: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    let at = crate::links::find_anchor(&owned, fragment)?;
    if list_item_indent(lines[at]).is_some() {
        return Some(vec![lines[at]]);
    }
    let bounds = |l: &str| {
        l.trim().is_empty()
            || crate::fold::heading_level(l).is_some()
            || is_fence(l)
            || list_item_indent(l).is_some()
    };
    let start = (0..at)
        .rev()
        .find(|&i| bounds(lines[i]))
        .map_or(0, |i| i + 1);
    let end = (at + 1..lines.len())
        .find(|&i| bounds(lines[i]))
        .unwrap_or(lines.len());
    Some(lines[start..end].to_vec())
}

/// The indent of a list item line — `- `, `* `, `+ `, `1. ` — or `None`.
fn list_item_indent(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    let bullet = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
        .is_some();
    let numbered = trimmed
        .find(". ")
        .is_some_and(|i| i > 0 && trimmed[..i].chars().all(|c| c.is_ascii_digit()));
    (bullet || numbered).then_some(indent)
}

/// A remembered read: the file's stamp and size, and what it held.
type EmbedMemo = (Option<std::time::SystemTime>, u64, std::rc::Rc<String>);

thread_local! {
    /// The embedded notes read so far, by path.
    static EMBED_MEMO: std::cell::RefCell<std::collections::HashMap<std::path::PathBuf, EmbedMemo>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// The first [`EMBED_READ_CAP`] bytes of `path`, re-read only when the file's
/// stamp or size has changed since — the card is asked for several times a
/// frame, and a stat is the most that should cost.
fn read_embed(path: &std::path::Path) -> Option<std::rc::Rc<String>> {
    let meta = std::fs::metadata(path).ok()?;
    let stamp = (meta.modified().ok(), meta.len());
    EMBED_MEMO.with(|memo| {
        let mut memo = memo.borrow_mut();
        if let Some((m, len, text)) = memo.get(path) {
            if (*m, *len) == stamp {
                return Some(text.clone());
            }
        }
        use std::io::Read;
        let mut buf = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .take(EMBED_READ_CAP)
            .read_to_end(&mut buf)
            .ok()?;
        let text = std::rc::Rc::new(String::from_utf8_lossy(&buf).into_owned());
        memo.insert(path.to_path_buf(), (stamp.0, stamp.1, text.clone()));
        Some(text)
    })
}

/// The colour an embed card's rail and title are drawn in.
pub fn embed_style() -> Style {
    theme::callout("note")
}

/// The card's rail: a quote bar and a space.
fn embed_rail(style: Style, src: usize) -> Vec<Cell> {
    let mut cells = at(theme::QUOTE_BAR, style, src);
    cells.extend(at(" ", style, src));
    cells
}

/// The source line of a `![[note]]` embed in the editor: the card's title
/// row. Columns walk along the source so a click anywhere on it lands on the
/// line, which reveals the syntax.
fn embed_title_line(src: &str, width: usize) -> RLine {
    let len = src.chars().count();
    let style = embed_style();
    let (text, text_style) = if let Some((name, label)) = attachment_embed_line(src) {
        match attachment_card(&name, label.as_deref()) {
            (text, true) => (text, style.add_modifier(Modifier::BOLD)),
            (text, false) => (text, theme::grey()),
        }
    } else if let Some(embed) = note_embed_line(src) {
        let card = embed_card(&embed);
        match card.found {
            embeds::Found::Missing => (format!("{} (no such note)", card.head()), theme::grey()),
            _ => (card.head(), style.add_modifier(Modifier::BOLD)),
        }
    } else {
        return RLine::raw(src);
    };
    let mut cells = embed_rail(style, 0);
    let room = if width == usize::MAX {
        usize::MAX
    } else {
        width.saturating_sub(2).max(1)
    };
    let text = if room == usize::MAX {
        text
    } else {
        truncate(&text, room)
    };
    cells.extend(text.chars().enumerate().map(|(i, ch)| Cell {
        ch,
        style: text_style,
        src: (i + 2).min(len),
    }));
    done(cells, src)
}

/// The one row a `![[report.pdf]]` embed draws, in either view: the label
/// (or the name), the file's size, and whether the file was found — `📎
/// report.pdf (1.2 MB)`, or `📎 report.pdf (no such file)`.
pub fn attachment_card(name: &str, label: Option<&str>) -> (String, bool) {
    let shown = label.unwrap_or(name);
    match embeds::file(name).and_then(|p| std::fs::metadata(p).ok()) {
        Some(meta) => (format!("📎 {shown} ({})", human_size(meta.len())), true),
        None => (format!("📎 {shown} (no such file)"), false),
    }
}

/// `812 B`, `1.2 KB`, `3.4 MB`, `1.0 GB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64 / 1024.0;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

/// The rows hung under a `![[note]]` line in the editor: the embedded note
/// itself, styled the way the editor styles its own lines and wrapped behind
/// the card's rail. Nothing at all while the vault is still being walked or
/// the note does not exist — the title row has already said so.
pub fn embed_rows(src: &str, width: usize) -> Vec<RLine> {
    let Some(embed) = note_embed_line(src) else {
        return Vec::new();
    };
    let card = embed_card(&embed);
    if !matches!(card.found, embeds::Found::Note(_)) {
        return Vec::new();
    }
    let style = embed_style();
    let room = if width == usize::MAX {
        usize::MAX
    } else {
        width.saturating_sub(2).max(1)
    };
    // an embedded note is not this note: nothing in it is the buffer's, so
    // no cell answers to a source column here
    let blank = |line: RLine| -> Vec<RLine> {
        let cells: Vec<Cell> = line
            .cells
            .into_iter()
            .map(|c| Cell { src: 0, ..c })
            .collect();
        let chars: Vec<char> = cells.iter().map(|c| c.ch).collect();
        wrap_breaks(&chars, room, room)
            .into_iter()
            .map(|(a, b)| {
                let mut row = embed_rail(style, 0);
                row.extend(cells[a..b].iter().cloned());
                RLine {
                    cells: row,
                    src_len: 0,
                }
            })
            .collect()
    };
    let lines = card.lines.clone();
    let blocks = blocks(&lines);
    (0..lines.len())
        .flat_map(|row| {
            let line = match block_at(&blocks, row) {
                Some(b) => style_block_line(&lines, b, row, room),
                None => style_line(&lines[row]),
            };
            blank(line)
        })
        .collect()
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

pub(crate) fn align_of(spec: &str) -> Align {
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
        note: 1,
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
pub fn table_line_editing(
    lines: &[String],
    block: &Block,
    row: usize,
    width: usize,
    raw_row: usize,
) -> RLine {
    let rows = &lines[block.start..=block.end];
    let r = row - block.start;
    table_row(
        &layout_memo(rows, width, Some(raw_row)),
        rows,
        r,
        r == raw_row,
    )
}

/// The rule drawn between two body rows of a table in the editor, to the
/// same column widths as its rows. `raw_row` is the cursor's row, as for
/// [`table_line_editing`], so the layout is the one the rows use.
pub fn table_rule_editing(
    lines: &[String],
    block: &Block,
    width: usize,
    raw_row: Option<usize>,
) -> RLine {
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
pub fn table_column_spans(
    lines: &[String],
    block: &Block,
    width: usize,
    raw_row: Option<usize>,
) -> Vec<(usize, usize)> {
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
pub fn tint_table_cells(
    line: &mut RLine,
    src: &str,
    selected: &dyn Fn(usize) -> bool,
    style: Style,
) {
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
                .map(|(i, ch)| Cell {
                    ch,
                    style: body,
                    src: i,
                })
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
    #[test]
    fn str_width_counts_joined_clusters_once() {
        assert_eq!(
            super::str_width("\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}"),
            2
        );
        assert_eq!(super::str_width("\u{1F1FA}\u{1F1F8}"), 2);
        assert_eq!(super::str_width("a\u{1F1FA}\u{1F1F8}b"), 4);
        // Combining marks keep their own column.
        assert_eq!(super::str_width("e\u{301}"), 2);
        assert_eq!(
            super::joined_cluster_at("a\u{1F1FA}\u{1F1F8}b", 2),
            Some((1, 3))
        );
        assert_eq!(super::joined_cluster_at("a\u{1F1FA}\u{1F1F8}b", 3), None);
    }

    #[test]
    fn heading_text_strips_hashes_block_id_and_closing_hashes() {
        assert_eq!(super::heading_text("## Setup ^abc ##"), Some("Setup"));
        assert_eq!(super::heading_text("   # Indented"), Some("Indented"));
        assert_eq!(super::heading_text("# C#"), Some("C#"));
        assert_eq!(super::heading_text("#"), None);
        assert_eq!(super::heading_text("plain"), None);
    }

    #[test]
    fn section_under_skips_fenced_code_and_matches_block_id_heading() {
        let lines = vec![
            "```sh",
            "# Setup",
            "echo",
            "```",
            "## Setup ^abc ##",
            "body",
            "## Next",
        ];
        assert_eq!(super::section_under(&lines, "setup"), Some(vec!["body"]));
    }

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
    fn bullet_glyph_follows_nesting_depth() {
        assert_eq!(text(&style_line("- one")), "• one");
        assert_eq!(text(&style_line("  - two")), "  ◦ two");
        assert_eq!(text(&style_line("    * three")), "    ▪ three");
        // the cycle starts over at the fourth level
        assert_eq!(text(&style_line("      + four")), "      • four");
        // a tab is one level, and mixes with spaces
        assert_eq!(text(&style_line("\t- two")), "\t◦ two");
        assert_eq!(text(&style_line("\t  - three")), "\t  ▪ three");
        // an odd space does not count
        assert_eq!(text(&style_line("   - two")), "   ◦ two");
        // the cursor still maps through the substituted glyph
        let deep = style_line("  - two");
        assert_eq!(deep.one_row().display_to_source(4), 4);
    }

    #[test]
    fn nested_checkboxes_keep_their_boxes() {
        assert_eq!(text(&style_line("  - [ ] later")), "  ☐ later");
        assert_eq!(text(&style_line("    - [x] done")), "    ✓ done");
        assert_eq!(task_prefix("  - [ ] later"), Some((2, 8)));
    }

    #[test]
    fn nested_bullets_in_a_quote_count_only_their_own_indent() {
        // the quote bar's own space is not list indentation
        assert_eq!(text(&style_line("> - one")), "▌ • one");
        assert_eq!(text(&style_line(">   - two")), "▌   ◦ two");
    }

    #[test]
    fn inline_markers_are_hidden() {
        let l = style_line("a **b** c *d* `e` ==f== ~~g~~");
        assert_eq!(text(&l), "a b c d e f g");
        assert_eq!(l.one_row().display_to_source(2), 4); // "b"
    }

    #[test]
    fn html_tags_stay_dimmed_and_a_kbd_body_reads_as_code() {
        let l = style_line("Press <kbd>Ctrl</kbd> now");
        assert_eq!(text(&l), "Press <kbd>Ctrl</kbd> now");
        // the tags are kept, dimmed
        assert_eq!(l.cells[6].style, theme::marker()); // <
        assert_eq!(l.cells[10].style, theme::marker()); // >
        assert_eq!(l.cells[15].style, theme::marker()); // < of </kbd>
                                                        // the body is inline code, and the prose around it is plain
        assert_eq!(l.cells[11].style.fg, theme::inline_code().fg); // C
        assert_eq!(l.cells[22].style, theme::PLAIN); // n
                                                     // every column is its own cell, so the cursor maps straight through
        for col in 0..l.cells.len() {
            assert_eq!(l.one_row().display_to_source(col), col);
        }
    }

    #[test]
    fn sub_and_sup_bodies_take_their_unicode_forms() {
        let l = style_line("H<sub>2</sub>O and x<sup>(n-1)</sup>");
        assert_eq!(text(&l), "H<sub>₂</sub>O and x<sup>⁽n⁻¹⁾</sup>");
        // the raised digit still stands for its own source column
        assert_eq!(l.one_row().display_to_source(6), 6);
        let two = l.cells.iter().find(|c| c.ch == '₂').unwrap();
        assert_eq!(two.src, 6);
        assert_eq!(two.style, theme::PLAIN);
    }

    #[test]
    fn u_underlines_and_mark_highlights_in_the_editor() {
        let l = style_line("<u>under</u> <mark>lit</mark>");
        assert_eq!(text(&l), "<u>under</u> <mark>lit</mark>");
        let u = l.cells.iter().find(|c| c.ch == 'n').unwrap();
        assert!(u.style.add_modifier.contains(Modifier::UNDERLINED));
        let m = l.cells.iter().find(|c| c.ch == 'l').unwrap();
        assert_eq!(m.style.bg, theme::highlight().bg);
        // the tags themselves carry neither style
        assert_eq!(l.cells[0].style, theme::marker());
        assert_eq!(l.cells[13].style, theme::marker());
    }

    #[test]
    fn br_is_a_return_glyph_and_a_comment_is_dimmed() {
        let l = style_line("a<br>b <!-- hush --> c<br/>d");
        assert_eq!(text(&l), "a↵b <!-- hush --> c↵d");
        assert_eq!(l.one_row().display_to_source(1), 1); // ↵ is the <
        assert_eq!(l.one_row().display_to_source(2), 5); // b
        let hush: Vec<&Cell> = l
            .cells
            .iter()
            .filter(|c| "<!-hush>".contains(c.ch))
            .collect();
        assert!(hush.iter().all(|c| c.style == theme::marker()));
        let d = l.cells.last().unwrap();
        assert_eq!(d.ch, 'd');
        assert_eq!(d.src, 27);
    }

    #[test]
    fn an_unknown_tag_is_dimmed_and_an_unpaired_one_styles_nothing() {
        let l = style_line("<span class=\"x\">y</span> <u>lonely");
        assert_eq!(text(&l), "<span class=\"x\">y</span> <u>lonely");
        assert_eq!(l.cells[0].style, theme::marker());
        assert_eq!(l.cells[15].style, theme::marker()); // the closing >
        assert_eq!(l.cells[16].style, theme::PLAIN); // y
        let e = l.cells.iter().rev().find(|c| c.ch == 'e').unwrap();
        assert!(!e.style.add_modifier.contains(Modifier::UNDERLINED));
        assert_eq!(l.cells[25].style, theme::marker()); // < of <u>
    }

    #[test]
    fn a_less_than_sign_is_not_a_tag() {
        for src in ["a < b and 1<2", "x <3 y", "a <- b", "1 <= 2 <> 3"] {
            let l = style_line(src);
            assert_eq!(text(&l), src);
            assert!(l.cells.iter().all(|c| c.style == theme::PLAIN), "{src}");
        }
    }

    #[test]
    fn html_tag_at_reads_the_grammar() {
        let t = |s: &str| html_tag_at(&s.chars().collect::<Vec<_>>(), 0);
        let kbd = t("<kbd>x").unwrap();
        assert_eq!((kbd.name.as_str(), kbd.closing, kbd.end), ("kbd", false, 5));
        let close = t("</KBD >").unwrap();
        assert_eq!(
            (close.name.as_str(), close.closing, close.end),
            ("kbd", true, 7)
        );
        let br = t("<br/>").unwrap();
        assert!(br.self_closing && br.name == "br" && br.end == 5);
        let attr = t("<a href=\"a>b\" title='c'>t</a>").unwrap();
        assert_eq!((attr.name.as_str(), attr.end), ("a", 24));
        assert!(t("<3").is_none());
        assert!(t("< b>").is_none());
        assert!(t("<b").is_none());
        assert!(t("<a <b>").is_none());
        assert!(t("</a b>").is_none());
        let chars: Vec<char> = "x <!-- a -- b --> y".chars().collect();
        assert_eq!(html_comment_end(&chars, 2), Some(17));
        assert_eq!(html_comment_end(&chars, 0), None);
        let open: Vec<char> = "<!-- never".chars().collect();
        assert_eq!(html_comment_end(&open, 0), None);
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
    fn numbered_tasks_draw_a_box_after_their_number() {
        let l = style_line("1. [ ] hi");
        assert_eq!(text(&l), format!("1. {} hi", theme::UNCHECKED));
        assert_eq!(l.src_len, 9);
        // the number is source, the box stands for the `[`, the text follows
        let row = l.one_row();
        assert_eq!(row.display_to_source(0), 0);
        assert_eq!(row.display_to_source(3), 3);
        assert_eq!(row.display_to_source(5), 7);
        let l = style_line("12) [x] hi");
        assert_eq!(text(&l), format!("12) {} hi", theme::CHECKED));
        let h = l.cells.iter().find(|c| c.ch == 'h').unwrap();
        assert!(h.style.add_modifier.contains(Modifier::CROSSED_OUT));
        // no box without the space after it, none for an unknown state
        assert_eq!(text(&style_line("1. [ ]hi")), "1. [ ]hi");
        assert_eq!(text(&style_line("1. [z] hi")), "1. [z] hi");
        // the prefix the click and the cursor line work from starts at the `[`
        assert_eq!(task_prefix("  1. [ ] a"), Some((5, 9)));
        assert_eq!(task_prefix("1. a"), None);
        assert_eq!(
            text(&raw_with_task("1. [x] **hi**", 8)),
            format!("1. {} **hi**", theme::CHECKED)
        );
        assert_eq!(text(&raw_with_task("1. [x] hi", 5)), "1. [x] hi");
        // wrapped rows hang under the text, past both number and box
        let rows = wrap_rline(&style_line("1. [ ] alpha beta gamma delta"), 12);
        assert_eq!(rows[1].indent, 5);
    }

    #[test]
    fn the_other_task_states_get_their_own_glyphs() {
        let l = style_line("- [/] a");
        assert_eq!(text(&l), format!("{} a", theme::IN_PROGRESS));
        assert_eq!(l.cells[0].style.fg, theme::done().fg);
        let l = style_line("- [-] a");
        assert_eq!(text(&l), format!("{} a", theme::CANCELLED));
        assert_eq!(l.cells[0].style.fg, theme::marker().fg);
        let a = l.cells.iter().find(|c| c.ch == 'a').unwrap();
        assert!(a.style.add_modifier.contains(Modifier::CROSSED_OUT));
        let l = style_line("- [>] a");
        assert_eq!(text(&l), format!("{} a", theme::FORWARDED));
        assert_eq!(l.cells[0].style.fg, theme::forwarded().fg);
        let l = style_line("- [?] a");
        assert_eq!(text(&l), format!("{} a", theme::QUESTION));
        let a = l.cells.iter().find(|c| c.ch == 'a').unwrap();
        assert!(!a.style.add_modifier.contains(Modifier::CROSSED_OUT));
        // an unknown state is a bullet with literal text
        assert_eq!(
            text(&style_line("- [z] a")),
            format!("{} [z] a", theme::BULLET)
        );
        // numbered items take the same states
        assert_eq!(
            text(&style_line("3. [/] a")),
            format!("3. {} a", theme::IN_PROGRESS)
        );
        assert_eq!(task_prefix("- [>] a"), Some((0, 6)));
        assert_eq!(
            text(&raw_with_task("- [-] a", 7)),
            format!("{} a", theme::CANCELLED)
        );
        // every glyph is one column wide, so the hang stays where it was
        for g in theme::TASK_GLYPHS {
            assert_eq!(str_width(g), 1, "{g}");
        }
        let rows = wrap_rline(&style_line("- [/] alpha beta gamma delta"), 12);
        assert_eq!(rows[1].indent, 2);
        let rows = wrap_rline(&style_line("- [?] alpha beta gamma delta"), 12);
        assert_eq!(rows[1].indent, 2);
        // a sentence that opens with a question mark is not a task
        let rows = wrap_rline(&style_line("? alpha beta gamma delta"), 12);
        assert_eq!(rows[1].indent, 0);
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
        let lines: Vec<String> = "| a | b | c |\n|---|---|---|"
            .lines()
            .map(String::from)
            .collect();
        let block = Block {
            kind: BlockKind::Table,
            start: 0,
            end: 1,
        };
        let mut l = table_line_editing(&lines, &block, 0, 40, 0);
        let tint = Style::new().bg(ratatui::style::Color::Red);
        tint_table_cells(&mut l, &lines[0], &|c| c >= 1, tint);
        let text: String = l.cells.iter().map(|c| c.ch).collect();
        assert_eq!(text, "a │ b │ c");
        let tinted: Vec<bool> = l
            .cells
            .iter()
            .map(|c| c.style.bg == Some(ratatui::style::Color::Red))
            .collect();
        // "a │ " untinted, then "b │ c" tinted, separator included
        assert_eq!(
            tinted,
            vec![false, false, false, false, true, true, true, true, true]
        );
    }

    #[test]
    fn inline_footnotes_are_numbered_in_the_editor() {
        let text = |l: &RLine| l.cells.iter().map(|c| c.ch).collect::<String>();
        let l = style_line("a^[note] b");
        assert_eq!(text(&l), "a¹note b");
        // the number sits on the caret, the brackets are gone, the body keeps
        // its own columns
        let srcs: Vec<usize> = l.cells.iter().map(|c| c.src).collect();
        assert_eq!(srcs, vec![0, 1, 3, 4, 5, 6, 8, 9]);
        assert_eq!(l.cells[1].style, theme::state());
        assert_eq!(l.cells[2].style, theme::marker());
        assert_eq!(l.src_len, 10);
        // references of both kinds share one count along the line
        assert_eq!(
            text(&style_line("[^1] and ^[two] and ^[three]")),
            "¹ and ²two and ³three"
        );
        // a line can start its count wherever the note has got to
        assert_eq!(text(&style_line_from("^[x]", 12)), "¹²x");
        // not a footnote: nothing inside, or never closed
        assert_eq!(text(&style_line("^[] and ^[open")), "^[] and ^[open");
    }

    #[test]
    fn escaped_and_commented_footnotes_do_not_take_a_number() {
        assert!(footnote_refs("\\^[lit] and \\[^1]").is_empty());
        assert!(footnote_refs("%% ^[hidden] %% text").is_empty());
        assert_eq!(footnote_refs("%% ^[a] %% ^[b]").len(), 1);
        let lines: Vec<String> = ["%%", "^[hidden]", "%%", "A^[one]"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(footnote_ordinal(&lines, 3), 1);
        let lines: Vec<String> = ["%%", "^[shown]", "A^[two]"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(footnote_ordinal(&lines, 2), 2);
    }

    #[test]
    fn inert_spans_hide_tags_and_wikilinks() {
        assert!(tags_in("%% #hidden %% text").is_empty());
        assert_eq!(tags_in("%% #a %% #b"), vec![(9, 11)]);
        assert!(tags_in("\\#escaped").is_empty());
        assert!(wikilinks("`[[code]]` text").is_empty());
        assert!(wikilinks("%% [[hidden]] %%").is_empty());
        assert_eq!(wikilinks("`[[a]]` [[b]]").len(), 1);
    }

    #[test]
    fn footnote_counts_number_every_line() {
        let lines = [
            "a^[1]", "```", "^[x]", "```", "%%", "^[y]", "%%", "b[^z]", "c",
        ];
        assert_eq!(footnote_counts(&lines), vec![1, 2, 2, 2, 2, 2, 2, 2, 3, 3]);
    }

    #[test]
    fn footnote_refs_are_counted_across_the_note() {
        let refs = footnote_refs("see[^a] and ^[b] `^[c]`");
        assert_eq!(refs.len(), 2);
        assert!(!refs[0].inline && refs[0].start == 3 && refs[0].end == 7);
        assert!(refs[1].inline && refs[1].start == 12 && refs[1].body == (14, 15));
        // a definition at the head of the line takes no number
        assert!(footnote_refs("[^1]: def").is_empty());
        assert!(footnote_refs("  [^1]: def").is_empty());
        assert_eq!(footnote_refs("x [^1]: y").len(), 1);
        assert!(footnote_refs("[^ spaced]").is_empty());

        let lines: Vec<String> = ["one[^a] two^[b]", "```", "^[in a fence]", "```", "^[c]"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(footnote_ordinal(&lines, 0), 1);
        assert_eq!(footnote_ordinal(&lines, 1), 3);
        assert_eq!(footnote_ordinal(&lines, 4), 3);
        let text = |l: &RLine| l.cells.iter().map(|c| c.ch).collect::<String>();
        assert_eq!(text(&style_line_in(&lines, 4)), "³c");
        assert_eq!(text(&style_line_in(&lines, 0)), "one^a two²b");

        // inside a callout card the count carries on too
        let lines: Vec<String> = ["one^[x]", "> [!note] T", "> body^[y]"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let block = Block {
            kind: BlockKind::Callout,
            start: 1,
            end: 2,
        };
        let row = callout_line(&lines, &block, 2, 40, false);
        assert!(text(&row).contains("body²y"), "{}", text(&row));
    }

    #[test]
    fn callouts_footnotes_and_maths_are_styled_in_the_editor() {
        let text = |l: &RLine| l.cells.iter().map(|c| c.ch).collect::<String>();
        // a callout's title line: glyph, type, title
        assert_eq!(
            text(&style_line("> [!todo] Warning!")),
            "▌ i todo · Warning!"
        );
        assert_eq!(text(&style_line("> [!danger]- ")), "▌ ✗ danger");
        assert_eq!(text(&style_line("> [!custom] T")), "▌ custom · T");
        // a plain quote is untouched
        assert_eq!(
            text(&style_line("> [x] not a callout")),
            "▌ [x] not a callout"
        );
        // footnotes: the reference and the definition
        assert_eq!(text(&style_line("word[^1] more")), "word¹ more");
        assert_eq!(text(&style_line("[^12]: Tickles")), "¹² Tickles");
        assert_eq!(text(&style_line("[^note]: x")), "^note x");
        // inline maths loses its dollars
        assert_eq!(text(&style_line("so $x^2$ and $$y$$.")), "so x^2 and y.");
        assert_eq!(text(&style_line("$5 and $6")), "$5 and $6");
        // a maths block: caps blank, body centred in italics
        let lines: Vec<String> = "$$\nE = mc^2\n$$".lines().map(String::from).collect();
        let blocks = blocks(&lines);
        assert_eq!(blocks[0].kind, BlockKind::Math);
        assert_eq!((blocks[0].start, blocks[0].end), (0, 2));
        assert_eq!(text(&style_block_line(&lines, &blocks[0], 0, 20)), "");
        assert_eq!(
            text(&style_block_line(&lines, &blocks[0], 1, 20)),
            "      E = mc^2"
        );
        let one: Vec<String> = vec!["$$a+b$$".to_string()];
        let b1 = super::blocks(&one);
        assert_eq!((b1[0].start, b1[0].end), (0, 0));
        assert_eq!(text(&style_block_line(&one, &b1[0], 0, 11)), "    a+b");
        // an unclosed opener is not a block
        assert!(super::blocks(&["$$".to_string(), "x".to_string()]).is_empty());
    }

    #[test]
    fn a_callout_is_a_card_in_the_editor_too() {
        let text = |l: &RLine| l.cells.iter().map(|c| c.ch).collect::<String>();
        let lines: Vec<String> = "> [!tip] Go\n> body **b**\nafter"
            .lines()
            .map(String::from)
            .collect();
        let bs = blocks(&lines);
        assert_eq!(bs[0].kind, BlockKind::Callout);
        assert_eq!((bs[0].start, bs[0].end), (0, 1));
        assert_eq!(
            text(&callout_line(&lines, &bs[0], 0, 20, false)),
            "╭─ ✓ tip · Go ─────╮"
        );
        assert_eq!(
            text(&callout_line(&lines, &bs[0], 1, 20, false)),
            "│ body b           │"
        );
        // the cursor's line shows the syntax, inside the card
        assert_eq!(
            text(&callout_line(&lines, &bs[0], 0, 20, true)),
            "╭─ [!tip] Go ──────╮"
        );
        assert_eq!(
            text(&callout_line(&lines, &bs[0], 1, 20, true)),
            "│ body **b**       │"
        );
        assert_eq!(text(&callout_close("tip", 20)), "╰──────────────────╯");
        // a click on the rail lands at the line's start, on the text after `> `
        let l = callout_line(&lines, &bs[0], 1, 20, false);
        assert_eq!(l.one_row().display_to_source(2), 2);
    }

    #[test]
    fn a_callout_inside_a_callout_is_a_card_of_its_own() {
        let text = |l: &RLine| l.cells.iter().map(|c| c.ch).collect::<String>();
        let lines: Vec<String> = "> [!note]+ Outer\n> a\n> > [!tip]- Inner\n> > b\n> c"
            .lines()
            .map(String::from)
            .collect();
        let bs = blocks(&lines);
        assert_eq!((bs[0].start, bs[0].end), (0, 4));
        let cards = callout_cards(&lines, &bs[0]);
        assert_eq!(cards.len(), 2);
        assert_eq!((cards[0].start, cards[0].end, cards[0].depth), (0, 4, 1));
        assert_eq!(
            (cards[0].kind.as_str(), cards[0].marker),
            ("note", Some('+'))
        );
        assert_eq!((cards[1].start, cards[1].end, cards[1].depth), (2, 3, 2));
        assert_eq!(
            (cards[1].kind.as_str(), cards[1].marker),
            ("tip", Some('-'))
        );
        assert_eq!(callout_card_at(&lines, &bs, 2).map(|c| c.depth), Some(2));
        assert!(callout_card_at(&lines, &bs, 1).is_none());
        // the open card that can fold carries ▾ ; the inner card sits inside the outer rails
        assert_eq!(
            text(&callout_line(&lines, &bs[0], 0, 30, false)),
            "╭─ ▾ i note · Outer ─────────╮"
        );
        assert_eq!(
            text(&callout_line(&lines, &bs[0], 2, 30, false)),
            "│ ╭─ ▾ ✓ tip · Inner ──────╮ │"
        );
        assert_eq!(
            text(&callout_line(&lines, &bs[0], 3, 30, false)),
            "│ │ b                      │ │"
        );
        assert_eq!(
            text(&callout_line(&lines, &bs[0], 4, 30, false)),
            "│ c                          │"
        );
        // the inner card closes under its last line, inside the outer rails;
        // the outer closes under the block
        let closes = callout_closes(&lines, &bs[0], 3, 30, &|_| false);
        assert_eq!(closes.len(), 1);
        assert_eq!(text(&closes[0]), "│ ╰────────────────────────╯ │");
        let closes = callout_closes(&lines, &bs[0], 4, 30, &|_| false);
        assert_eq!(closes.len(), 1);
        assert_eq!(text(&closes[0]), "╰────────────────────────────╯");
        assert!(callout_closes(&lines, &bs[0], 2, 30, &|_| false).is_empty());
        // both cards close under the inner one's last line when the outer ends there
        let short: Vec<String> = lines[..4].to_vec();
        let sb = blocks(&short);
        let closes = callout_closes(&short, &sb[0], 3, 30, &|_| false);
        assert_eq!(closes.len(), 2);
        assert!(text(&closes[0]).starts_with("│ ╰"));
        assert!(text(&closes[1]).starts_with('╰'));
        // a click on the inner rail lands on the inner `>`
        let l = callout_line(&lines, &bs[0], 3, 30, false);
        assert_eq!(l.one_row().display_to_source(2), 2);
    }

    #[test]
    fn a_folded_callout_title_carries_the_marker_and_the_count() {
        let text = |l: &RLine| l.cells.iter().map(|c| c.ch).collect::<String>();
        let lines: Vec<String> = "> [!tip]- Go\n> a\n> b".lines().map(String::from).collect();
        let bs = blocks(&lines);
        let l = callout_line_folded(&lines, &bs[0], 0, 30, false, Some(2));
        assert_eq!(text(&l), "╭─ ▸ ✓ tip · Go ─── 2 lines ─╮");
        assert_eq!(l.cells[3].style, theme::fold());
        // open, it says it can fold
        assert_eq!(
            text(&callout_line(&lines, &bs[0], 0, 30, false)),
            "╭─ ▾ ✓ tip · Go ─────────────╮"
        );
        // a title with no room for the count keeps its text
        let tight = callout_line_folded(&lines, &bs[0], 0, 18, false, Some(2));
        assert_eq!(text(&tight), "╭─ ▸ ✓ tip · Go ─╮");
        // the bottom edge sits right under the folded title
        let closes = callout_closes(&lines, &bs[0], 0, 30, &|l| l > 0);
        assert_eq!(closes.len(), 1);
        assert_eq!(text(&closes[0]), "╰────────────────────────────╯");
        assert!(callout_closes(&lines, &bs[0], 2, 30, &|l| l > 0).is_empty());
        // a callout without a marker has no ▾
        let plain: Vec<String> = vec!["> [!tip] Go".to_string()];
        let pb = blocks(&plain);
        assert_eq!(
            text(&callout_line(&plain, &pb[0], 0, 20, false)),
            "╭─ ✓ tip · Go ─────╮"
        );
        assert_eq!(fold_count(1), "1 line");
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
            Some((String::new(), "attachments/hero.jpg".into(), None))
        );
        assert_eq!(
            image_line("  ![[a.png|a cat]]  "),
            Some(("a cat".into(), "a.png".into(), None))
        );
        // a bare number after the pipe is Obsidian's width, not alt text
        assert_eq!(
            image_line("![[a.png|300]]"),
            Some((String::new(), "a.png".into(), Some(300)))
        );
        assert_eq!(
            embed_line("![[a.png|300x200]]"),
            Some((String::new(), "a.png".into(), Some(300)))
        );
        assert_eq!(
            embed_line("![[a.png|300x]]"),
            Some(("300x".into(), "a.png".into(), None))
        );
        assert_eq!(
            embed_line("![[a.png|0]]"),
            Some(("0".into(), "a.png".into(), None))
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
    fn a_note_embed_alone_on_a_line_is_an_embed_block() {
        assert_eq!(
            note_embed_line("  ![[Plan#Goals|the goals]]  "),
            Some(NoteEmbed {
                target: "Plan".into(),
                heading: Some("Goals".into()),
                label: Some("the goals".into()),
            })
        );
        assert_eq!(
            note_embed_line("![[plan]]"),
            Some(NoteEmbed {
                target: "plan".into(),
                heading: None,
                label: None,
            })
        );
        // a picture is a picture, and anything malformed is text
        assert_eq!(note_embed_line("![[a.png]]"), None);
        // a PDF is an attachment, not a note to read the first lines of
        assert_eq!(note_embed_line("![[report.pdf]]"), None);
        assert_eq!(
            attachment_embed_line("![[report.pdf|the report]]"),
            Some(("report.pdf".into(), Some("the report".into())))
        );
        assert_eq!(attachment_embed_line("![[a.png]]"), None);
        assert_eq!(attachment_embed_line("![[plan]]"), None);
        assert!(blocks(&["![[board.canvas]]".to_string()])
            .iter()
            .any(|b| b.kind == BlockKind::Embed));
        assert_eq!(note_embed_line("![[plan]] tail"), None);
        assert_eq!(note_embed_line("![[#Goals]]"), None);
        assert_eq!(note_embed_line("![[a]b]]"), None);
        assert_eq!(note_embed_line("[[plan]]"), None);
        let bs = blocks(&buf("![[plan]]\ntext\n![[a.png]]"));
        assert_eq!(bs[0].kind, BlockKind::Embed);
        assert_eq!((bs[0].start, bs[0].end), (0, 0));
        assert_eq!(bs[1].kind, BlockKind::Image);
    }

    #[test]
    fn an_embedded_note_in_a_sentence_is_drawn_as_a_link() {
        let _turn = colours();
        links::forget();
        // the `!` goes with the brackets, and the heading reads as ` › `
        let l = style_line("see ![[plan#Goals]] now");
        assert_eq!(text(&l), "see plan › Goals now");
        assert_eq!(l.cells[4].style.fg, theme::link().fg);
        // every drawn character still knows its column: the "p" of plan
        assert_eq!(l.one_row().display_to_source(4), 7);
        // a typed label is kept as typed
        assert_eq!(text(&style_line("![[plan#Goals|here]]")), "here");
        // ⌥⏎ on the `!` follows it, like anywhere else on the span
        let wiki = Some(LinkTarget::Wiki("plan#Goals".to_string()));
        assert_eq!(link_at("see ![[plan#Goals]] now", 4), wiki);
        assert_eq!(link_at("see ![[plan#Goals]] now", 18), wiki);
        assert_eq!(link_at("see ![[plan#Goals]] now", 3), None);
        // and the mentions scan counts an embed as the link it is
        assert_eq!(wikilinks("![[plan]]")[0].target, "plan");
        // a picture is still not a link
        assert_eq!(
            text(&style_line("see ![[pic.png]] now")),
            "see ![[pic.png]] now"
        );
        assert_eq!(link_at("see ![[pic.png]] now", 6), None);
    }

    /// A vault of one note, `plan.md`, for the embed cards to read.
    fn embed_vault(name: &str) -> std::path::PathBuf {
        let dir = crate::testutil::tmpdir("md", name);
        crate::testutil::write(
            &dir,
            "plan.md",
            "---\ntags: x\n---\n# Plan\n\nFirst line.\nSecond **line**.\n\n## Goals\n- ship it\n- test it\n\n- doc it\n- more\n- and more\n\n## Later\nNothing.\n",
        );
        dir
    }

    #[test]
    fn an_embed_card_reads_the_note_it_names() {
        let _turn = embeds::turn();
        let dir = embed_vault("card");
        embeds::install_dir(&dir);

        let whole = embed_card(&note_embed_line("![[Plan]]").unwrap());
        assert_eq!(whole.title, "Plan");
        assert_eq!(whole.head(), "Plan");
        // the front matter and the title line are not the body
        assert_eq!(
            whole.lines,
            vec![
                "First line.",
                "Second **line**.",
                "",
                "## Goals",
                "- ship it",
                "- test it",
                "",
                "- doc it",
                "- more",
                "- and more",
                "",
                "## Later",
                "Nothing.",
            ]
        );
        assert!(matches!(whole.found, embeds::Found::Note(_)));

        // a heading picks its section, whatever its case
        let goals = embed_card(&note_embed_line("![[plan#goals]]").unwrap());
        assert_eq!(goals.head(), "Plan › goals");
        assert_eq!(
            goals.lines,
            vec![
                "- ship it",
                "- test it",
                "",
                "- doc it",
                "- more",
                "- and more"
            ]
        );
        let later = embed_card(&note_embed_line("![[plan#Later]]").unwrap());
        assert_eq!(later.lines, vec!["Nothing."]);
        let nope = embed_card(&note_embed_line("![[plan#Nope]]").unwrap());
        assert_eq!(nope.lines, vec!["(no such heading)"]);

        // `^id` names a block: the list item that carries it, or the
        // paragraph around the line that does
        crate::testutil::write(
            &dir,
            "blocks.md",
            "# Blocks\n\nOne line.\nTwo line. ^para\nThree line.\n\n- a\n- b ^item\n- c\n",
        );
        let para = embed_card(&note_embed_line("![[blocks#^para]]").unwrap());
        assert_eq!(
            para.lines,
            vec!["One line.", "Two line. ^para", "Three line."]
        );
        let item = embed_card(&note_embed_line("![[blocks#^ITEM]]").unwrap());
        assert_eq!(item.lines, vec!["- b ^item"]);
        let none = embed_card(&note_embed_line("![[blocks#^gone]]").unwrap());
        assert_eq!(none.lines, vec!["(no such block)"]);

        // no such note: the target as typed, and nothing to show
        let gone = embed_card(&note_embed_line("![[gone]]").unwrap());
        assert_eq!(gone.found, embeds::Found::Missing);
        assert_eq!(gone.title, "gone");
        assert!(gone.lines.is_empty());

        // an edit on disk is seen on the next read
        crate::testutil::write(&dir, "plan.md", "# Plan\n\nRewritten.\n");
        let again = embed_card(&note_embed_line("![[plan]]").unwrap());
        assert_eq!(again.lines, vec!["Rewritten."]);

        embeds::forget();
        assert_eq!(
            embed_card(&note_embed_line("![[plan]]").unwrap()).found,
            embeds::Found::Unknown
        );
    }

    #[test]
    fn an_embed_is_a_card_in_the_editor() {
        let _turn = embeds::turn();
        let dir = embed_vault("editor");
        embeds::install_dir(&dir);
        let lines = buf("![[plan#Goals]]\nafter");
        let bs = blocks(&lines);
        assert_eq!(bs[0].kind, BlockKind::Embed);

        // the source line is the title row, in the callout colour
        let title = style_block_line(&lines, &bs[0], 0, 40);
        assert_eq!(text(&title), "▌ Plan › Goals");
        assert_eq!(title.cells[0].style.fg, theme::callout("note").fg);
        assert!(title.cells[2].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(title.src_len, "![[plan#Goals]]".chars().count());
        // a click on the title lands on the line
        let row = title.one_row();
        assert_eq!(row.display_to_source(0), 0);
        assert!(row.display_to_source(12) <= title.src_len);

        // the rows hung under it: the whole section, behind the rail
        let rows = embed_rows("![[plan#Goals]]", 40);
        let texts: Vec<String> = rows.iter().map(text).collect();
        assert_eq!(
            texts,
            vec![
                "▌ • ship it",
                "▌ • test it",
                "▌ ",
                "▌ • doc it",
                "▌ • more",
                "▌ • and more",
            ]
        );
        // a section of one line hangs one row
        assert_eq!(embed_rows("![[plan#Later]]", 40).len(), 1);
        // a narrow page wraps the rows rather than overrunning it, and every
        // row a line wraps into keeps its rail
        let narrow = embed_rows("![[plan#Goals]]", 8);
        assert!(
            narrow.iter().all(|r| str_width(&text(r)) <= 8),
            "{narrow:?}"
        );
        assert!(
            narrow.iter().all(|r| text(r).starts_with("▌ ")),
            "{narrow:?}"
        );
        assert!(narrow.len() > rows.len(), "{narrow:?}");

        // a note that is not there says so, in grey, and hangs nothing
        let gone = style_block_line(&buf("![[gone]]"), &bs[0], 0, 40);
        assert_eq!(text(&gone), "▌ gone (no such note)");
        assert_eq!(gone.cells[2].style.fg, theme::grey().fg);
        assert!(embed_rows("![[gone]]", 40).is_empty());

        // an un-walked vault: the title alone, nothing declared missing
        embeds::forget();
        assert_eq!(
            text(&style_block_line(&lines, &bs[0], 0, 40)),
            "▌ plan › Goals"
        );
        assert!(embed_rows("![[plan#Goals]]", 40).is_empty());
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
            Some(("a cat".into(), "x/cat.png".into(), None))
        );
        assert_eq!(
            image_line("![](p.png)"),
            Some((String::new(), "p.png".into(), None))
        );
    }

    #[test]
    fn front_matter_aliases_come_from_aliases_or_the_singular_alias() {
        assert_eq!(
            front_matter_aliases("---\naliases: [The Spec, \"spec sheet\"]\n---\n"),
            vec!["the spec", "spec sheet"]
        );
        assert_eq!(
            front_matter_aliases("---\nalias: Solo\n---\n"),
            vec!["solo"]
        );
        assert_eq!(
            front_matter_aliases("---\naliases:\n  - one\nalias: two\n---\n"),
            vec!["one", "two"]
        );
        assert!(front_matter_aliases("# none\nalias: no\n").is_empty());
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
    fn a_line_of_text_over_equals_or_dashes_is_a_setext_heading() {
        let lines = buf("Title\n===\n\nSub  \n----  \n");
        assert_eq!(
            blocks(&lines),
            vec![
                Block {
                    kind: BlockKind::Setext,
                    start: 0,
                    end: 1,
                },
                Block {
                    kind: BlockKind::Setext,
                    start: 3,
                    end: 4,
                },
            ]
        );
        assert_eq!(setext_level(&lines, 0), Some(1));
        assert_eq!(setext_level(&lines, 3), Some(2));
        // the underline is part of the heading, not a heading of its own
        assert_eq!(setext_level(&lines, 1), None);
        // and it takes three or more, the way a rule does
        assert!(blocks(&buf("Title\n==\n")).is_empty());
    }

    #[test]
    fn dashes_under_a_blank_a_list_or_a_heading_stay_a_rule() {
        for src in [
            "\n---\n",
            "- item\n---\n",
            "1. item\n---\n",
            "# h\n---\n",
            "> q\n---\n",
        ] {
            let bs = blocks(&buf(src));
            assert_eq!(bs.len(), 1, "{src:?}");
            assert_eq!(bs[0].kind, BlockKind::Rule, "{src:?}");
            assert_eq!(bs[0].start, 1, "{src:?}");
        }
        // a rule under a rule is two rules
        let bs = blocks(&buf("---\n---\n"));
        assert!(bs.iter().all(|b| b.kind == BlockKind::Rule));
        assert_eq!(bs.len(), 2);
    }

    #[test]
    fn a_setext_heading_is_drawn_like_an_atx_one_with_a_quiet_underline() {
        let lines = buf("Big **deal**\n===\nSmall\n---\n");
        let bs = blocks(&lines);
        let l = style_block_line(&lines, &bs[0], 0, 80);
        assert_eq!(text(&l), "Big deal");
        assert_eq!(l.cells[0].style, theme::heading(1));
        assert!(l.cells[4].style.add_modifier.contains(Modifier::BOLD));
        // every source column is still a cell, so the cursor maps through
        assert_eq!(l.src_len, 12);
        assert_eq!(l.one_row().display_to_source(4), 6);
        let u = style_block_line(&lines, &bs[0], 1, 80);
        assert_eq!(text(&u), "===");
        assert!(u.cells.iter().all(|c| c.style == theme::marker()));
        let l = style_block_line(&lines, &bs[1], 2, 80);
        assert_eq!(text(&l), "Small");
        assert_eq!(l.cells[0].style, theme::heading(2));
        assert_eq!(text(&style_block_line(&lines, &bs[1], 3, 80)), "---");
    }

    #[test]
    fn four_spaces_under_a_blank_line_open_an_indented_code_block() {
        let lines = buf("para\n\n    let x = 1;\n\tlet y = 2;\n\n    done\n\nafter\n");
        assert_eq!(
            blocks(&lines),
            vec![Block {
                kind: BlockKind::IndentedCode,
                start: 2,
                end: 5,
            }]
        );
        // at the top of the file too, with nothing above it
        let bs = blocks(&buf("    code\n\ntext\n"));
        assert_eq!(
            (bs[0].kind, bs[0].start, bs[0].end),
            (BlockKind::IndentedCode, 0, 0)
        );
        // and just under the front matter the scan started below
        let bs = blocks_from(&buf("---\na: b\n---\n    code\n"), 3);
        assert_eq!((bs[0].kind, bs[0].start), (BlockKind::IndentedCode, 3));
    }

    #[test]
    fn indented_lines_inside_a_list_are_the_list_not_code() {
        // a paragraph under an item, and a nested item, both keep the list
        assert!(blocks(&buf("- item\n\n    more of the item\n")).is_empty());
        assert!(blocks(&buf("- item\n  - nested\n\n    deeper\n")).is_empty());
        assert!(blocks(&buf("1. item\n\n    more\n")).is_empty());
        // an indented line right under text is a lazy continuation
        assert!(blocks(&buf("text\n    more text\n")).is_empty());
        // but a blank line after plain text is enough to set code off
        assert_eq!(
            blocks(&buf("text\n\n    code\n"))[0].kind,
            BlockKind::IndentedCode
        );
    }

    #[test]
    fn indented_code_swallows_what_would_otherwise_be_markdown() {
        let lines = buf("\n    ```\n    ---\n    | a |\n    # not a heading\n");
        let bs = blocks(&lines);
        assert_eq!(
            bs,
            vec![Block {
                kind: BlockKind::IndentedCode,
                start: 1,
                end: 4,
            }]
        );
        // drawn in the code colour with the indent kept, one cell per column
        let l = style_block_line(&lines, &bs[0], 2, 80);
        assert_eq!(text(&l), "    ---");
        assert!(l.cells.iter().all(|c| c.style == theme::code()));
        assert_eq!(l.one_row().display_to_source(5), 5);
        // a blank line inside the block is a blank row
        let lines = buf("\n    a\n\n    b\n");
        let bs = blocks(&lines);
        assert_eq!((bs[0].start, bs[0].end), (1, 3));
        assert_eq!(text(&style_block_line(&lines, &bs[0], 2, 80)), "");
    }

    #[test]
    fn a_fence_that_names_a_language_is_coloured_a_line_at_a_time() {
        let _lock = crate::testutil::serial();
        crate::highlight::set_enabled(true);
        theme::set_palette(theme::DARK);
        // parsed here rather than waited for: a test has no loop to poll the
        // highlighter's worker from
        crate::highlight::runs_now("rust", "// why\nlet n = 12;\n");
        let lines = buf("```rust\n// why\nlet n = 12;\n```\n");
        let bs = blocks(&lines);
        assert_eq!(bs[0].kind, BlockKind::Fence);
        // the cap row is the fence as typed, dim on the band
        let cap = style_block_line(&lines, &bs[0], 0, 80);
        assert_eq!(text(&cap), "     ```rust");
        assert!(cap
            .cells
            .iter()
            .all(|c| c.style.bg == Some(theme::DARK.code_bg)));
        // and a body line is coloured by role, one cell per source column,
        // behind the band's gutter
        let l = style_block_line(&lines, &bs[0], 2, 80);
        assert_eq!(text(&l), "  2  let n = 12;");
        let code = &l.cells[5..];
        assert_eq!(code[0].style.fg, Some(theme::DARK.code_keyword));
        assert_eq!(code[8].style.fg, Some(theme::DARK.code_number));
        assert_eq!(code[4].style, theme::code());
        assert_eq!(l.one_row().display_to_source(9), 4);
        // a comment leans, and it is the line above that says so
        let c = style_block_line(&lines, &bs[0], 1, 80);
        assert_eq!(c.cells[5].style.fg, Some(theme::DARK.code_comment));

        // a fence that names nothing catcher knows is drawn as it always was
        for src in [
            "```\nlet n = 12;\n```\n",
            "```gibberish\nlet n = 12;\n```\n",
        ] {
            let lines = buf(src);
            let l = style_block_line(&lines, &blocks(&lines)[0], 1, 80);
            assert!(
                l.cells[5..].iter().all(|c| c.style == theme::code()),
                "{src}"
            );
        }
        // nor is one with colour turned off
        crate::highlight::set_enabled(false);
        let l = style_block_line(&lines, &bs[0], 2, 80);
        assert!(l.cells[5..].iter().all(|c| c.style == theme::code()));
        crate::highlight::set_enabled(true);
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
    fn a_fence_in_the_editor_is_a_band_with_a_gutter_down_its_left() {
        let _lock = crate::testutil::serial();
        theme::set_palette(theme::DARK);
        crate::highlight::set_numbers(true);
        let lines = buf("```\nlet n = 1;\n    deep\n```\n");
        let bs = blocks(&lines);
        // a body row: two columns of side padding, then the number in the
        // gutter's colour, then the code
        let body = style_block_line(&lines, &bs[0], 1, 40);
        assert_eq!(text(&body), "  1  let n = 1;");
        assert_eq!(&text(&body)[..5], "  1  ");
        assert!(body.cells[..5].iter().all(|c| c.src == PAD));
        assert_eq!(body.cells[2].style, theme::code_gutter());
        assert_eq!(body.cells[2].ch, '1');
        // the ``` row sits on the same ground, markers and all
        let cap = style_block_line(&lines, &bs[0], 0, 40);
        assert_eq!(text(&cap), "     ```");
        assert!(cap
            .cells
            .iter()
            .all(|c| c.style.bg == Some(theme::DARK.code_bg)));
        // leading whitespace is ruled every fourth column
        let deep = style_block_line(&lines, &bs[0], 2, 40);
        assert_eq!(text(&deep), "  2  │   deep");
        assert_eq!(deep.cells[5].style, theme::code_guide());
        // a click in the gutter lands at the start of the line, and the
        // cursor's own column sits past the gutter rather than on it
        let row = body.one_row();
        assert_eq!(row.display_to_source(0), 0);
        assert_eq!(row.display_to_source(2), 0);
        assert_eq!(row.source_to_display(0), 5);
        assert_eq!(row.source_to_display(4), 9);
        // and the end of the line is just after the code, not past the fill
        // that paints the band to the page edge — on a ``` row too
        let full = wrap_rline(&body, 40).remove(0);
        assert_eq!(full.cells.len(), 40);
        assert_eq!(full.source_to_display(10), 15);
        let cap_row = wrap_rline(&cap, 40).remove(0);
        assert_eq!(cap_row.source_to_display(3), 8);

        // wrapped: the band is painted to the page edge and the continuation
        // row keeps a blank gutter, so the left edge is one straight line
        let rows = wrap_rline(&body, 12);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].cells.iter().map(|c| c.ch).collect::<String>(),
            "  1  let n ="
        );
        assert_eq!(
            rows[1].cells.iter().map(|c| c.ch).collect::<String>(),
            "     1;     "
        );
        assert!(rows[1].cells[..5].iter().all(|c| c.src == PAD));

        // numbers off: the band and its two-column inset, nothing more
        crate::highlight::set_numbers(false);
        let plain = style_block_line(&lines, &bs[0], 2, 40);
        assert_eq!(text(&plain), "      deep");
        assert!(plain.cells[..2].iter().all(|c| c.src == PAD));
        assert!(plain.cells[2..].iter().all(|c| c.style == theme::code()));
        crate::highlight::set_numbers(true);
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
        // the heading is shown, as `note › Method`; only the target drops it
        assert_eq!(text(&l), "note › Method");
        let chars: Vec<char> = "[[note#Method]]".chars().collect();
        let w = wikilink_at(&chars, 0).unwrap();
        assert_eq!(w.target, "note");
        assert_eq!(w.fragment.as_deref(), Some("Method"));
        assert_eq!(w.full_target(), "note#Method");
    }

    #[test]
    fn a_heading_link_draws_a_chevron_for_the_hash_and_keeps_its_columns() {
        let _turn = colours();
        // "[[note#Method]]": the `#` at column 6 becomes ` › `, one cell of
        // three characters, so the columns either side still map straight back
        let l = style_line("[[note#Method]]");
        let row = l.one_row();
        assert_eq!(row.display_to_source(0), 2); // the "n" of note
        assert_eq!(row.display_to_source(4), 6); // the chevron is the `#`
        assert_eq!(row.display_to_source(7), 7); // the "M" of Method
        assert!(l.cells.iter().all(|c| c.style.fg == theme::link().fg));
        // a block reference is shown the same way, caret and all
        assert_eq!(text(&style_line("[[note#^abc]]")), "note › ^abc");
        // an alias hides both the note and the heading
        assert_eq!(
            text(&style_line("[[note#Method|the method]]")),
            "the method"
        );
    }

    #[test]
    fn a_link_to_a_heading_in_this_note_is_a_link_with_no_note_to_resolve() {
        let _turn = colours();
        // `[[#Method]]` reads `› Method`, in link colour whatever the vault
        // knows: the note it names is the one on screen
        links::set_known(std::collections::HashSet::new());
        let l = style_line("[[#Method]]");
        assert_eq!(text(&l), "› Method");
        assert!(l.cells.iter().all(|c| c.style.fg == theme::link().fg));
        assert_eq!(l.one_row().display_to_source(2), 3); // the "M"
        assert_eq!(
            link_at("[[#Method]]", 3),
            Some(LinkTarget::Wiki("#Method".to_string()))
        );
        let chars: Vec<char> = "[[#Method]]".chars().collect();
        let w = wikilink_at(&chars, 0).unwrap();
        assert_eq!(w.target, "");
        assert_eq!(w.fragment.as_deref(), Some("Method"));
        // a `#` with nothing after it names nothing at all
        assert_eq!(wikilink_at(&"[[#]]".chars().collect::<Vec<_>>(), 0), None);
        links::set_known(std::collections::HashSet::from(["note".to_string()]));
        assert_eq!(
            style_line("[[note#Method]]").cells[0].style.fg,
            theme::link().fg
        );
    }

    #[test]
    fn link_at_carries_the_fragment_for_the_follower() {
        assert_eq!(
            link_at("see [[note#Method]] now", 6),
            Some(LinkTarget::Wiki("note#Method".to_string()))
        );
        assert_eq!(
            link_at("see [[note#^abc|x]] now", 6),
            Some(LinkTarget::Wiki("note#^abc".to_string()))
        );
        assert_eq!(
            link_at("see [[note]] now", 6),
            Some(LinkTarget::Wiki("note".to_string()))
        );
    }

    #[test]
    fn an_attachment_link_is_a_file_for_the_desktop() {
        assert!(is_attachment_path("report.pdf"));
        assert!(is_attachment_path("Board.CANVAS"));
        assert!(is_attachment_path("talk.m4a"));
        assert!(is_attachment_path("a.png"));
        assert!(!is_attachment_path("plan"));
        assert!(!is_attachment_path("plan.md"));
        assert_eq!(
            LinkTarget::wiki("report.pdf".into()),
            LinkTarget::File("report.pdf".into())
        );
        assert_eq!(
            LinkTarget::wiki("plan".into()),
            LinkTarget::Wiki("plan".into())
        );
        let file = LinkTarget::File("board.canvas".into());
        assert_eq!(LinkTarget::parse(&file.href()), file);
        // a body href spelled as the scheme comes back as the URL it was
        let url = LinkTarget::Url("attachment:x".into());
        assert_eq!(LinkTarget::parse(&url.href()), url);
        let line: Vec<char> = "see [[board.canvas]] now".chars().collect();
        assert_eq!(
            link_at(&line.iter().collect::<String>(), 8),
            Some(LinkTarget::File("board.canvas".into()))
        );
        assert_eq!(human_size(812), "812 B");
        assert_eq!(human_size(1_300_000), "1.2 MB");
    }

    #[test]
    fn split_fragment_separates_the_note_from_the_place_in_it() {
        assert_eq!(split_fragment("note#Method"), ("note", Some("Method")));
        assert_eq!(split_fragment(" note # Method "), ("note", Some("Method")));
        assert_eq!(split_fragment("note#^abc"), ("note", Some("^abc")));
        assert_eq!(split_fragment("#Method"), ("", Some("Method")));
        assert_eq!(split_fragment("note#"), ("note", None));
        assert_eq!(split_fragment("note"), ("note", None));
    }

    #[test]
    fn a_trailing_block_id_is_found_and_dimmed() {
        assert_eq!(
            block_id_at("some text ^abc-1"),
            Some((9, "abc-1".to_string()))
        );
        assert_eq!(
            block_id_at("some text ^abc-1  "),
            Some((9, "abc-1".to_string()))
        );
        assert_eq!(block_id_at("^abc"), Some((0, "abc".to_string())));
        // glued to a word, empty, or followed by anything: not an id
        assert_eq!(block_id_at("text^abc"), None);
        assert_eq!(block_id_at("text ^"), None);
        assert_eq!(block_id_at("text ^abc def"), None);
        assert_eq!(block_id_at("[[note#^abc]]"), None);
        assert_eq!(block_id_at(""), None);
        // drawn kept but dim, whatever the line was: a heading, a task
        let l = style_line("# Title ^abc");
        assert_eq!(text(&l), "Title ^abc");
        let tail: Vec<&Cell> = l.cells.iter().filter(|c| c.src >= 7).collect();
        assert_eq!(tail.len(), 5);
        assert!(tail.iter().all(|c| c.style == theme::marker()));
        assert_eq!(l.cells[0].style.fg, theme::heading(1).fg);
        let l = style_line("- [ ] task ^id");
        assert_eq!(text(&l), "\u{2610} task ^id");
        assert_eq!(l.cells.last().unwrap().style, theme::marker());
    }

    #[test]
    fn unmatched_or_escaped_brackets_stay_literal_text() {
        for src in [
            "[[unclosed",
            "[[a] b]]",
            "\\[[escaped]]",
            "![[embed.png]]",
            "[[ ]]",
            "[[#]]",
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
    fn a_markdown_link_to_an_md_file_is_a_note_and_a_url_is_not() {
        assert_eq!(
            link_at("[G](groceries.md)", 5),
            Some(LinkTarget::Wiki("groceries.md".to_string()))
        );
        assert_eq!(
            link_at("[x](https://a.md)", 5),
            Some(LinkTarget::Url("https://a.md".to_string()))
        );
        assert_eq!(
            link_at("[x](./stories/old%20name.md#Fruit)", 5),
            Some(LinkTarget::Wiki("stories/old name.md#Fruit".to_string()))
        );
        assert_eq!(
            link_at("[x](report.pdf)", 5),
            Some(LinkTarget::Url("report.pdf".to_string()))
        );
        assert_eq!(note_href("mailto:a.md"), None);
        assert_eq!(note_href("/abs/a.md"), None);
        assert_eq!(note_href(".md"), None);
        assert_eq!(
            LinkTarget::from_href("a b.md"),
            LinkTarget::Wiki("a b.md".to_string())
        );
    }

    #[test]
    fn md_links_skip_code_wikilinks_and_images() {
        let line = "`[a](b.md)` [[w]] ![i](p.png) [t](x.md) [u](https://y)";
        let l = md_links(line);
        assert_eq!(l.len(), 2);
        let src: Vec<char> = line.chars().collect();
        assert_eq!(l[0].href(&src), "x.md");
        assert_eq!(l[1].href(&src), "https://y");
        assert_eq!(
            &line[..]
                .chars()
                .skip(l[0].start)
                .take(l[0].end - l[0].start)
                .collect::<String>(),
            "[t](x.md)"
        );
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
        // and neither the cursor nor the styling finds a tag there either:
        // it is a link to a heading in this note
        assert_eq!(
            link_at("[[#heading]]", 3),
            Some(LinkTarget::Wiki("#heading".to_string()))
        );
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

    #[test]
    fn escaped_markers_are_literal_with_the_backslash_dimmed() {
        let l = style_line(r"\*not\* here");
        assert_eq!(text(&l), r"\*not\* here");
        assert_eq!(l.cells[0].style, theme::marker());
        assert_eq!(l.cells[1].style, theme::PLAIN);
        assert!(!l.cells[2].style.add_modifier.contains(Modifier::ITALIC));
        // every source column still yields exactly one cell, in order
        assert_eq!(l.cells.len(), l.src_len);
        for (k, c) in l.cells.iter().enumerate() {
            assert_eq!(c.src, k);
        }
        assert_eq!(l.one_row().display_to_source(3), 3);
        assert_eq!(l.one_row().display_to_source(6), 6);
    }

    #[test]
    fn an_escape_disarms_tags_wikilinks_and_bold() {
        let tag = style_line(r"\#2026 and #real");
        assert_eq!(text(&tag), r"\#2026 and #real");
        assert_eq!(tag.cells[1].style.fg, None);
        assert_eq!(tag.cells[2].style.fg, None);
        assert_eq!(tag.cells[11].style.fg, theme::tag().fg);
        let wiki = style_line(r"\[[x]]");
        assert_eq!(text(&wiki), r"\[[x]]");
        let bold = style_line(r"\*\*a**");
        assert_eq!(text(&bold), r"\*\*a**");
        assert!(bold
            .cells
            .iter()
            .all(|c| !c.style.add_modifier.contains(Modifier::BOLD)));
    }

    #[test]
    fn a_backslash_before_a_letter_or_at_the_end_is_ordinary_text() {
        let l = style_line(r"a\b *c*");
        assert_eq!(text(&l), r"a\b c");
        assert_eq!(l.cells[1].style, theme::PLAIN);
        assert!(l.cells[4].style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn two_trailing_spaces_show_a_hard_break_glyph() {
        let l = style_line("end  ");
        assert_eq!(text(&l), "end \u{21b5}");
        assert_eq!(l.cells[4].style, theme::marker());
        assert_eq!(l.cells[4].src, 4);
        assert_eq!(l.cells.len(), l.src_len);
        assert_eq!(l.one_row().display_to_source(4), 4);
        // three spaces: only the last one is the glyph
        assert_eq!(text(&style_line("end   ")), "end  \u{21b5}");
        // one trailing space, or a blank line, is no break
        assert_eq!(text(&style_line("end ")), "end ");
        assert_eq!(text(&style_line("   ")), "   ");
        // a blockquote or list item can end in one too
        assert_eq!(text(&style_line("> q  ")), "\u{258c} q \u{21b5}");
        assert_eq!(text(&style_line("- item  ")), "\u{2022} item \u{21b5}");
    }

    #[test]
    fn a_trailing_backslash_is_a_hard_break_unless_escaped() {
        let l = style_line(r"end\");
        assert_eq!(text(&l), "end\u{21b5}");
        assert_eq!(l.cells[3].style, theme::marker());
        assert_eq!(l.one_row().display_to_source(3), 3);
        // `\\` is an escaped backslash, not a break
        let l = style_line(r"end\\");
        assert_eq!(text(&l), r"end\\");
        assert_eq!(l.cells[3].style, theme::marker());
        assert_eq!(l.cells[4].style, theme::PLAIN);
        // and a third one is
        assert_eq!(text(&style_line(r"end\\\")), "end\\\\\u{21b5}");
    }

    #[test]
    fn hard_breaks_leave_fences_rules_tables_and_headings_alone() {
        assert_eq!(text(&style_line("```  ")), "```  ");
        assert_eq!(text(&style_line("| a |  ")), "| a |  ");
        assert_eq!(text(&style_line("# Title  ")), "Title  ");
        assert_eq!(text(&style_line("---  ")), "─────");
    }

    #[test]
    fn an_inline_comment_is_kept_dim_and_nothing_inside_it_is_markdown() {
        let l = style_line("a %% **b** [[c]] %% d");
        assert_eq!(text(&l), "a %% **b** [[c]] %% d");
        let src: Vec<char> = "a %% **b** [[c]] %% d".chars().collect();
        for (i, c) in l.cells.iter().enumerate() {
            assert_eq!(c.src, i);
            if (2..19).contains(&i) {
                assert_eq!(c.style, theme::marker(), "col {i}: {:?}", src[i]);
            }
        }
        assert_eq!(l.cells[0].style, theme::PLAIN);
        assert_eq!(l.cells[20].style, theme::PLAIN);
        assert_eq!(l.one_row().display_to_source(20), 20);
    }

    #[test]
    fn an_unclosed_comment_marker_is_literal_and_the_rest_still_styles() {
        let l = style_line("a %% **b**");
        assert_eq!(text(&l), "a %% b");
        assert_eq!(l.cells[2].style, theme::PLAIN);
        assert!(l.cells[5].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn a_comment_inside_inline_code_is_code() {
        let l = style_line("`%% x %%`");
        assert_eq!(text(&l), "%% x %%");
        assert!(l
            .cells
            .iter()
            .all(|c| c.style.fg == theme::inline_code().fg));
    }

    #[test]
    fn a_block_comment_is_one_block_drawn_quiet_and_an_unclosed_one_is_not() {
        let lines = buf("a\n%%\n# not a heading\n---\n%%\nb\n");
        let bs = blocks(&lines);
        assert_eq!(bs.len(), 1);
        assert_eq!(bs[0].kind, BlockKind::Comment);
        assert_eq!((bs[0].start, bs[0].end), (1, 4));
        for row in 1..=4 {
            let l = style_block_line(&lines, &bs[0], row, 40);
            assert_eq!(text(&l), lines[row]);
            assert!(l.cells.iter().all(|c| c.style == theme::marker()), "{row}");
            assert!(l.cells.iter().enumerate().all(|(i, c)| c.src == i));
        }
        // no partner: literal text (a setext underline makes it a heading),
        // and a rule below it is still a rule
        let lines = buf("%%\n---\n");
        assert_eq!(blocks(&lines)[0].kind, BlockKind::Setext);
        let lines = buf("%%\n\n---\n");
        let bs = blocks(&lines);
        assert_eq!(bs.len(), 1);
        assert_eq!(bs[0].kind, BlockKind::Rule);
        assert_eq!(text(&style_line("%%")), "%%");
    }

    #[test]
    fn a_comment_fence_inside_a_code_fence_is_code() {
        let lines = buf("```\n%%\n```\n%%\nx\n%%\n");
        let bs = blocks(&lines);
        assert_eq!(bs.len(), 2);
        assert_eq!(bs[0].kind, BlockKind::Fence);
        assert_eq!(
            (bs[1].kind, bs[1].start, bs[1].end),
            (BlockKind::Comment, 3, 5)
        );
    }

    #[test]
    fn comment_cuts_cover_block_and_inline_comments_but_not_code() {
        let md = "a %% x %% b\n%%\nhidden\n%%\n`%% c %%`\n```\n%% d %%\n```\n%% e %%\nf %% g\n";
        let cuts = comment_cuts(md);
        let cut: Vec<&str> = cuts.iter().map(|r| &md[r.clone()]).collect();
        // the comment between two spaces takes one of them along
        assert_eq!(cut, ["%% x %% ", "%%\nhidden\n%%\n", "%% e %%\n"]);
        let (stripped, map) = strip_comments(md);
        assert_eq!(stripped, "a b\n`%% c %%`\n```\n%% d %%\n```\nf %% g\n");
        assert_eq!(map, [(2, 8), (4, 13), (30, 8)]);
    }

    #[test]
    fn an_escape_wins_over_a_comment_and_an_html_tag() {
        // the backslash check runs before every other inline construct
        let l = style_line(r"\%% not a comment %% and \<u>not underlined</u>");
        assert_eq!(text(&l), r"\%% not a comment %% and \<u>not underlined</u>");
        assert!(l
            .cells
            .iter()
            .all(|c| c.style == theme::PLAIN || c.style == theme::marker()));
        let r =
            crate::render::render_page(r"\%% kept %%", 40, crate::config::TableStyle::default());
        assert_eq!(r.lines[0].text(), "%% kept %%");
    }
}

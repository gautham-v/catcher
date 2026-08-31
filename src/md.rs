//! Line-based markdown styling shared by the live-preview editor and the
//! full-page preview renderer.
//!
//! A source line becomes a [`RLine`]: one display cell per visible character,
//! each remembering which source column it came from. Keeping the mapping at
//! cell granularity makes both directions trivial — cursor placement from a
//! click, and selection highlighting from source columns — even when markers
//! like `## ` or `- [ ] ` are hidden or replaced.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

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

/// The one place colours live, so preview and live preview agree.
///
/// A neutral grey chassis with a single accent. Hue is never decoration: it
/// appears in exactly three places — the top-level heading, a checked task,
/// and the status bar when tinynote is talking about itself. Everything else
/// is a step on the grey ramp, which is why the ramp never reaches pure black
/// or pure white at either end: text that hits #ffffff on someone's custom
/// background looks like a bug, not emphasis.
///
/// The two palettes are the same structure at both polarities, not an
/// inversion — the code background goes *darker* than the page in light mode,
/// because "raised" means more contrast with the ground, not lighter.
pub mod theme {
    use super::*;
    use std::sync::OnceLock;

    /// Which polarity the terminal is showing.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
    pub enum Mode {
        #[default]
        Dark,
        Light,
    }

    /// Every colour tinynote can draw with, at one polarity.
    pub struct Palette {
        /// The one hue: h1, a checked mark, status-bar state.
        pub accent: Color,
        /// The brightest step — keys, group headings, anything that must lead.
        pub bright: Color,
        /// Second-rank headings and other structure that should recede.
        pub grey: Color,
        /// Markers, rules, quotes: present but never read first.
        pub dim: Color,
        /// Links, which lean on the underline rather than the colour.
        pub link: Color,
        /// Behind code, inline and fenced alike.
        pub code_bg: Color,
        /// Panel borders at rest.
        pub border: Color,
        /// Destructive confirmation, and nothing else.
        pub danger: Color,
        /// The ground a highlight or an inverted heading sits its text on.
        pub ground: Color,
    }

    const DARK: Palette = Palette {
        accent: Color::Rgb(0xff, 0x9e, 0x64),
        bright: Color::Rgb(0xe1, 0xe1, 0xe1),
        grey: Color::Rgb(0x78, 0x78, 0x78),
        dim: Color::Rgb(0x6c, 0x6c, 0x6c),
        link: Color::Rgb(0x9a, 0x9a, 0x9a),
        code_bg: Color::Rgb(0x1c, 0x1c, 0x1c),
        border: Color::Rgb(0x32, 0x32, 0x37),
        danger: Color::Rgb(0xf7, 0x76, 0x8e),
        ground: Color::Rgb(0x14, 0x14, 0x14),
    };

    const LIGHT: Palette = Palette {
        accent: Color::Rgb(0xb8, 0x5c, 0x18),
        bright: Color::Rgb(0x26, 0x26, 0x26),
        grey: Color::Rgb(0x76, 0x76, 0x76),
        dim: Color::Rgb(0x8d, 0x8d, 0x8d),
        link: Color::Rgb(0x5a, 0x58, 0x52),
        code_bg: Color::Rgb(0xe2, 0xe2, 0xe2),
        border: Color::Rgb(0xc8, 0xc8, 0xcd),
        danger: Color::Rgb(0xcd, 0x30, 0x48),
        ground: Color::Rgb(0xee, 0xee, 0xee),
    };

    static MODE: OnceLock<Mode> = OnceLock::new();

    /// Fix the polarity for the run. The first call wins; later ones are
    /// ignored, so a stray call in a test can never change a palette a
    /// rendered line was already measured against.
    pub fn set_mode(mode: Mode) {
        let _ = MODE.set(mode);
    }

    pub fn palette() -> &'static Palette {
        match MODE.get().copied().unwrap_or_default() {
            Mode::Dark => &DARK,
            Mode::Light => &LIGHT,
        }
    }

    /// Body text is never coloured: it inherits whatever foreground the
    /// terminal is already using, so a custom Ghostty theme keeps its own
    /// idea of what plain prose looks like.
    pub const PLAIN: Style = Style::new();

    /// Headings fall off into the ramp rather than each taking a hue: the
    /// accent leads, then grey, then weight alone.
    pub fn heading(level: usize) -> Style {
        match level {
            1 => Style::new()
                .fg(palette().accent)
                .add_modifier(Modifier::BOLD),
            2 => Style::new().fg(palette().grey).add_modifier(Modifier::BOLD),
            _ => Style::new().add_modifier(Modifier::BOLD),
        }
    }

    pub fn quote() -> Style {
        Style::new()
            .fg(palette().dim)
            .add_modifier(Modifier::ITALIC)
    }
    pub fn marker() -> Style {
        Style::new().fg(palette().dim)
    }
    /// Code carries no hue of its own — the raised background is the signal.
    pub fn code() -> Style {
        Style::new().bg(palette().code_bg)
    }
    pub fn link() -> Style {
        Style::new()
            .fg(palette().link)
            .add_modifier(Modifier::UNDERLINED)
    }
    pub fn highlight() -> Style {
        Style::new().fg(palette().ground).bg(palette().accent)
    }
    pub fn done() -> Style {
        Style::new().fg(palette().accent)
    }
    /// The text of a finished task: dim and struck through.
    pub fn done_text() -> Style {
        Style::new()
            .fg(palette().dim)
            .add_modifier(Modifier::CROSSED_OUT)
    }
    /// Status-bar state, panel titles: tinynote talking about itself.
    pub fn state() -> Style {
        Style::new().fg(palette().accent)
    }
    pub fn border() -> Style {
        Style::new().fg(palette().border)
    }
    pub fn danger() -> Style {
        Style::new().fg(palette().danger)
    }
    pub fn bright() -> Style {
        Style::new().fg(palette().bright)
    }

    pub const CHECKED: &str = "\u{2713}";
    pub const UNCHECKED: &str = "\u{2610}";
    pub const BULLET: &str = "\u{2022}";
    pub const QUOTE_BAR: &str = "\u{258c}";
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
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let cw = char_width(ch);
        if used + cw > width.saturating_sub(1) {
            break;
        }
        out.push(ch);
        used += cw;
    }
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
    if src.trim_start().starts_with("```") {
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

    // `code`
    if c == '`' {
        let end = find(b.src, i + 1, '`')?;
        return Some(delimited(b, i, i + 1, end, end + 1, theme::code()));
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
    if starts_url(b.src, i) {
        let mut end = i;
        while end < b.src.len() && !b.src[end].is_whitespace() {
            end += 1;
        }
        while end > i && matches!(b.src[end - 1], '.' | ',' | ')' | ']' | '!' | '?') {
            end -= 1;
        }
        let style = base.patch(theme::link());
        for k in i..end {
            b.keep(k, style);
        }
        return Some(end);
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

/// The URL of the markdown link or bare URL covering source column `col` of
/// `line`, if any. Used by modifier-click in the editor: the whole `[text](url)`
/// span counts, target included, so clicking anywhere on it follows the link.
pub fn link_at(line: &str, col: usize) -> Option<String> {
    let src: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < src.len() {
        // [text](url)
        // an image (`![alt](path)`) is not something to open in a browser
        if src[i] == '[' && (i == 0 || src[i - 1] != '!') {
            if let Some(close) = find(&src, i + 1, ']') {
                if src.get(close + 1) == Some(&'(') {
                    if let Some(paren) = find(&src, close + 2, ')') {
                        if (i..=paren).contains(&col) {
                            let url: String = src[close + 2..paren].iter().collect();
                            return (!url.trim().is_empty()).then(|| url.trim().to_string());
                        }
                        i = paren + 1;
                        continue;
                    }
                }
            }
        }
        if starts_url(&src, i) {
            let mut end = i;
            while end < src.len() && !src[end].is_whitespace() {
                end += 1;
            }
            while end > i && matches!(src[end - 1], '.' | ',' | ')' | ']' | '!' | '?') {
                end -= 1;
            }
            if (i..end).contains(&col) {
                return Some(src[i..end].iter().collect());
            }
            i = end.max(i + 1);
            continue;
        }
        i += 1;
    }
    None
}

/// Does a bare `http(s)://` URL start at `i`?
fn starts_url(src: &[char], i: usize) -> bool {
    let rest: String = src[i..].iter().take(8).collect();
    (rest.starts_with("http://") || rest.starts_with("https://"))
        && (i == 0 || !src[i - 1].is_alphanumeric())
}

fn find(src: &[char], from: usize, ch: char) -> Option<usize> {
    (from..src.len()).find(|&k| src[k] == ch)
}

fn find_pair(src: &[char], from: usize, ch: char) -> Option<usize> {
    (from..src.len().saturating_sub(1)).find(|&k| src[k] == ch && src[k + 1] == ch)
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    /// A ```-fenced code block, fences included.
    Fence,
    /// `---` / `***` / `___` alone on a line.
    Rule,
    /// A pipe table with its separator row.
    Table,
    /// A line holding nothing but `![alt](url)`.
    Image,
}

/// One block, as an inclusive range of source lines.
#[derive(Clone, Debug, PartialEq, Eq)]
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

fn is_fence(line: &str) -> bool {
    line.trim_start().starts_with("```")
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
fn is_table_rule(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|')
        && t.contains('-')
        && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

fn is_table_row(line: &str) -> bool {
    line.trim().starts_with('|') && line.trim().chars().count() > 1
}

/// `![alt](url)` and nothing else on the line — split into (alt, url).
pub fn image_line(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    let rest = t.strip_prefix("![")?;
    let close = rest.find("](")?;
    let alt = &rest[..close];
    let url = rest[close + 2..].strip_suffix(')')?;
    if alt.contains(']') || url.contains(')') || url.is_empty() {
        return None;
    }
    Some((alt.to_string(), url.to_string()))
}

/// Every block in the buffer, in order and never overlapping.
pub fn blocks(lines: &[String]) -> Vec<Block> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        // a fence swallows everything up to its close, so a `---` or a table
        // drawn inside a code sample is never mistaken for one
        if is_fence(&lines[i]) {
            let mut j = i + 1;
            while j < lines.len() && !is_fence(&lines[j]) {
                j += 1;
            }
            let end = j.min(lines.len() - 1);
            out.push(Block {
                kind: BlockKind::Fence,
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
        BlockKind::Rule => rule_line(src, width),
        BlockKind::Image => image_fallback_line(src),
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
struct TCell {
    start: usize,
    text: String,
}

/// Split `| a | b |` into its cells and the source columns of its pipes.
fn split_row(src: &str) -> (Vec<TCell>, Vec<usize>) {
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

/// Lay a table's rows out in aligned columns, and draw row `row` of it.
/// Every source row is exactly one display row, separator included.
fn table_line(rows: &[String], row: usize, width: usize) -> RLine {
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
        .map(|(_, (c, _))| c.iter().map(|c| str_width(&c.text)).collect())
        .collect();
    let widths = fit_widths(&column_widths(&measured, cols), width);

    let src = rows.get(row).map(String::as_str).unwrap_or("");
    // the separator row becomes the rule under the head
    if Some(row) == rule_row {
        let len = src.chars().count();
        let cells = table_rule(&widths)
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
        let text = truncate(&cell.text, *w);
        let (left, right) = pad_for(str_width(&text), *w, align);
        cells.extend(at(&" ".repeat(left), body, cell.start));
        for (i, ch) in text.chars().enumerate() {
            cells.push(Cell {
                ch,
                style: body,
                src: cell.start + i,
            });
        }
        let after = cell.start + text.chars().count();
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
        let line = "see [docs](http://x.y) now";
        assert_eq!(link_at(line, 3), None);
        assert_eq!(link_at(line, 4).as_deref(), Some("http://x.y"));
        assert_eq!(link_at(line, 6).as_deref(), Some("http://x.y"));
        assert_eq!(link_at(line, 21).as_deref(), Some("http://x.y"));
        assert_eq!(link_at(line, 22), None);

        // a bare URL, without its trailing punctuation
        let bare = "see https://x.y/z. ok";
        assert_eq!(link_at(bare, 4).as_deref(), Some("https://x.y/z"));
        assert_eq!(link_at(bare, 16).as_deref(), Some("https://x.y/z"));
        assert_eq!(link_at(bare, 17), None);

        // images are not links to open
        assert_eq!(link_at("![alt](attachments/a.png)", 8), None);
        // the second of two links on a line
        assert_eq!(
            link_at("[a](http://a) and [b](http://b)", 20).as_deref(),
            Some("http://b")
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
}


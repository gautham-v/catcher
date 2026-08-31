//! Markdown → styled cells for the full-page preview (^P).
//!
//! Block structure comes from pulldown-cmark here; the live-preview editor is
//! line-based instead. Both share the palette in [`crate::md::theme`].
//!
//! The preview keeps more than text: every cell remembers whether it belongs to
//! a link, every line remembers which source line it came from, and checkbox and
//! image lines are tagged. That is what makes the preview clickable — open a
//! link, toggle a checkbox, or click anywhere else to land in the editor at the
//! same place.

use crate::md::theme;
use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// One rendered character: what to draw, which link (if any) it belongs to, and
/// where in the source it came from — `None` for scaffolding the renderer added
/// itself (bullets, table padding, code-block indents, image labels).
#[derive(Clone, Debug, PartialEq)]
pub struct PCell {
    pub ch: char,
    pub style: Style,
    pub link: Option<usize>,
    /// (source line, source column in chars) this character was drawn from.
    pub src: Option<(usize, usize)>,
}

/// An inline image the preview would like to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageSpec {
    pub alt: String,
    pub url: String,
}

/// One rendered line, plus what a click on it should do.
#[derive(Clone, Debug, Default)]
pub struct PLine {
    pub cells: Vec<PCell>,
    /// Source line to toggle when this line's checkbox is clicked.
    pub checkbox: Option<usize>,
    /// Index into [`Rendered::images`] when this line stands in for an image.
    pub image: Option<usize>,
    /// Source line this rendered line came from, for click → cursor.
    pub src_line: Option<usize>,
}

/// Merge equal-styled cells into a ratatui line.
pub fn to_line(cells: &[PCell]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut text = String::new();
    let mut current: Option<Style> = None;
    for cell in cells {
        if current != Some(cell.style) {
            if let Some(s) = current {
                spans.push(Span::styled(std::mem::take(&mut text), s));
            }
            current = Some(cell.style);
        }
        text.push(cell.ch);
    }
    if let Some(s) = current {
        spans.push(Span::styled(text, s));
    }
    Line::from(spans)
}

impl PLine {
    /// The plain text of the line, for tests and debugging.
    #[cfg(test)]
    pub fn text(&self) -> String {
        self.cells.iter().map(|c| c.ch).collect()
    }
}

/// A whole rendered page.
#[derive(Clone, Debug, Default)]
pub struct Rendered {
    pub lines: Vec<PLine>,
    pub urls: Vec<String>,
    pub images: Vec<ImageSpec>,
}

impl Rendered {
    pub fn url(&self, i: usize) -> Option<&str> {
        self.urls.get(i).map(String::as_str)
    }
}

/// Options: GitHub-flavoured enough for notes.
fn options() -> Options {
    Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_TABLES
}

/// Unbounded-width render, for tests that don't care about the page width.
#[cfg(test)]
pub fn render(markdown: &str) -> Rendered {
    render_wide(markdown, usize::MAX)
}

/// Render for a page `width` columns wide; tables are laid out to fit it.
pub fn render_wide(markdown: &str, width: usize) -> Rendered {
    let mut r = Ren::new(markdown, width);
    r.run(markdown);
    r.finish()
}

/// Where cells are currently going: the page, or a table cell being measured.
enum Sink {
    Page,
    Table,
}

#[derive(Default)]
struct Table {
    aligns: Vec<Alignment>,
    rows: Vec<Vec<Vec<PCell>>>,
    in_head: bool,
    row: Vec<Vec<PCell>>,
}

struct Ren {
    /// The source, kept so cells can remember the column they came from.
    src: String,
    out: Rendered,
    cells: Vec<PCell>,
    cell_buf: Vec<PCell>,
    sink: Sink,
    styles: Vec<Style>,
    link: Option<usize>,
    prefix: String,
    list_depth: usize,
    in_code_block: bool,
    table: Option<Table>,
    /// Page width in columns, used to size tables.
    width: usize,
    /// Byte offset of the start of each source line.
    line_starts: Vec<usize>,
    /// Source line for the line currently being built.
    src_line: Option<usize>,
    pending_checkbox: Option<usize>,
    done_item: bool,
    image_alt: Option<(String, String)>,
}

impl Ren {
    fn new(markdown: &str, width: usize) -> Ren {
        let mut line_starts = vec![0usize];
        for (i, b) in markdown.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Ren {
            src: markdown.to_string(),
            out: Rendered::default(),
            cells: Vec::new(),
            cell_buf: Vec::new(),
            sink: Sink::Page,
            styles: vec![Style::default()],
            link: None,
            prefix: String::new(),
            list_depth: 0,
            in_code_block: false,
            table: None,
            width,
            line_starts,
            src_line: None,
            pending_checkbox: None,
            done_item: false,
            image_alt: None,
        }
    }

    fn style(&self) -> Style {
        *self.styles.last().unwrap()
    }

    fn buf(&mut self) -> &mut Vec<PCell> {
        match self.sink {
            Sink::Page => &mut self.cells,
            Sink::Table => &mut self.cell_buf,
        }
    }

    /// Push scaffolding the renderer invented: it maps back to no source column.
    fn push(&mut self, text: &str, style: Style, link: Option<usize>) {
        self.push_at(text, style, link, None);
    }

    /// Push text, optionally carrying the source byte offset of its first
    /// character so each cell remembers where it came from.
    fn push_at(&mut self, text: &str, style: Style, link: Option<usize>, off: Option<usize>) {
        let mut off = off;
        let mut cells: Vec<PCell> = Vec::with_capacity(text.len());
        for ch in text.chars() {
            cells.push(PCell {
                ch,
                style,
                link,
                src: off.map(|o| self.pos_of(o)),
            });
            if let Some(o) = off.as_mut() {
                *o += ch.len_utf8();
            }
        }
        self.buf().extend(cells);
    }

    fn line_of(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        }
    }

    /// Source byte offset → (line, column in chars).
    fn pos_of(&self, offset: usize) -> (usize, usize) {
        let line = self.line_of(offset);
        let start = self.line_starts.get(line).copied().unwrap_or(0);
        let offset = offset.min(self.src.len());
        let col = self.src.get(start..offset).map_or(0, |s| s.chars().count());
        (line, col)
    }

    fn flush(&mut self) {
        if self.cells.is_empty() {
            return;
        }
        let cells = std::mem::take(&mut self.cells);
        self.out.lines.push(PLine {
            cells,
            checkbox: self.pending_checkbox.take(),
            image: None,
            src_line: self.src_line,
        });
    }

    fn blank(&mut self) {
        self.flush();
        if !self
            .out
            .lines
            .last()
            .map(|l| l.cells.is_empty())
            .unwrap_or(true)
        {
            self.out.lines.push(PLine::default());
        }
    }

    /// Start a line inside a blockquote with its `▌ ` bars. Continuation lines
    /// get theirs at the soft break; this is the first line of each block.
    fn line_prefix(&mut self) {
        if self.prefix.is_empty() || !matches!(self.sink, Sink::Page) || !self.cells.is_empty() {
            return;
        }
        let p = self.prefix.clone();
        self.push(&p, theme::marker(), None);
    }

    fn indent(&self) -> String {
        format!(
            "{}{}",
            self.prefix,
            "  ".repeat(self.list_depth.saturating_sub(1))
        )
    }

    /// Text from the document: scan for `==highlight==` and bare URLs.
    /// `off` is the source byte offset of `text`, when it is a verbatim slice.
    fn emit_text(&mut self, text: &str, off: Option<usize>) {
        let base = self.style();
        let link = self.link;
        let chars: Vec<char> = text.chars().collect();
        // byte offset of each char, so every run knows where it started
        let mut byte_at: Vec<usize> = Vec::with_capacity(chars.len() + 1);
        let mut b = 0;
        for ch in &chars {
            byte_at.push(b);
            b += ch.len_utf8();
        }
        byte_at.push(b);
        let at = |i: usize| off.map(|o| o + byte_at[i]);

        let mut i = 0;
        let mut run = String::new();
        let mut run_start = 0usize;
        while i < chars.len() {
            // ==highlight==
            if chars[i] == '=' && chars.get(i + 1) == Some(&'=') {
                if let Some(end) = find_pair(&chars, i + 2) {
                    self.push_at(&std::mem::take(&mut run), base, link, at(run_start));
                    let body: String = chars[i + 2..end].iter().collect();
                    self.push_at(&body, base.patch(theme::highlight()), link, at(i + 2));
                    i = end + 2;
                    run_start = i;
                    continue;
                }
            }
            // bare URL, when not already inside a link
            if link.is_none() && starts_url(&chars, i) {
                let mut end = i;
                while end < chars.len() && !chars[end].is_whitespace() {
                    end += 1;
                }
                while end > i && matches!(chars[end - 1], '.' | ',' | ')' | ']' | '!' | '?') {
                    end -= 1;
                }
                let url: String = chars[i..end].iter().collect();
                self.push_at(&std::mem::take(&mut run), base, None, at(run_start));
                let idx = self.out.urls.len();
                self.out.urls.push(url.clone());
                self.push_at(&url, base.patch(theme::link()), Some(idx), at(i));
                i = end;
                run_start = i;
                continue;
            }
            if run.is_empty() {
                run_start = i;
            }
            run.push(chars[i]);
            i += 1;
        }
        self.push_at(&run, base, link, at(run_start));
    }

    fn run(&mut self, markdown: &str) {
        for (event, range) in Parser::new_ext(markdown, options()).into_offset_iter() {
            let src_line = self.line_of(range.start);
            if self.cells.is_empty() && matches!(self.sink, Sink::Page) {
                self.src_line = Some(src_line);
            }
            self.event(event, src_line, range);
        }
        self.flush();
    }

    fn event(&mut self, event: Event<'_>, src_line: usize, range: std::ops::Range<usize>) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                self.blank();
                self.src_line = Some(src_line);
                self.line_prefix();
                self.styles.push(theme::heading(level as usize));
            }
            Event::End(TagEnd::Heading(_)) => {
                self.styles.pop();
                self.flush();
            }
            Event::Start(Tag::Paragraph) => {
                if self.list_depth == 0 && self.table.is_none() {
                    self.blank();
                    self.src_line = Some(src_line);
                }
                self.line_prefix();
            }
            Event::End(TagEnd::Paragraph) => self.flush(),
            Event::Start(Tag::BlockQuote(_)) => {
                self.blank();
                self.prefix.push_str("▌ ");
                self.styles.push(theme::quote());
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                self.styles.pop();
                let n = self.prefix.len().saturating_sub("▌ ".len());
                self.prefix.truncate(n);
                self.flush();
            }
            Event::Start(Tag::List(_)) => {
                if self.list_depth == 0 {
                    self.blank();
                }
                self.list_depth += 1;
            }
            Event::End(TagEnd::List(_)) => {
                self.list_depth = self.list_depth.saturating_sub(1);
                self.flush();
            }
            Event::Start(Tag::Item) => {
                self.flush();
                self.src_line = Some(src_line);
                let text = format!("{}{} ", self.indent(), theme::BULLET);
                self.push(&text, theme::marker(), None);
            }
            Event::End(TagEnd::Item) => {
                if self.done_item {
                    self.styles.pop();
                    self.done_item = false;
                }
                self.flush()
            }
            Event::TaskListMarker(done) => {
                // replace the bullet we pushed at the start of the item
                self.cells.clear();
                let (mark, style) = if done {
                    (theme::CHECKED, theme::done())
                } else {
                    (theme::UNCHECKED, theme::marker())
                };
                let text = format!("{}{mark} ", self.indent());
                self.push(&text, style, None);
                self.pending_checkbox = Some(src_line);
                if done {
                    // done items read as struck-through and dim until the item ends
                    self.styles.push(self.style().patch(theme::done_text()));
                    self.done_item = true;
                }
            }
            Event::Start(Tag::CodeBlock(_)) => {
                self.blank();
                self.src_line = Some(src_line);
                self.in_code_block = true;
            }
            Event::End(TagEnd::CodeBlock) => {
                self.in_code_block = false;
                self.flush();
            }
            Event::Start(Tag::Emphasis) => self
                .styles
                .push(self.style().add_modifier(Modifier::ITALIC)),
            Event::Start(Tag::Strong) => {
                self.styles.push(self.style().add_modifier(Modifier::BOLD))
            }
            Event::Start(Tag::Strikethrough) => self
                .styles
                .push(self.style().add_modifier(Modifier::CROSSED_OUT)),
            Event::End(TagEnd::Emphasis)
            | Event::End(TagEnd::Strong)
            | Event::End(TagEnd::Strikethrough) => {
                self.styles.pop();
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                let idx = self.out.urls.len();
                self.out.urls.push(dest_url.into_string());
                self.link = Some(idx);
                self.styles.push(self.style().patch(theme::link()));
            }
            Event::End(TagEnd::Link) => {
                self.styles.pop();
                self.link = None;
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                self.image_alt = Some((String::new(), dest_url.into_string()));
            }
            Event::End(TagEnd::Image) => {
                if let Some((alt, url)) = self.image_alt.take() {
                    self.flush();
                    let idx = self.out.images.len();
                    self.out.images.push(ImageSpec {
                        alt: alt.clone(),
                        url: url.clone(),
                    });
                    let label = if alt.is_empty() {
                        format!("🖼 {url}")
                    } else {
                        format!("🖼 {alt} ({url})")
                    };
                    self.push(&label, theme::marker(), None);
                    let cells = std::mem::take(&mut self.cells);
                    self.out.lines.push(PLine {
                        cells,
                        checkbox: None,
                        image: Some(idx),
                        src_line: self.src_line,
                    });
                }
            }
            // tables
            Event::Start(Tag::Table(aligns)) => {
                self.blank();
                self.src_line = Some(src_line);
                self.table = Some(Table {
                    aligns,
                    ..Table::default()
                });
            }
            Event::End(TagEnd::Table) => self.emit_table(),
            Event::Start(Tag::TableHead) => {
                if let Some(t) = self.table.as_mut() {
                    t.in_head = true;
                }
            }
            Event::End(TagEnd::TableHead) | Event::End(TagEnd::TableRow) => {
                if let Some(t) = self.table.as_mut() {
                    let row = std::mem::take(&mut t.row);
                    t.rows.push(row);
                    t.in_head = false;
                }
            }
            Event::Start(Tag::TableRow) => {}
            Event::Start(Tag::TableCell) => {
                self.cell_buf.clear();
                self.sink = Sink::Table;
                if self.table.as_ref().is_some_and(|t| t.in_head) {
                    self.styles.push(self.style().add_modifier(Modifier::BOLD));
                }
            }
            Event::End(TagEnd::TableCell) => {
                if self.table.as_ref().is_some_and(|t| t.in_head) {
                    self.styles.pop();
                }
                self.sink = Sink::Page;
                let cell = std::mem::take(&mut self.cell_buf);
                if let Some(t) = self.table.as_mut() {
                    t.row.push(cell);
                }
            }
            Event::Code(code) => {
                let style = self.style().patch(theme::code());
                let link = self.link;
                // the range spans the backticks too; the content starts after them
                let ticks = self.src[range.clone()]
                    .chars()
                    .take_while(|c| *c == '`')
                    .count();
                self.push_at(&code.into_string(), style, link, Some(range.start + ticks));
            }
            Event::Text(text) => {
                if let Some((alt, _)) = self.image_alt.as_mut() {
                    alt.push_str(&text);
                } else if self.in_code_block {
                    let mut off = range.start;
                    // split_inclusive, not lines(): the line ending has to be
                    // counted as it is in the file, `\r\n` included, or every
                    // later offset in a CRLF note drifts by a byte a line
                    for raw in text.split_inclusive('\n') {
                        let l = raw.trim_end_matches('\n').trim_end_matches('\r');
                        // the two-space indent is ours; the code itself is the file's
                        self.push("  ", theme::code(), None);
                        self.push_at(l, theme::code(), None, Some(off));
                        off += raw.len();
                        let cells = std::mem::take(&mut self.cells);
                        self.out.lines.push(PLine {
                            cells,
                            checkbox: None,
                            image: None,
                            src_line: self.src_line,
                        });
                    }
                } else {
                    self.emit_text(&text, Some(range.start));
                }
            }
            Event::SoftBreak => {
                if matches!(self.sink, Sink::Table) {
                    self.push(" ", self.style(), self.link);
                } else {
                    self.flush();
                    self.src_line = Some(src_line);
                    if !self.prefix.is_empty() {
                        let p = self.prefix.clone();
                        self.push(&p, theme::marker(), None);
                    }
                }
            }
            Event::HardBreak => self.flush(),
            Event::Rule => {
                self.blank();
                self.push(&"─".repeat(40), theme::marker(), None);
                self.flush();
            }
            _ => {}
        }
    }

    /// Lay out the buffered table: aligned columns with a light rule under the head.
    fn emit_table(&mut self) {
        let Some(t) = self.table.take() else { return };
        if t.rows.is_empty() {
            return;
        }
        let cols = t.rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let measured: Vec<Vec<usize>> = t
            .rows
            .iter()
            .map(|r| r.iter().map(|c| cells_width(c)).collect())
            .collect();
        let widths = crate::md::fit_widths(&crate::md::column_widths(&measured, cols), self.width);
        let src_line = self.src_line;
        for (ri, row) in t.rows.iter().enumerate() {
            let mut cells: Vec<PCell> = Vec::new();
            for (ci, w) in widths.iter().enumerate().take(cols) {
                if ci > 0 {
                    cells.extend(str_cells(crate::md::COL_SEP, theme::marker()));
                }
                let empty: Vec<PCell> = Vec::new();
                let cell = truncate_cells(row.get(ci).unwrap_or(&empty), *w);
                let align = align_of(t.aligns.get(ci).copied().unwrap_or(Alignment::None));
                let (left, right) = crate::md::pad_for(cells_width(&cell), *w, align);
                cells.extend(str_cells(&" ".repeat(left), theme::PLAIN));
                cells.extend(cell.iter().cloned());
                cells.extend(str_cells(&" ".repeat(right), theme::PLAIN));
            }
            self.out.lines.push(PLine {
                cells,
                checkbox: None,
                image: None,
                src_line,
            });
            if ri == 0 {
                let rule = crate::md::table_rule(&widths);
                self.out.lines.push(PLine {
                    cells: str_cells(&rule, theme::marker()),
                    checkbox: None,
                    image: None,
                    src_line,
                });
            }
        }
    }

    fn finish(mut self) -> Rendered {
        self.flush();
        self.out
    }
}

/// pulldown's alignment in the shared vocabulary.
fn align_of(a: Alignment) -> crate::md::Align {
    match a {
        Alignment::Right => crate::md::Align::Right,
        Alignment::Center => crate::md::Align::Center,
        _ => crate::md::Align::Left,
    }
}

fn str_cells(s: &str, style: Style) -> Vec<PCell> {
    s.chars()
        .map(|ch| PCell {
            ch,
            style,
            link: None,
            src: None,
        })
        .collect()
}

/// A cell run cut to `width` columns, ellipsis included when it was cut.
fn truncate_cells(cells: &[PCell], width: usize) -> Vec<PCell> {
    if cells_width(cells) <= width {
        return cells.to_vec();
    }
    let mut out: Vec<PCell> = Vec::new();
    let mut used = 0;
    for c in cells {
        let cw = crate::md::char_width(c.ch);
        if used + cw > width.saturating_sub(1) {
            break;
        }
        out.push(c.clone());
        used += cw;
    }
    let style = out.last().map(|c| c.style).unwrap_or(theme::PLAIN);
    out.push(PCell {
        ch: '…',
        style,
        link: None,
        src: None,
    });
    out
}

/// Display width of a run of cells, in terminal columns.
pub fn cells_width(cells: &[PCell]) -> usize {
    cells.iter().map(|c| crate::md::char_width(c.ch)).sum()
}

fn starts_url(chars: &[char], i: usize) -> bool {
    let rest: String = chars[i..].iter().take(8).collect();
    (rest.starts_with("http://") || rest.starts_with("https://"))
        && (i == 0 || !chars[i - 1].is_alphanumeric())
}

fn find_pair(chars: &[char], from: usize) -> Option<usize> {
    (from..chars.len().saturating_sub(1)).find(|&k| chars[k] == '=' && chars[k + 1] == '=')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(r: &Rendered) -> String {
        r.lines
            .iter()
            .map(|l| l.text())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_without_panic() {
        let md = "# Title\n\nSome **bold** and *italic* and `code`.\n\n- one\n- [ ] task\n- [x] done\n\n> quote\n\n```\nlet x = 1;\n```\n\n---\n";
        let r = render(md);
        assert!(r.lines.len() > 5);
        let flat = flat(&r);
        assert!(flat.contains("Title"));
        assert!(flat.contains("bold"));
        assert!(flat.contains("let x = 1;"));
    }

    #[test]
    fn code_block_offsets_survive_crlf() {
        for md in [
            "# T\n\n```\nlet x = 1;\nlet y = 2;\n```\n\ntail\n",
            "# T\r\n\r\n```\r\nlet x = 1;\r\nlet y = 2;\r\n```\r\n\r\ntail\r\n",
        ] {
            let src_lines: Vec<&str> = md.lines().collect();
            for line in &render(md).lines {
                for c in &line.cells {
                    // every mapped cell points at its own character
                    if let Some((l, col)) = c.src {
                        let at = src_lines[l].chars().nth(col);
                        assert_eq!(at, Some(c.ch), "{md:?} at ({l},{col})");
                    }
                }
            }
        }
    }

    #[test]
    fn a_wide_table_is_squeezed_into_the_page_width() {
        let r = render_wide("| a | bbbbbbbbbbbbbbbbbbbb |\n| --- | --- |\n| 1 | 2 |\n", 16);
        for l in &r.lines {
            assert!(cells_width(&l.cells) <= 16);
        }
        let head: String = r.lines[0].cells.iter().map(|c| c.ch).collect();
        assert_eq!(head, "a │ bbbbbbbbbbb…");
    }

    #[test]
    fn tables_get_aligned_columns_and_a_head_rule() {
        let md = "| a | bbbb |\n| --- | ---: |\n| 1 | 2 |\n";
        let r = render(md);
        let rows: Vec<String> = r
            .lines
            .iter()
            .map(|l| l.text())
            .filter(|t| !t.trim().is_empty())
            .collect();
        assert_eq!(rows[0], "a │ bbbb");
        assert_eq!(rows[1], "──┼─────");
        assert_eq!(rows[2], "1 │    2"); // right aligned
                                         // the header is bold
        assert!(r.lines[0].cells[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD));
    }

    #[test]
    fn table_columns_are_measured_in_display_columns() {
        let r = render("| 漢字 | b |\n| --- | --- |\n| x | y |\n");
        let rows: Vec<&PLine> = r
            .lines
            .iter()
            .filter(|l| !l.text().trim().is_empty())
            .collect();
        let widths: Vec<usize> = rows.iter().map(|l| cells_width(&l.cells)).collect();
        // every row, rule included, lines up at the same width
        assert!(widths.windows(2).all(|w| w[0] == w[1]), "{widths:?}");
    }

    #[test]
    fn every_quoted_line_gets_its_bar() {
        // the first line of a quote needs the bar as much as its continuations
        let r = render("> first line\n> second line\n\nafter\n");
        let quoted: Vec<String> = r
            .lines
            .iter()
            .map(|l| l.text())
            .filter(|t| t.contains("line"))
            .collect();
        assert_eq!(quoted, vec!["▌ first line", "▌ second line"]);
        // text outside the quote keeps its bar off
        assert!(r.lines.iter().any(|l| l.text() == "after"));
    }

    #[test]
    fn highlight_gets_the_highlight_style() {
        let r = render("a ==wow== b");
        let line = r.lines.iter().find(|l| l.text().contains("wow")).unwrap();
        assert_eq!(line.text(), "a wow b");
        let cell = line.cells.iter().find(|c| c.ch == 'w').unwrap();
        assert_eq!(cell.style.bg, theme::highlight().bg);
    }

    #[test]
    fn checkboxes_render_and_remember_their_source_line() {
        let r = render("# t\n\n- [ ] todo\n- [x] done\n");
        let todo = r.lines.iter().find(|l| l.text().contains("todo")).unwrap();
        assert_eq!(todo.text(), "☐ todo");
        assert_eq!(todo.checkbox, Some(2));
        let done = r.lines.iter().find(|l| l.text().contains("done")).unwrap();
        assert_eq!(done.text(), "✓ done");
        assert_eq!(done.checkbox, Some(3));
        assert!(done.cells[0].style.fg == theme::done().fg);
    }

    #[test]
    fn links_and_bare_urls_are_recorded() {
        let r = render("see [docs](http://x.y) and https://z.example/p now");
        let line = r.lines.iter().find(|l| l.text().contains("docs")).unwrap();
        let docs = line.cells.iter().find(|c| c.ch == 'd').unwrap();
        assert_eq!(r.url(docs.link.unwrap()), Some("http://x.y"));
        let bare = line
            .cells
            .iter()
            .find(|c| c.link.map(|i| r.urls[i].starts_with("https://z")) == Some(true))
            .unwrap();
        assert_eq!(r.url(bare.link.unwrap()), Some("https://z.example/p"));
        assert!(line.text().contains("https://z.example/p"));
    }

    #[test]
    fn images_become_their_own_line() {
        let r = render("![a cat](cat.png)\n");
        let line = r.lines.iter().find(|l| l.image.is_some()).unwrap();
        assert_eq!(line.text(), "🖼 a cat (cat.png)");
        assert_eq!(
            r.images[line.image.unwrap()],
            ImageSpec {
                alt: "a cat".into(),
                url: "cat.png".into()
            }
        );
    }
}

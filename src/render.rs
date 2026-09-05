//! Markdown → styled cells for the full-page preview (^P).
//!
//! Block structure comes from pulldown-cmark here; the live-preview editor is
//! line-based instead. Both share the palette in [`crate::theme`].
//!
//! The preview keeps more than text: every cell remembers whether it belongs to
//! a link, every line remembers which source line it came from, and checkbox and
//! image lines are tagged. That is what makes the preview clickable — open a
//! link, toggle a checkbox, or click anywhere else to land in the editor at the
//! same place.

use crate::config::TableStyle;
use crate::theme;
use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
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
    /// This line is deliberately wider than the page and must not be
    /// soft-wrapped: it is one row of a scrolling table, and the page pans
    /// sideways across it instead.
    pub wide: bool,
    /// Columns the rows this line wraps into after the first are indented
    /// by, so a list item's continuation sits under its text, not its marker.
    pub hang: usize,
}

/// Wrap a page line to `width`, honouring its hanging indent: every row after
/// the first is pushed in under the text the first row began with.
pub fn wrap_pline(line: &PLine, width: usize) -> Vec<Vec<PCell>> {
    let width = width.max(1);
    if line.hang == 0 {
        return wrap_pcells(&line.cells, width);
    }
    let rest = width.saturating_sub(line.hang).max(4);
    wrap_hang(&line.cells, width, rest)
        .into_iter()
        .enumerate()
        .map(|(i, row)| {
            if i == 0 {
                row
            } else {
                let mut cells = str_cells(&" ".repeat(line.hang), theme::PLAIN);
                cells.extend(row);
                cells
            }
        })
        .collect()
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
    #[cfg(test)]
    pub fn url(&self, i: usize) -> Option<&str> {
        self.urls.get(i).map(String::as_str)
    }
}

/// Options: GitHub-flavoured enough for notes.
fn options() -> Options {
    Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_TABLES
        | Options::ENABLE_MATH
        | Options::ENABLE_FOOTNOTES
}

/// Unbounded-width render, for tests that don't care about the page width.
#[cfg(test)]
pub fn render(markdown: &str) -> Rendered {
    render_wide(markdown, usize::MAX)
}

/// Render for a page `width` columns wide, with the default table shape.
#[cfg(test)]
pub fn render_wide(markdown: &str, width: usize) -> Rendered {
    render_page(markdown, width, TableStyle::default())
}

/// Render for a page `width` columns wide, drawing wide tables the way the
/// settings ask for.
/// Test-only since the reading view started slicing the front matter off: the
/// app always knows what line its markdown began on, so it always has an
/// offset to pass. This is that call with the offset zero.
#[cfg(test)]
pub fn render_page(markdown: &str, width: usize, tables: TableStyle) -> Rendered {
    render_page_at(markdown, 0, width, tables)
}

/// The same, when `markdown` is a slice of a longer file that begins at source
/// line `first_line` — the reading view hands us a body with its front matter
/// already cut off. Every line number a cell reports is file-absolute, because
/// `PCell::src` and `PLine::src_line` are what a click in the preview turns
/// back into a position in the buffer.
pub fn render_page_at(
    markdown: &str,
    first_line: usize,
    width: usize,
    tables: TableStyle,
) -> Rendered {
    let mut r = Ren::new(markdown, first_line, width, tables);
    r.run(markdown);
    r.finish()
}

/// Add the linked-mentions footer to an already-rendered page: a rule, a count,
/// and one row per note that links here.
///
/// It is appended rather than rendered because it is not part of the note — the
/// file on disk says nothing about who points at it, and nothing the footer
/// draws should ever map back into the buffer. Every cell it makes carries no
/// source position and every line no source line, so a click in the footer can
/// open the note it names but can never land the cursor in the note you are
/// reading.
///
/// With no mentions there is no footer at all, not even a rule: a note nothing
/// links to should look like a note, not like a note with an empty drawer at
/// the bottom.
pub fn append_mentions(r: &mut Rendered, mentions: &[crate::mentions::Mention], width: usize) {
    if mentions.is_empty() {
        return;
    }
    let dim = theme::marker();
    r.lines.push(PLine::default());
    r.lines.push(PLine {
        // the same rule the document itself draws for `---`, so the footer is
        // separated the way a section of the note would be
        cells: str_cells(&"─".repeat(if width == usize::MAX { 40 } else { width }), dim),
        ..Default::default()
    });
    let count = match mentions.len() {
        1 => "1 note links here".to_string(),
        n => format!("{n} notes link here"),
    };
    r.lines.push(PLine {
        cells: str_cells(&count, dim),
        ..Default::default()
    });

    // one name column for the whole footer, so the excerpts line up and read
    // as a column rather than as ragged sentences
    let namew = mentions
        .iter()
        .map(|m| crate::md::str_width(&m.name))
        .max()
        .unwrap_or(0)
        .min(MAX_NAME_COLS)
        .min(width.saturating_sub(2));
    for m in mentions {
        let idx = r.urls.len();
        // an exact file, not a name to resolve again: two notes called `spec`
        // must not send the click to whichever one the resolver prefers
        r.urls
            .push(crate::md::LinkTarget::Note(m.path.to_string_lossy().into_owned()).href());
        let mut cells = str_cells("  ", dim);
        let mut name = truncate_cells(&str_cells(&m.name, theme::link()), namew);
        for c in &mut name {
            c.link = Some(idx);
        }
        let pad = namew.saturating_sub(cells_width(&name));
        cells.extend(name);
        cells.extend(str_cells(&" ".repeat(pad), dim));
        // ×3 is the whole reason the row collapsed, so its room is taken
        // before the excerpt's and it is never the thing that gets cut away
        let tail = if m.count > 1 {
            format!(" ×{}", m.count)
        } else {
            String::new()
        };
        let room = width
            .saturating_sub(cells_width(&cells) + 2 + crate::md::str_width(&tail))
            .min(MAX_EXCERPT_COLS);
        // a narrow page should show fewer things rather than shredded ones: an
        // excerpt with a dozen columns to live in says nothing worth the space
        if room >= 12 && !m.excerpt.is_empty() {
            cells.extend(str_cells("  ", dim));
            cells.extend(excerpt_cells(&m.excerpt, m.link, dim, room));
        }
        cells.extend(str_cells(&tail, dim));
        r.lines.push(PLine {
            // never wider than the page: the footer must not be the thing that
            // makes a page of prose pan sideways
            cells: truncate_cells(&cells, width),
            ..Default::default()
        });
    }
}

/// The widest an excerpt is ever drawn, however wide the window is. Past this
/// the eye stops reading the column and starts reading the page twice.
const MAX_EXCERPT_COLS: usize = 80;
/// The widest the name column gets: a longer name is cut, so one note with a
/// long name cannot push every excerpt off the page.
const MAX_NAME_COLS: usize = 28;

/// An excerpt styled the way the editor would style it — bold as bold, a
/// wikilink as its label — and cut to `room` columns around the link at
/// `link` (a char span in `excerpt`), so the link itself is always on screen.
/// Take the folded sections out of a rendered page, before the footer goes
/// on: every row drawn from a hidden line goes, a folded heading gets the
/// `▸ ` marker in front and how many lines it hides at the right edge, the
/// way the editor draws one, and the blank rows either side of a section
/// that is gone collapse to one, so two headings end up spaced the way they
/// would be with nothing between them.
///
/// Rows are dropped rather than the source cut: the renderer never sees a
/// section boundary that is not there, and every line number the page keeps
/// still counts from the top of the file.
pub fn apply_folds(
    r: &mut Rendered,
    visible: &crate::fold::Visible,
    folded: &[usize],
    width: usize,
) {
    if visible.is_plain() {
        return;
    }
    let lines = std::mem::take(&mut r.lines);
    let mut out: Vec<PLine> = Vec::with_capacity(lines.len());
    for mut line in lines {
        match line.src_line {
            Some(l) if visible.is_hidden(l) => continue,
            Some(l) if folded.contains(&l) && !line.cells.is_empty() => {
                // the marker only on the heading's first row: a heading that
                // wrapped carries its line number on every row it took
                let first = out.last().is_none_or(|p| p.src_line != Some(l));
                if first {
                    mark_folded(&mut line, l, visible.hidden_under(l), width);
                }
            }
            // a spacer next to one the section took with it
            None if line.cells.is_empty() && out.last().is_some_and(|p| p.cells.is_empty()) => {
                continue
            }
            _ => {}
        }
        out.push(line);
    }
    // a section folded at the very end leaves the spacer that stood before it
    while out.last().is_some_and(|l| l.cells.is_empty()) {
        out.pop();
    }
    r.lines = out;
}

/// The `▸ ` in front of a folded heading, standing for source column 0 the
/// way it does in the editor, and the count at the right edge when the row
/// has room for it and a gap besides.
fn mark_folded(line: &mut PLine, src: usize, hidden: usize, width: usize) {
    let marker: Vec<PCell> = theme::FOLDED
        .chars()
        .map(|ch| PCell {
            ch,
            style: theme::fold(),
            link: None,
            src: Some((src, 0)),
        })
        .collect();
    line.cells.splice(0..0, marker);
    let label = match hidden {
        1 => "1 line folded".to_string(),
        n => format!("{n} lines folded"),
    };
    let used = cells_width(&line.cells);
    let need = crate::md::str_width(&label) + 2;
    if width == usize::MAX || used + need > width {
        return;
    }
    line.cells.extend(str_cells(
        &" ".repeat(width - used - need + 2),
        theme::PLAIN,
    ));
    line.cells.extend(str_cells(&label, theme::marker()));
}

/// The cells carry no link and no source position: the footer is not the
/// note, and a click on an excerpt has nowhere in the note to go.
fn excerpt_cells(excerpt: &str, link: (usize, usize), base: Style, room: usize) -> Vec<PCell> {
    let cells: Vec<PCell> = crate::md::style_inline(excerpt)
        .into_iter()
        .map(|c| PCell {
            ch: c.ch,
            style: base.patch(c.style),
            link: None,
            src: Some((0, c.src)),
        })
        .collect();
    if cells_width(&cells) <= room {
        return strip_src(cells);
    }
    // where the link landed once the brackets were hidden
    let first = cells
        .iter()
        .position(|c| c.src.is_some_and(|s| s.1 >= link.0));
    let last = cells
        .iter()
        .rposition(|c| c.src.is_some_and(|s| s.1 < link.1));
    let (Some(first), Some(last)) = (first, last) else {
        return strip_src(truncate_cells(&cells, room));
    };
    let before = cells_width(&cells[..first]);
    let linkw = cells_width(&cells[first..=last]);
    // the link fits from the start: cut from the right as any row would be
    if before + linkw < room {
        return strip_src(truncate_cells(&cells, room));
    }
    // otherwise open a window with the link a third of the way in, so what
    // was said before it is read as context and what came after as the point
    let lead = room.saturating_sub(linkw + 2) / 3;
    let mut skip = first;
    let mut skipped = 0;
    while skip > 0 && skipped + crate::md::char_width(cells[skip - 1].ch) <= lead {
        skip -= 1;
        skipped += crate::md::char_width(cells[skip].ch);
    }
    let mut out = str_cells("…", base);
    out.extend(truncate_cells(&cells[skip..], room.saturating_sub(1)));
    strip_src(out)
}

/// Forget the source columns the inline styler recorded: they were only ever
/// there to find the link, and a footer cell must not map into the note.
fn strip_src(mut cells: Vec<PCell>) -> Vec<PCell> {
    for c in &mut cells {
        c.src = None;
    }
    cells
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
    /// How many `▌ ` rails the line being built sits behind — one per
    /// enclosing plain blockquote.
    rails: usize,
    /// Inside a callout box (`> [!type]`). Only the outermost callout gets a
    /// box; a callout inside it is drawn as a rail.
    boxed: bool,
    /// Columns the open box spans, fixed when it opened: the page width is
    /// narrowed while a table is laid out, and the box must not follow it.
    box_w: usize,
    /// A quote has just opened and nothing has been drawn inside it yet, so a
    /// block asking for a blank line above itself does not get a rail-only row.
    quote_fresh: bool,
    /// The last row emitted was a rail-only (or box-only) blank row.
    quote_blank: bool,
    /// Columns the continuation rows of the line being built hang in under
    /// its marker — a list item wraps under its text, not under its bullet.
    hang: usize,
    list_depth: usize,
    /// One entry per open list: the next number an ordered list will give its
    /// item, or `None` for a bulleted one.
    list_numbers: Vec<Option<u64>>,
    in_code_block: bool,
    /// Inside a ```mermaid fence: the body accumulated so far, and the source
    /// byte offset it started at. The whole body is held back until the
    /// closing fence, because whether it is drawn at all is only known once
    /// there is a diagram to draw.
    mermaid: Option<(String, usize)>,
    table: Option<Table>,
    /// How a table wider than the page is drawn.
    tables: TableStyle,
    /// Page width in columns, used to size tables.
    width: usize,
    /// Byte offset of the start of each source line.
    line_starts: Vec<usize>,
    /// Source line the slice being rendered starts at in the file.
    first_line: usize,
    /// Source line for the line currently being built.
    src_line: Option<usize>,
    pending_checkbox: Option<usize>,
    /// A footnote's superscript label, waiting for its first paragraph.
    footnote: Option<String>,
    done_item: bool,
    image_alt: Option<(String, String)>,
    /// Byte offset the renderer has already drawn past. pulldown-cmark has
    /// never heard of a wikilink and hands `[[a|b]]` back as a run of separate
    /// text events, one per bracket: the first of them is where the whole span
    /// is recognised and drawn from the source, and the rest have to be
    /// swallowed rather than drawn a second time.
    wiki_until: usize,
}

impl Ren {
    fn new(markdown: &str, first_line: usize, width: usize, tables: TableStyle) -> Ren {
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
            rails: 0,
            boxed: false,
            box_w: 0,
            quote_fresh: false,
            quote_blank: false,
            hang: 0,
            list_depth: 0,
            list_numbers: Vec::new(),
            in_code_block: false,
            mermaid: None,
            table: None,
            tables,
            width,
            line_starts,
            first_line,
            src_line: None,
            pending_checkbox: None,
            footnote: None,
            done_item: false,
            image_alt: None,
            wiki_until: 0,
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

    /// Source byte offset → (line, column in chars), the line counted from the
    /// top of the *file*. `line_of` stays slice-relative on purpose: its
    /// result indexes `line_starts` and `src`, both of which are the slice's.
    fn pos_of(&self, offset: usize) -> (usize, usize) {
        let line = self.line_of(offset);
        let start = self.line_starts.get(line).copied().unwrap_or(0);
        let offset = offset.min(self.src.len());
        let col = self.src.get(start..offset).map_or(0, |s| s.chars().count());
        (self.first_line + line, col)
    }

    fn flush(&mut self) {
        if self.cells.is_empty() {
            return;
        }
        let cells = std::mem::take(&mut self.cells);
        let checkbox = self.pending_checkbox.take();
        let src_line = self.src_line;
        let hang = std::mem::take(&mut self.hang);
        self.emit_wrapped(cells, checkbox, None, src_line, hang);
    }

    /// Width the text of a line may use once the quote decoration around it
    /// — box edges and rails — has taken its share.
    fn inner_width(&self) -> usize {
        let taken = self.rails * 2 + if self.boxed { 4 } else { 0 };
        if self.width == usize::MAX {
            usize::MAX
        } else {
            self.width.saturating_sub(taken).max(8)
        }
    }

    /// Width a `---` rule is drawn at: the full width of the page, inside any
    /// quote decoration. A width no page has means the caller did not care.
    fn rule_width(&self) -> usize {
        match self.inner_width() {
            usize::MAX => 40,
            w => w,
        }
    }

    /// Width a callout box is drawn at. A width no page has means the caller
    /// did not care, so the box takes a comfortable default.
    fn box_width(&self) -> usize {
        if self.width == usize::MAX {
            80
        } else {
            self.width.max(8)
        }
    }

    /// Wrap a line to the room inside its quote decoration and emit each row
    /// with its rails and box edges. Wrapping happens here, not in the draw,
    /// so every row a quote takes gets its bar — the draw only sees rows that
    /// already fit the page.
    fn emit_wrapped(
        &mut self,
        cells: Vec<PCell>,
        checkbox: Option<usize>,
        image: Option<usize>,
        src_line: Option<usize>,
        hang: usize,
    ) {
        if self.rails == 0 && !self.boxed {
            self.emit_line(PLine {
                cells,
                checkbox,
                image,
                src_line,
                wide: false,
                hang,
            });
            return;
        }
        let avail = self.inner_width();
        let rest = avail.saturating_sub(hang).max(4);
        for (i, row) in wrap_hang(&cells, avail, rest).into_iter().enumerate() {
            let mut cells = if i == 0 {
                Vec::new()
            } else {
                str_cells(&" ".repeat(hang), theme::PLAIN)
            };
            cells.extend(row);
            self.emit_line(PLine {
                cells,
                checkbox: if i == 0 { checkbox } else { None },
                image: if i == 0 { image } else { None },
                src_line,
                wide: false,
                hang: 0,
            });
        }
    }

    /// Put one row on the page, behind whatever rails and box edges the row
    /// is inside. Every row the renderer makes goes through here, so a table
    /// or a code line inside a quote is decorated like a paragraph is. A wide
    /// (panning) row is left bare: its edges would pan off with it.
    fn emit_line(&mut self, mut line: PLine) {
        if !line.wide && (self.rails > 0 || self.boxed) {
            let mut cells = Vec::new();
            if self.boxed {
                cells.extend(str_cells("│ ", theme::state()));
            }
            for _ in 0..self.rails {
                cells.extend(str_cells(
                    &format!("{} ", theme::QUOTE_BAR),
                    theme::marker(),
                ));
            }
            cells.extend(line.cells);
            if self.boxed {
                let pad = self.box_w.saturating_sub(cells_width(&cells) + 2);
                cells.extend(str_cells(&" ".repeat(pad), theme::PLAIN));
                cells.extend(str_cells(" │", theme::state()));
            }
            line.cells = cells;
        }
        self.quote_fresh = false;
        self.quote_blank = false;
        self.out.lines.push(line);
    }

    fn blank(&mut self) {
        self.flush();
        if self.rails > 0 || self.boxed {
            // a rail-only row, once, and never as the first thing in a quote
            if self.quote_fresh || self.quote_blank {
                return;
            }
            self.emit_line(PLine::default());
            self.quote_blank = true;
            return;
        }
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

    /// Open a callout box: the title row, in the accent.
    fn open_box(&mut self, kind: &str, title: &str) {
        let w = self.box_width();
        self.box_w = w;
        let mut cells = str_cells("╭─ ", theme::state());
        if let Some(g) = crate::md::callout_glyph(kind) {
            cells.extend(str_cells(&format!("{g} "), theme::state()));
        }
        cells.extend(str_cells(kind, theme::state()));
        if !title.is_empty() {
            cells.extend(str_cells(" · ", theme::state()));
            cells.extend(str_cells(
                title,
                theme::state().add_modifier(Modifier::BOLD),
            ));
        }
        cells.push(PCell {
            ch: ' ',
            style: theme::state(),
            link: None,
            src: None,
        });
        let cells = truncate_cells(&cells, w.saturating_sub(1));
        let mut row = cells;
        let dashes = w.saturating_sub(cells_width(&row) + 1);
        row.extend(str_cells(&"─".repeat(dashes), theme::state()));
        row.extend(str_cells("╮", theme::state()));
        self.out.lines.push(PLine {
            cells: row,
            checkbox: None,
            image: None,
            src_line: self.src_line,
            wide: false,
            hang: 0,
        });
    }

    fn close_box(&mut self) {
        let w = self.box_w;
        let row = format!("╰{}╯", "─".repeat(w.saturating_sub(2)));
        self.out.lines.push(PLine {
            cells: str_cells(&row, theme::state()),
            checkbox: None,
            image: None,
            src_line: self.src_line,
            wide: false,
            hang: 0,
        });
    }

    fn indent(&self) -> String {
        "  ".repeat(self.list_depth.saturating_sub(1))
    }

    /// The `[[wikilink]]` starting at byte offset `off` of the source, as
    /// (byte offset just past it, target, label start, label end).
    ///
    /// It reads the source rather than the event's text because pulldown hands
    /// the brackets back one at a time — there is no single event to split
    /// around. The escape and embed guards are repeated here rather than left
    /// to `md::wikilink_at` because the char slice below starts at `off` and
    /// cannot see the character before it.
    ///
    /// One limitation worth knowing: inside a GFM table cell an unescaped `|`
    /// is the cell delimiter, so `[[note|label]]` is cut into two cells before
    /// the renderer ever sees it. Obsidian has the same problem and the same
    /// answer (`\|`), and escaping it splits the events so the whole thing
    /// stays literal. Plain and `#heading` wikilinks in a cell are fine.
    fn wikilink_here(&self, off: usize) -> Option<(usize, String, usize, usize)> {
        if !crate::md::links::enabled() || !self.src[off..].starts_with("[[") {
            return None;
        }
        let before = &self.src[..off];
        if before.ends_with('\\') || before.ends_with('!') {
            return None;
        }
        // bounded by the line, and only paid for when a `[[` is really there
        let line_end = self.src[off..]
            .find('\n')
            .map_or(self.src.len(), |n| off + n);
        let chars: Vec<char> = self.src[off..line_end].chars().collect();
        let w = crate::md::wikilink_at(&chars, 0)?;
        let byte_at = byte_offsets(&chars, off);
        Some((
            byte_at[w.end],
            w.target,
            byte_at[w.label_start],
            byte_at[w.label_end],
        ))
    }

    /// An Obsidian embed, `![[picture.png]]`, whose `!` is at byte offset
    /// `off` and which has its line to itself, as (alt, url, byte offset just
    /// past the end of the line). Everything pulldown goes on to emit for the
    /// line is skipped the way the rest of a wikilink is.
    fn embed_here(&self, off: usize) -> Option<(String, String, usize)> {
        if !crate::md::links::enabled() || !self.src[off..].starts_with("![[") {
            return None;
        }
        let line_start = self.src[..off].rfind('\n').map_or(0, |n| n + 1);
        let line_end = self.src[off..]
            .find('\n')
            .map_or(self.src.len(), |n| off + n);
        let line = &self.src[line_start..line_end];
        if line.trim_start().len() != line_end - off {
            return None; // the embed is not the first thing on its line
        }
        let (alt, url) = crate::md::embed_line(line)?;
        Some((alt, url, line_end))
    }

    /// One picture on a line of its own: the `🖼 alt (url)` label that stands
    /// in for it, tagged with the image it stands for.
    fn emit_image(&mut self, alt: String, url: String) {
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
        let src_line = self.src_line;
        self.emit_wrapped(cells, None, Some(idx), src_line, 0);
    }

    /// Text from the document: scan for `==highlight==` and bare URLs.
    /// `off` is the source byte offset of `text`, when it is a verbatim slice.
    fn emit_text(&mut self, text: &str, off: Option<usize>) {
        let base = self.style();
        let link = self.link;
        let chars: Vec<char> = text.chars().collect();
        // byte offset of each char, so every run knows where it started
        let byte_at = byte_offsets(&chars, 0);
        let at = |i: usize| off.map(|o| o + byte_at[i]);

        let mut i = 0;
        let mut run = String::new();
        let mut run_start = 0usize;
        while i < chars.len() {
            // ==highlight==
            if chars[i] == '=' && chars.get(i + 1) == Some(&'=') {
                if let Some(end) = crate::md::find_pair(&chars, i + 2, '=') {
                    self.push_at(&std::mem::take(&mut run), base, link, at(run_start));
                    let body: String = chars[i + 2..end].iter().collect();
                    self.push_at(&body, base.patch(theme::highlight()), link, at(i + 2));
                    i = end + 2;
                    run_start = i;
                    continue;
                }
            }
            // bare URL, when not already inside a link
            if let Some(end) = crate::md::url_at(&chars, i).filter(|_| link.is_none()) {
                let url: String = chars[i..end].iter().collect();
                self.push_at(&std::mem::take(&mut run), base, None, at(run_start));
                let idx = self.out.urls.len();
                self.out
                    .urls
                    .push(crate::md::LinkTarget::Url(url.clone()).href());
                self.push_at(&url, base.patch(theme::link()), Some(idx), at(i));
                i = end;
                run_start = i;
                continue;
            }
            // #tag, when not inside a link. The first char of an event has no
            // char before it in `chars`, so the boundary is read off the
            // source: pulldown splits `x#y` and `` `x`#y `` into events that
            // both start at the `#`, and only the source tells them apart
            if link.is_none() && chars[i] == '#' && crate::md::tags::enabled() {
                let prev = match i {
                    0 => at(0).and_then(|o| self.src[..o].chars().next_back()),
                    _ => Some(chars[i - 1]),
                };
                if crate::md::tag_boundary(prev) {
                    if let Some(end) = crate::md::tag_at(&chars, i) {
                        let name: String = chars[i + 1..end].iter().collect();
                        self.push_at(&std::mem::take(&mut run), base, None, at(run_start));
                        let idx = self.out.urls.len();
                        self.out.urls.push(crate::md::LinkTarget::Tag(name).href());
                        let shown: String = chars[i..end].iter().collect();
                        self.push_at(&shown, base.patch(theme::tag()), Some(idx), at(i));
                        i = end;
                        run_start = i;
                        continue;
                    }
                }
            }
            if run.is_empty() {
                run_start = i;
            }
            run.push(chars[i]);
            i += 1;
        }
        self.push_at(&run, base, link, at(run_start));
    }

    /// A ```mermaid fence: the picture when catcher can draw one, and the
    /// source under a label saying what it is when it cannot. Both answers
    /// are honest — a diagram kind we have never heard of degrades to exactly
    /// what a fence looked like yesterday, with a word about why.
    fn emit_mermaid(&mut self, src: &str, off: usize) {
        match crate::mermaid::render(src, self.inner_width()) {
            Some(d) => self.emit_diagram(&d),
            None => self.emit_fence_label(src, off),
        }
    }

    /// Put a drawn diagram on the page, one row per row.
    ///
    /// Split out from `emit_mermaid` so it can be driven by a diagram built by
    /// hand: what the reading view owns here is the styling, the `wide` flag
    /// and the decoration `emit_line` adds, none of which care what drew the
    /// rows. A row wider than the page is marked `wide` and the page pans
    /// across it exactly as it pans a wide table.
    fn emit_diagram(&mut self, d: &crate::mermaid::Rendered) {
        for row in &d.rows {
            let mut cells: Vec<PCell> = Vec::new();
            for run in row {
                cells.extend(str_cells(&run.text, crate::md::mermaid_style(run.role)));
            }
            let wide = cells_width(&cells) > self.width;
            self.emit_line(PLine {
                cells,
                checkbox: None,
                image: None,
                src_line: self.src_line,
                wide,
                hang: 0,
            });
        }
    }

    /// The fallback: a label naming the diagram kind, then the fence's own
    /// source drawn exactly as a code block is — same indent, same colour and,
    /// above all, the same source offsets. Keeping the offsets is what leaves
    /// a click in a diagram catcher could not draw landing on the character it
    /// was aimed at.
    ///
    /// A label and not a box: the callout card is the app's one boxed
    /// construct, and a fence that could not be drawn has no business
    /// competing with it. The marker colour keeps it chrome — never the
    /// accent, which the note spends on its headings.
    fn emit_fence_label(&mut self, src: &str, off: usize) {
        let label = match crate::mermaid::kind_word(src) {
            Some(word) => format!("◇ mermaid · {word}"),
            None => "◇ mermaid".to_string(),
        };
        self.push(&label, theme::marker(), None);
        self.flush();
        let mut off = off;
        // split_inclusive, not lines(), for the same reason the code-block arm
        // uses it: the line ending is counted as it is in the file
        for raw in src.split_inclusive('\n') {
            let l = raw.trim_end_matches('\n').trim_end_matches('\r');
            self.push("  ", theme::code(), None);
            self.push_at(l, theme::code(), None, Some(off));
            off += raw.len();
            let cells = std::mem::take(&mut self.cells);
            let src_line = self.src_line;
            self.emit_wrapped(cells, None, None, src_line, 2);
        }
    }

    fn run(&mut self, markdown: &str) {
        for (event, range) in Parser::new_ext(markdown, options()).into_offset_iter() {
            // file-absolute, like `pos_of`: this is the number a preview click
            // and a checkbox toggle both index the buffer with
            let src_line = self.first_line + self.line_of(range.start);
            if self.cells.is_empty() && matches!(self.sink, Sink::Page) {
                self.src_line = Some(src_line);
            }
            self.event(event, src_line, range);
        }
        self.flush();
    }

    fn event(&mut self, event: Event<'_>, src_line: usize, range: std::ops::Range<usize>) {
        // a `[[wikilink]]` is drawn whole, from its own source, the moment the
        // first event inside it arrives; pulldown then goes on walking what is
        // left of the span one event at a time. Every one of those would draw
        // a second time — the leftover `]]` as text, but also an inline `` `x` ``
        // between the brackets as code, which `md::wikilink_at` allows inside a
        // target and which the live editor draws as part of the label. So the
        // whole span is skipped, not just its text.
        if range.start < self.wiki_until && emits_cells(&event) {
            return;
        }
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                self.blank();
                self.src_line = Some(src_line);
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
                // the first paragraph of a footnote carries its number
                if let Some(mark) = self.footnote.take() {
                    self.push(&format!("{mark} "), theme::state(), None);
                }
            }
            Event::End(TagEnd::Paragraph) => self.flush(),
            Event::Start(Tag::BlockQuote(_)) => {
                self.blank();
                self.src_line = Some(src_line);
                match callout_at(&self.src, range.start) {
                    Some((kind, title, end)) if !self.boxed => {
                        self.open_box(&kind, &title);
                        self.boxed = true;
                        // the `[!type] Title` line is the box's title, not
                        // its first paragraph: nothing in it is drawn again
                        self.wiki_until = self.wiki_until.max(end);
                    }
                    Some((_, _, end)) => {
                        self.rails += 1;
                        self.wiki_until = self.wiki_until.max(end);
                    }
                    None => self.rails += 1,
                }
                self.quote_fresh = true;
                self.styles.push(theme::quote());
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                self.styles.pop();
                self.flush();
                if self.rails > 0 {
                    self.rails -= 1;
                } else if self.boxed {
                    self.boxed = false;
                    self.close_box();
                }
                self.quote_fresh = false;
                self.quote_blank = false;
            }
            Event::Start(Tag::List(start)) => {
                if self.list_depth == 0 {
                    self.blank();
                }
                self.list_depth += 1;
                self.list_numbers.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                self.list_depth = self.list_depth.saturating_sub(1);
                self.list_numbers.pop();
                self.flush();
            }
            Event::Start(Tag::Item) => {
                self.flush();
                self.src_line = Some(src_line);
                // an ordered list keeps its numbers, as the file wrote them
                let marker = match self.list_numbers.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}.");
                        *n += 1;
                        m
                    }
                    _ => theme::BULLET.to_string(),
                };
                let text = format!("{}{marker} ", self.indent());
                self.hang = crate::md::str_width(&text);
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
                self.hang = crate::md::str_width(&text);
                self.push(&text, style, None);
                self.pending_checkbox = Some(src_line);
                if done {
                    // done items read as struck-through and dim until the item ends
                    self.styles.push(self.style().patch(theme::done_text()));
                    self.done_item = true;
                }
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                self.blank();
                self.src_line = Some(src_line);
                self.in_code_block = true;
                if matches!(&kind, CodeBlockKind::Fenced(info) if crate::mermaid::is_mermaid(info))
                {
                    self.mermaid = Some((String::new(), 0));
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((src, off)) = self.mermaid.take() {
                    self.emit_mermaid(&src, off);
                }
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
                // through `LinkTarget`, not straight in: a href written in the
                // note is a stranger's text, and `[x](note:/etc/passwd)` must
                // not arrive at the other end looking like a file the app
                // found for itself
                self.out
                    .urls
                    .push(crate::md::LinkTarget::Url(dest_url.into_string()).href());
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
                    self.emit_image(alt, url);
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
                let style = self.style().patch(theme::inline_code());
                let link = self.link;
                // the range spans the backticks too; the content starts after them
                let ticks = self.src[range.clone()]
                    .chars()
                    .take_while(|c| *c == '`')
                    .count();
                self.push_at(&code.into_string(), style, link, Some(range.start + ticks));
            }
            Event::InlineMath(text) => {
                let style = self.style().patch(theme::math());
                let link = self.link;
                self.push_at(&text, style, link, Some(range.start + 1));
            }
            Event::DisplayMath(text) => {
                // a displayed formula sits on rows of its own, centred
                self.blank();
                self.src_line = Some(src_line);
                let width = self.rule_width();
                for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
                    let w = crate::md::str_width(line);
                    self.push(&" ".repeat(width.saturating_sub(w) / 2), theme::PLAIN, None);
                    self.push(line, theme::math(), None);
                    self.flush();
                }
            }
            Event::FootnoteReference(label) => {
                self.push(&crate::md::superscript(&label), theme::state(), None);
            }
            Event::Start(Tag::FootnoteDefinition(label)) => {
                self.blank();
                self.src_line = Some(src_line);
                self.footnote = Some(crate::md::superscript(&label));
            }
            Event::End(TagEnd::FootnoteDefinition) => {
                self.footnote = None;
                self.flush();
            }
            Event::Text(text) => {
                if let Some((alt, _)) = self.image_alt.as_mut() {
                    alt.push_str(&text);
                } else if let Some((buf, off)) = self.mermaid.as_mut() {
                    // held back, not drawn: the fence is a diagram until the
                    // close proves otherwise, and a diagram is drawn whole
                    if buf.is_empty() {
                        *off = range.start;
                    }
                    buf.push_str(&text);
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
                        let src_line = self.src_line;
                        self.emit_wrapped(cells, None, None, src_line, 2);
                    }
                } else if let Some((alt, url, end)) = self.embed_here(range.start) {
                    // `![[picture.png]]` on a line of its own is a picture,
                    // the same as `![](picture.png)`; pulldown sees only text
                    self.emit_image(alt, url);
                    self.wiki_until = end;
                } else if let Some((end, target, ls, le)) = self.wikilink_here(range.start) {
                    // the label keeps its own source bytes, so `push_at` gives
                    // every cell its true (line, column) and a preview click
                    // lands inside the link rather than at the start of it
                    let label = self.src[ls..le].to_string();
                    let idx = self.out.urls.len();
                    self.out
                        .urls
                        .push(crate::md::LinkTarget::Wiki(target.clone()).href());
                    let style = crate::md::wiki_style(self.style(), &target);
                    self.push_at(&label, style, Some(idx), Some(ls));
                    self.wiki_until = end;
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
                }
            }
            Event::HardBreak => self.flush(),
            Event::Rule => {
                self.blank();
                self.push(&"─".repeat(self.rule_width()), theme::marker(), None);
                self.flush();
            }
            _ => {}
        }
    }

    /// Lay out the buffered table. Three shapes, because one shape cannot
    /// serve a two-column table and an eight-column one on the same page:
    /// a grid, a grid whose cells wrap, or one labelled block per row.
    fn emit_table(&mut self) {
        let Some(t) = self.table.take() else { return };
        if t.rows.is_empty() {
            return;
        }
        // inside a quote the table has only the room its rails leave it
        let page = self.width;
        self.width = self.inner_width();
        self.emit_table_in(&t);
        self.width = page;
    }

    fn emit_table_in(&mut self, t: &Table) {
        let cols = t.rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let measured: Vec<Vec<usize>> = t
            .rows
            .iter()
            .map(|r| r.iter().map(|c| cells_width(c)).collect())
            .collect();
        let natural = crate::md::column_widths(&measured, cols);
        let seps = crate::md::COL_SEP.chars().count() * cols.saturating_sub(1);
        let fits = natural.iter().sum::<usize>() + seps <= self.width;

        match self.table_shape(cols, seps, fits) {
            Shape::Grid { wrap } => {
                let widths = crate::md::fit_widths(&natural, self.width);
                self.emit_grid(t, cols, &widths, wrap, false);
            }
            Shape::Scroll => {
                let widths = self.scroll_widths(&natural);
                self.emit_grid(t, cols, &widths, true, true);
            }
            Shape::Cards => self.emit_cards(t, cols),
        }
    }

    /// Which shape this table gets. `auto` keeps the grid while its columns are
    /// still wide enough to read a phrase in, and gives up on it — rather than
    /// shaving every column to a stub and an ellipsis — once they are not.
    fn table_shape(&self, cols: usize, seps: usize, fits: bool) -> Shape {
        // below this the columns hit their floor and the grid runs off the
        // page whatever it is told to do, so cards are the only shape left
        let grid_possible = self.width >= cols * crate::md::MIN_COL + seps;
        match self.tables {
            TableStyle::Fit => Shape::Grid { wrap: false },
            TableStyle::Wrap if grid_possible => Shape::Grid { wrap: !fits },
            TableStyle::Wrap => Shape::Cards,
            TableStyle::Cards => Shape::Cards,
            TableStyle::Scroll => Shape::Scroll,
            // a table that already fits is left exactly as it was; one that
            // does not keeps its columns readable and pans instead
            TableStyle::Auto if fits => Shape::Grid { wrap: false },
            TableStyle::Auto => Shape::Scroll,
        }
    }

    /// Column widths for a scrolling table: each column as wide as its widest
    /// cell, capped so a single long URL cannot push every other column off
    /// the far side. The cap is a share of the page, not a fixed number, so it
    /// scales with the window the way Obsidian's does.
    fn scroll_widths(&self, natural: &[usize]) -> Vec<usize> {
        /// Narrowest a column is ever capped to, and the share of the page a
        /// single column may claim before it starts wrapping.
        const FLOOR: usize = 12;
        let cap = (self.width / 3).clamp(FLOOR, 44);
        natural.iter().map(|w| (*w).min(cap).max(1)).collect()
    }

    /// Aligned columns with a light rule under the head. `wrap` lets a cell
    /// that does not fit run onto further lines instead of being cut.
    fn emit_grid(&mut self, t: &Table, cols: usize, widths: &[usize], wrap: bool, wide: bool) {
        let src_line = self.src_line;
        for (ri, row) in t.rows.iter().enumerate() {
            let empty: Vec<PCell> = Vec::new();
            // every cell, already broken into the lines it will occupy
            let parts: Vec<Vec<Vec<PCell>>> = (0..cols)
                .map(|ci| {
                    let cell = row.get(ci).unwrap_or(&empty);
                    let w = widths.get(ci).copied().unwrap_or(0);
                    if wrap {
                        wrap_pcells(cell, w.max(1))
                    } else {
                        vec![truncate_cells(cell, w)]
                    }
                })
                .collect();
            let height = parts.iter().map(|p| p.len()).max().unwrap_or(1);
            for line in 0..height {
                let mut cells: Vec<PCell> = Vec::new();
                for (ci, w) in widths.iter().enumerate().take(cols) {
                    if ci > 0 {
                        cells.extend(str_cells(crate::md::COL_SEP, theme::marker()));
                    }
                    let blank: Vec<PCell> = Vec::new();
                    let part = parts[ci].get(line).unwrap_or(&blank);
                    let align = align_of(t.aligns.get(ci).copied().unwrap_or(Alignment::None));
                    let (left, right) = crate::md::pad_for(cells_width(part), *w, align);
                    cells.extend(str_cells(&" ".repeat(left), theme::PLAIN));
                    cells.extend(part.iter().cloned());
                    cells.extend(str_cells(&" ".repeat(right), theme::PLAIN));
                }
                self.emit_line(PLine {
                    cells,
                    checkbox: None,
                    image: None,
                    src_line,
                    wide,
                    hang: 0,
                });
            }
            // under the head, and between every pair of body rows
            let last = ri + 1 == t.rows.len();
            if ri == 0 || !last {
                let rule = crate::md::table_rule(widths);
                self.emit_line(PLine {
                    cells: str_cells(&rule, theme::marker()),
                    checkbox: None,
                    image: None,
                    src_line,
                    wide,
                    hang: 0,
                });
            }
        }
    }

    /// One block per row: the row's first cells as a heading, then every other
    /// column as `label  value` under it. Nothing is truncated, so a table
    /// twenty columns wide is still readable on an eighty-column terminal —
    /// it is simply taller.
    fn emit_cards(&mut self, t: &Table, cols: usize) {
        let src_line = self.src_line;
        let empty: Vec<PCell> = Vec::new();
        let head: Vec<String> = (0..cols)
            .map(|ci| {
                t.rows
                    .first()
                    .and_then(|r| r.get(ci))
                    .map(|c| c.iter().map(|p| p.ch).collect::<String>())
                    .unwrap_or_default()
                    .trim()
                    .to_string()
            })
            .collect();
        // the label column is as wide as the widest heading, so the values
        // line up down the whole table and can be read as a column
        let labelw = head
            .iter()
            .skip(1)
            .map(|h| crate::md::str_width(h))
            .max()
            .unwrap_or(0);

        let mut made: Vec<Vec<PCell>> = Vec::new();
        let push = |cells: Vec<PCell>, made: &mut Vec<Vec<PCell>>| made.push(cells);

        for (ri, row) in t.rows.iter().enumerate().skip(1) {
            if ri > 1 {
                push(Vec::new(), &mut made);
            }
            // the heading: the first column, which is nearly always the row's
            // name or date, marked with the same bar a blockquote uses
            let mut title = str_cells(&format!("{} ", theme::QUOTE_BAR), theme::state());
            let first = truncate_cells(row.first().unwrap_or(&empty), self.width.saturating_sub(2));
            title.extend(first.iter().map(|c| {
                let mut c = c.clone();
                c.style = c.style.patch(theme::heading(3)).fg(theme::palette().accent);
                c
            }));
            push(title, &mut made);

            for ci in 1..cols {
                let value = row.get(ci).unwrap_or(&empty);
                // an empty cell says nothing worth a line of its own
                if value.iter().all(|c| c.ch.is_whitespace()) {
                    continue;
                }
                let label = head.get(ci).cloned().unwrap_or_default();
                let pad = labelw.saturating_sub(crate::md::str_width(&label));
                let indent = 2 + labelw + 2;
                let avail = self.width.saturating_sub(indent).max(8);
                for (i, part) in wrap_pcells(value, avail).into_iter().enumerate() {
                    let mut cells = if i == 0 {
                        let mut c = str_cells("  ", theme::PLAIN);
                        c.extend(str_cells(&label, theme::marker()));
                        c.extend(str_cells(&" ".repeat(pad + 2), theme::PLAIN));
                        c
                    } else {
                        // continuation lines hang under the value, not the label
                        str_cells(&" ".repeat(indent), theme::PLAIN)
                    };
                    cells.extend(part);
                    push(cells, &mut made);
                }
            }
        }
        for cells in made {
            self.emit_line(PLine {
                cells,
                checkbox: None,
                image: None,
                src_line,
                wide: false,
                hang: 0,
            });
        }
    }

    fn finish(mut self) -> Rendered {
        self.flush();
        self.out
    }
}

/// The three ways a table can be laid out, once `auto` has made up its mind.
enum Shape {
    Grid {
        wrap: bool,
    },
    /// Natural column widths, capped so no one column runs away with the
    /// table, and the page pans across whatever that adds up to.
    Scroll,
    Cards,
}

/// Word-wrap a run of rendered cells into rows no wider than `width` display
/// columns. Shared by the preview's own soft wrap and by table cells, so a
/// wrapped cell breaks where a wrapped paragraph would.
pub fn wrap_pcells(cells: &[PCell], width: usize) -> Vec<Vec<PCell>> {
    if width == 0 {
        return vec![cells.to_vec()];
    }
    wrap_hang(cells, width, width)
}

/// Word-wrap like [`wrap_pcells`], but with `first` columns for the first row
/// and `rest` for every row after it — the room a hanging indent leaves.
fn wrap_hang(cells: &[PCell], first: usize, rest: usize) -> Vec<Vec<PCell>> {
    if cells_width(cells) <= first {
        return vec![cells.to_vec()];
    }
    let chars: Vec<char> = cells.iter().map(|c| c.ch).collect();
    crate::md::wrap_breaks(&chars, first, rest)
        .into_iter()
        .map(|(s, e)| cells[s..e].to_vec())
        .collect()
}

/// The Obsidian callout a blockquote starting at byte `start` opens with, if
/// any: `> [!type] Title` (a `-`/`+` fold marker after the type is ignored).
/// Returns (type, title, byte offset of the line ending), the offset so the
/// caller can skip everything the parser hands back from that line.
fn callout_at(src: &str, start: usize) -> Option<(String, String, usize)> {
    let rest = src.get(start..)?;
    let line_end = rest.find('\n').map_or(rest.len(), |i| i + 1);
    let line = &rest[..line_end];
    let body = line.trim_start_matches(['>', ' ', '\t']);
    let inner = body.strip_prefix("[!")?;
    let close = inner.find(']')?;
    let kind = inner[..close].trim();
    if kind.is_empty() || kind.chars().any(|c| c.is_whitespace()) {
        return None;
    }
    let after = inner[close + 1..].trim_start_matches(['-', '+']);
    let title = after.trim().to_string();
    Some((kind.to_lowercase(), title, start + line_end))
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
    let n = crate::md::cut_at(cells.iter().map(|c| crate::md::char_width(c.ch)), width);
    let mut out = cells[..n].to_vec();
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

/// Does this event put something on the page at a source offset of its own?
///
/// Only these can be dropped inside a span that has already been drawn.
/// Structural events are deliberately not in the list: their ranges cover the
/// whole construct they open, so skipping one that happens to start inside a
/// wikilink would leave the style stack unbalanced for the rest of the page.
fn emits_cells(event: &Event<'_>) -> bool {
    matches!(
        event,
        Event::Text(_)
            | Event::Code(_)
            | Event::Html(_)
            | Event::InlineHtml(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_)
            | Event::FootnoteReference(_)
    )
}

/// The byte offset, from `base`, at which each char of `chars` starts, plus
/// one past the last — so a run over chars can name where it sat in the source.
fn byte_offsets(chars: &[char], base: usize) -> Vec<usize> {
    let mut v = Vec::with_capacity(chars.len() + 1);
    let mut b = base;
    for ch in chars {
        v.push(b);
        b += ch.len_utf8();
    }
    v.push(b);
    v
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

    /// The whole path, from a fence to a picture on the page.
    #[test]
    fn a_flowchart_fence_reaches_the_page_as_rows_and_not_as_code() {
        let md = "```mermaid\nflowchart LR\n  A[Start] --> B[End]\n```\n";
        let flat = flat(&render_wide(md, 60));
        assert!(!flat.contains("◇ mermaid"), "{flat}");
        assert!(flat.contains("Start") && flat.contains("End"), "{flat}");
        assert!(flat.contains('╭'), "{flat}");
    }

    /// A diagram built by hand, drawn onto a page of `width` columns.
    ///
    /// The flow and sequence builders are their own piece of work; the reading
    /// view's share is the styling, the `wide` flag and the decoration a quote
    /// or a callout puts around a row, and none of the three care what drew the
    /// rows. `boxed` puts the page inside a callout card.
    fn page_of(d: &crate::mermaid::Rendered, width: usize, boxed: bool) -> Rendered {
        let mut r = Ren::new("", 0, width, TableStyle::default());
        if boxed {
            r.boxed = true;
            r.box_w = width;
        }
        r.emit_diagram(d);
        r.finish()
    }

    #[test]
    fn a_mermaid_fence_is_drawn_as_a_picture_in_the_reading_view() {
        use crate::mermaid::{Rendered as Diagram, Role, Run};
        let d = Diagram::new(vec![
            vec![Run::new("╭───────╮", Role::Line)],
            vec![
                Run::new("│ ", Role::Line),
                Run::new("Start", Role::Node),
                Run::new(" │", Role::Line),
            ],
        ]);
        let page = page_of(&d, 40, false);
        assert_eq!(flat(&page), "╭───────╮\n│ Start │");
        // the palette, through the one mapping both views share: the words the
        // author wrote in the body colour, the scaffolding in the marker one
        let start = page.lines[1].cells.iter().find(|c| c.ch == 'S').unwrap();
        assert_eq!(start.style, theme::PLAIN);
        assert_eq!(page.lines[0].cells[0].style, theme::marker());
        // and never the accent, which the note spends on its headings
        assert!(page
            .lines
            .iter()
            .all(|l| l.cells.iter().all(|c| c.style != theme::state())));
        assert!(page.lines.iter().all(|l| !l.wide));
    }

    #[test]
    fn a_mermaid_fence_catcher_cannot_draw_keeps_its_source_under_a_label() {
        let r = render_wide("```mermaid\ngantt\n  title Ship it\n```\n", 40);
        let flat = flat(&r);
        assert!(flat.contains("◇ mermaid"), "{flat}");
        // the source is still there, indented the way any code block is
        assert!(flat.contains("  gantt"), "{flat}");
        assert!(flat.contains("    title Ship it"), "{flat}");
        // a label, not a card: the callout box is the app's one boxed
        // construct, and the label is chrome rather than accent
        assert!(!flat.contains('╭'));
        let label = r.lines.iter().find(|l| l.text().contains('◇')).unwrap();
        assert!(label.cells.iter().all(|c| c.style == theme::marker()));
    }

    #[test]
    fn the_label_names_the_diagram_kind_and_not_just_mermaid() {
        assert!(flat(&render_wide("```mermaid\ngantt\n```\n", 40)).contains("◇ mermaid · gantt"));
        // a comment above the header does not hide what the diagram is
        let commented = "```mermaid\n%% mine\nclassDiagram\n```\n";
        assert!(flat(&render_wide(commented, 40)).contains("◇ mermaid · classDiagram"));
        // and a fence with nothing in it to name says only what it is
        let empty = flat(&render_wide("```mermaid\n\n```\n", 40));
        assert!(empty.contains("◇ mermaid"));
        assert!(!empty.contains('·'));
    }

    #[test]
    fn a_diagram_wider_than_the_page_is_marked_wide_so_the_page_pans() {
        use crate::mermaid::{Rendered as Diagram, Role, Run};
        let d = Diagram::new(vec![
            vec![Run::new("─".repeat(60), Role::Line)],
            vec![Run::new("short", Role::Node)],
        ]);
        let page = page_of(&d, 20, false);
        // nothing is cut and nothing is wrapped: the row is left whole and the
        // page pans across it, exactly as it does for a wide table
        assert!(page.lines[0].wide);
        assert_eq!(cells_width(&page.lines[0].cells), 60);
        assert!(!page.lines[1].wide);
    }

    #[test]
    fn a_diagram_inside_a_callout_keeps_its_rail() {
        use crate::mermaid::{Rendered as Diagram, Role, Run};
        let d = Diagram::new(vec![vec![Run::new("A ─▶ B", Role::Line)]]);
        let page = page_of(&d, 20, true);
        assert_eq!(flat(&page), "│ A ─▶ B           │");
        // a wide row is left bare instead: its edges would pan off with it
        let wide = Diagram::new(vec![vec![Run::new("─".repeat(40), Role::Line)]]);
        assert_eq!(flat(&page_of(&wide, 20, true)), "─".repeat(40));
    }

    #[test]
    fn a_fence_that_only_looks_like_mermaid_is_still_code() {
        let flat = flat(&render_wide("```mermaidjs\ngraph TD\n```\n", 40));
        assert!(!flat.contains('◇'), "{flat}");
        assert!(flat.contains("  graph TD"), "{flat}");
    }

    #[test]
    fn an_undrawn_diagram_keeps_the_source_columns_a_click_needs() {
        let md = "# T\n\n```mermaid\ngantt\n  title Ship it\n```\n";
        let src_lines: Vec<&str> = md.lines().collect();
        for line in &render_wide(md, 40).lines {
            for c in &line.cells {
                // every mapped cell still points at its own character, so a
                // click in a diagram we did not draw lands where it was aimed
                if let Some((l, col)) = c.src {
                    assert_eq!(src_lines[l].chars().nth(col), Some(c.ch), "at ({l},{col})");
                }
            }
        }
    }

    const WIDE: &str = "| a | bbbbbbbbbbbbbbbbbbbb |\n| --- | --- |\n| 1 | 2 |\n";

    /// The table this whole feature exists for: eight columns of real content.
    const JOB_LOG: &str = concat!(
        "| date | company | title | comp | location | path | doc | status |\n",
        "|---|---|---|---|---|---|---|---|\n",
        "| 2026-08-25 | Harrison Consulting | Director of Product | $220K/yr | ",
        "Seattle, WA | LinkedIn Easy Apply | doc | applied |\n",
    );

    #[test]
    fn a_wide_table_is_squeezed_into_the_page_width() {
        let r = render_page(WIDE, 16, TableStyle::Fit);
        for l in &r.lines {
            assert!(cells_width(&l.cells) <= 16);
        }
        let head: String = r.lines[0].cells.iter().map(|c| c.ch).collect();
        assert_eq!(head, "a │ bbbbbbbbbbb…");
    }

    #[test]
    fn a_line_is_either_inside_the_page_or_marked_wide() {
        // the whole contract in one assertion: a shape either fits the page,
        // or says it does not so the view pans across it instead of wrapping
        for width in [24usize, 40, 80, 100] {
            for style in [
                TableStyle::Auto,
                TableStyle::Scroll,
                TableStyle::Wrap,
                TableStyle::Cards,
            ] {
                for l in &render_page(JOB_LOG, width, style).lines {
                    assert!(
                        l.wide || cells_width(&l.cells) <= width,
                        "{style:?} at {width}: {:?}",
                        l.text()
                    );
                }
            }
        }
    }

    #[test]
    fn a_scrolling_table_keeps_its_columns_and_cuts_nothing() {
        let r = render_page(JOB_LOG, 60, TableStyle::Scroll);
        let table: Vec<&PLine> = r.lines.iter().filter(|l| l.wide).collect();
        assert!(!table.is_empty());
        let text: String = table.iter().map(|l| l.text()).collect();
        // no column was shaved down to an ellipsis
        assert!(!text.contains('…'), "{text}");
        // and the words are whole, not broken across a nine-column cell
        assert!(text.contains("Harrison"), "{text}");
        assert!(text.contains("applied"), "{text}");
        // the table is genuinely wider than the page — that is the point
        assert!(table.iter().any(|l| cells_width(&l.cells) > 60));
        // every row of it is the same width, so the columns line up while it pans
        let widths: Vec<usize> = table.iter().map(|l| cells_width(&l.cells)).collect();
        assert!(widths.windows(2).all(|w| w[0] == w[1]), "{widths:?}");
    }

    #[test]
    fn one_runaway_column_cannot_push_the_others_off_the_far_side() {
        let md = concat!(
            "| a | b |\n|---|---|\n",
            "| short | https://example.com/an/extremely/long/url/that/goes/on/and/on/forever |\n",
        );
        let r = render_page(md, 60, TableStyle::Scroll);
        // capped at a third of the page, so the long cell wraps rather than
        // making the table hundreds of columns wide
        for l in r.lines.iter().filter(|l| l.wide) {
            assert!(cells_width(&l.cells) <= 60 + 60 / 3, "{:?}", l.text());
        }
    }

    #[test]
    fn wrapping_keeps_every_word_a_squeezed_grid_would_have_cut() {
        let r = render_page(WIDE, 16, TableStyle::Wrap);
        let text: String = r.lines.iter().map(|l| l.text()).collect();
        assert!(text.contains("bbbbbbbb"), "{text:?}");
        // nothing was cut, so no ellipsis was needed
        assert!(!text.contains('…'), "{text:?}");
    }

    #[test]
    fn cards_label_every_value_and_truncate_nothing() {
        let md = concat!(
            "| date | company | status |\n|---|---|---|\n",
            "| 2026-08-25 | Harrison Consulting | applied |\n",
        );
        let r = render_page(md, 30, TableStyle::Cards);
        let text: String = r.lines.iter().map(|l| format!("{}\n", l.text())).collect();
        // the first column heads the block; the rest are labelled under it
        assert!(text.contains("2026-08-25"), "{text}");
        assert!(text.contains("company"), "{text}");
        assert!(text.contains("Harrison Consulting"), "{text}");
        assert!(text.contains("status"), "{text}");
        assert!(text.contains("applied"), "{text}");
        // the header row is the labels, never a card of its own
        assert!(!text.contains("▌ date"), "{text}");
        assert!(!text.contains('…'), "{text}");
    }

    #[test]
    fn auto_leaves_a_table_that_fits_alone_and_scrolls_one_that_does_not() {
        // two roomy columns: still a grid, with the head rule under it
        let narrow = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        let grid: String = render_page(narrow, 80, TableStyle::Auto)
            .lines
            .iter()
            .map(|l| format!("{}\n", l.text()))
            .collect();
        assert!(grid.contains('┼'), "{grid}");
        assert!(!grid.contains('▌'), "{grid}");

        // one that does not fit keeps its columns and pans instead
        let r = render_page(JOB_LOG, 60, TableStyle::Auto);
        assert!(r.lines.iter().any(|l| l.wide));
        let text: String = r.lines.iter().map(|l| l.text()).collect();
        assert!(!text.contains('…'), "{text}");
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
    fn ordered_lists_keep_their_numbers() {
        let r = render("3. three\n4. four\n   - sub\n5. five\n\n- plain\n");
        let f = flat(&r);
        assert!(f.contains("3. three\n4. four\n  • sub\n5. five"), "{f}");
        assert!(f.contains("• plain"), "{f}");
    }

    #[test]
    fn wrapped_list_item_hangs_under_its_text() {
        let r = render_wide("1. alpha beta gamma delta epsilon zeta\n", 20);
        let item = r.lines.iter().find(|l| l.text().starts_with("1. ")).unwrap();
        assert_eq!(item.hang, 3);
        let rows: Vec<String> = wrap_pline(item, 20)
            .iter()
            .map(|c| c.iter().map(|x| x.ch).collect())
            .collect();
        assert!(rows.len() > 1, "{rows:?}");
        assert!(rows[0].starts_with("1. alpha"), "{rows:?}");
        assert!(rows[1].starts_with("   ") && !rows[1].starts_with("    "), "{rows:?}");
    }

    #[test]
    fn rule_spans_the_page() {
        let r = render_wide("a\n\n---\n\nb\n", 60);
        assert!(r.lines.iter().any(|l| l.text() == "─".repeat(60)), "{}", flat(&r));
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
    fn the_rail_runs_down_blank_and_wrapped_rows_alike() {
        let md = "> one two three four five six seven eight nine ten\n>\n> - alpha beta gamma delta epsilon zeta eta\n\nafter\n";
        let r = render_wide(md, 24);
        let rows: Vec<String> = r.lines.iter().map(|l| l.text()).collect();
        let quoted: Vec<&String> = rows.iter().filter(|t| t.starts_with("▌")).collect();
        // one paragraph and one bullet, each wrapped, with a blank row between
        assert!(quoted.len() >= 5, "{rows:?}");
        assert!(
            quoted.iter().any(|t| t.trim() == "▌"),
            "blank row keeps its bar: {rows:?}"
        );
        for t in &quoted {
            assert!(crate::md::str_width(t) <= 24, "{t:?}");
        }
        // the wrapped bullet hangs under its text, not under the bullet
        let bullet = rows.iter().position(|t| t.contains("• alpha")).unwrap();
        assert!(
            rows[bullet + 1].starts_with("▌   "),
            "{:?}",
            rows[bullet + 1]
        );
        // nothing after the quote carries a bar, and the quote body is not dim
        assert!(rows.iter().any(|t| t == "after"));
        let body = r
            .lines
            .iter()
            .find(|l| l.text().contains("one two"))
            .unwrap();
        let word = body.cells.iter().find(|c| c.ch == 'o').unwrap();
        assert_eq!(word.style, Style::default());
    }

    #[test]
    fn footnotes_and_maths_render_on_the_page() {
        let r = render_wide("Tea[^1] and $x$.\n\n$$\nE = mc^2\n$$\n\n[^1]: Tickles\n", 20);
        let rows: Vec<String> = r.lines.iter().map(|l| l.text()).collect();
        assert!(rows.iter().any(|t| t == "Tea¹ and x."), "{rows:?}");
        assert!(rows.iter().any(|t| t.trim() == "E = mc^2" && t.starts_with("     ")), "{rows:?}");
        assert!(rows.iter().any(|t| t == "¹ Tickles"), "{rows:?}");
        assert!(rows.iter().all(|t| !t.contains("[^1]") && !t.contains("$$")), "{rows:?}");
    }

    #[test]
    fn a_callout_becomes_a_box_the_width_of_the_page() {
        let md = "> [!summary] TL;DR\n> **Situation:**\n>\n> - At Airstream, during the MY22 launch, the connected vehicle platform.\n\nafter\n";
        let w = 40;
        let r = render_wide(md, w);
        let rows: Vec<String> = r.lines.iter().map(|l| l.text()).collect();
        let top = rows.iter().find(|t| t.starts_with('╭')).expect("top edge");
        let bottom = rows
            .iter()
            .find(|t| t.starts_with('╰'))
            .expect("bottom edge");
        assert_eq!(crate::md::str_width(top), w, "{top:?}");
        assert_eq!(crate::md::str_width(bottom), w, "{bottom:?}");
        assert!(top.ends_with('╮') && bottom.ends_with('╯'));
        assert!(top.contains("≡ summary · TL;DR"), "{top:?}");
        let ti = rows.iter().position(|t| t.starts_with('╭')).unwrap();
        let bi = rows.iter().position(|t| t.starts_with('╰')).unwrap();
        assert!(bi > ti + 2);
        for t in &rows[ti + 1..bi] {
            assert!(t.starts_with('│') && t.ends_with('│'), "{t:?}");
            assert_eq!(crate::md::str_width(t), w, "{t:?}");
        }
        assert!(rows.iter().all(|t| !t.contains("[!summary]")), "{rows:?}");
        assert!(rows.iter().any(|t| t.contains("Situation:")));
        // the bullet wrapped inside the box, and blank quoted rows are bare box rows
        assert!(rows[ti + 1..bi]
            .iter()
            .any(|t| t.trim_matches(|c| c == '│' || c == ' ').is_empty()));
        assert!(
            rows[ti + 1..bi]
                .iter()
                .filter(|t| t.contains("Airstream") || t.contains("platform"))
                .count()
                >= 2
        );
        // text in the box still knows its source line
        let sit = r
            .lines
            .iter()
            .find(|l| l.text().contains("Situation"))
            .unwrap();
        assert_eq!(
            sit.cells.iter().find(|c| c.ch == 'S').unwrap().src,
            Some((1, 4))
        );
        assert!(rows.iter().any(|t| t == "after"));
    }

    #[test]
    fn a_callout_without_a_title_and_with_a_fold_marker_still_boxes() {
        let r = render_wide("> [!tip]- \n> body\n", 30);
        let rows: Vec<String> = r.lines.iter().map(|l| l.text()).collect();
        let top = rows.iter().find(|t| t.starts_with('╭')).unwrap();
        assert!(top.contains("✓ tip ─"), "{top:?}");
        assert!(!top.contains('·'));
        assert!(rows.iter().any(|t| t.starts_with("│ body")));
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

    /// What the reading view hands the renderer: the body, and the line it
    /// starts on, with the front matter already cut away.
    fn body_of(content: &str) -> Rendered {
        let (skip, first) = crate::notes::front_matter_range(content)
            .map_or((0, 0), |r| (r.end, content[..r.end].lines().count()));
        render_page_at(&content[skip..], first, usize::MAX, TableStyle::default())
    }

    #[test]
    fn a_page_rendered_from_a_slice_still_reports_file_line_numbers() {
        let r = render_page_at("# Title\n\nprose\n", 4, usize::MAX, TableStyle::default());
        let title = r.lines.iter().find(|l| l.text().contains("Title")).unwrap();
        assert_eq!(title.src_line, Some(4));
        assert_eq!(title.cells[0].src, Some((4, 2)));
        let prose = r.lines.iter().find(|l| l.text().contains("prose")).unwrap();
        assert_eq!(prose.src_line, Some(6));
        assert_eq!(prose.cells[0].src, Some((6, 0)));
    }

    #[test]
    fn the_reading_view_renders_the_body_and_never_the_front_matter() {
        let r = body_of("---\ntype: log\ntags: work\n---\n\n# Title\n\nprose\n");
        let page: String = r
            .lines
            .iter()
            .map(|l| l.text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!page.contains("type: log"));
        assert!(!page.contains("tags"));
        assert!(!page.contains("---"));
        assert!(page.contains("Title"));
        assert!(page.contains("prose"));
        // a note without front matter is unchanged, offset and all
        let plain = body_of("# Title\n");
        assert_eq!(plain.lines[0].src_line, Some(0));
    }

    #[test]
    fn a_checkbox_under_front_matter_still_maps_to_its_own_source_line() {
        // the number the toggle indexes the buffer with, so an off-by-N here
        // would silently tick the wrong box
        let r = body_of("---\ntype: log\n---\n\n- [ ] todo\n- [x] done\n");
        let todo = r.lines.iter().find(|l| l.text().contains("todo")).unwrap();
        assert_eq!(todo.checkbox, Some(4));
        let done = r.lines.iter().find(|l| l.text().contains("done")).unwrap();
        assert_eq!(done.checkbox, Some(5));
        // and a click on the word lands inside it, not at the line's start
        assert_eq!(done.cells[2].src, Some((5, 6)));
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
    fn a_tag_is_drawn_in_the_accent_and_records_a_tag_target() {
        crate::md::tags::set_enabled(true);
        let r = render("see #work now\n");
        assert_eq!(flat(&r).trim(), "see #work now");
        let cell = r.lines[0].cells.iter().find(|c| c.link.is_some()).unwrap();
        assert_eq!(cell.ch, '#');
        assert_eq!(cell.style.fg, theme::tag().fg);
        assert_eq!(r.url(cell.link.unwrap()), Some("tag:work"));
    }

    #[test]
    fn a_tag_in_code_a_heading_marker_or_a_url_is_not_recorded() {
        crate::md::tags::set_enabled(true);
        let r = render("# Title\n\n`#code` and https://x.y/#frag and x#y\n\n```\n#fence\n```\n");
        assert!(
            r.urls.iter().all(|u| !u.starts_with("tag:")),
            "{:?}",
            r.urls
        );
        // and the one right after a code span is not one either: the char
        // before it is a backtick, whatever pulldown split the events on
        let r = render("`x`#glued\n");
        assert!(r.urls.is_empty());
        let r = render("`x` #free\n");
        assert_eq!(r.urls, vec!["tag:free"]);
    }

    #[test]
    fn a_wikilink_renders_as_its_text_and_records_a_wikilink_target() {
        let r = render("see [[note]] now\n");
        assert_eq!(flat(&r).trim(), "see note now");
        let cell = r.lines[0].cells.iter().find(|c| c.link.is_some()).unwrap();
        assert_eq!(r.url(cell.link.unwrap()), Some("wikilink:note"));
        // a piped one shows only its label, and the target loses the heading
        let r = render("[[stories/story-matrix#Method|the matrix]]\n");
        assert_eq!(flat(&r).trim(), "the matrix");
        assert_eq!(r.url(0), Some("wikilink:stories/story-matrix"));
    }

    #[test]
    fn brackets_pulldown_hands_back_one_at_a_time_are_not_drawn_twice() {
        // pulldown gives `[`, `[`, `note`, `]`, `]` as five separate text
        // events; without the watermark the closing pair is drawn after the
        // label and the line reads "note]]"
        let r = render("see [[note]] and [[a|b]] here\n");
        let text = flat(&r);
        assert_eq!(text.trim(), "see note and b here");
        assert!(!text.contains(']'), "{text}");
        assert_eq!(text.matches("here").count(), 1);
    }

    #[test]
    fn nothing_else_pulldown_finds_inside_a_wikilink_is_drawn_after_it_either() {
        // `md::wikilink_at` lets a backtick sit inside a target — it bails on
        // `[`, `]` and a newline, and on nothing else — so the live editor
        // draws this whole label. pulldown, which knows nothing of wikilinks,
        // sees inline code in the middle of it and hands back a `Code` event
        // for the `b`; drawn, it would be a second `b` after the label and the
        // two views would disagree about one line
        let r = render("[[a `b` c]] tail\n");
        assert_eq!(flat(&r).trim(), "a `b` c tail");
    }

    #[test]
    fn a_href_in_the_note_can_never_claim_the_scheme_the_app_uses_for_a_file() {
        // the footer's own rows name a file by path; a note body saying the
        // same words is a stranger's text and must reach the desktop opener
        // instead of `App::open_path`
        let r = render("[report](note:/etc/passwd)\n");
        assert_eq!(
            crate::md::LinkTarget::parse(r.url(0).unwrap()),
            crate::md::LinkTarget::Url("note:/etc/passwd".to_string())
        );
        let r = render("<https://x.y/a>\n");
        assert_eq!(
            crate::md::LinkTarget::parse(r.url(0).unwrap()),
            crate::md::LinkTarget::Url("https://x.y/a".to_string())
        );
    }

    #[test]
    fn a_wikilink_in_a_table_cell_is_still_a_link() {
        let r = render_wide("| a | b |\n| - | - |\n| [[note]] | x |\n", 40);
        let row = r
            .lines
            .iter()
            .find(|l| l.text().contains("note"))
            .expect("the cell is drawn");
        assert!(row.cells.iter().any(|c| c.link.is_some()));
        // the column is measured from the label, not from the source: the
        // brackets are gone, so nothing pads out to their width
        assert!(!row.text().contains("[["), "{}", row.text());
        assert_eq!(r.urls.iter().filter(|u| *u == "wikilink:note").count(), 1);
    }

    #[test]
    fn a_wikilink_in_a_list_item_is_still_a_link() {
        let r = render("- see [[note]]\n- and [[other]]\n");
        let linked: Vec<String> = r
            .lines
            .iter()
            .filter(|l| l.cells.iter().any(|c| c.link.is_some()))
            .map(|l| l.text())
            .collect();
        assert_eq!(linked.len(), 2, "{linked:?}");
        assert_eq!(r.urls, vec!["wikilink:note", "wikilink:other"]);
    }

    #[test]
    fn an_escaped_or_embedded_wikilink_is_left_as_text() {
        let r = render("\\[[x]] and ![[y.png]]\n");
        let text = flat(&r);
        assert!(text.contains("[[x]]"), "{text}");
        assert!(text.contains("[[y.png]]"), "{text}");
        assert!(r.urls.is_empty(), "{:?}", r.urls);
    }

    #[test]
    fn a_wikilink_cell_remembers_the_source_column_of_its_label() {
        // preview click → edit indexes the buffer with this, so the first
        // label cell has to be the label's own column, not the bracket's
        let r = render("see [[note|label]] now\n");
        let cell = r.lines[0].cells.iter().find(|c| c.link.is_some()).unwrap();
        assert_eq!(cell.ch, 'l');
        assert_eq!(cell.src, Some((0, "see [[note|".len())));
    }

    #[test]
    fn an_obsidian_embed_is_an_image_in_the_reading_view() {
        let r = render("before\n\n![[attachments/hero.jpg|the hero]]\n\nafter\n");
        let line = r.lines.iter().find(|l| l.image.is_some()).unwrap();
        assert_eq!(
            r.images[line.image.unwrap()],
            ImageSpec {
                alt: "the hero".into(),
                url: "attachments/hero.jpg".into(),
            }
        );
        assert_eq!(line.text(), "🖼 the hero (attachments/hero.jpg)");
        // nothing of the source syntax leaks out after the picture
        let all: String = r
            .lines
            .iter()
            .map(|l| l.text())
            .collect::<Vec<_>>()
            .join("|");
        assert!(!all.contains("]]") && !all.contains("![["), "{all}");
        assert!(all.contains("after"));
        // a note embed and a mid-sentence embed stay text
        let r = render("![[plan]]\n");
        assert!(r.lines.iter().all(|l| l.image.is_none()));
        assert!(r.lines.iter().any(|l| l.text().contains("![[plan]]")));
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

    fn mention(name: &str, excerpt: &str, count: usize) -> crate::mentions::Mention {
        // the link span is whatever the scan would have recorded for the
        // first wikilink; an excerpt without one has an empty span
        let link = crate::md::wikilinks(excerpt)
            .first()
            .map(|w| (w.start, w.end))
            .unwrap_or((0, 0));
        crate::mentions::Mention {
            path: std::path::PathBuf::from(format!("/vault/{name}.md")),
            name: name.to_string(),
            excerpt: excerpt.to_string(),
            link,
            count,
        }
    }

    fn footer_row<'a>(r: &'a Rendered, name: &str) -> &'a PLine {
        r.lines
            .iter()
            .find(|l| l.text().starts_with(&format!("  {name}")))
            .unwrap()
    }

    #[test]
    fn no_mentions_means_no_footer_line_at_all() {
        let mut r = render("# Spec\n\nbody\n");
        let before = r.lines.len();
        append_mentions(&mut r, &[], 60);
        // not even a rule, and certainly not "0 notes link here"
        assert_eq!(r.lines.len(), before);
        assert!(!r.lines.iter().any(|l| l.text().contains("link here")));
    }

    #[test]
    fn the_footer_names_each_note_once_and_counts_the_rest() {
        let mut r = render("# Spec\n");
        append_mentions(
            &mut r,
            &[
                mention("meta-os-control", "…see [[spec]] for the", 3),
                mention("ford-mvp", "…pulled from [[spec]]", 1),
            ],
            60,
        );
        let text: Vec<String> = r.lines.iter().map(|l| l.text()).collect();
        assert!(text.iter().any(|t| t == "2 notes link here"));
        let first = text.iter().find(|t| t.contains("meta-os-control")).unwrap();
        assert!(first.contains("…see spec for the"));
        // several mentions in one note are one row, with the count beside it
        assert!(first.ends_with(" ×3"));
        let second = text.iter().find(|t| t.contains("ford-mvp")).unwrap();
        assert!(!second.contains('×'));
        // one note reads as one note
        let mut one = render("# Spec\n");
        append_mentions(&mut one, &[mention("meta", "x", 1)], 60);
        assert!(one.lines.iter().any(|l| l.text() == "1 note links here"));
    }

    #[test]
    fn every_footer_row_is_a_link_to_the_note_that_mentions_this_one() {
        let mut r = render("# Spec\n");
        append_mentions(&mut r, &[mention("meta", "about [[spec]]", 1)], 60);
        let row = r.lines.iter().find(|l| l.text().contains("meta")).unwrap();
        let link = row
            .cells
            .iter()
            .find(|c| c.ch == 'm')
            .unwrap()
            .link
            .unwrap();
        // an exact file, so the click cannot land on another note of the same
        // name, and never a url the desktop would be handed
        assert_eq!(r.url(link), Some("note:/vault/meta.md"));
        assert_eq!(
            crate::md::LinkTarget::parse(r.url(link).unwrap()),
            crate::md::LinkTarget::Note("/vault/meta.md".to_string())
        );
        // the excerpt is not part of the link
        assert!(row
            .cells
            .iter()
            .filter(|c| c.link.is_some())
            .all(|c| "meta".contains(c.ch)));
    }

    fn folded(md: &str, heads: &[usize], width: usize) -> Rendered {
        let lines: Vec<String> = md.lines().map(String::from).collect();
        let blocks = crate::md::blocks(&lines);
        let visible = crate::fold::Visible::new(&lines, &blocks, heads);
        let mut r = render_wide(md, width);
        apply_folds(&mut r, &visible, heads, width);
        r
    }

    fn texts(r: &Rendered) -> Vec<String> {
        r.lines.iter().map(PLine::text).collect()
    }

    #[test]
    fn a_folded_section_loses_its_rows_and_the_heading_says_how_many() {
        let md = "# Title\nintro\n## One\na\nb\n## Two\nc\n";
        let r = folded(md, &[2], 40);
        let t = texts(&r);
        assert!(t.iter().all(|l| l != "a" && l != "b"), "{t:?}");
        let head = r.lines.iter().find(|l| l.src_line == Some(2)).unwrap();
        let text = head.text();
        assert!(text.starts_with("▸ One"), "{text:?}");
        assert!(text.ends_with("2 lines folded"), "{text:?}");
        assert_eq!(cells_width(&head.cells), 40);
        // the marker stands for the first `#`, the way it does in the editor
        assert_eq!(head.cells[0].src, Some((2, 0)));
        assert_eq!(head.cells[0].style, theme::fold());
        // Two follows One after one blank row, not the two either side of
        // the section that went
        let one = r.lines.iter().position(|l| l.src_line == Some(2)).unwrap();
        assert!(r.lines[one + 1].cells.is_empty());
        assert_eq!(r.lines[one + 2].src_line, Some(5));
        assert!(!r.lines[one + 2].text().starts_with('▸'));
        // the plain page is what it was
        let plain = folded(md, &[], 40);
        assert_eq!(texts(&plain), texts(&render_wide(md, 40)));
    }

    #[test]
    fn a_fold_at_the_end_of_the_page_leaves_no_blank_behind() {
        let md = "# Title\n## Last\nx\ny\n";
        let r = folded(md, &[1], 40);
        assert!(!r.lines.last().unwrap().cells.is_empty());
        assert_eq!(r.lines.last().unwrap().src_line, Some(1));
        // one hidden line reads in the singular, and a page too narrow for
        // the count keeps the heading text and the marker
        assert!(folded("# T\n## L\nx\n", &[1], 40).lines[2]
            .text()
            .ends_with("1 line folded"));
        let tight = folded("# T\n## Last\nx\n", &[1], 12);
        assert_eq!(tight.lines[2].text(), "▸ Last");
    }

    #[test]
    fn everything_a_section_holds_goes_with_it() {
        let md = "# T\n## One\n- [ ] task\n```rust\nlet x = 1;\n```\n| a | b |\n| - | - |\n| 1 | 2 |\n> quoted\n![pic](p.png)\n```mermaid\ngraph LR\nA-->B\n```\n## Two\nend\n";
        let r = folded(md, &[1], 40);
        for l in &r.lines {
            assert!(
                l.src_line.is_none_or(|s| s == 0 || s == 1 || s >= 15),
                "row from a hidden line: {:?} {:?}",
                l.src_line,
                l.text()
            );
        }
        let all = texts(&r).join("\n");
        for gone in ["task", "let x", "│", "quoted", "pic", "A", "-->"] {
            assert!(!all.contains(gone), "{gone:?} in {all:?}");
        }
        assert!(all.contains("end"));
        assert!(r.lines.iter().all(|l| l.image.is_none()));
        // the checkbox under the fold is not there to click
        assert!(r.lines.iter().all(|l| l.checkbox.is_none()));
    }

    #[test]
    fn a_footer_row_carries_no_source_position_so_a_click_cannot_land_in_the_note() {
        let mut r = render("# Spec\n");
        let before = r.lines.len();
        append_mentions(&mut r, &[mention("meta", "about [[spec]]", 1)], 60);
        for line in &r.lines[before..] {
            assert_eq!(line.src_line, None);
            assert_eq!(line.checkbox, None);
            assert!(!line.wide);
            assert!(line.cells.iter().all(|c| c.src.is_none()));
        }
    }

    #[test]
    fn the_footer_never_makes_a_page_wider_than_the_page() {
        let mut r = render_wide("# Spec\n", 30);
        append_mentions(
            &mut r,
            &[mention(
                "a-note-with-a-very-long-name-indeed",
                "a sentence far longer than the page could ever hold, on and on",
                12,
            )],
            30,
        );
        assert!(r.lines.iter().all(|l| cells_width(&l.cells) <= 30));
    }

    #[test]
    fn the_footer_names_a_note_by_its_file_not_its_first_line() {
        let mut r = render("# Spec\n");
        let mut m = mention("meta-os-control", "about [[spec]]", 1);
        m.path = std::path::PathBuf::from("/vault/deep/meta-os-control.md");
        append_mentions(&mut r, &[m], 60);
        let row = footer_row(&r, "meta-os-control");
        let name: String = row
            .cells
            .iter()
            .filter(|c| c.link.is_some())
            .map(|c| c.ch)
            .collect();
        assert_eq!(name, "meta-os-control");
        assert!(row
            .cells
            .iter()
            .filter(|c| c.link.is_some())
            .all(|c| c.style == theme::link()));
    }

    #[test]
    fn the_excerpt_is_styled_rather_than_shown_as_raw_markdown() {
        let mut r = render("# Spec\n");
        append_mentions(
            &mut r,
            &[mention(
                "meta",
                "**Projects:** [[spec|the spec]] and `code`",
                1,
            )],
            80,
        );
        let row = footer_row(&r, "meta");
        let text = row.text();
        assert!(!text.contains("**"), "{text}");
        assert!(!text.contains("[["), "{text}");
        assert!(!text.contains('`'), "{text}");
        assert!(text.contains("Projects: the spec and code"), "{text}");
        // bold is bold, and the link reads as a link
        let p = row.cells.iter().find(|c| c.ch == 'P').unwrap();
        assert!(p.style.add_modifier.contains(Modifier::BOLD));
        let t = row.cells.iter().find(|c| c.ch == 't').unwrap();
        assert!(t.style.add_modifier.contains(Modifier::UNDERLINED));
        // and nothing in the excerpt maps back into the note
        assert!(row.cells.iter().all(|c| c.src.is_none()));
    }

    #[test]
    fn a_long_excerpt_is_cut_around_the_link_so_the_link_stays_on_screen() {
        let mut r = render("# Spec\n");
        let before = "word ".repeat(30);
        let after = " tail".repeat(30);
        let excerpt = format!("{before}[[spec]]{after}");
        append_mentions(&mut r, &[mention("meta", &excerpt, 1)], 60);
        let row = footer_row(&r, "meta");
        let text = row.text();
        assert!(text.contains("spec"), "{text}");
        // the window opens with an ellipsis, right after the name column
        assert!(text.starts_with("  meta  …"), "{text}");
        assert!(cells_width(&row.cells) <= 60);
        // and one that fits from the start is not moved
        let mut r = render("# Spec\n");
        let excerpt = format!("[[spec]]{after}");
        append_mentions(&mut r, &[mention("meta", &excerpt, 1)], 60);
        assert!(footer_row(&r, "meta").text().contains("  spec tail"));
    }

    #[test]
    fn a_narrow_page_keeps_the_titles_and_drops_the_excerpts() {
        let mut r = render_wide("# Spec\n", 18);
        append_mentions(&mut r, &[mention("meta", "about the spec", 1)], 18);
        let row = r.lines.iter().find(|l| l.text().contains("meta")).unwrap();
        assert!(!row.text().contains("about"));
    }
}

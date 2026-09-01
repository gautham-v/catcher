use crate::app::{App, EditRow, Item, Overlay, View};
use crate::config::BorderStyle;
use crate::md::theme;
use crate::render::PCell;
use crate::tree::RowKind;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;
use ratatui_image::{CropOptions, Resize, StatefulImage};
use std::time::SystemTime;

/// Chrome that should read as present but never first. Not `DarkGray`: many
/// terminal profiles set that slot almost to the background, which is where
/// status text goes to die.
fn dim() -> Style {
    theme::marker()
}

/// Every panel in the app: one border style, one corner shape, both settable.
fn panel(app: &App) -> Block<'static> {
    let block = Block::default().border_style(theme::border());
    match app.config.borders {
        BorderStyle::Rounded => block.borders(Borders::ALL).border_type(BorderType::Rounded),
        BorderStyle::Square => block.borders(Borders::ALL).border_type(BorderType::Plain),
        // no border still needs the one-cell inset, or the text would sit
        // flush against whatever is behind the overlay
        BorderStyle::None => block
            .borders(Borders::NONE)
            .padding(ratatui::widgets::Padding::new(1, 1, 0, 0)),
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let status_h = if app.config.status_bar { 1 } else { 0 };
    let [content, status] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(status_h)]).areas(f.area());

    // a centred column, like a note page; `page_width: full` fills the window
    let width = match app.config.page_width {
        0 => content.width,
        w => content.width.min(w),
    };
    let [_, page, _] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(width),
        Constraint::Fill(1),
    ])
    .areas(content);
    let page = page.inner(ratatui::layout::Margin {
        horizontal: 1,
        vertical: 1,
    });
    app.editor_area = page;

    match app.view {
        View::Edit => draw_editor(f, app, page),
        View::Preview => draw_preview(f, app, page),
    }

    if app.config.status_bar {
        draw_status(f, app, status);
    }

    if app.overlay == Overlay::None {
        draw_peek(f, app);
    }

    match app.overlay {
        Overlay::Palette | Overlay::QuickOpen => draw_palette(f, app),
        Overlay::ConfirmDelete => draw_confirm(f, app),
        Overlay::ConfirmCreate => draw_create(f, app),
        Overlay::RenameFile => draw_rename(f, app),
        Overlay::Help => draw_help(f, app),
        Overlay::None => {}
    }
}

/// Live preview: every line is styled markdown except the block the cursor is
/// in, which shows its raw source so the syntax is there to edit.
///
/// Long lines soft-wrap, so a source line is as many screen rows as it needs;
/// an `![](…)` line the terminal can draw takes the rows its picture needs
/// instead. Scrolling, the cursor and hit-testing therefore all work in
/// display rows, summed over measured heights rather than counted in lines.
fn draw_editor(f: &mut Frame, app: &mut App, area: Rect) {
    let crow = app.editor.cursor.0;
    let blocks = app.blocks();
    let width = area.width.max(1) as usize;
    let (cseg, cdisp) = app.cursor_seg(&blocks, width);
    app.editor.scroll_into_view(area.height as usize);

    let dir = app.note_dir();
    let n = app.editor.lines().len();

    // the pass above counted one row per line; wrapped lines and pictures both
    // take more, so walk the top down until the cursor's own display row fits
    if app.editor.following() {
        let mut top = app.editor.scroll.min(crow);
        while top < crow {
            let used: u16 = (top..crow)
                .map(|r| row_height(app, &blocks, r, &dir, area.width).0)
                .sum::<u16>()
                // only the rows of the cursor's line up to the cursor itself
                + cseg as u16
                + 1;
            if used <= area.height {
                break;
            }
            top += 1;
        }
        app.editor.scroll = top;
    }

    // lay the visible rows out, giving drawable images the height they measured
    let top = app.editor.scroll.min(n.saturating_sub(1));
    // (source line, rows, image url) — one entry per source line
    let mut plan: Vec<(usize, u16, Option<String>)> = Vec::new();
    let mut used = 0u16;
    let mut row = top;
    while row < n && used < area.height {
        let (natural, url) = row_height(app, &blocks, row, &dir, area.width);
        // a picture taller than the page is clamped to the page, exactly as
        // the preview does, so it can still be shown whole at some scroll
        // position instead of leaving a band that is blank at every one
        let natural = match url {
            Some(_) => crate::images::band_rows(natural, area.height),
            None => natural,
        };
        // every drawn line gets at least one row; a hidden one gets none at
        // all, which is the only way a source line takes no space on screen
        let h = if natural == 0 {
            0
        } else {
            natural.min(area.height - used).max(1)
        };
        plan.push((row, h, url));
        used += h;
        row += 1;
    }

    let mut lines: Vec<Line> = Vec::new();
    let mut images: Vec<(Rect, String, bool)> = Vec::new();
    app.edit_rows.clear();
    let mut y = area.y;
    for (row, h, url) in &plan {
        match url {
            Some(url) => {
                app.edit_rows.push(EditRow {
                    rect: Rect::new(area.x, y, area.width, *h),
                    line: *row,
                    seg: 0,
                });
                for _ in 0..*h {
                    lines.push(Line::default());
                }
                // a picture cut off by the bottom of the page is drawn cropped
                // to the rows it did get, so it scrolls into view row by row
                images.push((Rect::new(area.x, y, area.width, *h), url.clone(), false));
            }
            None => {
                let segs = app.wrapped(*row, &blocks, width);
                let selection = app.editor.selection_on(*row);
                for (i, seg) in segs.iter().enumerate().take(*h as usize) {
                    app.edit_rows.push(EditRow {
                        rect: Rect::new(area.x, y + i as u16, area.width, 1),
                        line: *row,
                        seg: i,
                    });
                    lines.push(seg.to_line(selection));
                }
            }
        }
        y += h;
    }
    f.render_widget(Paragraph::new(lines), area);

    for (rect, url, clip_top) in images {
        if let Some(protocol) = app.images.protocol(&url, &dir) {
            f.render_stateful_widget(cropped(clip_top), rect, protocol);
        }
    }

    if app.overlay == Overlay::None {
        if let Some(band) = app
            .edit_rows
            .iter()
            .find(|r| r.line == crow && r.seg == cseg)
            .map(|r| r.rect)
        {
            let x = area.x + cdisp as u16;
            if x < area.x + area.width {
                f.set_cursor_position((x, band.y));
            }
        }
    }
}

/// How many rows source line `row` occupies, and the image URL when it is one
/// the terminal can draw. A picture takes the rows it measured; everything
/// else takes the rows it soft-wrapped to.
fn row_height(
    app: &mut App,
    blocks: &[crate::md::Block],
    row: usize,
    dir: &std::path::Path,
    width: u16,
) -> (u16, Option<String>) {
    // front matter set to `hide` takes no rows at all, the way a picture takes
    // more than one: what the buffer's shape on screen is, is the draw's
    // business rather than the buffer's
    if app.hidden_row(blocks, row) {
        return (0, None);
    }
    let url = crate::md::block_at(blocks, row)
        .filter(|b| b.kind == crate::md::BlockKind::Image && !app.revealed(b))
        .and_then(|b| app.editor.lines().get(b.start))
        .and_then(|src| crate::md::image_line(src))
        .map(|(_, url)| url);
    if let Some(url) = url {
        // no picture (unsupported terminal, missing file) keeps the text line
        if let Some(h) = app.images.rows(&url, dir, width) {
            return (h, Some(url));
        }
    }
    let rows = app.wrapped(row, blocks, width.max(1) as usize).len();
    (rows.max(1) as u16, None)
}

/// One screen row of the preview, after wrapping and image expansion.
struct Row {
    cells: Vec<PCell>,
    checkbox: Option<usize>,
    src_line: Option<usize>,
    /// A row of a scrolling table: never soft-wrapped, panned instead.
    wide: bool,
}

/// The rendered page: pre-wrapped so every row is exactly one screen line,
/// which is what makes link and checkbox hit-testing exact.
fn draw_preview(f: &mut Frame, app: &mut App, area: Rect) {
    let width = area.width.max(1) as usize;
    // front matter is metadata, not prose: the reading view never shows it,
    // whatever the editor has been told to do with it. The body's lines are
    // still counted from the top of the file, so a click on a checkbox or a
    // word still lands on the source line it came from.
    let content = &app.active_note().content;
    let (skip, first) = crate::notes::front_matter_range(content)
        .map_or((0, 0), |r| (r.end, content[..r.end].lines().count()));
    let mut rendered =
        crate::render::render_page_at(&content[skip..], first, width, app.config.table_style);
    // the footer arrives a frame or two late by design: the scan behind it
    // reads every note body under the roots, and a page that waited for that
    // would be a page that stuttered on every open
    // and only where there are links to count: with `wikilinks` off `[[x]]` is
    // literal text the reading view draws as typed, and a footer listing notes
    // that "link here" would be counting something the page does not show
    if app.config.linked_mentions && app.config.wikilinks {
        let rows = app.linked_mentions();
        crate::render::append_mentions(&mut rendered, &rows, width);
    }
    let dir = app.note_dir();

    // wrap, then give drawable images the rows they need
    let mut rows: Vec<Row> = Vec::new();
    // (first page row, rows, image index) — kept aside from the visible window
    // so a band whose own first row is scrolled off the top is still drawn
    let mut bands: Vec<(usize, u16, usize)> = Vec::new();
    for pline in &rendered.lines {
        if let Some(idx) = pline.image {
            let url = rendered.images[idx].url.clone();
            if let Some(natural) = app.images.rows(&url, &dir, area.width) {
                // measured once from the full page width, so the rows an image
                // reserves never change with the scroll
                let h = crate::images::band_rows(natural, area.height);
                bands.push((rows.len(), h, idx));
                for _ in 0..h {
                    rows.push(Row {
                        cells: Vec::new(),
                        checkbox: None,
                        src_line: pline.src_line,
                        wide: false,
                    });
                }
                continue;
            }
        }
        // a wide row is one row, however long it is: it pans, it never wraps
        if pline.wide {
            rows.push(Row {
                cells: pline.cells.clone(),
                checkbox: pline.checkbox,
                src_line: pline.src_line,
                wide: true,
            });
            continue;
        }
        for (i, cells) in wrap_cells(&pline.cells, width).into_iter().enumerate() {
            rows.push(Row {
                cells,
                checkbox: if i == 0 { pline.checkbox } else { None },
                src_line: pline.src_line,
                wide: false,
            });
        }
    }

    // the furthest right the page can pan: the widest table on it, less the
    // page itself. Measured here because only the draw knows the real width.
    let widest = rows
        .iter()
        .filter(|r| r.wide)
        .map(|r| crate::render::cells_width(&r.cells))
        .max()
        .unwrap_or(0);
    app.preview_hmax = widest.saturating_sub(width) as u16;
    app.preview_hscroll = app.preview_hscroll.min(app.preview_hmax);
    let pan = app.preview_hscroll as usize;

    // clamp the scroll so the page can't be scrolled off the bottom
    let height = area.height as usize;
    let max_scroll = rows.len().saturating_sub(height.max(1)) as u16;
    app.preview_scroll = app.preview_scroll.min(max_scroll);
    let top = app.preview_scroll as usize;

    app.preview_links.clear();
    app.preview_checkboxes.clear();
    app.preview_rows.clear();
    app.preview_page_rows.clear();
    let span = app.preview_span();

    let mut lines: Vec<Line> = Vec::new();
    let mut images: Vec<(Rect, usize, bool)> = Vec::new();
    // the chevrons go on the first wide row on screen, and only there
    let mut marked = false;
    for (start, h, idx) in &bands {
        // a band only partly on screen is drawn cropped to its visible slice,
        // so a picture scrolls in and out instead of popping into view whole
        if let Some(s) = crate::images::band_slice(*start, *h, top, area.height) {
            images.push((
                Rect::new(area.x, area.y + s.offset, area.width, s.rows),
                *idx,
                s.clip_top,
            ));
        }
    }
    for (i, row) in rows.iter().skip(top).take(height).enumerate() {
        let y = area.y + i as u16;
        let rect = Rect::new(area.x, y, area.width, 1);
        let page_row = top + i;
        // a wide row shows the slice the pan has arrived at; everything else
        // starts at column zero, so prose never moves when a table does
        let offset = if row.wide { pan } else { 0 };
        let mut shown = if row.wide {
            columns_from(&row.cells, pan, width)
        } else {
            row.cells.clone()
        };
        // a table that carries on past an edge says so — but once per table,
        // on its topmost visible row. A marker on every row would stripe the
        // whole page with chevrons to say one thing.
        if row.wide && !marked {
            marked = true;
            if pan > 0 {
                edge(&mut shown, 0, '‹');
            }
            if crate::render::cells_width(&row.cells) > pan + width && !shown.is_empty() {
                let last = shown.len() - 1;
                edge(&mut shown, last, '›');
            }
        }
        app.preview_page_rows.push((page_row, rect, offset));
        lines.push(crate::render::to_line(&selected(
            &shown, page_row, offset, span,
        )));
        // every drawn row is recorded, source line or not: a row the renderer
        // invented — a blank line, a footer row — is still a row a selection
        // can be dragged across, and one that is missing here silently kills
        // the copy of everything around it
        app.preview_rows.push((rect, row.src_line, shown.clone()));
        if row.checkbox.is_some() {
            if let Some(src) = row.src_line {
                app.preview_checkboxes.push((rect, src));
            }
        }
        // link hit boxes are measured on what is actually on screen, so a
        // half-panned link is clickable over exactly the part you can see
        for (start, len, url) in link_runs(&shown, &rendered) {
            let x = area.x + start as u16;
            app.preview_links
                .push((Rect::new(x, y, len as u16, 1), url));
        }
    }
    f.render_widget(Paragraph::new(lines), area);

    for (rect, idx, clip_top) in images {
        let url = rendered.images[idx].url.clone();
        if let Some(protocol) = app.images.protocol(&url, &dir) {
            f.render_stateful_widget(cropped(clip_top), rect, protocol);
        }
    }
}

/// Overwrite one drawn cell with a continuation mark.
fn edge(cells: &mut [PCell], at: usize, ch: char) {
    if let Some(c) = cells.get_mut(at) {
        c.ch = ch;
        c.style = theme::state();
        c.link = None;
    }
}

/// The `width` display columns of `cells` starting at column `from`. A wide
/// character straddling the edge is dropped rather than half-drawn.
fn columns_from(cells: &[PCell], from: usize, width: usize) -> Vec<PCell> {
    let mut out = Vec::new();
    let mut col = 0;
    for c in cells {
        let w = crate::md::char_width(c.ch);
        if col >= from && col + w <= from + width {
            out.push(c.clone());
        }
        col += w;
    }
    out
}

/// A row's cells with the selected span inverted. The preview has no cursor,
/// so the highlight *is* the feedback that a drag is doing anything.
fn selected(
    cells: &[PCell],
    page_row: usize,
    offset: usize,
    span: Option<(crate::app::PSel, crate::app::PSel)>,
) -> Vec<PCell> {
    let Some(((sr, sc), (er, ec))) = span else {
        return cells.to_vec();
    };
    if page_row < sr || page_row > er {
        return cells.to_vec();
    }
    let from = if page_row == sr { sc } else { 0 };
    let to = if page_row == er { ec } else { usize::MAX };
    let mut out = Vec::with_capacity(cells.len());
    let mut col = offset;
    for c in cells {
        let mut c = c.clone();
        if col >= from && col < to {
            c.style = c.style.add_modifier(Modifier::REVERSED);
        }
        col += crate::md::char_width(c.ch);
        out.push(c);
    }
    out
}

/// The image widget for one band. Cropping, not fitting: the protocol state
/// already holds the picture at the size of the whole band (see
/// `images::fit_px`), so a rect shorter than the band takes the slice of the
/// picture that belongs on those rows instead of squashing the whole thing
/// into them. `clip_top` keeps the bottom of the picture, for a band whose
/// first rows have scrolled off the top of the page.
fn cropped(clip_top: bool) -> StatefulImage<ratatui_image::protocol::StatefulProtocol> {
    StatefulImage::default().resize(Resize::Crop(Some(CropOptions {
        clip_top,
        clip_left: false,
    })))
}

/// Contiguous runs of cells belonging to the same link: (column, width, url).
fn link_runs(cells: &[PCell], rendered: &crate::render::Rendered) -> Vec<(usize, usize, String)> {
    let mut runs = Vec::new();
    let mut i = 0;
    let mut x = 0; // display column of cells[i]
    while i < cells.len() {
        match cells[i].link {
            Some(idx) => {
                let start = x;
                while i < cells.len() && cells[i].link == Some(idx) {
                    x += crate::md::char_width(cells[i].ch);
                    i += 1;
                }
                if let Some(url) = rendered.url(idx) {
                    runs.push((start, x - start, url.to_string()));
                }
            }
            None => {
                x += crate::md::char_width(cells[i].ch);
                i += 1;
            }
        }
    }
    runs
}

/// Word-wrap a rendered line into rows no wider than `width` *display columns*,
/// so a line of CJK or emoji wraps where it actually reaches the edge. The
/// segmentation is [`crate::md::wrap_breaks`], shared with the editor's soft
/// wrap; the preview's own rows are already indented by the renderer, so every
/// row here gets the full width.
fn wrap_cells(cells: &[PCell], width: usize) -> Vec<Vec<PCell>> {
    crate::render::wrap_pcells(cells, width)
}

/// The bottom line: what note you are on, what mode you are in, and what the
/// keys do — laid out so a narrow window loses the least useful part first
/// rather than letting the two halves collide.
fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    use crate::config::StatusItem;
    let note = app.active_note();
    // the whole path, `~/`-shortened: with quick-open reaching other folders,
    // *which* log.md you are in is the thing the bar alone can tell you
    let path = crate::index::short(&note.path);
    let name = note
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mode = match app.view {
        View::Edit => "edit",
        View::Preview => "preview",
    };
    let status = app.status.as_ref().map(|(m, _)| m.clone());
    let items = &app.config.status_bar_items;
    let shows = |i: StatusItem| items.contains(&i);

    // the left half is what the note is; the right half is what you can do
    let mut left_text = String::new();
    for item in items {
        let piece = match item {
            StatusItem::Path => path.clone(),
            StatusItem::Name => name.clone(),
            StatusItem::Mode => mode.to_string(),
            StatusItem::Properties => match properties_label(&note.content) {
                Some(s) => s,
                None => continue,
            },
            _ => continue,
        };
        if piece.is_empty() {
            continue;
        }
        if !left_text.is_empty() {
            left_text.push_str("  ");
        }
        left_text.push_str(&piece);
    }
    let status = status.filter(|_| shows(StatusItem::Message));
    // `key_hints` is the older, blunter switch for the same thing; off still
    // means no keys, whatever the item list says
    let hints = if shows(StatusItem::Keys) && app.config.key_hints {
        hint_pairs(app)
    } else {
        Vec::new()
    };

    let total = area.width as usize;
    let left_w = if left_text.is_empty() {
        0
    } else {
        crate::md::str_width(&left_text) + 1
    };
    let status_w = status
        .as_ref()
        .map(|m| crate::md::str_width(m) + 3)
        .unwrap_or(0);
    // three columns of air between the two halves, so a bar that is only just
    // wide enough still reads as two things and not one run-on string
    let room = total.saturating_sub(left_w + status_w + 3);

    // full hints if they fit; bare keys if they do not; then keys dropped from
    // the right, which is the order they are worth least in
    let right = fit_hints(&hints, room);

    // a long path gives up its head, not its tail: the filename is the part
    // that identifies the note
    let left = Line::from(Span::styled(
        format!(" {}", truncate_left(&left_text, total.saturating_sub(8))),
        dim(),
    ));
    let mut right_spans = Vec::new();
    if let Some(msg) = status {
        right_spans.push(Span::styled(format!("{msg}   "), theme::state()));
    }
    if !right.is_empty() {
        right_spans.push(Span::styled(format!("{right} "), dim()));
    }
    f.render_widget(Paragraph::new(left), area);
    f.render_widget(
        Paragraph::new(Line::from(right_spans)).right_aligned(),
        area,
    );
}

/// "2 properties", or nothing at all when the note has no front matter — a
/// note with no metadata should not have to say so. The count comes from the
/// note's content rather than the buffer because `sync_editor_to_note`
/// refreshes that on every edit, so the number moves as the block is typed.
fn properties_label(content: &str) -> Option<String> {
    crate::notes::front_matter_range(content)?;
    Some(match crate::notes::property_count(content) {
        1 => "1 property".to_string(),
        n => format!("{n} properties"),
    })
}

/// (key, what it does) for the status bar, in the order they are worth
/// keeping — built from the bindings in force, so a rebound key is right here
/// too and an unbound one is never offered.
fn hint_pairs(app: &App) -> Vec<(String, &'static str)> {
    use crate::keys::Action;
    let keys = &app.config.keys;
    let mut pairs: Vec<(String, &'static str)> = Vec::new();
    // ← → only earns a place in the bar when there is something to pan
    if app.view == View::Preview && app.preview_hmax > 0 {
        pairs.push(("← →".to_string(), "table"));
    }
    // only the two that get you everywhere else: the card lists every other
    // binding, and a bar that lists them all is a bar you stop reading
    for (action, what) in [(Action::Help, "help"), (Action::Quit, "quit")] {
        let key = keys.label(action);
        if !key.is_empty() {
            pairs.push((key, what));
        }
    }
    pairs
}

/// The most of `hints` that fits in `room` columns: worded if they all fit,
/// bare keys if they do not, and keys dropped from the right after that.
fn fit_hints(hints: &[(String, &'static str)], room: usize) -> String {
    let worded = hints
        .iter()
        .map(|(k, w)| format!("{k} {w}"))
        .collect::<Vec<_>>()
        .join("  ");
    if crate::md::str_width(&worded) <= room {
        return worded;
    }
    // the words are the first thing to go: a key on its own is still a
    // reminder that the key exists, which is all the bar is for
    let mut keys: Vec<&str> = hints.iter().map(|(k, _)| k.as_str()).collect();
    while !keys.is_empty() {
        let line = keys.join("  ");
        if crate::md::str_width(&line) <= room {
            return line;
        }
        keys.pop();
    }
    String::new()
}

fn overlay_rect(f: &Frame, height: u16) -> Rect {
    overlay_rect_wide(f, height, 72)
}

/// A centred overlay `height` rows tall, at most `max` columns wide. The
/// palette takes a bigger `max` than the little prompts: on a wide terminal
/// there is room for a description that does not end in an ellipsis, and no
/// reason to leave it empty.
fn overlay_rect_wide(f: &Frame, height: u16, max: u16) -> Rect {
    let area = f.area();
    let width = area.width.saturating_sub(4).min(max);
    let x = (area.width - width) / 2;
    Rect::new(x, overlay_top(f), width, height.min(overlay_budget(f)))
}

fn overlay_top(f: &Frame) -> u16 {
    (f.area().height / 5).max(1)
}

/// How many rows an overlay can actually occupy, given where `overlay_rect_wide`
/// puts it. Worth asking before laying anything out: `Layout` handed more
/// constraints than the box has rows gives the last ones a height of zero, and
/// a footer hint that is silently not there is worse than no hint at all.
fn overlay_budget(f: &Frame) -> u16 {
    f.area().height.saturating_sub(overlay_top(f) + 1)
}

fn draw_palette(f: &mut Frame, app: &mut App) {
    let quick = app.overlay == Overlay::QuickOpen;
    let browse = quick && app.browse;
    let items = palette_rows(app);
    // two border rows, the prompt, the rule under it, and the footer hint ^O
    // carries. Derived from the box's own budget rather than guessed at, so
    // the tree scrolls inside the rows it actually has on an 80x24 terminal.
    let chrome = if quick { 5 } else { 4 };
    // more rows on a taller window, since there is nothing else to use the
    // space for while the palette is open
    let room = overlay_budget(f).saturating_sub(chrome).clamp(1, 16) as usize;
    let shown = items.len().min(room) as u16;
    let rect = overlay_rect_wide(f, shown + chrome, 100);
    app.overlay_rect = rect;
    f.render_widget(Clear, rect);
    let block = panel(app);
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let rows = vec![Constraint::Length(1); shown as usize + 2 + usize::from(quick)];
    let chunks = Layout::vertical(rows).split(inner);

    // ❯ marks where you type, so the prompt reads as an input and not as the
    // first row of the list. The palette is monochrome throughout: it is
    // chrome over the note, and a hue here would compete with the one the
    // note itself uses for headings.
    let caret = Span::styled(" ❯ ", theme::bright());
    let prompt = if app.query.is_empty() {
        let hint = if browse {
            "browse folders — tab returns to search"
        } else if quick {
            "open a note — any folder, most recent first"
        } else {
            "search notes or run a command"
        };
        Line::from(vec![caret, Span::styled(hint, dim())])
    } else {
        Line::from(vec![
            caret,
            Span::styled(
                app.query.as_str(),
                Style::new().add_modifier(Modifier::BOLD),
            ),
        ])
    };
    f.render_widget(Paragraph::new(prompt), chunks[0]);
    f.set_cursor_position((
        chunks[0].x + 3 + crate::md::str_width(&app.query) as u16,
        chunks[0].y,
    ));
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(inner.width.saturating_sub(2) as usize),
            theme::border(),
        )))
        .style(theme::border()),
        chunks[1].inner(ratatui::layout::Margin {
            horizontal: 1,
            vertical: 0,
        }),
    );

    app.selected = app.selected.min(items.len().saturating_sub(1));
    app.palette_rows.clear();
    let start = app
        .selected
        .saturating_sub(shown.saturating_sub(1) as usize);
    // the widest key on screen, so the key column lines up down the list
    let keyw = items
        .iter()
        .filter_map(|r| match &r.item {
            Item::Command(c) => c.action().map(|a| app.config.keys.label(a)),
            _ => None,
        })
        .map(|k| crate::md::str_width(&k))
        .max()
        .unwrap_or(0);

    for (row_i, (i, row)) in items
        .iter()
        .enumerate()
        .skip(start)
        .take(shown as usize)
        .enumerate()
    {
        let area = chunks[row_i + 2];
        let selected = i == app.selected;
        let (name, detail, tag) = (&row.name, &row.detail, row.tag);
        let item = &row.item;
        // quick-open is all notes, so the tag would say the same thing on
        // every row; in the palette it is what tells a command from a note
        let tag = if quick && tag == "note" { "" } else { tag };
        let key = match item {
            Item::Command(c) => c
                .action()
                .map(|a| app.config.keys.label(a))
                .unwrap_or_default(),
            _ => String::new(),
        };

        // the whole row lifts onto a raised background rather than taking a
        // hue: monochrome, and it marks where you are without repainting the
        // row inside out
        let row_bg = if selected { theme::row() } else { Style::new() };
        let title = if selected {
            row_bg
                .fg(theme::palette().bright)
                .add_modifier(Modifier::BOLD)
        } else {
            row_bg
        };
        // a narrower name column leaves the description room to finish its
        // sentence, which is what a first-time reader is actually reading
        // …except in the tree, where the indent lives inside the name column:
        // a note three folders down would be all indent and an ellipsis at 22
        let namew = (area.width as usize)
            .saturating_sub(6 + keyw + 3 + tag_width(tag))
            .clamp(8, if browse { 40 } else { 22 });
        let padded = pad_to(name, namew);
        // three columns of air before the key, so the description never runs
        // into it however long it is
        let detailw =
            (area.width as usize).saturating_sub(4 + namew + 2 + keyw + 3 + tag_width(tag));

        let mut spans = vec![
            Span::styled("  ", row_bg),
            Span::styled(padded, title),
            Span::styled("  ", row_bg),
            Span::styled(truncate(detail, detailw), row_bg.patch(dim())),
        ];
        if !tag.is_empty() {
            spans.push(Span::styled(format!(" · {tag}"), row_bg.patch(dim())));
        }
        f.render_widget(Paragraph::new(Line::from(spans)).style(row_bg), area);
        // the key sits in its own right-hand column, where the eye can find
        // it without reading the description first
        if !key.is_empty() {
            let style = if selected {
                row_bg.fg(theme::palette().bright)
            } else {
                dim()
            };
            f.render_widget(
                Paragraph::new(Span::styled(format!("{key}  "), style))
                    .style(row_bg)
                    .right_aligned(),
                area,
            );
        }
        app.palette_rows.push((area, item.clone()));
    }

    // what tab does from here. Never pushed into `palette_rows`: a hint line
    // that opened a note when clicked would be a bug, not a shortcut.
    if quick {
        let hint = if browse {
            "←→ fold  ↵ open  tab: search"
        } else {
            "tab: browse"
        };
        f.render_widget(
            Paragraph::new(Span::styled(format!("  {hint}"), dim())),
            chunks[chunks.len() - 1],
        );
    }
}

/// One drawn palette row: what choosing it runs, and the strings it shows.
struct PRow {
    item: Item,
    name: String,
    detail: String,
    tag: &'static str,
}

/// The rows to draw, from whichever list the overlay is showing. Browse mode
/// builds its text here rather than in `row_text` because a tree row is not
/// one item looked up in isolation: its indent, its fold marker and its note
/// count all come from where it sits in the tree, and the tree is walked once
/// here rather than once per row.
fn palette_rows(app: &App) -> Vec<PRow> {
    if app.overlay == Overlay::QuickOpen && app.browse {
        let now = SystemTime::now();
        return app
            .browse_rows()
            .into_iter()
            .map(|r| match r.kind {
                RowKind::Folder {
                    key,
                    name,
                    notes,
                    open,
                } => PRow {
                    // ▾ and ▸ are one column wide and take no colour of their
                    // own, in keeping with the ‹ › ❯ the app already draws
                    name: format!(
                        "{}{} {name}",
                        "  ".repeat(r.depth),
                        if open { '▾' } else { '▸' }
                    ),
                    detail: match (open, notes) {
                        // an open folder's contents are right there to count
                        (true, _) => String::new(),
                        (false, 1) => "1 note".to_string(),
                        (false, n) => format!("{n} notes"),
                    },
                    tag: "",
                    item: Item::Folder(key),
                },
                RowKind::Note {
                    entry,
                    name,
                    modified,
                } => PRow {
                    // one level further in than its folder's name, so the
                    // fold marker column stays the folder's own
                    name: format!("{}{name}", "  ".repeat(r.depth + 1)),
                    detail: crate::index::age(modified, now),
                    tag: "",
                    item: Item::Entry(entry),
                },
            })
            .collect();
    }
    app.overlay_items()
        .into_iter()
        .map(|item| {
            let (name, detail, tag) = row_text(app, &item);
            PRow {
                item,
                name,
                detail,
                tag,
            }
        })
        .collect()
}

/// Title, description and type tag for one palette row.
fn row_text(app: &App, item: &Item) -> (String, String, &'static str) {
    match item {
        Item::Command(c) => {
            let (n, d) = c.label();
            (n.to_string(), d.to_string(), "")
        }
        Item::Note(idx) => {
            let n = &app.notes[*idx];
            // the filename, as in the open picker: the search ranks it first,
            // then the title, then the body
            (n.name(), n.snippet(), "note")
        }
        // a typed path that exists — labelled so it is clear this is the file
        // on disk and not a search hit
        Item::Path(path) => (
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            crate::index::short(path.parent().unwrap_or(path)),
            "path",
        ),
        Item::Entry(idx) => match app.open_index.get(*idx) {
            // the filename, not the title: it is the name the note is found
            // under on disk, and the one a wikilink reaches it by
            Some(e) => (e.name(), e.folder.clone(), "note"),
            None => (String::new(), String::new(), ""),
        },
        // a folder row writes its own text in `palette_rows`, where the tree
        // around it is still in hand; it never reaches here
        Item::Folder(_) => (String::new(), String::new(), ""),
    }
}

fn tag_width(tag: &str) -> usize {
    if tag.is_empty() {
        0
    } else {
        crate::md::str_width(tag) + 3
    }
}

/// `text` cut to `width` columns and padded out to it, so a column of them
/// lines up whatever is in each.
fn pad_to(text: &str, width: usize) -> String {
    let cut = truncate(text, width);
    let pad = width.saturating_sub(crate::md::str_width(&cut));
    format!("{cut}{}", " ".repeat(pad))
}

fn truncate(text: &str, width: usize) -> String {
    crate::md::truncate(text, width)
}

/// The help card: every binding, in the groups `app::SHORTCUTS` declares. Sized
/// to its content and centred, and dismissed by any key at all — it is a
/// reference to glance at, not a mode to get stuck in.
/// The peek popup: the first rows of the note a hovered (or ⌥P'd) wikilink
/// points at, floated just under the link — or above it when the link is
/// near the bottom — and titled with the file's name.
fn draw_peek(f: &mut Frame, app: &App) {
    let Some(peek) = &app.peek else {
        return;
    };
    let screen = f.area();
    let width = 60.min(screen.width.saturating_sub(2)).max(10);
    let inner_w = width.saturating_sub(2) as usize;
    let rendered = crate::render::render_page_at(&peek.body, 0, inner_w, app.config.table_style);
    let mut lines: Vec<Line> = rendered
        .lines
        .iter()
        .take(crate::app::PEEK_ROWS)
        .map(|l| crate::render::to_line(&l.cells))
        .collect();
    if lines.is_empty() {
        lines.push(Line::from(Span::styled("(empty note)", dim())));
    } else if rendered.lines.len() > crate::app::PEEK_ROWS {
        lines.push(Line::from(Span::styled("…", dim())));
    }
    let height = (lines.len() as u16 + 2).min(screen.height);

    // under the link when there is room, over it when there is not, and
    // never off the right edge
    let a = peek.anchor;
    let y = if a.y + 1 + height <= screen.height {
        a.y + 1
    } else {
        a.y.saturating_sub(height)
    };
    let x = a.x.min(screen.width.saturating_sub(width));
    let rect = Rect::new(x, y, width, height);

    f.render_widget(Clear, rect);
    let title = format!(" {} ", truncate(&peek.name, inner_w.saturating_sub(2)));
    let block = panel(app).title(Span::styled(title, theme::state()));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_help(f: &mut Frame, app: &App) {
    // the settable bindings first, as the settings currently have them
    let bound = app.config.keys.card_rows();
    let groups: Vec<(&str, Vec<(String, &str)>)> = std::iter::once(("keys", bound))
        .chain(crate::app::SHORTCUTS.iter().map(|(g, rows)| {
            (
                *g,
                rows.iter()
                    .map(|(k, w)| (k.to_string(), *w))
                    .collect::<Vec<_>>(),
            )
        }))
        .collect();

    // the widest key column, so the descriptions line up down the whole card
    let keyw = groups
        .iter()
        .flat_map(|(_, rows)| rows.iter())
        .map(|(k, _)| crate::md::str_width(k))
        .max()
        .unwrap_or(0);

    // typing filters the card: a row matches on its keys, its description or
    // its group, so "save" finds ^S and "word" finds ⌥←
    let q = app.help_query.trim();
    let groups: Vec<(&str, Vec<(String, &str)>)> = groups
        .into_iter()
        .map(|(group, rows)| {
            let rows = rows
                .into_iter()
                .filter(|(k, w)| crate::search::fuzzy(q, &format!("{group} {k} {w}")).is_some())
                .collect::<Vec<_>>();
            (group, rows)
        })
        .filter(|(_, rows)| !rows.is_empty())
        .collect();

    let mut lines: Vec<Line> = Vec::new();
    for (i, (group, rows)) in groups.iter().enumerate() {
        if i > 0 {
            lines.push(Line::default());
        }
        lines.push(Line::from(Span::styled(
            format!(" {group}"),
            theme::bright().add_modifier(Modifier::BOLD),
        )));
        for (key, what) in rows.iter() {
            let pad = " ".repeat(keyw.saturating_sub(crate::md::str_width(key)));
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {pad}{key}  "),
                    theme::bright().add_modifier(Modifier::BOLD),
                ),
                Span::styled(what.to_string(), dim()),
            ]));
        }
    }
    if groups.is_empty() {
        lines.push(Line::from(Span::styled(" nothing matches", dim())));
    }
    lines.push(Line::default());
    lines.push(if q.is_empty() {
        Line::from(Span::styled(" type to search · esc closes", dim()))
    } else {
        Line::from(vec![
            Span::styled(" search ", dim()),
            Span::styled(app.help_query.clone(), theme::state()),
            Span::styled("  · esc closes", dim()),
        ])
    });

    let area = f.area();
    let width = (keyw as u16 + 54).min(area.width.saturating_sub(2));
    let height = (lines.len() as u16 + 2).min(area.height);
    let rect = Rect::new(
        (area.width.saturating_sub(width)) / 2,
        (area.height.saturating_sub(height)) / 2,
        width,
        height,
    );
    f.render_widget(Clear, rect);
    let block = panel(app).title(Span::styled(" help ", theme::state()));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    f.render_widget(Paragraph::new(lines), inner);
}

/// The prompt a `[[wikilink]]` that resolves to nothing puts up. No danger
/// border: making a note is not a destructive thing, and borrowing the delete
/// prompt's colour for it would teach the eye the wrong lesson.
fn draw_create(f: &mut Frame, app: &mut App) {
    let rect = overlay_rect(f, 4);
    f.render_widget(Clear, rect);
    let block = panel(app);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    // what will be made and where it will land, so the prompt is never a
    // surprise: the filename the target ends in, in the note's own folder plus
    // whatever folder part the target carried
    let (name, dir) = app.pending_create().unwrap_or_else(|| {
        (
            app.pending_link.clone().unwrap_or_default(),
            crate::index::short(&app.note_dir()),
        )
    });
    let lines = vec![
        Line::from(Span::styled(
            format!(" create \u{201c}{name}\u{201d}?"),
            theme::state().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(" this makes a new note in {dir}. enter to confirm, esc to cancel."),
            dim(),
        )),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_confirm(f: &mut Frame, app: &mut App) {
    let rect = overlay_rect(f, 4);
    f.render_widget(Clear, rect);
    let block = panel(app).border_style(theme::danger());
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let title = app.active_note().title();
    let lines = vec![
        Line::from(Span::styled(
            format!(" delete “{title}”?"),
            theme::danger().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            " this deletes the file. enter to confirm, esc to cancel.",
            dim(),
        )),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

/// The inline rename prompt: the same box as the delete confirmation, with an
/// editable filename stem instead of a question.
fn draw_rename(f: &mut Frame, app: &mut App) {
    let rect = overlay_rect(f, 4);
    f.render_widget(Clear, rect);
    let block = panel(app);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let lines = vec![
        Line::from(vec![
            Span::styled(" rename file  ", theme::state()),
            Span::styled(
                app.rename_input.as_str(),
                Style::new().add_modifier(Modifier::BOLD),
            ),
            Span::styled(".md", dim()),
        ]),
        Line::from(Span::styled(
            " enter renames the file on disk, esc cancels.",
            dim(),
        )),
    ];
    f.render_widget(Paragraph::new(lines), inner);
    f.set_cursor_position((
        inner.x + 14 + crate::md::str_width(&app.rename_input) as u16,
        inner.y,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::render;

    fn cells(text: &str) -> Vec<PCell> {
        text.chars()
            .map(|ch| PCell {
                ch,
                style: Style::default(),
                link: None,
                src: None,
            })
            .collect()
    }

    #[test]
    fn wrapping_breaks_on_spaces_and_keeps_every_word() {
        let rows = wrap_cells(&cells("the quick brown fox"), 10);
        let text: Vec<String> = rows
            .iter()
            .map(|r| r.iter().map(|c| c.ch).collect())
            .collect();
        assert_eq!(text, vec!["the quick", "brown fox"]);
        assert!(rows.iter().all(|r| r.len() <= 10));
    }

    #[test]
    fn wrapping_a_short_line_leaves_it_alone() {
        let rows = wrap_cells(&cells("short"), 10);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn wrapping_counts_display_columns_not_characters() {
        // eight CJK characters are sixteen columns wide
        let rows = wrap_cells(&cells("漢字漢字漢字漢字"), 10);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| crate::render::cells_width(r) <= 10));
    }

    #[test]
    fn the_status_bar_counts_properties_only_when_there_are_some_to_count() {
        assert_eq!(
            properties_label("---\ntype: log\ntags: work\n---\n# t\n").as_deref(),
            Some("2 properties")
        );
        assert_eq!(
            properties_label("---\ntype: log\n---\n").as_deref(),
            Some("1 property")
        );
        // an empty block still says so: it is front matter, it just has
        // nothing in it yet
        assert_eq!(
            properties_label("---\n---\nbody\n").as_deref(),
            Some("0 properties")
        );
        // no block at all leaves the item out of the bar entirely
        assert_eq!(properties_label("# Title\n\nbody\n"), None);
        assert_eq!(properties_label("---\nunterminated: yes\n"), None);
    }

    #[test]
    fn link_runs_cover_the_link_text_only() {
        let r = render("see [docs](http://x.y) now");
        let line = &r.lines[0];
        let runs = link_runs(&line.cells, &r);
        assert_eq!(runs.len(), 1);
        let (start, len, url) = &runs[0];
        assert_eq!(*start, 4);
        assert_eq!(*len, 4);
        assert_eq!(url, "http://x.y");
    }
}

/// Cut from the left, with a leading `…`: for a path, whose end is the part
/// worth keeping.
fn truncate_left(text: &str, width: usize) -> String {
    if crate::md::str_width(text) <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in text.chars().rev() {
        let cw = crate::md::str_width(&c.to_string());
        if w + cw + 1 > width {
            break;
        }
        out.insert(0, c);
        w += cw;
    }
    format!("…{out}")
}

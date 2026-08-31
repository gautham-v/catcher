use crate::app::{App, Item, Overlay, View};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

const DIM: Style = Style::new().fg(Color::DarkGray);

pub fn draw(f: &mut Frame, app: &mut App) {
    let [content, status] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(f.area());

    // a centred column, like a note page
    let width = content.width.min(100);
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
        View::Edit => f.render_widget(&app.textarea, page),
        View::Preview => {
            let text = crate::render::render(&app.active_note().content);
            f.render_widget(
                Paragraph::new(text)
                    .wrap(Wrap { trim: false })
                    .scroll((app.preview_scroll, 0)),
                page,
            );
        }
    }

    draw_status(f, app, status);

    match app.overlay {
        Overlay::Palette => draw_palette(f, app),
        Overlay::ConfirmDelete => draw_confirm(f, app),
        Overlay::None => {}
    }
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let title = app.active_note().title();
    let mode = match app.view {
        View::Edit => "",
        View::Preview => "  preview",
    };
    let left = Line::from(vec![
        Span::styled(format!(" {title}"), DIM),
        Span::styled(mode, Style::new().fg(Color::Yellow)),
    ]);
    let mut right_spans = Vec::new();
    if let Some((msg, _)) = &app.status {
        right_spans.push(Span::styled(
            format!("{msg}   "),
            Style::new().fg(Color::Magenta),
        ));
    }
    right_spans.push(Span::styled(
        format!(
            "{}  {} note{}  ^K palette  ^N new  ^P preview  ^Q quit ",
            if app.saved() { "saved" } else { "…" },
            app.notes.len(),
            if app.notes.len() == 1 { "" } else { "s" },
        ),
        DIM,
    ));
    let right = Line::from(right_spans);
    f.render_widget(Paragraph::new(left), area);
    f.render_widget(Paragraph::new(right).right_aligned(), area);
}

fn overlay_rect(f: &Frame, height: u16) -> Rect {
    let area = f.area();
    let width = area.width.saturating_sub(4).min(72);
    let x = (area.width - width) / 2;
    let y = (area.height / 5).max(1);
    Rect::new(x, y, width, height.min(area.height.saturating_sub(y + 1)))
}

fn draw_palette(f: &mut Frame, app: &mut App) {
    let items = app.palette_items();
    let shown = items.len().min(10) as u16;
    let rect = overlay_rect(f, shown + 3);
    f.render_widget(Clear, rect);
    let block = Block::default().borders(Borders::ALL).border_style(DIM);
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let mut rows = vec![Constraint::Length(1); shown as usize + 1];
    rows[0] = Constraint::Length(1);
    let chunks = Layout::vertical(rows).split(inner);

    let prompt = if app.query.is_empty() {
        Line::from(Span::styled("search notes or run a command…", DIM))
    } else {
        Line::from(app.query.as_str())
    };
    f.render_widget(Paragraph::new(prompt), chunks[0]);
    f.set_cursor_position((chunks[0].x + app.query.len() as u16, chunks[0].y));

    app.selected = app.selected.min(items.len().saturating_sub(1));
    app.palette_rows.clear();
    let start = app
        .selected
        .saturating_sub(shown.saturating_sub(1) as usize);
    for (row_i, (i, item)) in items
        .iter()
        .enumerate()
        .skip(start)
        .take(shown as usize)
        .enumerate()
    {
        let area = chunks[row_i + 1];
        let selected = i == app.selected;
        let (name, detail, tag) = match item {
            Item::Command(c) => {
                let (n, d) = c.label();
                (n.to_string(), d.to_string(), "command")
            }
            Item::Note(idx) => {
                let n = &app.notes[*idx];
                (n.title(), n.snippet(), "")
            }
        };
        let base = if selected {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new()
        };
        let line = Line::from(vec![
            Span::styled(format!(" {name}  "), base.add_modifier(Modifier::BOLD)),
            Span::styled(detail.chars().take(40).collect::<String>(), base.patch(DIM)),
        ]);
        f.render_widget(Paragraph::new(line).style(base), area);
        if !tag.is_empty() {
            f.render_widget(
                Paragraph::new(Span::styled(format!("{tag} "), base.patch(DIM))).right_aligned(),
                area,
            );
        }
        app.palette_rows.push((area, item.clone()));
    }
}

fn draw_confirm(f: &mut Frame, app: &mut App) {
    let rect = overlay_rect(f, 4);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Red));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let title = app.active_note().title();
    let lines = vec![
        Line::from(Span::styled(
            format!(" delete “{title}”?"),
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            " this deletes the file. enter to confirm, esc to cancel.",
            DIM,
        )),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

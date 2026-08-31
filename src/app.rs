use crate::notes::{self, Note};
use crate::search;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tui_textarea::TextArea;

const AUTOSAVE_AFTER: Duration = Duration::from_millis(500);

#[derive(PartialEq, Clone, Copy)]
pub enum View {
    Edit,
    Preview,
}

#[derive(PartialEq, Clone, Copy)]
pub enum Overlay {
    None,
    Palette,
    ConfirmDelete,
}

#[derive(Clone, PartialEq)]
pub enum Command {
    NewNote,
    DeleteNote,
    TogglePreview,
    Quit,
}

impl Command {
    pub fn label(&self) -> (&'static str, &'static str) {
        match self {
            Command::NewNote => ("New note", "create an empty note and start typing"),
            Command::DeleteNote => ("Delete note", "remove the note on screen"),
            Command::TogglePreview => ("Toggle preview", "rendered markdown, read-only"),
            Command::Quit => ("Quit", "save and exit"),
        }
    }
}

#[derive(Clone, PartialEq)]
pub enum Item {
    Note(usize),
    Command(Command),
}

pub struct App {
    pub dir: PathBuf,
    pub notes: Vec<Note>,
    pub active: usize,
    pub textarea: TextArea<'static>,
    pub view: View,
    pub overlay: Overlay,
    pub query: String,
    pub selected: usize,
    pub preview_scroll: u16,
    pub status: Option<(String, Instant)>,
    pub quit: bool,
    dirty: bool,
    last_edit: Instant,
    // rects cached from the last draw, for mouse hit-testing
    pub editor_area: Rect,
    pub palette_rows: Vec<(Rect, Item)>,
}

impl App {
    pub fn new() -> Result<Self> {
        let dir = notes::notes_dir()?;
        let mut all = notes::load_all(&dir)?;
        if all.is_empty() {
            all.push(notes::create(&dir)?);
        }
        let mut app = App {
            dir,
            notes: all,
            active: 0,
            textarea: TextArea::default(),
            view: View::Edit,
            overlay: Overlay::None,
            query: String::new(),
            selected: 0,
            preview_scroll: 0,
            status: None,
            quit: false,
            dirty: false,
            last_edit: Instant::now(),
            editor_area: Rect::default(),
            palette_rows: Vec::new(),
        };
        app.load_active_into_editor();
        Ok(app)
    }

    pub fn active_note(&self) -> &Note {
        &self.notes[self.active]
    }

    fn load_active_into_editor(&mut self) {
        let lines: Vec<String> = self.notes[self.active]
            .content
            .lines()
            .map(String::from)
            .collect();
        let mut ta = TextArea::new(lines);
        ta.set_cursor_line_style(ratatui::style::Style::default());
        self.textarea = ta;
        self.preview_scroll = 0;
    }

    fn sync_editor_to_note(&mut self) {
        let content = self.textarea.lines().join("\n");
        if content != self.notes[self.active].content {
            self.notes[self.active].content = content;
            self.dirty = true;
            self.last_edit = Instant::now();
        }
    }

    pub fn save_now(&mut self) {
        if !self.dirty {
            return;
        }
        match notes::save(&self.dir, &self.notes[self.active]) {
            Ok(path) => {
                self.notes[self.active].path = path;
                self.notes[self.active].modified = std::time::SystemTime::now();
                self.dirty = false;
            }
            Err(e) => self.flash(format!("save failed: {e}")),
        }
    }

    pub fn maybe_autosave(&mut self) {
        if self.dirty && self.last_edit.elapsed() >= AUTOSAVE_AFTER {
            self.save_now();
        }
    }

    pub fn saved(&self) -> bool {
        !self.dirty
    }

    pub fn flash(&mut self, msg: String) {
        self.status = Some((msg, Instant::now()));
    }

    pub fn tick(&mut self) {
        self.maybe_autosave();
        if let Some((_, at)) = self.status {
            if at.elapsed() > Duration::from_secs(3) {
                self.status = None;
            }
        }
    }

    fn switch_to(&mut self, idx: usize) {
        self.save_now();
        self.active = idx;
        self.view = View::Edit;
        self.load_active_into_editor();
    }

    fn new_note(&mut self) {
        self.save_now();
        match notes::create(&self.dir) {
            Ok(n) => {
                self.notes.insert(0, n);
                self.active = 0;
                self.view = View::Edit;
                self.load_active_into_editor();
            }
            Err(e) => self.flash(format!("create failed: {e}")),
        }
    }

    fn delete_active(&mut self) {
        let title = self.active_note().title();
        if let Err(e) = notes::delete(&self.notes[self.active]) {
            self.flash(format!("delete failed: {e}"));
            return;
        }
        self.notes.remove(self.active);
        self.dirty = false;
        if self.notes.is_empty() {
            match notes::create(&self.dir) {
                Ok(n) => self.notes.push(n),
                Err(e) => {
                    self.flash(format!("create failed: {e}"));
                    self.quit = true;
                    return;
                }
            }
        }
        self.active = 0;
        self.load_active_into_editor();
        self.view = View::Edit;
        self.flash(format!("deleted “{title}”"));
    }

    /// Palette rows for the current query: commands and notes, best first.
    pub fn palette_items(&self) -> Vec<Item> {
        let commands = [
            Command::NewNote,
            Command::DeleteNote,
            Command::TogglePreview,
            Command::Quit,
        ];
        let mut scored: Vec<(i64, Item)> = Vec::new();
        for c in commands {
            if let Some(s) = search::fuzzy(&self.query, c.label().0) {
                // with no query, commands lead; with a query, notes outrank them slightly
                let bias = if self.query.is_empty() { 1000 } else { 0 };
                scored.push((s + bias, Item::Command(c)));
            }
        }
        for (i, n) in self.notes.iter().enumerate() {
            if let Some(s) = search::score_note(&self.query, &n.title(), &n.content) {
                scored.push((s, Item::Note(i)));
            }
        }
        scored.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
        scored.into_iter().map(|(_, it)| it).collect()
    }

    fn run_item(&mut self, item: Item) {
        self.overlay = Overlay::None;
        match item {
            Item::Note(i) => self.switch_to(i),
            Item::Command(Command::NewNote) => self.new_note(),
            Item::Command(Command::TogglePreview) => self.toggle_preview(),
            Item::Command(Command::Quit) => {
                self.save_now();
                self.quit = true;
            }
            Item::Command(Command::DeleteNote) => {
                self.overlay = Overlay::ConfirmDelete;
            }
        }
    }

    fn toggle_preview(&mut self) {
        self.view = match self.view {
            View::Edit => View::Preview,
            View::Preview => View::Edit,
        };
        self.preview_scroll = 0;
    }

    fn open_palette(&mut self) {
        self.query.clear();
        self.selected = 0;
        self.overlay = Overlay::Palette;
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // global bindings
        match (ctrl, key.code) {
            (true, KeyCode::Char('q')) | (true, KeyCode::Char('c')) => {
                self.sync_editor_to_note();
                self.save_now();
                self.quit = true;
                return;
            }
            (true, KeyCode::Char('k')) => {
                if self.overlay == Overlay::Palette {
                    self.overlay = Overlay::None;
                } else {
                    self.open_palette();
                }
                return;
            }
            (true, KeyCode::Char('n')) => {
                self.overlay = Overlay::None;
                self.new_note();
                return;
            }
            (true, KeyCode::Char('p')) if self.overlay == Overlay::None => {
                self.toggle_preview();
                return;
            }
            _ => {}
        }

        match self.overlay {
            Overlay::Palette => self.on_palette_key(key),
            Overlay::ConfirmDelete => match key.code {
                KeyCode::Enter => {
                    self.overlay = Overlay::None;
                    self.delete_active();
                }
                KeyCode::Esc => self.overlay = Overlay::None,
                _ => {}
            },
            Overlay::None => match self.view {
                View::Preview => match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.preview_scroll = self.preview_scroll.saturating_sub(1)
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.preview_scroll = self.preview_scroll.saturating_add(1)
                    }
                    KeyCode::PageUp => self.preview_scroll = self.preview_scroll.saturating_sub(10),
                    KeyCode::PageDown => {
                        self.preview_scroll = self.preview_scroll.saturating_add(10)
                    }
                    KeyCode::Esc | KeyCode::Enter | KeyCode::Char('e') => self.view = View::Edit,
                    _ => {}
                },
                View::Edit => {
                    self.textarea.input(key);
                    self.sync_editor_to_note();
                }
            },
        }
    }

    fn on_palette_key(&mut self, key: KeyEvent) {
        let count = self.palette_items().len();
        match key.code {
            KeyCode::Esc => self.overlay = Overlay::None,
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                if count > 0 && self.selected + 1 < count {
                    self.selected += 1;
                }
            }
            KeyCode::Enter => {
                if let Some(item) = self.palette_items().get(self.selected).cloned() {
                    self.run_item(item);
                }
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.selected = 0;
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.query.push(c);
                self.selected = 0;
            }
            _ => {}
        }
    }

    pub fn on_mouse(&mut self, ev: MouseEvent) {
        match ev.kind {
            MouseEventKind::ScrollUp => match (self.overlay, self.view) {
                (Overlay::Palette, _) => self.selected = self.selected.saturating_sub(1),
                (_, View::Preview) => self.preview_scroll = self.preview_scroll.saturating_sub(2),
                (_, View::Edit) => self
                    .textarea
                    .scroll(tui_textarea::Scrolling::Delta { rows: -2, cols: 0 }),
            },
            MouseEventKind::ScrollDown => match (self.overlay, self.view) {
                (Overlay::Palette, _) => {
                    let count = self.palette_items().len();
                    if count > 0 && self.selected + 1 < count {
                        self.selected += 1;
                    }
                }
                (_, View::Preview) => self.preview_scroll = self.preview_scroll.saturating_add(2),
                (_, View::Edit) => self
                    .textarea
                    .scroll(tui_textarea::Scrolling::Delta { rows: 2, cols: 0 }),
            },
            MouseEventKind::Down(MouseButton::Left) => {
                let (x, y) = (ev.column, ev.row);
                if self.overlay == Overlay::Palette {
                    if let Some((_, item)) = self
                        .palette_rows
                        .iter()
                        .find(|(r, _)| r.contains(ratatui::layout::Position { x, y }))
                        .cloned()
                    {
                        self.run_item(item);
                    } else {
                        self.overlay = Overlay::None; // click outside dismisses
                    }
                } else if self.overlay == Overlay::ConfirmDelete {
                    self.overlay = Overlay::None;
                } else if self.view == View::Preview
                    && self
                        .editor_area
                        .contains(ratatui::layout::Position { x, y })
                {
                    self.view = View::Edit;
                }
            }
            _ => {}
        }
    }
}

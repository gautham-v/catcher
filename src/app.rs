use crate::config::Config;
use crate::editor::{Editor, Pos};
use crate::images::Images;
use crate::md;
use crate::notes::{self, Note};
use crate::search;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const AUTOSAVE_AFTER: Duration = Duration::from_millis(500);

/// Which note a CLI invocation asked to open.
enum Want {
    Path(PathBuf),
    Title(String),
}

/// The note whose *title* best fuzzy-matches `name`, if any matches at all.
/// Bodies are deliberately not searched: `tinynote groceries` should either
/// land on the note called Groceries or make one, never on a note that merely
/// mentions the word.
pub fn best_title_match(notes: &[Note], name: &str) -> Option<usize> {
    notes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| search::fuzzy(name, &n.title()).map(|s| (s, i)))
        // ties go to the more recently modified note (notes are newest first)
        .max_by_key(|(s, i)| (*s, std::cmp::Reverse(*i)))
        .map(|(_, i)| i)
}

/// One drawn display row of the editor: which source line it belongs to, and
/// which of that line's soft-wrapped rows it is.
#[derive(Clone, Copy, Debug)]
pub struct EditRow {
    pub rect: Rect,
    pub line: usize,
    pub seg: usize,
}

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
    RenameFile,
}

#[derive(Clone, PartialEq)]
pub enum Command {
    NewNote,
    DeleteNote,
    RenameFile,
    TogglePreview,
    OpenSettings,
    Quit,
}

const COMMANDS: [Command; 6] = [
    Command::NewNote,
    Command::DeleteNote,
    Command::RenameFile,
    Command::TogglePreview,
    Command::OpenSettings,
    Command::Quit,
];

impl Command {
    pub fn label(&self) -> (&'static str, &'static str) {
        match self {
            Command::NewNote => ("New note", "create an empty note and start typing"),
            Command::DeleteNote => ("Delete note", "remove the note on screen"),
            Command::RenameFile => ("Rename file", "change the filename on disk"),
            Command::TogglePreview => ("Toggle preview", "rendered markdown, read-only"),
            Command::OpenSettings => ("Open settings", "edit config.toml in $EDITOR"),
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
    pub config: Config,
    pub dir: PathBuf,
    pub notes: Vec<Note>,
    pub active: usize,
    pub editor: Editor,
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
    /// The screen band each *display row* of the editor occupied in the last
    /// draw. A soft-wrapped line contributes one entry per wrapped row and a
    /// picture one tall entry, so this is what turns a click back into a
    /// (line, wrapped row) pair.
    pub edit_rows: Vec<EditRow>,
    pub palette_rows: Vec<(Rect, Item)>,
    /// Preview hit regions from the last draw: link spans, checkbox lines, and
    /// every row's source line (for click → edit at the same place).
    pub preview_links: Vec<(Rect, String)>,
    pub preview_checkboxes: Vec<(Rect, usize)>,
    pub preview_rows: Vec<(Rect, usize, Vec<crate::render::PCell>)>,
    pub images: Images,
    dragging: bool,
    /// Set when the palette asks for the config file; `main` suspends the TUI,
    /// runs $EDITOR, and clears it.
    pub edit_config: bool,
    /// Buffer for the inline rename prompt.
    pub rename_input: String,
    /// True when the session is rooted outside the configured notes dir (a
    /// `tinynote <file>` / `<dir>` invocation). Foreign filenames are then
    /// never renamed to follow a title — Obsidian links depend on them — and
    /// image paste is refused rather than scattering attachments about.
    pub foreign_root: bool,
}

impl App {
    /// Build the app for one of the CLI's launch shapes.
    pub fn launch(launch: crate::cli::Launch) -> Result<Self> {
        use crate::cli::Launch;
        let config = Config::load()?;
        config.ensure_dirs()?;

        // where this session is rooted, and which note it should open on
        let (dir, want): (PathBuf, Option<Want>) = match &launch {
            Launch::Default => (config.notes_dir.clone(), None),
            Launch::Name(n) => (config.notes_dir.clone(), Some(Want::Title(n.clone()))),
            Launch::Dir(d) => (std::fs::canonicalize(d).unwrap_or_else(|_| d.clone()), None),
            Launch::File(f) => {
                let f = std::fs::canonicalize(f).unwrap_or_else(|_| f.clone());
                let parent = f
                    .parent()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("."));
                (parent, Some(Want::Path(f)))
            }
        };
        let foreign_root = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone())
            != std::fs::canonicalize(&config.notes_dir)
                .unwrap_or_else(|_| config.notes_dir.clone());

        let mut all = notes::load_all(&dir)?;
        let mut active = 0;
        match want {
            Some(Want::Path(p)) => {
                active = all.iter().position(|n| n.path == p).unwrap_or(0);
            }
            Some(Want::Title(name)) => match best_title_match(&all, &name) {
                Some(i) => active = i,
                None => {
                    all.insert(0, notes::create_with(&dir, format!("# {name}\n"))?);
                    active = 0;
                }
            },
            None => {}
        }
        if all.is_empty() {
            all.push(notes::create(&dir)?);
        }

        let mut app = App {
            rename_input: String::new(),
            foreign_root,
            images: Images::new(config.attachments_dir.clone()),
            config,
            dir,
            notes: all,
            active,
            editor: Editor::default(),
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
            edit_rows: Vec::new(),
            palette_rows: Vec::new(),
            preview_links: Vec::new(),
            preview_checkboxes: Vec::new(),
            preview_rows: Vec::new(),
            dragging: false,
            edit_config: false,
        };
        app.load_active_into_editor();
        Ok(app)
    }

    pub fn active_note(&self) -> &Note {
        &self.notes[self.active]
    }

    fn load_active_into_editor(&mut self) {
        self.editor = Editor::new(&self.notes[self.active].content);
        self.preview_scroll = 0;
    }

    fn sync_editor_to_note(&mut self) {
        let content = self.editor.text();
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
        let allow_rename = !self.foreign_root;
        let dir = self.dir.clone();
        match notes::save(&dir, &mut self.notes[self.active], allow_rename) {
            Ok(_) => self.dirty = false,
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
        let mut scored: Vec<(i64, Item)> = Vec::new();
        for c in COMMANDS {
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
            Item::Command(Command::OpenSettings) => {
                self.save_now();
                self.edit_config = true;
            }
            Item::Command(Command::Quit) => {
                self.save_now();
                self.quit = true;
            }
            Item::Command(Command::DeleteNote) => {
                self.overlay = Overlay::ConfirmDelete;
            }
            Item::Command(Command::RenameFile) => self.open_rename(),
        }
    }

    fn open_rename(&mut self) {
        self.save_now();
        self.rename_input = self
            .active_note()
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.overlay = Overlay::RenameFile;
    }

    /// Commit the inline rename. The file moves; the title is left alone, and
    /// because the filename no longer matches it, saves stop renaming.
    fn commit_rename(&mut self) {
        self.overlay = Overlay::None;
        let stem = self.rename_input.clone();
        if stem.trim().is_empty() {
            self.flash("rename cancelled — empty name".to_string());
            return;
        }
        match notes::rename_file(&mut self.notes[self.active], &stem) {
            Ok(path) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.flash(format!("renamed → {name}"));
            }
            Err(e) => self.flash(format!("rename failed: {e}")),
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
            (true, KeyCode::Char('q')) => {
                self.sync_editor_to_note();
                self.save_now();
                self.quit = true;
                return;
            }
            (true, KeyCode::Char('c')) => {
                self.copy_selection();
                return;
            }
            (true, KeyCode::Char('v')) => {
                self.overlay = Overlay::None;
                self.paste();
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
            (true, KeyCode::Char('p')) => {
                self.overlay = Overlay::None;
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
            Overlay::RenameFile => match key.code {
                KeyCode::Enter => self.commit_rename(),
                KeyCode::Esc => self.overlay = Overlay::None,
                KeyCode::Backspace => {
                    self.rename_input.pop();
                }
                KeyCode::Char(c) if !ctrl => self.rename_input.push(c),
                _ => {}
            },
            Overlay::None => match self.view {
                View::Preview => match key.code {
                    KeyCode::Up => self.preview_scroll = self.preview_scroll.saturating_sub(1),
                    KeyCode::Down => self.preview_scroll = self.preview_scroll.saturating_add(1),
                    KeyCode::PageUp => self.preview_scroll = self.preview_scroll.saturating_sub(10),
                    KeyCode::PageDown => {
                        self.preview_scroll = self.preview_scroll.saturating_add(10)
                    }
                    KeyCode::Esc | KeyCode::Enter | KeyCode::Char('e') => self.view = View::Edit,
                    _ => {}
                },
                View::Edit => {
                    // Up/Down move by display row, which only the view knows,
                    // so they are handled here rather than in the buffer.
                    // ⌘↑/⌘↓ (document ends) stay with the buffer.
                    let plain = !key.modifiers.intersects(
                        KeyModifiers::SUPER | KeyModifiers::CONTROL | KeyModifiers::ALT,
                    );
                    let select = key.modifiers.contains(KeyModifiers::SHIFT);
                    match key.code {
                        KeyCode::Up if plain => self.move_vertical(false, select),
                        KeyCode::Down if plain => self.move_vertical(true, select),
                        _ => {
                            if self.editor.on_key(key) {
                                self.sync_editor_to_note();
                            }
                        }
                    }
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

    /// Map a screen cell to a source position, undoing the centred-column
    /// offsets, the scroll, and any marker hiding on the clicked line.
    ///
    /// Rows are looked up in the bands the last draw recorded, because a drawn
    /// image is taller than one screen row; only if nothing was drawn yet does
    /// the plain one-row-per-line arithmetic stand in.
    pub fn pos_at(&self, x: u16, y: u16) -> Pos {
        let (arith_row, dcol) = screen_to_cell(self.editor_area, self.editor.scroll, x, y);
        let hit = self
            .edit_rows
            .iter()
            .find(|r| y >= r.rect.y && y < r.rect.y + r.rect.height)
            .or_else(|| {
                // above or below the drawn bands: clamp to the first/last one
                let first = self.edit_rows.first()?;
                let last = self.edit_rows.last()?;
                Some(if y < first.rect.y { first } else { last })
            });
        let (row, seg) = match hit {
            Some(r) => (r.line, r.seg),
            None => (arith_row, 0),
        };
        let row = row.min(self.editor.lines().len().saturating_sub(1));
        // a line drawn as a picture has no columns to click; reveal it at its start
        let blocks = self.blocks();
        if self.drawn_image_row(&blocks, row) {
            return self.editor.clamp((row, 0));
        }
        let width = self.editor_area.width as usize;
        let segs = md::wrap_rline(&self.line_view_in(row, &blocks, width), width);
        let col = match segs.get(seg).or_else(|| segs.last()) {
            Some(seg) => seg.display_to_source(dcol),
            None => 0,
        };
        self.editor.clamp((row, col))
    }

    /// The wrapped rows of one source line, as the draw lays them out.
    pub fn wrapped(&self, row: usize, blocks: &[md::Block], width: usize) -> Vec<md::Seg> {
        md::wrap_rline(&self.line_view_in(row, blocks, width), width)
    }

    /// Where the cursor sits in its own line's wrapping: (wrapped row index,
    /// display column on that row).
    pub fn cursor_seg(&self, blocks: &[md::Block], width: usize) -> (usize, usize) {
        let (row, col) = self.editor.cursor;
        let segs = self.wrapped(row, blocks, width);
        let last = segs.len().saturating_sub(1);
        let i = segs.iter().position(|s| s.owns_src(col)).unwrap_or(last);
        (i, segs.get(i).map_or(0, |s| s.source_to_display(col)))
    }

    /// Up/Down by *display* row, so the cursor walks through a soft-wrapped
    /// line the way it does in any other editor. The target column is the
    /// display column the cursor is on now, mapped back through the row it
    /// lands on.
    fn move_vertical(&mut self, down: bool, select: bool) {
        let width = self.editor_area.width.max(1) as usize;
        let blocks = self.blocks();
        let (row, _) = self.editor.cursor;
        let (seg, dcol) = self.cursor_seg(&blocks, width);
        let segs = self.wrapped(row, &blocks, width);
        let target = if down && seg + 1 < segs.len() {
            Some((row, seg + 1))
        } else if !down && seg > 0 {
            Some((row, seg - 1))
        } else if down {
            (row + 1 < self.editor.lines().len()).then_some((row + 1, 0))
        } else {
            row.checked_sub(1).map(|r| (r, usize::MAX))
        };
        let Some((trow, tseg)) = target else {
            // already on the first/last display row: go to the buffer's edge
            let to = if down {
                let last = self.editor.lines().len() - 1;
                (last, self.editor.lines()[last].chars().count())
            } else {
                (0, 0)
            };
            self.editor.move_cursor(to, select);
            return;
        };
        let tsegs = self.wrapped(trow, &blocks, width);
        let tseg = tseg.min(tsegs.len().saturating_sub(1));
        let col = tsegs
            .get(tseg)
            .map_or(0, |s: &md::Seg| s.display_to_source(dcol));
        self.editor.move_cursor((trow, col), select);
    }

    /// Every block in the buffer. Cheap enough to recompute per frame.
    pub fn blocks(&self) -> Vec<md::Block> {
        md::blocks(self.editor.lines())
    }

    /// Does the cursor — or either end of a selection — sit inside `block`?
    /// If it does the block shows its raw source, so the syntax is editable.
    pub fn revealed(&self, block: &md::Block) -> bool {
        revealed_by(block, self.editor.cursor.0, self.editor.selection())
    }

    /// Is `row` an image line that is currently drawn as a picture rather than
    /// as text? (Used for hit-testing; the draw decides the same way.)
    fn drawn_image_row(&self, blocks: &[md::Block], row: usize) -> bool {
        md::block_at(blocks, row)
            .is_some_and(|b| b.kind == md::BlockKind::Image && !self.revealed(b))
            && self
                .edit_rows
                .iter()
                .any(|r| r.line == row && r.rect.height > 1)
    }

    /// How a source line is drawn: raw on the cursor's line (and throughout
    /// the block the cursor is in), styled everywhere else, with the block
    /// spans and page width already to hand.
    pub fn line_view_in(&self, row: usize, blocks: &[md::Block], width: usize) -> md::RLine {
        view_line(
            self.editor.lines(),
            blocks,
            row,
            width,
            self.editor.cursor.0,
            self.editor.selection(),
        )
    }

    /// The folder image references on this note resolve against.
    pub fn note_dir(&self) -> PathBuf {
        self.active_note()
            .path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| self.dir.clone())
    }

    /// A click in the preview: open a link, toggle a checkbox, or drop into the
    /// editor at the same spot.
    fn click_preview(&mut self, x: u16, y: u16) {
        let at = ratatui::layout::Position { x, y };
        if let Some((_, url)) = self.preview_links.iter().find(|(r, _)| r.contains(at)) {
            let url = url.clone();
            self.open_url(&url);
            return;
        }
        if let Some((_, row)) = self.preview_checkboxes.iter().find(|(r, _)| r.contains(at)) {
            let row = *row;
            self.toggle_checkbox(row);
            return;
        }
        let hit = self
            .preview_rows
            .iter()
            .find(|(r, _, _)| r.contains(at))
            .map(|(r, row, cells)| (*r, *row, cells.clone()));
        self.view = View::Edit;
        if let Some((rect, row, cells)) = hit {
            let dcol = x.saturating_sub(rect.x) as usize;
            let pos = cell_source(&cells, dcol).unwrap_or((row, 0));
            self.editor.clear_selection();
            self.editor.set_cursor(pos);
        }
    }

    /// Hand a URL to the desktop.
    fn open_url(&mut self, url: &str) {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        match std::process::Command::new(opener)
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => self.flash(format!("opened {url}")),
            Err(e) => self.flash(format!("open failed: {e}")),
        }
    }

    /// Flip `- [ ]` ↔ `- [x]` on a source line, from the preview.
    pub fn toggle_checkbox(&mut self, row: usize) {
        let Some(line) = self.editor.lines().get(row) else {
            return;
        };
        let Some(toggled) = toggle_task(line) else {
            return;
        };
        self.editor.set_line(row, toggled);
        self.sync_editor_to_note();
    }

    fn copy_selection(&mut self) {
        match self.editor.selected_text() {
            Some(text) if !text.is_empty() => {
                let chars = text.chars().count();
                if crate::clipboard::copy(&text) {
                    self.flash(format!("copied {chars} chars"));
                } else {
                    self.flash("copy failed".to_string());
                }
            }
            _ => self.flash("nothing selected — ^Q quits".to_string()),
        }
    }

    /// ^V: an image off the clipboard becomes an attachment and a markdown
    /// image link; anything else pastes as text. Failures flash, never panic.
    fn paste(&mut self) {
        if self.view != View::Edit {
            self.view = View::Edit;
        }
        match crate::clipboard::paste() {
            crate::clipboard::Paste::Image(png) => self.paste_image(&png),
            crate::clipboard::Paste::Text(text) => {
                self.editor.insert_str(&text);
                self.sync_editor_to_note();
                self.flash(format!("pasted {} chars", text.chars().count()));
            }
            crate::clipboard::Paste::Empty => self.flash("clipboard is empty".to_string()),
        }
    }

    fn paste_image(&mut self, png: &[u8]) {
        if self.foreign_root {
            self.flash("image paste disabled outside notes dir".to_string());
            return;
        }
        let title = self.active_note().title();
        match notes::write_attachment(&self.config.attachments_dir, &title, png) {
            Ok(path) => {
                let link = self.config.link_for(&path);
                self.editor.insert_str(&format!("![]({link})"));
                self.sync_editor_to_note();
                self.save_now();
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| link.clone());
                self.flash(format!("pasted image → {name}"));
            }
            Err(e) => self.flash(format!("image paste failed: {e}")),
        }
    }

    /// Reload the config after the settings file was edited.
    pub fn reload_config(&mut self) {
        match Config::load() {
            Ok(config) => {
                let moved = config.notes_dir != self.config.notes_dir;
                // keep the probed graphics support: it is only asked for once,
                // in raw mode at startup, and cannot be asked for again here
                self.images.set_attachments(config.attachments_dir.clone());
                self.config = config;
                if moved {
                    self.flash("notes_dir changed — restart tinynote".to_string());
                } else {
                    self.flash("settings reloaded".to_string());
                }
            }
            Err(e) => self.flash(format!("config reload failed: {e}")),
        }
    }

    pub fn on_mouse(&mut self, ev: MouseEvent) {
        match ev.kind {
            MouseEventKind::ScrollUp => match (self.overlay, self.view) {
                (Overlay::Palette, _) => self.selected = self.selected.saturating_sub(1),
                (_, View::Preview) => self.preview_scroll = self.preview_scroll.saturating_sub(2),
                (_, View::Edit) => self.editor.scroll_by(-2),
            },
            MouseEventKind::ScrollDown => match (self.overlay, self.view) {
                (Overlay::Palette, _) => {
                    let count = self.palette_items().len();
                    if count > 0 && self.selected + 1 < count {
                        self.selected += 1;
                    }
                }
                (_, View::Preview) => self.preview_scroll = self.preview_scroll.saturating_add(2),
                (_, View::Edit) => self.editor.scroll_by(2),
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
                } else if matches!(self.overlay, Overlay::ConfirmDelete | Overlay::RenameFile) {
                    self.overlay = Overlay::None;
                } else if self.view == View::Preview
                    && self
                        .editor_area
                        .contains(ratatui::layout::Position { x, y })
                {
                    self.click_preview(x, y);
                } else if self.view == View::Edit
                    && self
                        .editor_area
                        .contains(ratatui::layout::Position { x, y })
                {
                    let pos = self.pos_at(x, y);
                    // modifier-click follows a link instead of moving the cursor
                    if follows_link(ev.modifiers) {
                        if let Some(url) = self
                            .editor
                            .lines()
                            .get(pos.0)
                            .and_then(|l| md::link_at(l, pos.1))
                        {
                            self.open_url(&url);
                            return;
                        }
                    }
                    self.editor.clear_selection();
                    self.editor.set_cursor(pos);
                    self.editor.anchor = Some(self.editor.cursor);
                    self.dragging = true;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                let pos = self.pos_at(ev.column, ev.row);
                self.editor.set_cursor(pos);
            }
            MouseEventKind::Up(MouseButton::Left) if self.dragging => {
                self.dragging = false;
                if self.editor.selection().is_some() {
                    self.copy_selection();
                } else {
                    self.editor.clear_selection();
                }
            }
            _ => {}
        }
    }
}

/// A block shows its raw source while the cursor, or either end of a
/// selection, is inside its line range — Obsidian's rule, applied to whole
/// blocks rather than single lines so a table or a code fence doesn't come
/// apart while it is being edited.
pub fn revealed_by(block: &md::Block, cursor_row: usize, selection: Option<(Pos, Pos)>) -> bool {
    block.contains(cursor_row)
        || selection.is_some_and(|((sr, _), (er, _))| block.contains(sr) || block.contains(er))
}

/// How one source line is drawn in the live preview.
fn view_line(
    lines: &[String],
    blocks: &[md::Block],
    row: usize,
    width: usize,
    cursor_row: usize,
    selection: Option<(Pos, Pos)>,
) -> md::RLine {
    let src = lines.get(row).map(String::as_str).unwrap_or("");
    if let Some(block) = md::block_at(blocks, row) {
        return if revealed_by(block, cursor_row, selection) {
            md::RLine::raw(src)
        } else {
            md::style_block_line(lines, block, row, width)
        };
    }
    if row == cursor_row {
        md::RLine::raw(src)
    } else {
        md::style_line(src)
    }
}

/// Flip the `[ ]`/`[x]` box of a task line, keeping everything else intact.
pub fn toggle_task(line: &str) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    // only the leading list marker counts, so text that merely looks like a
    // second checkbox later on the line is left alone
    let mut start = 0;
    while matches!(chars.get(start), Some(' ') | Some('\t')) {
        start += 1;
    }
    if !matches!(chars.get(start), Some('-') | Some('*') | Some('+')) {
        return None;
    }
    if chars.get(start + 1) != Some(&' ') || chars.get(start + 2) != Some(&'[') {
        return None;
    }
    let idx = start + 3; // the box's inner character
    if chars.get(idx + 1) != Some(&']') {
        return None;
    }
    let inner = *chars.get(idx)?;
    let new = match inner {
        ' ' => 'x',
        'x' | 'X' => ' ',
        _ => return None,
    };
    let mut out: String = chars[..idx].iter().collect();
    out.push(new);
    out.extend(chars[idx + 1..].iter());
    Some(out)
}

/// Where a click on display column `dcol` of a rendered preview row lands in
/// the source. Rendered rows carry the source position of every character they
/// drew, so wrapped continuations, table cells, indented code and quote bars all
/// map back exactly; scaffolding the renderer invented has no source position,
/// so the nearest real character on the row wins.
fn cell_source(cells: &[crate::render::PCell], dcol: usize) -> Option<Pos> {
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

/// Does this click's modifier set mean "follow the link" rather than "put the
/// cursor here"? SGR mouse reporting only carries shift/alt/ctrl, so Cmd-click
/// is not something a terminal can report — ctrl or alt is the working path.
/// SUPER is accepted anyway, for a terminal that ever does report it.
fn follows_link(m: KeyModifiers) -> bool {
    m.intersects(KeyModifiers::SUPER | KeyModifiers::CONTROL | KeyModifiers::ALT)
}

/// Screen cell → (buffer line, display column), undoing the centred column's
/// origin and the editor's scroll. Clicks left of/above the page clamp to it.
fn screen_to_cell(area: Rect, scroll: usize, x: u16, y: u16) -> (usize, usize) {
    (
        scroll + y.saturating_sub(area.y) as usize,
        x.saturating_sub(area.x) as usize,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::md;

    #[test]
    fn task_lines_toggle_both_ways() {
        assert_eq!(toggle_task("- [ ] milk").as_deref(), Some("- [x] milk"));
        assert_eq!(toggle_task("  - [x] milk").as_deref(), Some("  - [ ] milk"));
        assert_eq!(toggle_task("- milk"), None);
        assert_eq!(toggle_task("plain text"), None);
    }

    #[test]
    fn only_the_leading_list_marker_is_a_task() {
        // a second box further along the line is text, not a checkbox
        assert_eq!(
            toggle_task("* [ ] a - [ ] b").as_deref(),
            Some("* [x] a - [ ] b")
        );
        assert_eq!(toggle_task("see the - [ ] convention"), None);
        assert_eq!(toggle_task("+ [x] plus"), Some("+ [ ] plus".to_string()));
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
    fn a_block_reveals_its_source_only_while_the_cursor_is_inside_it() {
        let lines: Vec<String> = "text\n```\nlet x = 1;\n```\nafter"
            .lines()
            .map(String::from)
            .collect();
        let blocks = md::blocks(&lines);
        let view = |row, cursor| {
            view_line(&lines, &blocks, row, 20, cursor, None)
                .cells
                .iter()
                .map(|c| c.ch)
                .collect::<String>()
        };
        // cursor outside the fence: the caps are drawn, the body is code
        assert_eq!(view(2, 0), "let x = 1;");
        // the caps are quiet: no backticks while the cursor is outside
        assert_eq!(view(1, 0), "");
        assert_eq!(view(3, 0), "");
        // cursor on any line of the fence reveals the whole block raw
        assert_eq!(view(1, 2), "```");
        assert_eq!(view(2, 2), "let x = 1;");
        // and every line of the block is raw, not just the cursor's own
        let table: Vec<String> = "| a | bbbb |\n| --- | --- |\n| 1 | 2 |"
            .lines()
            .map(String::from)
            .collect();
        let bs = md::blocks(&table);
        let raw = |row, cursor| {
            view_line(&table, &bs, row, 20, cursor, None)
                .cells
                .iter()
                .map(|c| c.ch)
                .collect::<String>()
        };
        assert_eq!(raw(0, 9), "a │ bbbb"); // cursor elsewhere: laid out
        assert_eq!(raw(0, 2), "| a | bbbb |"); // cursor on row 2: all raw
        assert_eq!(raw(1, 2), "| --- | --- |");
    }

    #[test]
    fn a_selection_touching_a_block_reveals_it_too() {
        let lines: Vec<String> = "---\ntext".lines().map(String::from).collect();
        let blocks = md::blocks(&lines);
        let rule = &blocks[0];
        assert!(!revealed_by(rule, 1, None));
        // a selection that starts in the block, or ends in it, counts
        assert!(revealed_by(rule, 1, Some(((0, 0), (1, 2)))));
        assert!(revealed_by(rule, 5, Some(((0, 0), (0, 3)))));
        assert!(!revealed_by(rule, 5, Some(((1, 0), (1, 2)))));
    }

    #[test]
    fn only_a_modified_click_follows_a_link() {
        assert!(!follows_link(KeyModifiers::NONE));
        assert!(!follows_link(KeyModifiers::SHIFT));
        assert!(follows_link(KeyModifiers::CONTROL));
        assert!(follows_link(KeyModifiers::ALT));
        assert!(follows_link(KeyModifiers::SUPER));
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

    /// The wrapped rows of a styled source line, at a given page width.
    fn segs(src: &str, width: usize) -> Vec<md::Seg> {
        md::wrap_rline(&md::style_line(src), width)
    }

    #[test]
    fn a_click_on_a_wrapped_row_lands_on_that_row_s_source_column() {
        // "the quick brown fox jumps" at width 10 wraps to three rows
        let rows = segs("the quick brown fox jumps", 10);
        assert_eq!(rows.len(), 3);
        let text = |s: &md::Seg| s.cells.iter().map(|c| c.ch).collect::<String>();
        assert_eq!(text(&rows[0]), "the quick");
        assert_eq!(text(&rows[1]), "brown fox");
        assert_eq!(text(&rows[2]), "jumps");
        // column 0 of the second row is "b" — source column 10
        assert_eq!(rows[1].display_to_source(0), 10);
        assert_eq!(rows[1].display_to_source(6), 16);
        // past the end of a row lands on the space it broke at
        assert_eq!(rows[1].display_to_source(30), 19);
        // and past the end of the last row lands at end of the source line
        assert_eq!(rows[2].display_to_source(30), 25);
    }

    #[test]
    fn wrapped_clicks_are_display_width_aware() {
        // eight CJK characters are sixteen columns: five fit a ten-column row
        let rows = segs("漢字漢字漢字漢字", 10);
        assert_eq!(rows.len(), 2);
        // the right half of a wide character still lands on that character
        assert_eq!(rows[0].display_to_source(1), 0);
        assert_eq!(rows[0].display_to_source(2), 1);
        assert_eq!(rows[1].display_to_source(0), 5);
        // an emoji line, ditto
        let rows = segs("😀😀😀 tail", 6);
        assert_eq!(rows[0].display_to_source(2), 1);
        assert_eq!(rows[1].display_to_source(0), 4);
    }

    #[test]
    fn a_wrapped_list_item_hangs_its_continuation_under_the_text() {
        // "- " is drawn as "• ", so continuations sit two columns in
        let rows = segs("- alpha beta gamma", 10);
        assert!(rows.len() > 1);
        assert_eq!(rows[0].indent, 0);
        assert_eq!(rows[1].indent, 2);
        // the indent is blank, so a click inside it lands on the row's first
        // real character rather than on the row above
        let first = rows[1].cells[0].src;
        assert_eq!(rows[1].display_to_source(0), first);
        assert_eq!(rows[1].display_to_source(2), first);
        // a checkbox item hangs under its text too ("✓ " / "☐ " are two wide)
        let rows = segs("- [ ] alpha beta gamma delta", 12);
        assert_eq!(rows[1].indent, 2);
        // a quote's bar counts as well
        let rows = segs("> alpha beta gamma", 10);
        assert_eq!(rows[1].indent, 2);
        // plain text hangs at nothing
        assert_eq!(segs("alpha beta gamma", 10)[1].indent, 0);
    }

    #[test]
    fn every_source_column_belongs_to_exactly_one_wrapped_row() {
        for src in [
            "the quick brown fox jumps over the lazy dog",
            "- [ ] a task whose text runs well past the edge of the page",
            "supercalifragilisticexpialidocious and more",
            "",
        ] {
            let rows = segs(src, 12);
            let len = src.chars().count();
            for col in 0..=len {
                // the row the cursor sits on: the first that reaches past `col`
                let i = rows
                    .iter()
                    .position(|s| s.owns_src(col))
                    .unwrap_or(rows.len() - 1);
                let seg = &rows[i];
                // and the round trip through that row's columns is exact
                let d = seg.source_to_display(col);
                assert!(d >= seg.indent, "column {col} of {src:?} left its row");
                // back again lands on the same column, or on the next one
                // actually drawn when this one's marker is hidden
                let back = seg.display_to_source(d);
                assert!(
                    back >= col && back <= seg.end_src.max(col),
                    "column {col} of {src:?} came back as {back}"
                );
            }
            // the rows cover the whole line, in order
            assert_eq!(rows.last().unwrap().end_src, len);
        }
    }
}

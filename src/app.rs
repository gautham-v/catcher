use crate::commands;
use crate::config::{Config, FrontMatter, PreviewClick};
use crate::editor::{Editor, Pos};
use crate::images::Images;
use crate::index;
use crate::keys::Action;
use crate::md;
use crate::notes::{self, Note};
use crate::search;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Which note a CLI invocation asked to open.
enum Want {
    Path(PathBuf),
    Title(String),
}

/// The note whose *title* best fuzzy-matches `name`, if any matches at all.
/// Bodies are deliberately not searched: `catcher groceries` should either
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

/// A point in the rendered preview: which row of the whole page, and which
/// display column of it. Rows are page rows rather than screen rows so a
/// selection survives scrolling.
pub type PSel = (usize, usize);

#[derive(PartialEq, Clone, Copy)]
pub enum View {
    Edit,
    Preview,
}

#[derive(PartialEq, Clone, Copy)]
pub enum Overlay {
    None,
    Palette,
    /// ^O: every note in the vault, recently opened first.
    QuickOpen,
    ConfirmDelete,
    RenameFile,
    /// Move the open note to another folder under the session root.
    MoveFile,
    Help,
}

#[derive(Clone, PartialEq)]
pub enum Command {
    NewNote,
    DailyNote,
    QuickOpen,
    SearchAll,
    DeleteNote,
    RenameFile,
    MoveFile,
    TogglePreview,
    Shortcuts,
    OpenSettings,
    FoldSection,
    UnfoldSection,
    FoldAll,
    UnfoldAll,
    Quit,
    ToggleCheckbox,
    MoveLineUp,
    MoveLineDown,
    ToggleHeading,
    InsertDate,
    CopyPath,
    RevealFile,
    SplitRight,
    SplitDown,
    NewTab,
}

const COMMANDS: [Command; 25] = [
    Command::NewNote,
    Command::DailyNote,
    Command::QuickOpen,
    Command::SearchAll,
    Command::DeleteNote,
    Command::RenameFile,
    Command::MoveFile,
    Command::TogglePreview,
    Command::Shortcuts,
    Command::OpenSettings,
    Command::FoldSection,
    Command::UnfoldSection,
    Command::FoldAll,
    Command::UnfoldAll,
    Command::Quit,
    Command::ToggleCheckbox,
    Command::MoveLineUp,
    Command::MoveLineDown,
    Command::ToggleHeading,
    Command::InsertDate,
    Command::CopyPath,
    Command::RevealFile,
    Command::SplitRight,
    Command::SplitDown,
    Command::NewTab,
];

impl Command {
    /// The action this command runs, when it is one a key can be bound to —
    /// which is how the palette knows what key to show beside it.
    pub fn action(&self) -> Option<Action> {
        Some(match self {
            // palette-only: a move is rare enough that it earns no key
            Command::MoveFile => return None,
            Command::NewNote => Action::NewNote,
            Command::DailyNote => Action::DailyNote,
            Command::QuickOpen => Action::QuickOpen,
            Command::SearchAll => Action::SearchAll,
            Command::DeleteNote => Action::DeleteNote,
            Command::RenameFile => Action::RenameFile,
            Command::TogglePreview => Action::TogglePreview,
            Command::Shortcuts => Action::Help,
            Command::OpenSettings => Action::Settings,
            Command::FoldSection => Action::FoldSection,
            Command::UnfoldSection => Action::UnfoldSection,
            Command::FoldAll => Action::FoldAll,
            Command::UnfoldAll => Action::UnfoldAll,
            Command::Quit => Action::Quit,
            Command::ToggleCheckbox => Action::ToggleCheckbox,
            Command::MoveLineUp => Action::MoveLineUp,
            Command::MoveLineDown => Action::MoveLineDown,
            Command::ToggleHeading => Action::ToggleHeading,
            Command::InsertDate => Action::InsertDate,
            Command::CopyPath => Action::CopyPath,
            Command::RevealFile => Action::RevealFile,
            Command::SplitRight => Action::OpenSplitRight,
            Command::SplitDown => Action::OpenSplitDown,
            Command::NewTab => Action::OpenTab,
        })
    }

    pub fn label(&self) -> (&'static str, &'static str) {
        match self {
            Command::NewNote => ("New note", "an empty note, ready to type"),
            Command::DailyNote => ("Today's note", "one note a day, made if missing"),
            Command::QuickOpen => ("Open note", "any folder, recent first"),
            Command::SearchAll => ("Search in all files", "type to search note contents"),
            Command::DeleteNote => ("Delete note", "delete the file on disk"),
            Command::RenameFile => ("Rename file", "change the name on disk"),
            Command::MoveFile => ("Move to folder", "another folder under this one"),
            Command::TogglePreview => ("Reading view", "the page, rendered"),
            Command::Shortcuts => ("Help", "every key, on one card"),
            Command::OpenSettings => ("Settings", "edit them here, as a note"),
            Command::FoldSection => ("Fold section", "hide what is under this heading"),
            Command::UnfoldSection => ("Unfold section", "show it again"),
            Command::FoldAll => ("Fold all", "every section, headings only"),
            Command::UnfoldAll => ("Unfold all", "open every fold in the note"),
            Command::Quit => ("Quit", "save and exit"),
            Command::ToggleCheckbox => ("Toggle checkbox", "item → [ ] → [x] → item"),
            Command::MoveLineUp => ("Move line up", "the line or selection, one up"),
            Command::MoveLineDown => ("Move line down", "the line or selection, one down"),
            Command::ToggleHeading => ("Toggle heading", "#, ##, ###, then none"),
            Command::InsertDate => ("Insert today's date", "2026-09-01, at the cursor"),
            Command::CopyPath => ("Copy path", "the note's path, to the clipboard"),
            Command::RevealFile => ("Reveal in Finder", "show the file on disk"),
            Command::SplitRight => ("Open in split right", "this note again, beside this one"),
            Command::SplitDown => ("Open in split down", "this note again, below this one"),
            Command::NewTab => ("Open in new tab", "this note again, in a terminal tab"),
        }
    }
}

/// The bindings that are *not* settable: editor motions, and the keys an
/// overlay answers to while it is open. The rebindable ones come from
/// [`crate::keys::Keymap`], so a rebound key is right on this card too.
pub const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    (
        "editing",
        &[
            ("⌘A", "select all"),
            ("⌥⌫", "delete the word before the cursor"),
            ("⌘⌫", "delete to the start of the line"),
            ("tab", "indent (tab_width spaces)"),
        ],
    ),
    (
        "moving",
        &[
            ("⌥← ⌥→", "by word  ·  on a heading, fold / unfold"),
            ("⌘← ⌘→", "start / end of line"),
            ("⌘↑ ⌘↓", "start / end of note"),
            ("⇧ + any motion", "extend the selection"),
            ("click, drag", "place the cursor, select (drag copies)"),
            (
                "⌥click  ^click",
                "open the link, [[wikilink]] or #tag under the pointer",
            ),
            ("wheel", "scroll without moving the cursor"),
            (
                "hover (reading view)",
                "rest on a [[wikilink]] to peek at the note",
            ),
        ],
    ),
    (
        "palette",
        &[
            ("type", "fuzzy-search titles and bodies"),
            ("↑ ↓", "move  ·  ⏎ open or run  ·  esc close"),
            ("tab", "in ^O, next tab: recent · tree · contents"),
            ("← →", "in the tree, fold and unfold a folder"),
            (
                "⌥⏎  ⌥⇧⏎  ⌘⏎",
                "open the note in a split right / below / a new tab (⌥click too)",
            ),
        ],
    ),
    (
        "preview",
        &[
            ("↑ ↓  pgup pgdn", "scroll"),
            ("← →", "pan a table too wide for the page"),
            ("drag", "select text — it copies on release"),
            ("click", "a link opens it, a checkbox toggles it"),
            ("^P  esc  ⏎", "back to editing"),
        ],
    ),
];

#[derive(Clone, PartialEq)]
pub enum Item {
    /// A file from the quick-open index, which may live in another folder and
    /// may not be loaded into this session at all yet.
    Entry(usize),
    /// A path typed into quick-open that turned out to exist.
    Path(PathBuf),
    /// A folder row in the ^O tree, by the key `tree` gave it. Choosing one
    /// folds or unfolds it — a folder is not somewhere to go, so the overlay
    /// stays exactly where it was.
    Folder(String),
    /// A folder the open note can be moved into, from the move picker.
    MoveTo(PathBuf),
    /// One line of a note, from the contents tab: the entry and the line.
    /// Choosing it opens the note there.
    Line(usize, usize),
    /// A row that is only there to be read — "…and N more". Choosing it
    /// does nothing and the overlay stays.
    Notice,
    Command(Command),
}

pub struct App {
    pub config: Config,
    /// The terminal title last written, so nothing is sent when unchanged.
    last_title: Option<String>,
    pub dir: PathBuf,
    pub notes: Vec<Note>,
    pub active: usize,
    pub editor: Editor,
    pub view: View,
    pub overlay: Overlay,
    pub query: String,
    pub selected: usize,
    pub preview_scroll: u16,
    /// How far the page has panned sideways, in columns. Only lines a table
    /// marked `wide` move; prose stays where it is, so the note never slides
    /// out from under you.
    pub preview_hscroll: u16,
    /// The furthest right the page can pan, worked out by the last draw from
    /// the widest table on it. Zero when nothing overflows.
    pub preview_hmax: u16,
    /// When the terminal's polarity was last checked against the system's.
    theme_checked: Instant,
    pub status: Option<(String, Instant)>,
    pub quit: bool,
    dirty: bool,
    last_edit: Instant,
    /// When the open note's file was last compared with what is on disk.
    disk_checked: Instant,
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
    /// The pictures on screen in the last preview draw, for a click to zoom.
    pub preview_images: Vec<(Rect, String)>,
    /// Every picture in the rendered page, in order, so the zoom can step
    /// from one to the next whether or not it is scrolled into view.
    pub preview_image_urls: Vec<String>,
    /// The picture taking the whole terminal, if one is: its URL as written.
    pub zoom: Option<String>,
    pub preview_checkboxes: Vec<(Rect, usize)>,
    /// Every drawn row, its source line, and the cells it drew. The source
    /// line is `None` for a row the renderer invented — a blank line between
    /// paragraphs, a linked-mentions row — which can be selected and copied
    /// but is nowhere in the buffer to click into.
    pub preview_rows: Vec<(Rect, Option<usize>, Vec<crate::render::PCell>)>,
    pub images: Images,
    dragging: bool,
    /// Every `.md` file quick-open can reach, rebuilt each time ^O opens.
    pub open_index: Vec<index::Entry>,
    /// A walk started on a thread and not yet collected — the launch one,
    /// which must not hold up the first frame.
    index_rx: Option<std::sync::mpsc::Receiver<Vec<index::Entry>>>,
    /// True while ^O is showing the folder tree instead of the ranked list.
    pub browse: bool,
    /// True while ^O is searching note contents instead. Never set together
    /// with `browse`: the tabs are recent, tree, contents, one at a time.
    pub contents: bool,
    /// Every body the contents tab searches, one per `open_index` entry, read
    /// once when the tab is entered so a keystroke never touches the disk.
    contents_bodies: Vec<Option<String>>,
    /// A source line the reading view should scroll to on its next draw. Only
    /// the draw knows which page row a line lands on once wrapped.
    pub preview_goto: Option<usize>,
    /// ^O opened by following a `#tag`: the tag, and which index entries
    /// carry it. The list is cut to those and the query narrows within them.
    pub tag_filter: Option<(String, Vec<usize>)>,
    /// Which folders the tree has unfolded. Session only, and deliberately not
    /// in the settings note: which folders are open is where you are in a
    /// session, not something you configure. A `BTreeSet` rather than a hash
    /// so the tests see one order and not whichever one they got.
    pub tree_open: BTreeSet<String>,
    /// Where the last draw put the overlay box, so a click on its own footer
    /// hint is not read as a click outside it.
    pub overlay_rect: Rect,
    /// Who links to the note on screen: scanned on a worker thread the first
    /// time the reading view asks, and kept until a save says look again.
    pub mentions: crate::mentions::Backlinks,
    /// Recently opened notes, most recent first; persisted between runs.
    pub recents: Vec<PathBuf>,
    /// A preview selection, as (page row, display column) pairs into the rows
    /// the last draw laid out. Anchor first, moving end second.
    pub preview_sel: Option<(PSel, PSel)>,
    /// True while the pointer is down and dragging out a preview selection.
    preview_dragging: bool,
    /// The rows the last draw put on screen: (page row index, rect, the
    /// display column the row's first drawn cell stands for). That last one is
    /// the pan for a scrolling table row and zero for everything else, and is
    /// what keeps a click landing on the character under the pointer.
    pub preview_page_rows: Vec<(usize, Rect, usize)>,
    /// Buffer for the inline rename prompt.
    pub rename_input: String,
    /// Where you have been, for ^B and ^F.
    pub history: crate::history::History,
    /// What has been typed into the shortcuts card, which filters its rows.
    pub help_query: String,
    /// True when the session is rooted outside the configured notes dir (a
    /// `catcher <file>` / `<dir>` invocation). Renaming and image paste are
    /// decided per note now — quick-open can reach anywhere — but the flag
    /// still says what kind of session this is.
    #[allow(dead_code)]
    pub foreign_root: bool,
    /// The wikilink the pointer is resting on in the reading view, and when
    /// it arrived there. A peek opens once it has stayed put for a moment.
    hover: Option<(String, Rect, Instant)>,
    /// The peek popup on screen, if any: the note a link points at, loaded
    /// once when the link under the pointer changed and not touched again
    /// for every pointer twitch after.
    pub peek: Option<Peek>,
    /// Which headings are folded, per note, for as long as the app runs.
    folds: crate::fold::Folds,
    /// The open note's lines as they stand on screen, rebuilt whenever the
    /// buffer or its folds change. Everything that walks the note by line —
    /// the draw, ↑↓, the wheel — asks this rather than the buffer.
    pub visible: crate::fold::Visible,
}

/// A floating glimpse of another note, Obsidian-style.
#[derive(Clone, Debug)]
pub struct Peek {
    /// The link target it was opened for, so a hover over the same link is a
    /// no-op rather than a re-read.
    pub target: String,
    /// The note the link resolved to, which a click or Enter opens.
    pub path: PathBuf,
    /// False when the link names no note yet: the popup is then a card
    /// offering to make it, and opening it makes it.
    pub exists: bool,
    /// The file's name, which titles the popup.
    pub name: String,
    /// The note's markdown with any front matter already cut off.
    pub body: String,
    /// The screen band of the link it belongs to, which the popup sits beside.
    pub anchor: Rect,
    /// The whole note rendered at the popup's inner width, cached by the
    /// first draw and reused until the width changes.
    pub rows: Vec<ratatui::text::Line<'static>>,
    /// The width `rows` were rendered at; zero until the first draw.
    pub rows_width: usize,
    /// The first row on show.
    pub scroll: usize,
    /// How many rows the last draw had room for, which bounds the scroll.
    pub view_rows: usize,
    /// Where the last draw put the popup, for pointer hit-testing.
    pub rect: Rect,
}

impl Peek {
    /// The wikilink target the popup was opened for, without its `wikilink:`
    /// dress — what `create_from_link` wants.
    fn target_name(&self) -> String {
        match md::LinkTarget::parse(&self.target) {
            md::LinkTarget::Wiki(t) => t,
            _ => self.target.clone(),
        }
    }

    /// Render the body for `width`, unless the cache already is.
    pub fn ensure_rendered(&mut self, width: usize, tables: crate::config::TableStyle) {
        if self.rows_width == width {
            return;
        }
        let rendered = crate::render::render_page_at(&self.body, 0, width, tables);
        // wrapped at draw width the way the reading view is, so a paragraph
        // that overruns the popup folds instead of falling off its right edge;
        // a wide row (a table too broad to fold) stays one row, as it does there
        self.rows = rendered
            .lines
            .iter()
            .flat_map(|l| {
                if l.wide {
                    vec![crate::render::to_line(&l.cells)]
                } else {
                    crate::render::wrap_pcells(&l.cells, width.max(1))
                        .iter()
                        .map(|cells| crate::render::to_line(cells))
                        .collect()
                }
            })
            .collect();
        self.rows_width = width;
        self.clamp();
    }

    /// The furthest `scroll` may go: the last row lands on the last line.
    pub fn max_scroll(&self) -> usize {
        self.rows.len().saturating_sub(self.view_rows.max(1))
    }

    pub fn clamp(&mut self) {
        self.scroll = self.scroll.min(self.max_scroll());
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.max_scroll() as isize;
        self.scroll = (self.scroll as isize + delta).clamp(0, max) as usize;
    }

    /// Whether the pointer is inside the popup as last drawn.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.rect.contains(ratatui::layout::Position { x, y })
    }
}

/// The one sentence for a link to a note that is not there yet, shared by
/// the status bar and the peek card. `key` is whatever follow-link is bound
/// to, so a rebound key is named right.
fn missing_link_hint(name: &str, key: &str) -> String {
    format!("no note called \u{201c}{name}\u{201d} \u{b7} {key} creates it")
}

/// How long the pointer rests on a link before it is taken as a request to
/// peek, rather than as a path across the page.
const PEEK_DWELL: Duration = Duration::from_millis(300);
/// The most of the screen the peek may take, in percent of its height.
pub const PEEK_MAX_HEIGHT_PCT: u16 = 40;

impl App {
    /// Build the app for one of the CLI's launch shapes.
    pub fn launch(launch: crate::cli::Launch) -> Result<Self> {
        use crate::cli::Launch;
        let config = Config::load()?;
        config.ensure_dirs()?;
        // before anything is rendered: every style resolves against this
        config.apply();

        // where this session is rooted, and which note it should open on
        let (dir, want): (PathBuf, Option<Want>) = match &launch {
            Launch::Default => (config.notes_dir.clone(), None),
            Launch::Name(n) => (config.notes_dir.clone(), Some(Want::Title(n.clone()))),
            Launch::Today => {
                let path = crate::daily::ensure(
                    &config.daily_dir(),
                    &config.daily_template(),
                    crate::dates::today(),
                )?;
                (config.notes_dir.clone(), Some(Want::Path(path)))
            }
            Launch::Dir(d) => (std::fs::canonicalize(d).unwrap_or_else(|_| d.clone()), None),
            Launch::File(f) => {
                let f = std::fs::canonicalize(f).unwrap_or_else(|_| f.clone());
                let parent = f
                    .parent()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("."));
                (parent, Some(Want::Path(f)))
            }
            Launch::In { root, file } => {
                let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
                let file = std::fs::canonicalize(file).unwrap_or_else(|_| file.clone());
                (root, Some(Want::Path(file)))
            }
        };
        let foreign_root = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone())
            != std::fs::canonicalize(&config.notes_dir)
                .unwrap_or_else(|_| config.notes_dir.clone());

        let recents = index::load_recent();
        // a plain `catcher` picks up where you left off: the note you had
        // open when you closed it, wherever it lives. Only a launch that names
        // nothing gets this — `catcher <file>` asked for something else.
        let restore = matches!(launch, Launch::Default)
            .then(|| recents.first().cloned())
            .flatten();

        let mut all = notes::load_all(&dir)?;
        let mut active = 0;
        match want {
            Some(Want::Path(p)) => match all.iter().position(|n| n.path == p) {
                Some(i) => active = i,
                // the daily note may sit outside the notes dir; read it in
                // the way ^O would
                None => {
                    all.insert(0, notes::load_one(&p)?);
                    active = 0;
                }
            },
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
            // an empty folder plus a note to restore is not an empty session:
            // creating an untitled note here would leave a stray file behind
            if let Some(note) = restore.as_ref().and_then(|p| notes::load_one(p).ok()) {
                all.push(note);
            }
        }
        if all.is_empty() {
            all.push(notes::create(&dir)?);
        }

        let mut app = App {
            rename_input: String::new(),
            help_query: String::new(),
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
            preview_hscroll: 0,
            preview_hmax: 0,
            theme_checked: Instant::now(),
            status: None,
            quit: false,
            last_title: None,
            dirty: false,
            disk_checked: Instant::now(),
            last_edit: Instant::now(),
            editor_area: Rect::default(),
            edit_rows: Vec::new(),
            palette_rows: Vec::new(),
            preview_links: Vec::new(),
            preview_images: Vec::new(),
            preview_image_urls: Vec::new(),
            zoom: None,
            preview_checkboxes: Vec::new(),
            preview_rows: Vec::new(),
            dragging: false,
            open_index: Vec::new(),
            index_rx: None,
            browse: false,
            tag_filter: None,
            tree_open: BTreeSet::new(),
            contents: false,
            contents_bodies: Vec::new(),
            preview_goto: None,
            overlay_rect: Rect::default(),
            hover: None,
            peek: None,
            mentions: crate::mentions::Backlinks::default(),
            recents,
            preview_sel: None,
            preview_dragging: false,
            preview_page_rows: Vec::new(),
            history: crate::history::History::default(),
            folds: crate::fold::Folds::default(),
            visible: crate::fold::Visible::default(),
        };
        app.remember_active();
        app.load_active_into_editor();
        // after the session exists, so a last note from another folder is
        // pulled in the same way quick-open pulls one
        if let Some(path) = restore {
            if path.exists() {
                app.open_path(&path);
            }
        }
        // one vault walk at startup, so a broken `[[link]]` is red soon after
        // the first frame rather than only after the first ^O. Without it
        // every link in the vault draws as though it resolved, which makes the
        // whole point of the broken colour invisible for as long as it lasts.
        // On a thread, because a big vault is tens of megabytes of reading and
        // none of it is owed to the first frame: links draw as resolvable
        // until it lands, which is exactly what an un-walked session does.
        if app.config.wikilinks {
            app.start_index_scan();
        }
        Ok(app)
    }

    pub fn active_note(&self) -> &Note {
        &self.notes[self.active]
    }

    fn load_active_into_editor(&mut self) {
        self.editor = Editor::new(&self.notes[self.active].content);
        self.editor.tab_width = self.config.tab_width;
        let row = opening_row(self.editor.lines(), self.config.front_matter);
        if row > 0 {
            self.editor.move_cursor((row, 0), false);
        }
        self.preview_scroll = 0;
        self.preview_hscroll = 0;
        self.preview_goto = None;
        self.preview_sel = None;
        // a note switched to may have changed since its folds were made — a
        // link rewrite put a fresh copy in `notes` — so the folds are settled
        // against the text about to be shown, not the text they were made on
        let blocks = self.blocks();
        self.folds
            .settle(&self.notes[self.active].path, self.editor.lines(), &blocks);
        self.refresh_visible();
        self.sync_title();
    }

    /// Rebuild the line → row mapping from the buffer and the note's folds.
    fn refresh_visible(&mut self) {
        let blocks = self.blocks();
        let folded = self.folds.of(&self.notes[self.active].path);
        self.visible = crate::fold::Visible::new(self.editor.lines(), &blocks, &folded);
    }

    /// Is `row` a heading the note has folded?
    pub fn folded_here(&self, row: usize) -> bool {
        self.folds.is_folded(&self.notes[self.active].path, row)
    }

    /// The folded headings of the open note, in order.
    pub fn folded_lines(&self) -> Vec<usize> {
        self.folds.of(&self.notes[self.active].path)
    }

    /// What a folded heading says at its right edge.
    pub fn fold_label(&self, row: usize) -> Option<String> {
        if !self.folded_here(row) {
            return None;
        }
        Some(match self.visible.hidden_under(row) {
            1 => "1 line folded".to_string(),
            n => format!("{n} lines folded"),
        })
    }

    /// Is the cursor on a heading line — the one place the fold keys apply?
    fn on_heading(&self) -> bool {
        crate::fold::heading_at(self.editor.lines(), &self.blocks(), self.editor.cursor.0).is_some()
    }

    /// Does this key fold rather than move? In the editor only on a heading,
    /// and never while a selection is being extended: ⇧⌥← is a motion
    /// wherever the cursor is. The reading view has nothing for the arrows
    /// to move, so there they always fold.
    fn fold_key_applies(&self, key: &KeyEvent) -> bool {
        self.overlay == Overlay::None
            && match self.view {
                View::Edit => fold_key_takes(key, self.on_heading()),
                View::Preview => !key.modifiers.contains(KeyModifiers::SHIFT),
            }
    }

    /// The line the fold keys act on: the cursor's in the editor. The reading
    /// view has no cursor, so there it is the heading the selection starts
    /// on, or failing that the first heading on screen.
    fn fold_target(&self) -> Option<usize> {
        match self.view {
            View::Edit => Some(self.editor.cursor.0),
            View::Preview => {
                let blocks = self.blocks();
                let heading = |line: usize| {
                    crate::fold::heading_at(self.editor.lines(), &blocks, line).is_some()
                };
                let at_sel = self.preview_span().and_then(|((row, _), _)| {
                    let i = self
                        .preview_page_rows
                        .iter()
                        .position(|(r, _, _)| *r == row)?;
                    self.preview_rows.get(i)?.1.filter(|&l| heading(l))
                });
                at_sel.or_else(|| {
                    self.preview_rows
                        .iter()
                        .filter_map(|(_, l, _)| *l)
                        .find(|&l| heading(l))
                })
            }
        }
    }

    /// A fold changed under the reading view: its rows are about to mean
    /// other lines, so a selection anchored to them is dropped.
    fn folds_changed(&mut self) {
        self.refresh_visible();
        if self.view == View::Preview {
            self.preview_sel = None;
        }
    }

    fn fold_section(&mut self) {
        let Some(row) = self.fold_target() else {
            self.flash("no heading on screen".to_string());
            return;
        };
        self.fold_line(row);
    }

    /// Fold the section under `row`, saying why when it cannot.
    fn fold_line(&mut self, row: usize) {
        let path = self.notes[self.active].path.clone();
        let blocks = self.blocks();
        let is_heading = crate::fold::heading_at(self.editor.lines(), &blocks, row).is_some();
        match self.folds.fold(&path, self.editor.lines(), &blocks, row) {
            Some(_) => self.folds_changed(),
            None if is_heading => self.flash("nothing under this heading".to_string()),
            None => self.flash("not on a heading".to_string()),
        }
    }

    fn unfold_section(&mut self) {
        let Some(row) = self.fold_target() else {
            self.flash("no heading on screen".to_string());
            return;
        };
        self.unfold_line(row);
    }

    /// Open the fold on `row`, saying so when there is none.
    fn unfold_line(&mut self, row: usize) {
        let path = self.notes[self.active].path.clone();
        if self.folds.unfold(&path, row) {
            self.folds_changed();
        } else {
            self.flash("nothing folded here".to_string());
        }
    }

    /// A click on a heading in the reading view: open its fold, or close it.
    /// Whether anything changed — a heading with nothing under it is left to
    /// the click's other meanings.
    fn toggle_fold(&mut self, row: usize) -> bool {
        if self.folded_here(row) {
            self.unfold_line(row);
            return true;
        }
        let path = self.notes[self.active].path.clone();
        let blocks = self.blocks();
        let folded = self
            .folds
            .fold(&path, self.editor.lines(), &blocks, row)
            .is_some();
        if folded {
            self.folds_changed();
        }
        folded
    }

    fn fold_all(&mut self) {
        let path = self.notes[self.active].path.clone();
        let blocks = self.blocks();
        let n = self.folds.fold_all(&path, self.editor.lines(), &blocks);
        self.folds_changed();
        // the cursor may have been inside a section that just closed
        let row = self.editor.cursor.0;
        self.leave_folds(row);
        self.flash(format!("folded {n} sections"));
    }

    fn unfold_all(&mut self) {
        let path = self.notes[self.active].path.clone();
        let n = self.folds.unfold_all(&path);
        self.folds_changed();
        self.flash(format!("opened {n} folds"));
    }

    /// A cursor that landed inside a fold is put on the nearest line on
    /// screen: past the fold when it was moving down, on the heading when up.
    /// Called after every motion, so the cursor is never on a hidden line.
    fn leave_folds(&mut self, before: usize) {
        let (row, col) = self.editor.cursor;
        if !self.visible.is_hidden(row) {
            return;
        }
        let to = if row > before {
            self.visible.next_visible(row)
        } else {
            self.visible.prev_visible(row)
        }
        .or_else(|| self.visible.prev_visible(row))
        .unwrap_or(0);
        let keep = self.editor.selection().is_some();
        self.editor.move_cursor(self.editor.clamp((to, col)), keep);
    }

    /// An edit put the cursor inside a fold — enter at the end of a folded
    /// heading, a backspace that joined the line after one: the section
    /// opens, because what was just typed must be on screen.
    fn reveal_cursor(&mut self) {
        let row = self.editor.cursor.0;
        if !self.visible.is_hidden(row) {
            return;
        }
        let path = self.notes[self.active].path.clone();
        let blocks = self.blocks();
        self.folds.reveal(&path, self.editor.lines(), &blocks, row);
        self.refresh_visible();
    }

    /// Scroll the editor by rows on screen, so a wheel tick over a fold does
    /// not spend itself on lines nobody can see.
    fn scroll_edit(&mut self, delta: isize) {
        let row = self.visible.line_to_row(self.editor.scroll) as isize + delta;
        let last = self.visible.rows().saturating_sub(1) as isize;
        let line = self.visible.row_to_line(row.clamp(0, last) as usize);
        self.editor
            .scroll_by(line as isize - self.editor.scroll as isize);
    }

    /// The coarse one-row-per-line scroll pass the buffer makes, done in rows
    /// on screen so the lines a fold hides do not count toward the page.
    pub fn scroll_cursor_into_view(&mut self, height: usize) {
        if self.visible.is_plain() {
            self.editor.scroll_into_view(height);
        } else if self.editor.following() {
            self.editor.scroll =
                self.visible
                    .scroll_for(self.editor.scroll, self.editor.cursor.0, height);
        }
    }

    /// Put the open note's name in the terminal's title, if it is not there
    /// already. Off means the title is left alone.
    pub fn sync_title(&mut self) {
        if !self.config.window_title {
            return;
        }
        let title = self.notes[self.active]
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if self.last_title.as_deref() != Some(title.as_str()) {
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::terminal::SetTitle(title.as_str())
            );
            self.last_title = Some(title);
        }
    }

    fn sync_editor_to_note(&mut self) {
        let content = self.editor.text();
        if content != self.notes[self.active].content {
            self.notes[self.active].content = content;
            self.dirty = true;
            self.last_edit = Instant::now();
            let blocks = self.blocks();
            self.folds
                .settle(&self.notes[self.active].path, self.editor.lines(), &blocks);
            self.refresh_visible();
            self.reveal_cursor();
        }
    }

    /// Is `path` a file catcher may rename to follow its title? Only inside
    /// the configured notes dir, and only while `rename_files` is on: an
    /// Obsidian vault's links are its filenames, and moving one breaks them.
    fn may_rename(&self, path: &Path) -> bool {
        if !self.config.rename_files {
            return false;
        }
        let parent = match path.parent() {
            Some(p) => std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()),
            None => return false,
        };
        let notes_dir = std::fs::canonicalize(&self.config.notes_dir)
            .unwrap_or_else(|_| self.config.notes_dir.clone());
        parent == notes_dir
    }

    /// Is the note on screen the settings note?
    pub fn editing_settings(&self) -> bool {
        crate::config::settings_path()
            .ok()
            .and_then(|p| std::fs::canonicalize(p).ok())
            .zip(std::fs::canonicalize(&self.active_note().path).ok())
            .is_some_and(|(a, b)| a == b)
    }

    pub fn save_now(&mut self) {
        if !self.dirty {
            return;
        }
        // never write over what another program put there since we last
        // looked: disk wins, and the buffer catches up instead
        if notes::check_disk(&self.notes[self.active]) == notes::Disk::Changed {
            self.reload_from_disk();
            return;
        }
        let path = self.notes[self.active].path.clone();
        let allow_rename = self.may_rename(&path);
        // the note's own folder, not the session's: quick-open reaches into
        // other directories, and a note must always be written back where it
        // actually lives
        let dir = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.dir.clone());
        let was_settings = self.editing_settings();
        match notes::save(&dir, &mut self.notes[self.active], allow_rename) {
            Ok(now) => {
                self.dirty = false;
                // a save is the only way a body under the roots changes from
                // inside catcher, and it is what makes a mention you have
                // just typed turn up in the footer of the note it names
                self.mentions.invalidate();
                if now != path {
                    // the folds are kept by path, and the text they were
                    // made on has not changed
                    self.folds.relocate(&path, &now);
                    if let Some(done) = self.update_links(&path, &now) {
                        self.flash(format!("renamed · {done}"));
                    }
                }
            }
            Err(e) => {
                self.flash(format!("save failed: {e}"));
                return;
            }
        }
        if was_settings {
            self.reload_config();
        }
    }

    /// The note at `old` now lives at `new`: point every `[[wikilink]]` under
    /// the roots at the new name, and refresh any of those notes this session
    /// holds a copy of, so switching to one does not show the old text. What
    /// was done, in words, or `None` when nothing was — or the setting is off.
    fn update_links(&mut self, old: &Path, new: &Path) -> Option<String> {
        if !self.config.update_links {
            return None;
        }
        let report = crate::links::retarget(old, new, &self.index_roots());
        for path in &report.notes {
            let Some(i) = self.notes.iter().position(|n| {
                std::fs::canonicalize(&n.path).unwrap_or_else(|_| n.path.clone()) == *path
            }) else {
                continue;
            };
            if let Ok(fresh) = notes::load_one(&self.notes[i].path) {
                self.notes[i] = fresh;
            }
        }
        report.describe()
    }

    pub fn maybe_autosave(&mut self) {
        let after = Duration::from_millis(self.config.autosave_ms);
        if self.dirty && self.last_edit.elapsed() >= after {
            self.save_now();
        }
    }

    pub fn flash(&mut self, msg: String) {
        self.status = Some((msg, Instant::now()));
    }

    /// Every half second, ask whether another program has touched the open
    /// note's file. A change is taken as it stands; a deletion is announced
    /// once and the buffer kept, so the next save puts the file back.
    fn watch_disk(&mut self) {
        if self.disk_checked.elapsed() < Duration::from_millis(500) {
            return;
        }
        self.disk_checked = Instant::now();
        match notes::check_disk(&self.notes[self.active]) {
            notes::Disk::Unchanged => {}
            notes::Disk::Changed => self.reload_from_disk(),
            notes::Disk::Gone => {
                // forget the stamp so this is said once, and so the file
                // coming back reads as a change
                self.notes[self.active].stamp = None;
                self.flash("deleted on disk".to_string());
            }
        }
    }

    /// Replace the buffer with the file as another program left it. One undo
    /// step, so ^Z brings back what was on screen — edits that had not made
    /// it to disk included.
    fn reload_from_disk(&mut self) {
        let dropped = self.dirty;
        if let Err(e) = notes::reload(&mut self.notes[self.active]) {
            self.flash(format!("reload failed: {e}"));
            return;
        }
        self.editor.replace_all(&self.notes[self.active].content);
        self.dirty = false;
        self.mentions.invalidate();
        // the folds were made on the old text: move the ones whose heading
        // survived, drop the rest, and rebuild the rows — otherwise the draw
        // goes on skipping lines by their old numbers
        let blocks = self.blocks();
        self.folds
            .settle(&self.notes[self.active].path, self.editor.lines(), &blocks);
        self.refresh_visible();
        // the cursor kept its place by number, which may now be inside a
        // fold: another program's edit is not a reason to open one, so the
        // cursor goes to the heading instead
        let row = self.editor.cursor.0;
        self.leave_folds(row);
        self.flash(if dropped {
            "reloaded: changed on disk, your last edits were dropped".to_string()
        } else {
            "reloaded: changed on disk".to_string()
        });
    }

    pub fn tick(&mut self) {
        self.watch_disk();
        self.maybe_autosave();
        self.follow_system_theme();
        self.poll_index_scan();
        self.maybe_peek();
        // a filename that followed its title on save
        self.sync_title();
        if let Some((_, at)) = self.status {
            if at.elapsed() > Duration::from_secs(3) {
                self.status = None;
            }
        }
    }

    /// With `theme: auto` on a terminal that was found to track the system
    /// appearance, flip the palette when the system does — the terminal has
    /// already repainted itself by then, and the old palette reads wrong on
    /// it. Checked every couple of seconds; a change reloads the settings so
    /// colour overrides still sit on top of the new base.
    fn follow_system_theme(&mut self) {
        if self.config.theme != crate::config::Theme::Auto
            || !crate::md::theme::follows_system()
            || self.theme_checked.elapsed() < Duration::from_secs(2)
        {
            return;
        }
        self.theme_checked = Instant::now();
        let Some(mode) = crate::md::theme::system_mode() else {
            return;
        };
        if mode != crate::md::theme::detected() {
            crate::md::theme::set_detected(mode);
            if let Ok(config) = Config::load() {
                config.apply();
                self.config = config;
            }
        }
    }

    /// Every way of opening a note that already exists ends here or in
    /// `open_path`. Neither touches `view`: following a link or picking a
    /// note from ^O while reading keeps you reading, and the same from the
    /// editor keeps you editing. Only a note that did not exist a moment ago
    /// (^N, a link's offer to create) and the settings note force the editor.
    fn switch_to(&mut self, idx: usize) {
        self.save_now();
        self.active = idx;
        self.load_active_into_editor();
        self.remember_active();
    }

    /// Put the note on screen at the front of the recents list, which is what
    /// quick-open ranks by.
    fn remember_active(&mut self) {
        let path = self.notes[self.active].path.clone();
        // history is every landing, the settings note included: ^B from it
        // should go back to what you were reading
        self.history.push(&path);
        // the settings note has its own key and its own palette row; putting
        // it at the top of "recently opened" would only push notes down
        if self.editing_settings() {
            return;
        }
        index::push_recent(&mut self.recents, &path);
    }

    /// ^B / ^F: the note before or after this one in the history. Opening
    /// goes through `open_path`, which saves first and pushes the landing —
    /// a push of the entry just made current is a no-op, so the stack stays
    /// where it is.
    fn nav_history(&mut self, back: bool) {
        let exists = |p: &Path| p.exists();
        let target = if back {
            self.history.back(exists)
        } else {
            self.history.forward(exists)
        };
        match target {
            Some(path) => self.open_path(&path),
            None => self.flash(
                if back {
                    "nothing to go back to"
                } else {
                    "nothing ahead"
                }
                .to_string(),
            ),
        }
    }

    /// Open any `.md` file by path, from anywhere. Already-loaded notes are
    /// switched to; anything else is read in and added to this session, so it
    /// can be edited, saved and switched back to like any other note.
    pub fn open_path(&mut self, path: &Path) {
        let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(i) = self.notes.iter().position(|n| {
            std::fs::canonicalize(&n.path).unwrap_or_else(|_| n.path.clone()) == target
        }) {
            self.switch_to(i);
            return;
        }
        match notes::load_one(&target) {
            Ok(note) => {
                self.save_now();
                self.notes.insert(0, note);
                self.active = 0;
                self.load_active_into_editor();
                self.remember_active();
            }
            Err(e) => self.flash(format!("open failed: {e}")),
        }
    }

    /// ^,: the settings, opened as a note. Regenerated first when it is
    /// missing, so there is always a document with every setting in it.
    fn open_settings(&mut self) {
        let path = match crate::config::settings_path() {
            Ok(p) => p,
            Err(e) => {
                self.flash(format!("settings: {e}"));
                return;
            }
        };
        if !path.exists() {
            if let Err(e) = crate::config::Config::load() {
                self.flash(format!("settings: {e}"));
                return;
            }
        }
        self.open_path(&path);
        // the settings are for editing, whatever view you came from
        self.view = View::Edit;
        self.flash("settings — ^S applies them".to_string());
    }

    /// Build the quick-open index. Rebuilt on every open rather than cached:
    /// notes are files, and anything could have written one since.
    fn refresh_index(&mut self) {
        self.open_index = index::scan(&self.index_roots(), &self.recents);
        // a walk started earlier answers with an older vault than the one just
        // read, so whatever it says is no longer wanted
        self.index_rx = None;
        self.refresh_links();
    }

    /// The same walk, on a thread, for the one caller that must not wait for
    /// it: launch. Nothing on the first frame needs the index — it colours
    /// `[[wikilinks]]`, and an un-walked vault draws them as resolvable — so
    /// the walk is started and [`App::tick`] takes the answer whenever it
    /// lands. This is what mentions.rs does for the same reason, and for the
    /// same reason it is not joined: quitting mid-walk drops the receiver.
    fn start_index_scan(&mut self) {
        let roots = self.index_roots();
        let recents = self.recents.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(index::scan(&roots, &recents));
        });
        self.index_rx = Some(rx);
    }

    /// Take a walk started by [`App::start_index_scan`] if it has finished.
    fn poll_index_scan(&mut self) {
        let Some(rx) = self.index_rx.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(entries) => {
                self.open_index = entries;
                self.index_rx = None;
                self.refresh_links();
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            // only a panic in the walk can do this; there is nothing to wait
            // for any more, and ^O will walk again itself
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.index_rx = None,
        }
    }

    /// The vault changed under the session in a way a walk has to see: a note
    /// renamed, or one deleted. Without it the index keeps an entry for a path
    /// that is not there any more, and `follow_wikilink` resolves against that
    /// entry, opens nothing, and never reaches the rescan-and-offer-to-create
    /// fallback that exists for exactly this; the link also goes on drawing as
    /// resolvable, and the footer goes on listing a note that has moved.
    fn reindex(&mut self) {
        self.refresh_index();
        self.mentions.invalidate();
    }

    /// The folders a walk covers. Pulled out of `refresh_index` because the
    /// linked-mentions scan has to walk exactly what quick-open walks — a note
    /// that links here from a folder the index cannot see would be a mention
    /// of a note you could never open from the footer.
    ///
    /// The recents list is deliberately not in it: those are scattered single
    /// files, not folders to walk.
    fn index_roots(&self) -> Vec<PathBuf> {
        if !self.config.quick_open_recursive {
            return vec![self.dir.clone()];
        }
        let mut roots = vec![self.config.notes_dir.clone()];
        if !self.quick_open_root_is_notes_dir() {
            roots.push(self.dir.clone());
        }
        // vaults and work folders the user has named in the settings
        roots.extend(self.config.quick_open_dirs.iter().cloned());
        roots
    }

    /// Tell the styling which `[[wikilink]]` targets this vault actually has,
    /// so a link to a note that is not there is drawn as broken. Done once per
    /// index walk and never per frame: it is a set of a few thousand strings,
    /// and the styling only ever reads it.
    fn refresh_links(&mut self) {
        crate::md::links::set_known(self.open_index.iter().flat_map(index::link_keys).collect());
    }

    /// Follow a `[[wikilink]]`, or offer to make the note it names.
    ///
    /// Opening goes through `open_path`, which already saves the note on
    /// screen, switches to an already-loaded one, and remembers the new one in
    /// the recents list — so the save-back rules for a note outside the notes
    /// dir are inherited rather than written a second time here.
    /// The pointer moved. Cheap on purpose — terminals send one of these for
    /// every cell the pointer crosses — so it only compares against the link
    /// boxes the last draw cached, and touches no file. The read happens in
    /// [`Self::maybe_peek`] once the pointer has rested for [`PEEK_DWELL`].
    fn on_hover(&mut self, x: u16, y: u16) {
        if self.view != View::Preview || self.overlay != Overlay::None {
            return;
        }
        // moving about inside the popup is reading it, not leaving the link
        if self.peek.as_ref().is_some_and(|p| p.contains(x, y)) {
            return;
        }
        let at = ratatui::layout::Position { x, y };
        let hit = self
            .preview_links
            .iter()
            .find(|(r, _)| r.contains(at))
            .filter(|(_, url)| {
                matches!(
                    md::LinkTarget::parse(url),
                    md::LinkTarget::Wiki(_) | md::LinkTarget::Note(_)
                )
            });
        match hit {
            Some((rect, url)) => {
                let same = self.hover.as_ref().is_some_and(|(u, _, _)| u == url);
                if !same {
                    self.hover = Some((url.clone(), *rect, Instant::now()));
                }
                if self.peek.as_ref().is_some_and(|p| &p.target != url) {
                    self.peek = None;
                }
            }
            None => {
                self.hover = None;
                self.peek = None;
            }
        }
    }

    /// Open the peek for a hover that has lasted long enough.
    fn maybe_peek(&mut self) {
        let Some((url, rect, since)) = self.hover.clone() else {
            return;
        };
        if since.elapsed() < PEEK_DWELL || self.peek.as_ref().is_some_and(|p| p.target == url) {
            return;
        }
        if let Some(peek) = self.load_peek(&url, rect) {
            self.peek = Some(peek);
        } else {
            // nothing to show: forget the hover so this is not retried every
            // tick for as long as the pointer sits there
            self.hover = None;
        }
    }

    /// ⌥P: peek at the [[wikilink]] under the editor cursor, which is the
    /// only cursor the app has — the reading view is pointer-driven.
    fn peek_at_cursor(&mut self) {
        let pos = self.editor.cursor;
        let target = self
            .editor
            .lines()
            .get(pos.0)
            .and_then(|l| md::link_at(l, pos.1));
        let url = match target {
            Some(t @ (md::LinkTarget::Wiki(_) | md::LinkTarget::Note(_))) => t.href(),
            _ => {
                self.flash("no wikilink here".to_string());
                return;
            }
        };
        // beside the cursor's row when it is on screen, else the top of the page
        let anchor = self
            .edit_rows
            .iter()
            .find(|r| r.line == pos.0)
            .map(|r| r.rect)
            .unwrap_or(Rect::new(
                self.editor_area.x,
                self.editor_area.y,
                self.editor_area.width,
                1,
            ));
        match self.load_peek(&url, anchor) {
            Some(p) => self.peek = Some(p),
            None => self.flash("no such note".to_string()),
        }
    }

    /// Read the note a link names, for a peek. A wikilink that resolves to
    /// nothing gets a card saying so instead of nothing at all — the hover
    /// that asked is the moment you want to know. Deliberately no vault
    /// re-walk here, which is a cost a hover must not pay.
    fn load_peek(&self, url: &str, anchor: Rect) -> Option<Peek> {
        let path = match md::LinkTarget::parse(url) {
            md::LinkTarget::Note(p) => PathBuf::from(p),
            md::LinkTarget::Wiki(t) => match index::resolve(&self.open_index, &t) {
                Some(e) => e.path.clone(),
                None => match best_title_match(&self.notes, &t) {
                    Some(i) => self.notes[i].path.clone(),
                    None => return Some(self.missing_peek(url, &t, anchor)),
                },
            },
            md::LinkTarget::Url(_) | md::LinkTarget::Tag(_) => return None,
        };
        // an open note may have edits the disk has not seen yet
        let content = match self.notes.iter().find(|n| n.path == path) {
            Some(n) => n.content.clone(),
            None => std::fs::read_to_string(&path).ok()?,
        };
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| url.to_string());
        Some(Peek {
            target: url.to_string(),
            path,
            exists: true,
            name,
            body: notes::body_after_front_matter(&content).to_string(),
            anchor,
            rows: Vec::new(),
            rows_width: 0,
            scroll: 0,
            view_rows: 0,
            rect: Rect::default(),
        })
    }

    /// The card a peek at a link to nowhere shows: the same words the status
    /// bar uses, so the two never disagree about what the key does.
    fn missing_peek(&self, url: &str, target: &str, anchor: Rect) -> Peek {
        let name = Self::link_note_path(target)
            .map(|(_, n)| n)
            .unwrap_or_else(|| target.to_string());
        Peek {
            target: url.to_string(),
            path: PathBuf::new(),
            exists: false,
            name: name.clone(),
            body: missing_link_hint(&name, &self.config.keys.label(Action::FollowLink)),
            anchor,
            rows: Vec::new(),
            rows_width: 0,
            scroll: 0,
            view_rows: 0,
            rect: Rect::default(),
        }
    }

    /// Open the peeked note for real, putting the popup away. A peek at a
    /// note that is not there yet makes it, as following the link would.
    fn open_peek(&mut self) {
        if let Some(peek) = self.peek.take() {
            self.hover = None;
            if peek.exists {
                self.open_path(&peek.path);
            } else {
                self.create_from_link(&peek.target_name());
            }
        }
    }

    /// What the status bar says under a cursor resting on a `[[wikilink]]`
    /// to nowhere, and nothing for any other spot. Asks the styling, not the
    /// index, so the bar and the grey underline agree — an un-walked vault
    /// draws every link as resolvable and says nothing.
    pub fn cursor_link_hint(&self) -> Option<String> {
        if self.view != View::Edit || self.overlay != Overlay::None {
            return None;
        }
        let pos = self.editor.cursor;
        let target = match self
            .editor
            .lines()
            .get(pos.0)
            .and_then(|l| md::link_at(l, pos.1))?
        {
            md::LinkTarget::Wiki(t) => t,
            _ => return None,
        };
        if md::links::resolves(&target) {
            return None;
        }
        let name = Self::link_note_path(&target).map(|(_, n)| n)?;
        Some(missing_link_hint(
            &name,
            &self.config.keys.label(Action::FollowLink),
        ))
    }

    fn follow_wikilink(&mut self, target: &str) {
        if let Some(path) = index::resolve(&self.open_index, target).map(|e| e.path.clone()) {
            self.open_path(&path);
            return;
        }
        // a note written since the last walk is the ordinary miss, and one
        // vault walk to be sure is cheap next to telling someone their link is
        // broken when it is not
        self.refresh_index();
        if let Some(path) = index::resolve(&self.open_index, target).map(|e| e.path.clone()) {
            self.open_path(&path);
            return;
        }
        // still nothing: the link is a note that has not been written yet,
        // and following it is how it gets written
        self.create_from_link(target);
    }

    /// The folder and filename a link target names, relative to the note the
    /// link was written in. `None` for a target that would write outside the
    /// vault: a link target is note text, and note text must never be able to
    /// name `/etc/passwd` or climb out with `..`.
    fn link_note_path(target: &str) -> Option<(PathBuf, String)> {
        let t = target.trim().trim_end_matches(".md");
        if t.is_empty() || t.starts_with('/') || t.starts_with('~') {
            return None;
        }
        let t = t.replace('\\', "/");
        if t.split('/')
            .any(|seg| seg == ".." || seg == "." || seg.is_empty())
        {
            return None;
        }
        let (folder, name) = match t.rsplit_once('/') {
            Some((f, n)) => (PathBuf::from(f), n.to_string()),
            None => (PathBuf::new(), t.clone()),
        };
        Some((folder, name))
    }

    /// Where a note made from a link lands: the folder of the note the link
    /// is in, plus whatever folder part the target carried. Free-standing so
    /// the test needs no app behind it.
    fn create_dir(note_dir: &Path, folder: &Path) -> PathBuf {
        note_dir.join(folder)
    }

    /// Make the note an unresolved wikilink names, beside the note the link
    /// was written in, and open it. No confirmation: the grey underline and
    /// the status bar have already said what the key will do, and ^B goes
    /// back to where the link was.
    fn create_from_link(&mut self, target: &str) {
        let Some((folder, name)) = Self::link_note_path(target) else {
            self.flash(format!("“{target}” is not a name a note can have"));
            return;
        };
        let dir = Self::create_dir(&self.note_dir(), &folder);
        match notes::create_named(&dir, &name, format!("# {name}\n\n")) {
            Ok(note) => {
                let path = note.path.clone();
                self.open_path(&path);
                // a note that is still just its title is for writing, not reading
                self.view = View::Edit;
                // the link that made this note stops being red at once
                self.refresh_index();
                self.flash(format!("created \u{201c}{name}\u{201d}"));
            }
            Err(e) => self.flash(format!("create failed: {e}")),
        }
    }

    /// A typed path — `~/vault/spec.md`, `/tmp/x.md` — as an openable file.
    /// The escape hatch for a note in a folder catcher has never been shown:
    /// you can always say where it is.
    fn typed_path(&self) -> Option<PathBuf> {
        let q = self.query.trim();
        if !(q.starts_with('/') || q.starts_with("~/")) {
            return None;
        }
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let path = match q.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(q),
        };
        // a bare name is taken to mean the markdown file of that name
        let path = if path.extension().is_none() {
            path.with_extension("md")
        } else {
            path
        };
        path.is_file().then_some(path)
    }

    fn quick_open_root_is_notes_dir(&self) -> bool {
        std::fs::canonicalize(&self.dir).unwrap_or_else(|_| self.dir.clone())
            == std::fs::canonicalize(&self.config.notes_dir)
                .unwrap_or_else(|_| self.config.notes_dir.clone())
    }

    fn open_quick_open(&mut self) {
        self.query.clear();
        self.selected = 0;
        self.tag_filter = None;
        // before `enter_browse`, which reads the index it builds
        self.refresh_index();
        self.overlay = Overlay::QuickOpen;
        self.contents = false;
        self.browse = self.config.quick_open_browse;
        if self.browse {
            self.enter_browse();
        }
    }

    /// ⇧^F: ^O, opened straight on the contents tab.
    fn open_search_all(&mut self) {
        self.open_quick_open();
        self.tag_filter = None;
        self.browse = false;
        self.enter_contents();
    }

    /// Entering the contents tab reads every body the index reaches, once.
    /// A vault of a few thousand notes is a moment's work, and after it each
    /// keystroke is string search over memory.
    fn enter_contents(&mut self) {
        self.contents = true;
        self.selected = 0;
        // a note this session holds is searched as it stands in memory: the
        // file may be an autosave behind, and a hit's line number has to be
        // right for the buffer it opens into
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let open: Vec<(PathBuf, &str)> = self
            .notes
            .iter()
            .map(|n| (canon(&n.path), n.content.as_str()))
            .collect();
        self.contents_bodies = self
            .open_index
            .iter()
            .map(|e| {
                let here = canon(&e.path);
                match open.iter().find(|(p, _)| *p == here) {
                    Some((_, body)) => Some(body.to_string()),
                    None => crate::contents::body_of(&e.path),
                }
            })
            .collect();
    }

    /// The contents rows for the current query: a header per note, its
    /// matching lines under it, and a count of what the cap left out.
    pub fn contents_rows(&self) -> Vec<crate::contents::Row> {
        let (hits, more) = crate::contents::search(
            &self.contents_bodies,
            &self.query,
            crate::contents::MAX_HITS,
        );
        crate::contents::rows(&hits, more)
    }

    /// Open the note `entry` names with line `line` in view: under the cursor
    /// in the editor, at the top of the page in the reading view.
    fn open_at_line(&mut self, entry: usize, line: usize) {
        let Some(path) = self.open_index.get(entry).map(|e| e.path.clone()) else {
            return;
        };
        self.open_path(&path);
        // an open that failed flashed and left the old note up, and its
        // cursor must not go to a line number that belongs to another file
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        if canon(&self.active_note().path) != canon(&path) {
            return;
        }
        self.editor.set_cursor((line, 0));
        // a hit inside a folded section is one you asked to see: the fold
        // opens, the way it does for a line you have just typed on
        self.reveal_cursor();
        self.preview_goto = Some(line);
    }

    /// Quick-open rows for the current query. With no query this is simply the
    /// index order — most recently opened first, then most recently modified —
    /// which is the whole point of having a second list beside the palette.
    pub fn open_items(&self) -> Vec<Item> {
        let pool: Vec<usize> = match &self.tag_filter {
            Some((_, hits)) => hits.clone(),
            None => (0..self.open_index.len()).collect(),
        };
        if self.query.is_empty() {
            return pool.into_iter().map(Item::Entry).collect();
        }
        // a path that exists is not a guess, so it leads the list outright —
        // unless the list is a tag's, which a path is no part of
        if let Some(path) = self.typed_path().filter(|_| self.tag_filter.is_none()) {
            let mut rows = vec![Item::Path(path.clone())];
            rows.extend(
                self.open_index
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.path != path)
                    .filter(|(_, e)| search::fuzzy(&self.query, &e.rel).is_some())
                    .map(|(i, _)| Item::Entry(i)),
            );
            return rows;
        }
        let n = self.open_index.len().max(1) as i64;
        let mut scored: Vec<(i64, usize)> = Vec::new();
        for i in pool {
            let e = &self.open_index[i];
            // the filename is what the list shows, so it is what people
            // search by; the title is a second chance, and the folder path a
            // weaker third, so "applications/log" finds it too
            let Some(base) = search::score_entry(&self.query, &e.name(), &e.title, &e.rel) else {
                continue;
            };
            // a nudge, not a verdict: recency breaks ties between equally good
            // matches without ever burying a better one
            let recency = ((n - i as i64) * 20) / n;
            scored.push((base + recency, i));
        }
        scored.sort_by_key(|(s, i)| (std::cmp::Reverse(*s), *i));
        scored.into_iter().map(|(_, i)| Item::Entry(i)).collect()
    }

    /// The tree rows for the current query and fold state. Rebuilt on each
    /// call rather than cached, which is the same bargain `open_items` already
    /// makes: nothing at all is built while ^O is shut, and an index of a few
    /// thousand notes is a handful of string compares and one sort.
    pub fn browse_rows(&self) -> Vec<crate::tree::Row> {
        crate::tree::rows(&self.open_index, &self.tree_open, &self.query)
    }

    /// tab: the same overlay, the next way of looking at it — recent, tree,
    /// contents, and round again. The query survives every swap: typing
    /// `log` and then wanting to see *where* the log notes live, or which
    /// notes *say* it, is the whole reason to have the tabs.
    fn next_tab(&mut self) {
        // a tag's list is one list; the tree has no filtered view of itself
        if self.tag_filter.is_some() {
            return;
        }
        if self.browse {
            self.browse = false;
            self.enter_contents();
        } else if self.contents {
            self.contents = false;
            self.selected = 0;
        } else {
            self.browse = true;
            self.enter_browse();
        }
    }

    /// Entering browse mode unfolds the folder you are already in and selects
    /// the note you have open, so the first thing the tree tells you is where
    /// you are rather than where the vault starts.
    fn enter_browse(&mut self) {
        let active = std::fs::canonicalize(&self.active_note().path).ok();
        // the query comes along, because the tree about to be drawn is the
        // filtered one: a row counted against the whole vault would put the
        // selection on some unrelated folder, or past the end of the rows
        let query = self.query.clone();
        self.selected = crate::tree::reveal(
            &self.open_index,
            &mut self.tree_open,
            active.as_deref(),
            &query,
        );
    }

    fn toggle_folder(&mut self, key: &str) {
        let query = self.query.clone();
        self.selected = crate::tree::toggle(&self.open_index, &mut self.tree_open, key, &query);
    }

    /// →: unfold a folder, step into one already unfolded, open a note.
    fn browse_right(&mut self) {
        let rows = self.browse_rows();
        let Some(row) = rows.get(self.selected) else {
            return;
        };
        match &row.kind {
            crate::tree::RowKind::Folder { key, open, .. } => {
                if !*open {
                    let key = key.clone();
                    self.toggle_folder(&key);
                } else if rows
                    .get(self.selected + 1)
                    .is_some_and(|next| next.depth > row.depth)
                {
                    self.selected += 1;
                }
            }
            crate::tree::RowKind::Note { entry, .. } => {
                let entry = *entry;
                self.run_item(Item::Entry(entry));
            }
        }
    }

    /// ←: fold a folder, or leave for the folder this row lives in.
    fn browse_left(&mut self) {
        let rows = self.browse_rows();
        let Some(row) = rows.get(self.selected) else {
            return;
        };
        if let crate::tree::RowKind::Folder { key, open, .. } = &row.kind {
            // a filtered tree is unfolded whatever the fold set says, so
            // folding there would move nothing on screen; go up instead of
            // appearing to do nothing at all
            if *open && self.query.is_empty() {
                let key = key.clone();
                self.toggle_folder(&key);
                return;
            }
        }
        if let Some(up) = crate::tree::parent_of(&rows, self.selected) {
            self.selected = up;
        }
    }

    /// The rows the open overlay is showing, whichever overlay that is.
    pub fn overlay_items(&self) -> Vec<Item> {
        match self.overlay {
            Overlay::Palette => self.palette_items(),
            // the tree's notes reuse `Item::Entry`, so opening one from here
            // goes down the exact path quick-open already uses; only a folder
            // needed a variant of its own
            Overlay::QuickOpen if self.browse => self
                .browse_rows()
                .iter()
                .map(|r| match &r.kind {
                    crate::tree::RowKind::Folder { key, .. } => Item::Folder(key.clone()),
                    crate::tree::RowKind::Note { entry, .. } => Item::Entry(*entry),
                })
                .collect(),
            Overlay::QuickOpen if self.contents => self
                .contents_rows()
                .into_iter()
                .map(|r| match r {
                    // a header opens the note at its top, like any note row
                    crate::contents::Row::Note(entry) => Item::Entry(entry),
                    crate::contents::Row::Hit(h) => Item::Line(h.entry, h.line),
                    crate::contents::Row::More(_) => Item::Notice,
                })
                .collect(),
            Overlay::QuickOpen => self.open_items(),
            Overlay::MoveFile => self.move_items(),
            _ => Vec::new(),
        }
    }

    fn new_note(&mut self) {
        self.save_now();
        match notes::create(&self.dir) {
            Ok(n) => {
                self.notes.insert(0, n);
                self.active = 0;
                self.view = View::Edit;
                self.load_active_into_editor();
                self.remember_active();
            }
            Err(e) => self.flash(format!("create failed: {e}")),
        }
    }

    /// Today's note, made from the template the first time and then simply
    /// opened: an existing file is never rewritten.
    fn open_daily(&mut self) {
        self.save_now();
        let (dir, template) = (self.config.daily_dir(), self.config.daily_template());
        let today = crate::dates::today();
        let made = !crate::daily::path(&dir, today).exists();
        match crate::daily::ensure(&dir, &template, today) {
            Ok(path) => {
                self.open_path(&path);
                self.view = View::Edit;
                // a file that was not there a moment ago: the same re-walk a
                // note made from a link gets, so a `[[link]]` to today stops
                // being grey and the template's own links count as mentions
                if made {
                    self.reindex();
                }
            }
            Err(e) => self.flash(format!("daily note: {e}")),
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
        self.history.push(&self.notes[0].path.clone());
        // the file is gone, and an index that still lists it is worse than no
        // index at all: `follow_wikilink` would resolve a `[[link]]` against
        // the entry, open nothing, and never reach the offer to create it
        self.reindex();
        self.flash(format!("deleted “{title}”"));
    }

    /// Palette rows for the current query: commands only, best first. Notes
    /// live behind ^O, the way Obsidian keeps them out of its palette too.
    pub fn palette_items(&self) -> Vec<Item> {
        let mut scored: Vec<(i64, Item)> = Vec::new();
        for c in COMMANDS {
            if let Some(s) = search::fuzzy(&self.query, c.label().0) {
                scored.push((s, Item::Command(c)));
            }
        }
        scored.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
        scored.into_iter().map(|(_, it)| it).collect()
    }

    /// Every folder under the session root the open note could move to,
    /// the root itself first, the rest in path order. Hidden folders and the
    /// attachments folder are skipped; so is the folder the note is in now.
    pub fn move_targets(&self) -> Vec<(PathBuf, usize)> {
        fn walk(dir: &Path, skip: &Path, out: &mut Vec<(PathBuf, usize)>) {
            let Ok(rd) = std::fs::read_dir(dir) else {
                return;
            };
            let mut notes = 0;
            let mut subs = Vec::new();
            for e in rd.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                if p.is_dir() {
                    if p != skip {
                        subs.push(p);
                    }
                } else if p.extension().is_some_and(|x| x == "md") {
                    notes += 1;
                }
            }
            out.push((dir.to_path_buf(), notes));
            subs.sort();
            for s in subs {
                walk(&s, skip, out);
            }
        }
        let mut out = Vec::new();
        walk(&self.dir, &self.config.attachments_dir, &mut out);
        let here = self
            .active_note()
            .path
            .parent()
            .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
        out.retain(|(d, _)| Some(std::fs::canonicalize(d).unwrap_or_else(|_| d.clone())) != here);
        out
    }

    /// What a move target is called in the picker: `/` for the root, the
    /// path below it for anything else.
    pub fn move_label(&self, dir: &Path) -> String {
        match dir.strip_prefix(&self.dir) {
            Ok(rel) if rel.as_os_str().is_empty() => "/".to_string(),
            Ok(rel) => format!("{}/", rel.display()),
            Err(_) => crate::index::short(dir),
        }
    }

    /// Move-picker rows for the current query, best first.
    pub fn move_items(&self) -> Vec<Item> {
        let mut scored: Vec<(i64, Item)> = Vec::new();
        for (dir, _) in self.move_targets() {
            if let Some(s) = search::fuzzy(&self.query, &self.move_label(&dir)) {
                scored.push((s, Item::MoveTo(dir)));
            }
        }
        scored.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
        scored.into_iter().map(|(_, it)| it).collect()
    }

    fn run_item(&mut self, item: Item) {
        // a folder is not somewhere to go: folding it leaves the overlay open
        // and the tree exactly where it was, which is also what makes clicking
        // a folder row work, since a click routes through here too
        if let Item::Folder(key) = item {
            self.toggle_folder(&key);
            return;
        }
        if item == Item::Notice {
            return;
        }
        self.overlay = Overlay::None;
        match item {
            Item::Entry(i) => {
                if let Some(path) = self.open_index.get(i).map(|e| e.path.clone()) {
                    self.open_path(&path);
                }
            }
            Item::Path(path) => self.open_path(&path),
            Item::Line(entry, line) => self.open_at_line(entry, line),
            // handled above, before the overlay was closed
            Item::Folder(_) | Item::Notice => {}
            Item::Command(Command::NewNote) => self.new_note(),
            Item::Command(Command::DailyNote) => self.open_daily(),
            Item::Command(Command::QuickOpen) => self.open_quick_open(),
            Item::Command(Command::SearchAll) => self.open_search_all(),
            Item::Command(Command::TogglePreview) => self.toggle_preview(),
            Item::Command(Command::Shortcuts) => {
                self.help_query.clear();
                self.overlay = Overlay::Help;
            }
            Item::Command(Command::OpenSettings) => self.open_settings(),
            Item::Command(Command::Quit) => {
                self.save_now();
                self.quit = true;
            }
            Item::Command(Command::DeleteNote) => {
                self.overlay = Overlay::ConfirmDelete;
            }
            Item::Command(Command::RenameFile) => self.open_rename(),
            Item::Command(Command::MoveFile) => self.open_move(),
            // the rest are plain actions: one path, whether by key or palette
            Item::Command(c) => {
                if let Some(a) = c.action() {
                    self.run_action(a);
                }
            }
            Item::MoveTo(dir) => self.commit_move(&dir),
        }
    }

    fn open_move(&mut self) {
        self.save_now();
        self.query.clear();
        self.selected = 0;
        self.overlay = Overlay::MoveFile;
    }

    /// Move the open note into `dir`. The filename comes along (made unique
    /// if the folder already has one by that name); the title is untouched.
    fn commit_move(&mut self, dir: &Path) {
        let label = self.move_label(dir);
        let old = self.active_note().path.clone();
        match notes::move_file(&mut self.notes[self.active], dir) {
            Ok(path) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                // the note is somewhere else now: the index, the recents, the
                // history and the folds all named the old path
                self.folds.relocate(&old, &path);
                self.reindex();
                self.remember_active();
                self.flash(format!("moved → {label}{name}"));
            }
            Err(e) => self.flash(format!("move failed: {e}")),
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
        let old = self.active_note().path.clone();
        match notes::rename_file(&mut self.notes[self.active], &stem) {
            Ok(path) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let done = if path != old {
                    self.folds.relocate(&old, &path);
                    self.update_links(&old, &path)
                } else {
                    None
                };
                // the filename is one of the names a `[[wikilink]]` reaches a
                // note by, so a rename changes what resolves and what does not
                self.reindex();
                self.sync_title();
                match done {
                    Some(done) => self.flash(format!("renamed → {name} · {done}")),
                    None => self.flash(format!("renamed → {name}")),
                }
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
        self.preview_hscroll = 0;
        self.preview_goto = None;
    }

    /// Pan the page sideways, clamped to what the last draw measured. A
    /// selection is dropped: it is anchored to columns that are about to mean
    /// something else on screen.
    fn pan(&mut self, by: i32) {
        let to = (self.preview_hscroll as i32 + by).clamp(0, self.preview_hmax as i32);
        if to as u16 != self.preview_hscroll {
            self.preview_hscroll = to as u16;
            self.preview_sel = None;
        }
    }

    fn open_palette(&mut self) {
        self.query.clear();
        self.selected = 0;
        self.overlay = Overlay::Palette;
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        // a zoomed picture takes every key: the arrows step between the
        // note's pictures, anything else puts it away
        if self.zoom.is_some() {
            match key.code {
                KeyCode::Left | KeyCode::Up | KeyCode::PageUp => self.zoom_step(-1),
                KeyCode::Right | KeyCode::Down | KeyCode::PageDown => self.zoom_step(1),
                _ => self.unzoom(),
            }
            return;
        }
        // the arrows and page keys read the peek; any other key puts it away
        // (the peek action itself opens a new one)
        if let Some(peek) = self.peek.as_mut() {
            let page = peek.view_rows.max(1) as isize;
            let delta = match key.code {
                KeyCode::Up => Some(-1),
                KeyCode::Down => Some(1),
                KeyCode::PageUp => Some(-page),
                KeyCode::PageDown => Some(page),
                _ => None,
            };
            if let Some(d) = delta {
                peek.scroll_by(d);
                return;
            }
            if key.code == KeyCode::Enter {
                self.open_peek();
                return;
            }
        }
        self.peek = None;
        self.hover = None;
        let cmd = key.modifiers.contains(KeyModifiers::SUPER);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        if cmd && matches!(key.code, KeyCode::Char('a')) {
            self.editor.select_all();
            return;
        }
        // ⇧^Z is redo, the way every other editor spells it — checked before
        // the keymap, which sees ^Z and ⇧^Z as the same key on purpose
        if shift && (ctrl || cmd) && matches!(key.code, KeyCode::Char('z' | 'Z')) {
            self.redo();
            return;
        }
        // ⏎ with a modifier on a ^O row opens the note beside this one; it
        // must get there before the keymap, where ⌥⏎ is follow-link
        if self.overlay == Overlay::QuickOpen && key.code == KeyCode::Enter {
            self.on_palette_key(key);
            return;
        }
        // whatever the settings say this key does, if anything. The fold keys
        // are the word-motion arrows, and only a heading line takes them:
        // anywhere else the editor gets the key and moves by word, as before
        if let Some(action) = self.config.keys.action(&key) {
            let fold_key = matches!(action, Action::FoldSection | Action::UnfoldSection);
            if !fold_key || self.fold_key_applies(&key) {
                self.run_action(action);
                return;
            }
        }

        match self.overlay {
            // the card is searchable, so typing filters it rather than
            // dismissing it; esc and enter are how you leave
            Overlay::Help => {
                if !edit_line(&mut self.help_query, &key) {
                    match key.code {
                        KeyCode::Esc | KeyCode::Enter => self.overlay = Overlay::None,
                        _ => {}
                    }
                }
            }
            Overlay::Palette | Overlay::QuickOpen | Overlay::MoveFile => self.on_palette_key(key),
            Overlay::ConfirmDelete => match key.code {
                KeyCode::Enter => {
                    self.overlay = Overlay::None;
                    self.delete_active();
                }
                KeyCode::Esc => self.overlay = Overlay::None,
                _ => {}
            },
            Overlay::RenameFile => {
                if !edit_line(&mut self.rename_input, &key) {
                    match key.code {
                        KeyCode::Enter => self.commit_rename(),
                        KeyCode::Esc => self.overlay = Overlay::None,
                        _ => {}
                    }
                }
            }
            Overlay::None => match self.view {
                View::Preview => match key.code {
                    // ← and → pan a table too wide for the page; with nothing
                    // to pan they do nothing rather than something surprising
                    KeyCode::Left => self.pan(-4),
                    KeyCode::Right => self.pan(4),
                    KeyCode::Home => self.preview_hscroll = 0,
                    KeyCode::Up => self.preview_scroll = self.preview_scroll.saturating_sub(1),
                    KeyCode::Down => self.preview_scroll = self.preview_scroll.saturating_add(1),
                    KeyCode::PageUp => self.preview_scroll = self.preview_scroll.saturating_sub(10),
                    KeyCode::PageDown => {
                        self.preview_scroll = self.preview_scroll.saturating_add(10)
                    }
                    // esc drops a selection before it drops the preview, the
                    // same order it takes in the editor
                    KeyCode::Esc if self.preview_sel.is_some() => self.preview_sel = None,
                    KeyCode::Esc if self.preview_hscroll > 0 => self.preview_hscroll = 0,
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
                    let before = self.editor.cursor.0;
                    match key.code {
                        KeyCode::Up if plain => self.move_vertical(false, select),
                        KeyCode::Down if plain => self.move_vertical(true, select),
                        _ => {
                            if self.editor.on_key(key) {
                                self.sync_editor_to_note();
                            }
                        }
                    }
                    self.leave_folds(before);
                }
            },
        }
    }

    /// Do one of the actions a key can be bound to. The single place a
    /// binding leads, so the palette, the help card and the settings all agree
    /// about what a key does.
    fn run_action(&mut self, action: Action) {
        match action {
            Action::Palette => {
                if matches!(self.overlay, Overlay::Palette | Overlay::QuickOpen) {
                    self.overlay = Overlay::None;
                } else {
                    self.open_palette();
                }
            }
            Action::QuickOpen => {
                if self.overlay == Overlay::QuickOpen {
                    self.overlay = Overlay::None;
                } else {
                    self.open_quick_open();
                }
            }
            Action::NewNote => {
                self.overlay = Overlay::None;
                self.new_note();
            }
            Action::Settings => {
                self.overlay = Overlay::None;
                self.open_settings();
            }
            Action::TogglePreview => {
                self.overlay = Overlay::None;
                self.toggle_preview();
            }
            Action::Save => {
                self.sync_editor_to_note();
                // saving the settings note reports what it applied, which is
                // more use than being told the file was written
                let settings = self.editing_settings();
                self.save_now();
                if !settings {
                    self.flash("saved".to_string());
                }
            }
            Action::Help => {
                self.help_query.clear();
                self.overlay = if self.overlay == Overlay::Help {
                    Overlay::None
                } else {
                    Overlay::Help
                };
            }
            Action::Quit => {
                self.sync_editor_to_note();
                self.save_now();
                self.quit = true;
            }
            Action::Copy => {
                if self.view == View::Preview {
                    self.copy_preview_selection();
                } else {
                    self.copy_selection();
                }
            }
            Action::Cut => self.cut_selection(),
            Action::Paste => {
                self.overlay = Overlay::None;
                self.paste();
            }
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
            Action::DeleteNote => {
                self.overlay = Overlay::ConfirmDelete;
            }
            Action::RenameFile => self.open_rename(),
            Action::FollowLink => self.follow_link_at_cursor(),
            Action::NavBack => self.nav_history(true),
            Action::NavForward => self.nav_history(false),
            Action::Peek => self.peek_at_cursor(),
            Action::SearchAll => {
                if self.overlay == Overlay::QuickOpen && self.contents {
                    self.overlay = Overlay::None;
                } else {
                    self.open_search_all();
                }
            }
            Action::DailyNote => {
                self.overlay = Overlay::None;
                self.open_daily();
            }
            Action::ToggleCheckbox => self.edit_lines(commands::toggle_checkbox),
            Action::ToggleHeading => self.edit_lines(commands::cycle_heading),
            Action::MoveLineUp => self.move_lines(false),
            Action::MoveLineDown => self.move_lines(true),
            Action::InsertDate => {
                self.enter_edit_view();
                let (y, m, d) = crate::dates::today();
                self.editor.insert_str(&crate::dates::iso(y, m, d));
                self.sync_editor_to_note();
            }
            Action::CopyPath => self.copy_path(),
            Action::RevealFile => self.reveal_file(),
            Action::OpenSplitRight => self.open_beside(crate::terminal::Place::SplitRight, None),
            Action::OpenSplitDown => self.open_beside(crate::terminal::Place::SplitDown, None),
            Action::OpenTab => self.open_beside(crate::terminal::Place::Tab, None),
            Action::FoldSection => self.fold_section(),
            Action::UnfoldSection => self.unfold_section(),
            Action::FoldAll => self.fold_all(),
            Action::UnfoldAll => self.unfold_all(),
        }
    }

    /// The editing commands act on the buffer, so like undo they land in the
    /// edit view whichever one was showing.
    fn enter_edit_view(&mut self) {
        self.overlay = Overlay::None;
        self.view = View::Edit;
    }

    /// Rewrite the cursor line or the selection with `f`, as one undo step.
    fn edit_lines(&mut self, f: fn(&str) -> String) {
        self.enter_edit_view();
        self.editor.map_selected_lines(f);
        self.sync_editor_to_note();
    }

    fn move_lines(&mut self, down: bool) {
        self.enter_edit_view();
        if self.editor.move_selected_lines(down) {
            self.sync_editor_to_note();
        } else {
            self.flash("nowhere to move".to_string());
        }
    }

    fn copy_path(&mut self) {
        self.overlay = Overlay::None;
        let path = self.active_note().path.clone();
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        if crate::clipboard::copy(&path.to_string_lossy()) {
            self.flash("path copied".to_string());
        } else {
            self.flash("copy failed".to_string());
        }
    }

    /// Show the file in Finder — or, elsewhere, open its folder, since there
    /// is no portable way to select a file.
    fn reveal_file(&mut self) {
        self.overlay = Overlay::None;
        self.save_now();
        let path = self.active_note().path.clone();
        let mut cmd = if cfg!(target_os = "macos") {
            let mut c = std::process::Command::new("open");
            c.arg("-R").arg(&path);
            c
        } else {
            let mut c = std::process::Command::new("xdg-open");
            c.arg(path.parent().unwrap_or(&path));
            c
        };
        match cmd
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => {}
            Err(e) => self.flash(format!("reveal failed: {e}")),
        }
    }

    fn on_palette_key(&mut self, key: KeyEvent) {
        // tab steps ^O through its tabs, round and round. The command
        // palette has no second view of itself, so it never sees this.
        if key.code == KeyCode::Tab && self.overlay == Overlay::QuickOpen {
            self.next_tab();
            return;
        }
        // the query is a one-line input, and the Mac editing keys have to work
        // in it: nothing is more annoying than a search box you can only
        // backspace out of one character at a time
        if edit_line(&mut self.query, &key) {
            self.selected = 0;
            return;
        }
        let tree = self.browse && self.overlay == Overlay::QuickOpen;
        match key.code {
            KeyCode::Esc => self.overlay = Overlay::None,
            KeyCode::Right if tree => self.browse_right(),
            KeyCode::Left if tree => self.browse_left(),
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                // the row count is only ever wanted here, and asking for it
                // builds every row of the overlay — in browse mode the whole
                // tree — so a typed character must not pay for one
                let count = self.overlay_items().len();
                if count > 0 && self.selected + 1 < count {
                    self.selected += 1;
                }
            }
            KeyCode::Enter => {
                if let Some(item) = self.overlay_items().get(self.selected).cloned() {
                    match beside_place(key.modifiers) {
                        Some(place) if self.overlay == Overlay::QuickOpen => {
                            self.run_item_beside(item, place)
                        }
                        _ => self.run_item(item),
                    }
                }
            }
            _ => {}
        }
    }

    /// ⌥⏎ / ⌥⇧⏎ / ⌘⏎ (or ⌥click) on a ^O row: the note opens in a new split
    /// or tab and this one stays where it is. A folder row folds as it would
    /// on a plain ⏎, since there is nothing to open.
    fn run_item_beside(&mut self, item: Item, place: crate::terminal::Place) {
        let path = match &item {
            Item::Entry(i) | Item::Line(i, _) => self.open_index.get(*i).map(|e| e.path.clone()),
            Item::Path(p) => Some(p.clone()),
            _ => return self.run_item(item),
        };
        if let Some(path) = path {
            self.overlay = Overlay::None;
            self.open_beside(place, Some(path));
        }
    }

    /// Ask the terminal for a new split or tab running catcher on `path`
    /// (this note, if none), rooted where this session is, so ^O there sees
    /// the same vault. The new surface takes focus, as the terminal's own
    /// split would.
    fn open_beside(&mut self, place: crate::terminal::Place, path: Option<PathBuf>) {
        self.overlay = Overlay::None;
        self.save_now();
        let path = path.unwrap_or_else(|| self.active_note().path.clone());
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("catcher"));
        let argv = vec![
            exe.to_string_lossy().into_owned(),
            "--root".to_string(),
            self.dir.to_string_lossy().into_owned(),
            path.to_string_lossy().into_owned(),
        ];
        match crate::terminal::open_beside(place, &argv) {
            Ok(()) => {
                let what = match place {
                    crate::terminal::Place::Tab => "tab",
                    _ => "split",
                };
                let term = crate::terminal::backend().unwrap_or("terminal");
                self.flash(format!("{what} → {term}"));
            }
            Err(e) => self.flash(e),
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
            // the next line on screen, which is past a fold rather than in it
            self.visible.next_visible(row + 1).map(|r| (r, 0))
        } else {
            row.checked_sub(1)
                .and_then(|r| self.visible.prev_visible(r))
                .map(|r| (r, usize::MAX))
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
    ///
    /// This is the one place the `front_matter` setting is consulted for the
    /// editor. Making the block first and starting the markdown scan below it
    /// is what keeps the closing `---` from becoming a rule and `tags:` from
    /// picking up emphasis; everything downstream — reveal-on-cursor,
    /// wrapping, click hit-testing — then does the right thing by itself.
    pub fn blocks(&self) -> Vec<md::Block> {
        blocks_with(self.editor.lines(), self.config.front_matter)
    }

    /// Does the cursor — or either end of a selection — sit inside `block`?
    /// If it does the block shows its raw source, so the syntax is editable.
    pub fn revealed(&self, block: &md::Block) -> bool {
        revealed_by(block, self.editor.cursor.0, self.editor.selection())
    }

    /// Is `row` a line the draw skips entirely? Only front matter set to
    /// `hide` ever is, and it strikes the same bargain a code fence does: the
    /// text is still in the file, and moving the cursor into the block brings
    /// the whole thing back.
    pub fn hidden_row(&self, blocks: &[md::Block], row: usize) -> bool {
        self.visible.is_hidden(row)
            || hidden_by(
                md::block_at(blocks, row),
                self.config.front_matter,
                self.editor.cursor.0,
                self.editor.selection(),
            )
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
        let line = view_line(
            self.editor.lines(),
            blocks,
            row,
            width,
            self.editor.cursor.0,
            self.editor.selection(),
        );
        if self.folded_here(row) {
            fold_marked(line)
        } else {
            line
        }
    }

    /// The folder image references on this note resolve against.
    pub fn note_dir(&self) -> PathBuf {
        self.active_note()
            .path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| self.dir.clone())
    }

    /// The notes that link to the one on screen, or nothing at all while the
    /// scan for them is still running.
    ///
    /// Owned rows rather than a borrow: this takes `&mut self` because it may
    /// start or poll a scan, and the rest of the preview draw goes on to
    /// mutate the app heavily. The rows are a handful of small structs, so the
    /// clone costs nothing worth arranging the draw around.
    pub fn linked_mentions(&mut self) -> Vec<crate::mentions::Mention> {
        let path = self.active_note().path.clone();
        // the title as it stands in the buffer, not on disk: a note renamed by
        // its heading answers to the new name from the next frame
        let title = notes::title_of(&self.active_note().content);
        let roots = self.index_roots();
        self.mentions
            .rows_for(&path, || {
                // canonicalized only when a scan is actually about to start,
                // so the cached frame costs a path compare and no syscall
                let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                (
                    crate::mentions::target_entry(&canon, &title, &roots),
                    roots.clone(),
                )
            })
            .to_vec()
    }

    /// A click in the preview: open a link, toggle a checkbox, or start a
    /// selection. It deliberately does *not* jump into the editor any more —
    /// the preview is for reading, and a click that changed mode made it
    /// impossible to drag out a quote and copy it. `preview_click: edit` in the
    /// settings puts the old behaviour back.
    fn click_preview(&mut self, x: u16, y: u16) {
        let at = ratatui::layout::Position { x, y };
        if let Some((_, url)) = self.preview_links.iter().find(|(r, _)| r.contains(at)) {
            let url = url.clone();
            self.follow(md::LinkTarget::parse(&url));
            return;
        }
        if let Some((_, row)) = self.preview_checkboxes.iter().find(|(r, _)| r.contains(at)) {
            let row = *row;
            self.toggle_checkbox(row);
            return;
        }
        // a picture takes the whole terminal until the next key or click
        if let Some((_, url)) = self.preview_images.iter().find(|(r, _)| r.contains(at)) {
            self.zoom = Some(url.clone());
            self.peek = None;
            self.hover = None;
            return;
        }
        // a heading row folds and unfolds, the way ⌥← and ⌥→ do on it; a
        // link on the heading was answered above, so only the text is left
        let heading = self
            .preview_rows
            .iter()
            .find(|(r, _, _)| r.contains(at))
            .and_then(|(_, line, _)| *line)
            .filter(|&l| crate::fold::heading_at(self.editor.lines(), &self.blocks(), l).is_some());
        if heading.is_some_and(|row| self.toggle_fold(row)) {
            return;
        }
        if self.config.preview_click == PreviewClick::Edit {
            self.edit_at_preview(x, y);
            return;
        }
        self.preview_sel = self.preview_point(x, y).map(|p| (p, p));
        self.preview_dragging = true;
    }

    /// Step the zoomed picture to the previous or next one in the note,
    /// stopping at either end rather than wrapping.
    fn zoom_step(&mut self, delta: isize) {
        let Some(url) = self.zoom.as_ref() else {
            return;
        };
        let urls = &self.preview_image_urls;
        let Some(i) = urls.iter().position(|u| u == url) else {
            return;
        };
        let j = (i as isize + delta).clamp(0, urls.len() as isize - 1) as usize;
        if j != i {
            self.zoom = Some(urls[j].clone());
        }
    }

    fn unzoom(&mut self) {
        self.zoom = None;
        self.images.unzoom();
    }

    /// The old behaviour, kept behind `preview_click: edit`: land in the
    /// editor at the spot that was clicked.
    fn edit_at_preview(&mut self, x: u16, y: u16) {
        let hit = self
            .preview_rows
            .iter()
            .find(|(r, _, _)| r.contains(ratatui::layout::Position { x, y }))
            .map(|(r, row, cells)| (*r, *row, cells.clone()));
        self.view = View::Edit;
        if let Some((rect, row, cells)) = hit {
            let dcol = x.saturating_sub(rect.x) as usize;
            // a row the renderer invented is nowhere in the buffer: better to
            // leave the cursor where it was than to answer a click on the
            // footer with the top of the note
            if let Some(pos) = cell_source(&cells, dcol).or_else(|| row.map(|r| (r, 0))) {
                self.editor.clear_selection();
                self.editor.set_cursor(pos);
            }
        }
    }

    /// Where a screen point lands in the rendered page. Points above or below
    /// the drawn rows clamp to the first or last one, so dragging off the top
    /// or bottom of the window extends the selection rather than stalling.
    fn preview_point(&self, x: u16, y: u16) -> Option<PSel> {
        let first = self.preview_page_rows.first()?;
        let last = self.preview_page_rows.last()?;
        let (page_row, rect, offset) = if y < first.1.y {
            *first
        } else if y > last.1.y {
            *last
        } else {
            *self
                .preview_page_rows
                .iter()
                .find(|(_, r, _)| y >= r.y && y < r.y + r.height)
                .unwrap_or(last)
        };
        Some((page_row, x.saturating_sub(rect.x) as usize + offset))
    }

    /// The preview selection ordered from its earlier point to its later one.
    pub fn preview_span(&self) -> Option<(PSel, PSel)> {
        let (a, b) = self.preview_sel?;
        Some(if a <= b { (a, b) } else { (b, a) })
    }

    /// Copy whatever the preview selection covers, as the rendered text a
    /// reader is actually looking at — a table comes out as its drawn columns,
    /// not as pipes.
    fn copy_preview_selection(&mut self) {
        let Some(text) = self.preview_selected_text() else {
            return;
        };
        let text = text.trim_end_matches('\n').to_string();
        if text.trim().is_empty() {
            return;
        }
        let chars = text.chars().count();
        if crate::clipboard::copy(&text) {
            self.flash(format!("copied {chars} chars"));
        } else {
            self.flash("copy failed".to_string());
        }
    }

    /// The text the preview selection covers, assembled from the rows the last
    /// draw recorded. `None` when nothing is selected.
    pub fn preview_selected_text(&self) -> Option<String> {
        Some(selected_text(
            &self.preview_page_rows,
            &self.preview_rows,
            self.preview_span()?,
        ))
    }

    /// The one door a link goes through, so a `wikilink:` href can never be
    /// handed to `open`/`xdg-open` and a URL can never be looked for in the
    /// vault.
    fn follow(&mut self, target: md::LinkTarget) {
        match target {
            md::LinkTarget::Url(u) => self.open_url(&u),
            md::LinkTarget::Wiki(t) => self.follow_wikilink(&t),
            // the app drew this one from a file it had already found, so there
            // is nothing left to resolve
            md::LinkTarget::Note(p) => self.open_path(Path::new(&p)),
            md::LinkTarget::Tag(t) => self.open_tag(&t),
        }
    }

    /// Following a `#tag`: ^O, cut to the notes that carry it. Saved first,
    /// because the tag scan reads files and the note on screen may have just
    /// gained the tag being followed.
    fn open_tag(&mut self, tag: &str) {
        self.save_now();
        self.refresh_index();
        let hits = index::with_tag(&self.open_index, tag);
        if hits.is_empty() {
            self.flash(format!("no notes tagged #{tag}"));
            return;
        }
        self.query.clear();
        self.selected = 0;
        self.tag_filter = Some((tag.to_string(), hits));
        self.browse = false;
        self.contents = false;
        self.overlay = Overlay::QuickOpen;
    }

    /// ⌥⏎: the link under the cursor, from the keyboard. Plain enter has to go
    /// on inserting a newline, so following one needs a key of its own.
    fn follow_link_at_cursor(&mut self) {
        let pos = self.editor.cursor;
        match self
            .editor
            .lines()
            .get(pos.0)
            .and_then(|l| md::link_at(l, pos.1))
        {
            Some(t) => self.follow(t),
            None => self.flash("no link here".to_string()),
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

    /// ^Z / ^Y. Undo works on the buffer, so it also has to put the note and
    /// the autosave timer back in step with it.
    fn undo(&mut self) {
        self.overlay = Overlay::None;
        self.view = View::Edit;
        if self.editor.undo() {
            self.sync_editor_to_note();
        } else {
            self.flash("nothing to undo".to_string());
        }
    }

    fn redo(&mut self) {
        self.overlay = Overlay::None;
        self.view = View::Edit;
        if self.editor.redo() {
            self.sync_editor_to_note();
        } else {
            self.flash("nothing to redo".to_string());
        }
    }

    fn cut_selection(&mut self) {
        if self.view != View::Edit {
            self.flash("nothing selected".to_string());
            return;
        }
        match self.editor.selected_text() {
            Some(text) if !text.is_empty() => {
                let chars = text.chars().count();
                if crate::clipboard::copy(&text) {
                    self.editor.delete_selection();
                    self.sync_editor_to_note();
                    self.flash(format!("cut {chars} chars"));
                } else {
                    self.flash("copy failed — nothing cut".to_string());
                }
            }
            _ => self.flash("nothing selected".to_string()),
        }
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
        if !self.may_rename(&self.active_note().path.clone()) {
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

    /// Re-read the settings note and apply it. Everything but `notes_dir`
    /// takes effect on the very next frame — the colours, the page width, the
    /// table style — which is what makes editing settings in the app worth
    /// doing at all.
    pub fn reload_config(&mut self) {
        match Config::load() {
            Ok(config) => {
                let moved = config.notes_dir != self.config.notes_dir;
                // keep the probed graphics support: it is only asked for once,
                // in raw mode at startup, and cannot be asked for again here
                self.images.set_attachments(config.attachments_dir.clone());
                config.apply();
                self.editor.tab_width = config.tab_width;
                self.config = config;
                self.sync_title();
                // the roots to walk, or the setting itself, may have moved
                self.mentions.invalidate();
                if moved {
                    self.flash("notes_dir changed — restart catcher".to_string());
                } else {
                    self.flash("settings applied".to_string());
                }
            }
            Err(e) => self.flash(format!("settings reload failed: {e}")),
        }
    }

    pub fn on_mouse(&mut self, ev: MouseEvent) {
        // over a zoomed picture the wheel steps between pictures and a click
        // closes it; the pointer moving is nothing
        if self.zoom.is_some() {
            match ev.kind {
                MouseEventKind::ScrollUp | MouseEventKind::ScrollLeft => self.zoom_step(-1),
                MouseEventKind::ScrollDown | MouseEventKind::ScrollRight => self.zoom_step(1),
                MouseEventKind::Down(_) => self.unzoom(),
                _ => {}
            }
            return;
        }
        // the wheel over an open peek turns its pages, not the one beneath
        if let (MouseEventKind::ScrollUp | MouseEventKind::ScrollDown, Some(peek)) =
            (ev.kind, self.peek.as_mut())
        {
            if peek.contains(ev.column, ev.row) {
                let d = if ev.kind == MouseEventKind::ScrollUp {
                    -2
                } else {
                    2
                };
                peek.scroll_by(d);
                return;
            }
        }
        match ev.kind {
            MouseEventKind::ScrollUp => match (self.overlay, self.view) {
                (Overlay::Palette | Overlay::QuickOpen | Overlay::MoveFile, _) => {
                    self.selected = self.selected.saturating_sub(1)
                }
                (_, View::Preview) => self.preview_scroll = self.preview_scroll.saturating_sub(2),
                (_, View::Edit) => self.scroll_edit(-2),
            },
            MouseEventKind::ScrollDown => match (self.overlay, self.view) {
                (Overlay::Palette | Overlay::QuickOpen | Overlay::MoveFile, _) => {
                    let count = self.overlay_items().len();
                    if count > 0 && self.selected + 1 < count {
                        self.selected += 1;
                    }
                }
                (_, View::Preview) => self.preview_scroll = self.preview_scroll.saturating_add(2),
                (_, View::Edit) => self.scroll_edit(2),
            },
            MouseEventKind::ScrollLeft if self.view == View::Preview => self.pan(-4),
            MouseEventKind::ScrollRight if self.view == View::Preview => self.pan(4),
            MouseEventKind::Moved => self.on_hover(ev.column, ev.row),
            MouseEventKind::Down(MouseButton::Left) => {
                let (x, y) = (ev.column, ev.row);
                // a click on the popup opens the note it shows
                if self.peek.as_ref().is_some_and(|p| p.contains(x, y)) {
                    self.open_peek();
                    return;
                }
                self.peek = None;
                self.hover = None;
                if matches!(
                    self.overlay,
                    Overlay::Palette | Overlay::QuickOpen | Overlay::MoveFile
                ) {
                    if let Some((_, item)) = self
                        .palette_rows
                        .iter()
                        .find(|(r, _)| r.contains(ratatui::layout::Position { x, y }))
                        .cloned()
                    {
                        match beside_place(ev.modifiers) {
                            Some(place) if self.overlay == Overlay::QuickOpen => {
                                self.run_item_beside(item, place)
                            }
                            _ => self.run_item(item),
                        }
                    } else if !self
                        .overlay_rect
                        .contains(ratatui::layout::Position { x, y })
                    {
                        // outside the box dismisses; inside it but not on a row
                        // is the prompt, the rule or the footer hint, and a hint
                        // line that closed the overlay when clicked would be a
                        // small betrayal
                        self.overlay = Overlay::None;
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
                        if let Some(target) = self
                            .editor
                            .lines()
                            .get(pos.0)
                            .and_then(|l| md::link_at(l, pos.1))
                        {
                            self.follow(target);
                            return;
                        }
                    }
                    self.editor.clear_selection();
                    self.editor.set_cursor(pos);
                    self.editor.anchor = Some(self.editor.cursor);
                    self.dragging = true;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.preview_dragging => {
                if let (Some(point), Some((anchor, _))) =
                    (self.preview_point(ev.column, ev.row), self.preview_sel)
                {
                    self.preview_sel = Some((anchor, point));
                }
                // dragging past an edge scrolls the page, so a selection can
                // run past what happens to be on screen
                if ev.row <= self.editor_area.y {
                    self.preview_scroll = self.preview_scroll.saturating_sub(1);
                } else if ev.row + 1 >= self.editor_area.y + self.editor_area.height {
                    self.preview_scroll = self.preview_scroll.saturating_add(1);
                }
            }
            MouseEventKind::Up(MouseButton::Left) if self.preview_dragging => {
                self.preview_dragging = false;
                match self.preview_span() {
                    // a plain click, not a drag: nothing to copy, and the
                    // stray one-cell selection would only be visual noise
                    Some((a, b)) if a == b => self.preview_sel = None,
                    Some(_) => self.copy_preview_selection(),
                    None => {}
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

/// Every block in a buffer, with front matter made a block of its own unless
/// the settings say to leave it as ordinary markdown. Prepending the block and
/// starting the markdown scan on the line *below* it is what keeps the closing
/// `---` from being read as a rule and `tags:` from picking up emphasis;
/// filtering afterwards would not, since a stray ``` inside the block would
/// still have swallowed the rest of the note.
pub fn blocks_with(lines: &[String], front_matter: FrontMatter) -> Vec<md::Block> {
    if front_matter != FrontMatter::Show {
        if let Some(end) = notes::front_matter_end(lines.iter().map(String::as_str)) {
            let mut out = vec![md::Block {
                kind: md::BlockKind::FrontMatter,
                start: 0,
                end,
            }];
            out.extend(md::blocks_from(lines, end + 1));
            return out;
        }
    }
    md::blocks(lines)
}

/// Which line a note opens with the cursor on: the top of the file, except
/// when front matter is hidden and the file starts with some.
///
/// `hide` promises the block is not drawn until the cursor moves into it, and
/// a cursor parked at (0, 0) is already inside it — so without this every note
/// with front matter would open showing the very metadata the setting was
/// turned on to stop showing, and only start hiding it once you pressed ↓ past
/// it. The body is where the writing is; ↑ still walks back into the block.
pub fn opening_row(lines: &[String], front_matter: FrontMatter) -> usize {
    if front_matter != FrontMatter::Hide {
        return 0;
    }
    match notes::front_matter_end(lines.iter().map(String::as_str)) {
        // a file that is nothing but front matter has no body line to sit on,
        // and the block shows itself rather than leaving an empty screen
        Some(end) => (end + 1).min(lines.len().saturating_sub(1)),
        None => 0,
    }
}

/// Is a source line one the draw skips entirely, spending no rows on it at
/// all? Only front matter set to `hide` ever is, and it strikes the same
/// bargain a code fence strikes: the text is still in the file, and moving the
/// cursor into the block brings the whole thing back.
pub fn hidden_by(
    block: Option<&md::Block>,
    front_matter: FrontMatter,
    cursor_row: usize,
    selection: Option<(Pos, Pos)>,
) -> bool {
    front_matter == FrontMatter::Hide
        && block.is_some_and(|b| {
            b.kind == md::BlockKind::FrontMatter && !revealed_by(b, cursor_row, selection)
        })
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

/// Does a fold key fold here, or fall through to the editor as the word
/// motion it also is? A heading takes it; a heading with shift held does not,
/// because ⇧⌥← is extending a selection and that is a motion anywhere.
fn fold_key_takes(key: &KeyEvent, on_heading: bool) -> bool {
    on_heading && !key.modifiers.contains(KeyModifiers::SHIFT)
}

/// A folded heading's line with the `▸ ` marker in front. The marker stands
/// for source column 0 — the first `#` — so a click on it lands at the start
/// of the line and the cursor's own column mapping is untouched.
pub fn fold_marked(mut line: md::RLine) -> md::RLine {
    let marker = md::theme::FOLDED.chars().map(|ch| md::Cell {
        ch,
        style: md::theme::fold(),
        src: 0,
    });
    line.cells.splice(0..0, marker);
    line
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

/// Apply one key to a single-line text input — the palette's query and the
/// rename prompt. Returns true when it handled the key.
///
/// The cursor is always at the end, so this is typing and deleting rather than
/// full editing, but every way a Mac hand knows to delete works. Ghostty (and
/// most terminals) rewrite ⌘⌫ to Ctrl-U and ⌥⌫ to Ctrl-W before the app sees
/// them, so both spellings are taken.
fn edit_line(input: &mut String, key: &KeyEvent) -> bool {
    let m = key.modifiers;
    let cmd = m.contains(KeyModifiers::SUPER);
    let ctrl = m.contains(KeyModifiers::CONTROL);
    let alt = m.contains(KeyModifiers::ALT);
    match key.code {
        // ⌘⌫ and its legacy spelling: clear the whole line
        KeyCode::Backspace if cmd => input.clear(),
        KeyCode::Char('u') if ctrl => input.clear(),
        // ⌥⌫ and its legacy spelling: the word before the cursor
        KeyCode::Backspace if alt => delete_prev_word(input),
        KeyCode::Char('w') if ctrl => delete_prev_word(input),
        KeyCode::Backspace => {
            input.pop();
        }
        // ^-chords belong to the app; a plain character is what was typed
        KeyCode::Char(c) if !ctrl && !cmd && !alt => input.push(c),
        _ => return false,
    }
    true
}

/// Drop the trailing word of `input`, and the run of spaces before it.
fn delete_prev_word(input: &mut String) {
    while input.ends_with(' ') {
        input.pop();
    }
    while !input.is_empty() && !input.ends_with(' ') {
        input.pop();
    }
}

/// The text of `cells` between two display columns, as drawn. Columns rather
/// than indices because that is what a pointer lands on, and a wide character
/// covers two of them. `offset` is the column the first cell stands for — the
/// pan, for a row of a scrolling table.
/// The text a preview selection covers, from the rows the last draw recorded.
///
/// Free-standing so the rule for a row that has no cells can be tested without
/// a terminal behind it: such a row contributes its newline and nothing else,
/// rather than abandoning the whole copy. Blank lines between paragraphs are
/// already like that, and the linked-mentions footer makes them easy to drag
/// across.
fn selected_text(
    page_rows: &[(usize, Rect, usize)],
    rows: &[(Rect, Option<usize>, Vec<crate::render::PCell>)],
    ((sr, sc), (er, ec)): (PSel, PSel),
) -> String {
    let empty: Vec<crate::render::PCell> = Vec::new();
    let mut out = String::new();
    for (page_row, rect, offset) in page_rows {
        let row = *page_row;
        if row < sr || row > er {
            continue;
        }
        let cells = rows
            .iter()
            .find(|(r, _, _)| r.y == rect.y)
            .map(|(_, _, cells)| cells)
            .unwrap_or(&empty);
        let from = if row == sr { sc } else { *offset };
        let to = if row == er { ec } else { usize::MAX };
        out.push_str(&slice_cells(cells, *offset, from, to));
        if row < er {
            out.push('\n');
        }
    }
    out
}

fn slice_cells(cells: &[crate::render::PCell], offset: usize, from: usize, to: usize) -> String {
    let mut out = String::new();
    let mut col = offset;
    for c in cells {
        let w = md::char_width(c.ch);
        if col >= from && col < to {
            out.push(c.ch);
        }
        col += w;
    }
    out.trim_end().to_string()
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

/// Which way ⌥ / ⌥⇧ / ⌘ send a picker row: a split right, a split below,
/// a new tab. `None` is a plain open, in place.
fn beside_place(m: KeyModifiers) -> Option<crate::terminal::Place> {
    use crate::terminal::Place;
    if m.contains(KeyModifiers::SUPER) {
        Some(Place::Tab)
    } else if m.contains(KeyModifiers::ALT) && m.contains(KeyModifiers::SHIFT) {
        Some(Place::SplitDown)
    } else if m.contains(KeyModifiers::ALT) {
        Some(Place::SplitRight)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn peek_wraps_long_paragraphs_at_width() {
        let mut p = super::Peek {
            path: PathBuf::new(),
            exists: true,
            target: String::new(),
            name: String::new(),
            body: "# A heading that is longer than thirty columns\n\nAs Lead PM for a connected vehicle platform, I nudged a long paragraph across many rows.".into(),
            anchor: super::Rect::default(),
            rows: Vec::new(),
            rows_width: 0,
            scroll: 0,
            view_rows: 5,
            rect: super::Rect::default(),
        };
        p.ensure_rendered(30, crate::config::TableStyle::default());
        assert!(
            p.rows.len() > 2,
            "expected wrapped rows, got {}",
            p.rows.len()
        );
        for row in &p.rows {
            assert!(row.width() <= 30, "row wider than 30: {:?}", row);
        }
    }

    #[test]
    fn peek_scroll_clamps_to_content() {
        let mut p = super::Peek {
            path: PathBuf::new(),
            exists: true,
            target: String::new(),
            name: String::new(),
            body: String::new(),
            anchor: super::Rect::default(),
            rows: (0..20)
                .map(|i| ratatui::text::Line::from(i.to_string()))
                .collect(),
            rows_width: 10,
            scroll: 0,
            view_rows: 5,
            rect: super::Rect::default(),
        };
        assert_eq!(p.max_scroll(), 15);
        p.scroll_by(-3);
        assert_eq!(p.scroll, 0);
        p.scroll_by(7);
        assert_eq!(p.scroll, 7);
        p.scroll_by(100);
        assert_eq!(p.scroll, 15);
        // a shorter window than the content, and content shorter than the window
        p.view_rows = 30;
        p.clamp();
        assert_eq!(p.scroll, 0);
        assert_eq!(p.max_scroll(), 0);
    }

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
    fn the_mac_delete_keys_work_in_a_one_line_input() {
        use crossterm::event::KeyEvent;
        let key = |c, m| KeyEvent::new(c, m);
        let mut q = String::from("job application log");

        // ⌥⌫, and the Ctrl-W that Ghostty rewrites it to
        assert!(edit_line(
            &mut q,
            &key(KeyCode::Backspace, KeyModifiers::ALT)
        ));
        assert_eq!(q, "job application ");
        assert!(edit_line(
            &mut q,
            &key(KeyCode::Char('w'), KeyModifiers::CONTROL)
        ));
        assert_eq!(q, "job ");

        // ⌘⌫, and the Ctrl-U it arrives as
        assert!(edit_line(
            &mut q,
            &key(KeyCode::Backspace, KeyModifiers::SUPER)
        ));
        assert_eq!(q, "");
        q.push_str("more");
        assert!(edit_line(
            &mut q,
            &key(KeyCode::Char('u'), KeyModifiers::CONTROL)
        ));
        assert_eq!(q, "");

        // plain typing and plain backspace still do the obvious thing
        assert!(edit_line(
            &mut q,
            &key(KeyCode::Char('a'), KeyModifiers::NONE)
        ));
        assert!(edit_line(
            &mut q,
            &key(KeyCode::Char('b'), KeyModifiers::SHIFT)
        ));
        assert_eq!(q, "ab");
        assert!(edit_line(
            &mut q,
            &key(KeyCode::Backspace, KeyModifiers::NONE)
        ));
        assert_eq!(q, "a");

        // and a key the input has no use for is left for the caller
        assert!(!edit_line(&mut q, &key(KeyCode::Down, KeyModifiers::NONE)));
        assert!(!edit_line(&mut q, &key(KeyCode::Esc, KeyModifiers::NONE)));
        assert_eq!(q, "a");
    }

    #[test]
    fn a_preview_selection_takes_the_text_it_covers_by_column() {
        let cells = pcells("date  │  company");
        // a selection that starts mid-word takes only what it covers
        assert_eq!(slice_cells(&cells, 0, 0, 4), "date");
        assert_eq!(slice_cells(&cells, 0, 9, usize::MAX), "company");
        // trailing padding a table put there is not worth copying
        assert_eq!(slice_cells(&cells, 0, 0, 8), "date  │");
        // a column range past the end of the row is simply the rest of it
        assert_eq!(slice_cells(&cells, 0, 99, usize::MAX), "");
        // a panned row's first cell stands for a later column
        assert_eq!(slice_cells(&cells, 6, 6, 10), "date");
    }

    fn pcells(text: &str) -> Vec<crate::render::PCell> {
        text.chars()
            .map(|ch| crate::render::PCell {
                ch,
                style: ratatui::style::Style::default(),
                link: None,
                src: None,
            })
            .collect()
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
    fn front_matter_is_a_block_of_its_own_unless_the_setting_shows_it() {
        let lines: Vec<String> = "---\ntags: work\n---\n\n# Title\n\n---\n"
            .lines()
            .map(String::from)
            .collect();
        let dim = blocks_with(&lines, FrontMatter::Dim);
        assert_eq!(dim[0].kind, md::BlockKind::FrontMatter);
        assert_eq!((dim[0].start, dim[0].end), (0, 2));
        // the rule further down is still a rule, and the block's own closing
        // fence never became one
        assert_eq!(dim.len(), 2);
        assert_eq!(dim[1].kind, md::BlockKind::Rule);
        assert_eq!(dim[1].start, 6);
        // hide reads the block the same way; only the drawing differs
        assert_eq!(blocks_with(&lines, FrontMatter::Hide), dim);

        // show leaves it to the markdown scanner, which sees two rules
        let shown = blocks_with(&lines, FrontMatter::Show);
        assert!(shown.iter().all(|b| b.kind == md::BlockKind::Rule));
        assert_eq!(
            shown.iter().map(|b| b.start).collect::<Vec<_>>(),
            vec![0, 2, 6]
        );
    }

    #[test]
    fn a_note_with_hidden_front_matter_opens_on_its_body_and_not_inside_the_block() {
        let lines: Vec<String> = "---\ntags: work\n---\n# Title\n"
            .lines()
            .map(String::from)
            .collect();
        // a cursor at row 0 is inside the block, and a revealed block is a
        // drawn one: `hide` would show the metadata on every note that has any
        assert_eq!(opening_row(&lines, FrontMatter::Hide), 3);
        // the other two draw the block, so there is nothing to step over
        assert_eq!(opening_row(&lines, FrontMatter::Dim), 0);
        assert_eq!(opening_row(&lines, FrontMatter::Show), 0);
        // a note with no front matter, and one that is nothing but front
        // matter, both open at the only place they can
        let plain: Vec<String> = vec!["# Title".to_string()];
        assert_eq!(opening_row(&plain, FrontMatter::Hide), 0);
        let only: Vec<String> = "---\ntags: work\n---".lines().map(String::from).collect();
        assert_eq!(opening_row(&only, FrontMatter::Hide), 2);
    }

    #[test]
    fn the_cursor_reveals_front_matter_the_way_it_reveals_a_fence() {
        let lines: Vec<String> = "---\ntags: work\n---\n# Title\n"
            .lines()
            .map(String::from)
            .collect();
        let blocks = blocks_with(&lines, FrontMatter::Dim);
        let view = |row, cursor| {
            view_line(&lines, &blocks, row, 20, cursor, None)
                .cells
                .iter()
                .map(|c| c.ch)
                .collect::<String>()
        };
        // cursor outside: shown exactly as typed, fences and all — never
        // stretched into a rule the way a thematic break would be
        assert_eq!(view(0, 3), "---");
        assert_eq!(view(1, 3), "tags: work");
        // cursor anywhere inside reveals the whole block raw
        assert_eq!(view(2, 0), "---");
        assert_eq!(view(1, 0), "tags: work");
    }

    #[test]
    fn hidden_front_matter_is_skipped_until_the_cursor_moves_into_it() {
        let lines: Vec<String> = "---\ntags: work\n---\n# Title\n"
            .lines()
            .map(String::from)
            .collect();
        let blocks = blocks_with(&lines, FrontMatter::Hide);
        let block = md::block_at(&blocks, 1);
        // cursor down in the prose: the block takes no rows
        assert!(hidden_by(block, FrontMatter::Hide, 3, None));
        // cursor inside it, or a selection reaching into it: back it comes
        assert!(!hidden_by(block, FrontMatter::Hide, 1, None));
        assert!(!hidden_by(
            block,
            FrontMatter::Hide,
            3,
            Some(((0, 0), (3, 2)))
        ));
        // dim and show never hide anything, and prose is never hidden either
        assert!(!hidden_by(block, FrontMatter::Dim, 3, None));
        assert!(!hidden_by(
            md::block_at(&blocks, 3),
            FrontMatter::Hide,
            0,
            None
        ));
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
            // hidden columns run in a block here — `[[`, the target and the
            // pipe, then `]]` — so a wrap point can fall inside one
            "see [[stories/story-matrix|the matrix]] and then some more text",
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

    #[test]
    fn the_fold_keys_only_take_a_heading_and_never_a_shifted_motion() {
        let key = |m| KeyEvent::new(KeyCode::Left, m);
        assert!(fold_key_takes(&key(KeyModifiers::ALT), true));
        // off a heading the editor keeps its word motion
        assert!(!fold_key_takes(&key(KeyModifiers::ALT), false));
        // and a selection being extended is a motion even on one
        assert!(!fold_key_takes(
            &key(KeyModifiers::ALT | KeyModifiers::SHIFT),
            true
        ));
    }

    #[test]
    fn a_folded_heading_carries_its_marker_and_still_maps_its_columns() {
        let line = fold_marked(md::style_line("## Title"));
        let text: String = line.cells.iter().map(|c| c.ch).collect();
        assert_eq!(text, "\u{25b8} Title");
        assert_eq!(line.cells[0].style, md::theme::fold());
        let seg = line.one_row();
        // the marker stands for the start of the line; the text is where it was
        assert_eq!(seg.display_to_source(0), 0);
        assert_eq!(seg.display_to_source(2), 3);
        assert_eq!(seg.source_to_display(3), 2);
        assert_eq!(seg.source_to_display(0), 0);
        // the cursor's own line shows its source, marker and all
        let raw = fold_marked(md::RLine::raw("## Title"));
        let text: String = raw.cells.iter().map(|c| c.ch).collect();
        assert_eq!(text, "\u{25b8} ## Title");
    }

    #[test]
    fn every_fold_command_has_a_palette_row_and_an_action() {
        for c in [
            Command::FoldSection,
            Command::UnfoldSection,
            Command::FoldAll,
            Command::UnfoldAll,
        ] {
            assert!(COMMANDS.contains(&c));
            assert!(c.action().is_some());
            assert!(!c.label().0.is_empty());
        }
    }

    #[test]
    fn a_wikilink_target_that_climbs_out_of_the_vault_is_refused() {
        // a link target is note text, and the only thing between a pasted
        // vault and a write outside it is this check
        for bad in ["../x", "/etc/x", "~/x", "a/../../x", "", "  ", "a//b"] {
            assert!(App::link_note_path(bad).is_none(), "{bad}");
        }
        let (folder, name) = App::link_note_path("stories/story-matrix.md").unwrap();
        assert_eq!(folder, PathBuf::from("stories"));
        assert_eq!(name, "story-matrix");
        let (folder, name) = App::link_note_path("Story Matrix").unwrap();
        assert_eq!(folder, PathBuf::new());
        assert_eq!(name, "Story Matrix");
    }

    #[test]
    fn a_note_made_from_a_link_lands_beside_the_note_that_links_to_it() {
        let here = PathBuf::from("/vault/work");
        let (folder, _) = App::link_note_path("plan").unwrap();
        assert_eq!(
            App::create_dir(&here, &folder),
            PathBuf::from("/vault/work")
        );
        // a folder in the target is kept, under the same starting point
        let (folder, _) = App::link_note_path("stories/plan").unwrap();
        assert_eq!(
            App::create_dir(&here, &folder),
            PathBuf::from("/vault/work/stories")
        );
    }

    #[test]
    fn the_missing_link_hint_names_the_note_and_the_key() {
        assert_eq!(
            super::missing_link_hint("plan", "⌥⏎"),
            "no note called \u{201c}plan\u{201d} \u{b7} ⌥⏎ creates it"
        );
    }

    #[test]
    fn a_peek_at_a_link_to_nowhere_is_a_card_that_offers_to_create() {
        let p = super::Peek {
            path: PathBuf::new(),
            exists: false,
            target: "wikilink:plan".into(),
            name: "plan".into(),
            body: super::missing_link_hint("plan", "⌥⏎"),
            anchor: super::Rect::default(),
            rows: Vec::new(),
            rows_width: 0,
            scroll: 0,
            view_rows: 0,
            rect: super::Rect::default(),
        };
        assert_eq!(p.target_name(), "plan");
        assert!(p.body.contains("creates it"));
    }

    #[test]
    fn a_preview_selection_across_a_row_with_no_source_line_still_copies() {
        use crate::render::PCell;
        let cells = |text: &str, src: Option<usize>| -> Vec<PCell> {
            text.chars()
                .enumerate()
                .map(|(i, ch)| PCell {
                    ch,
                    style: md::theme::PLAIN,
                    link: None,
                    src: src.map(|l| (l, i)),
                })
                .collect()
        };
        let rect = |y: u16| Rect::new(0, y, 20, 1);
        let page_rows = vec![(0, rect(0), 0), (1, rect(1), 0), (2, rect(2), 0)];
        let span = ((0, 0), (2, 4));

        // the middle row is a blank line or a footer row: drawn, selectable,
        // and nowhere in the buffer
        let rows = vec![
            (rect(0), Some(3), cells("alpha", Some(3))),
            (rect(1), None, Vec::new()),
            (rect(2), Some(5), cells("beta", Some(5))),
        ];
        assert_eq!(selected_text(&page_rows, &rows, span), "alpha\n\nbeta");

        // and a row the draw recorded nothing at all for costs its own text,
        // never the text of everything around it
        let rows = vec![
            (rect(0), Some(3), cells("alpha", Some(3))),
            (rect(2), Some(5), cells("beta", Some(5))),
        ];
        assert_eq!(selected_text(&page_rows, &rows, span), "alpha\n\nbeta");
    }
}

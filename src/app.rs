use crate::commands;
use crate::config::{Config, FrontMatter, PreviewClick};
use crate::editor::{Editor, Pos};
use crate::images::{FileIndex, Images, Lookup};
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

mod peek;
mod table_edit;

use peek::missing_link_hint;
pub use peek::Peek;
use table_edit::{cell_source, screen_to_cell, slice_cells};
pub use table_edit::{CellSel, SelKind, TableEdge, TableHandle};

/// Which note a CLI invocation asked to open.
enum Want {
    Path(PathBuf),
    Title(String),
    New(String),
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

/// The two blank columns drawn before every row of a table in the editor,
/// where the row grip appears.
pub const TABLE_GUTTER: usize = 2;

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

/// Which of ^O's four tabs is on screen: the ranked list, the folder
/// tree, a search over note contents, or every tag in the vault.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum QuickTab {
    Recent,
    Tree,
    Contents,
    Tags,
    /// The bookmarked notes, in the order they were bookmarked.
    Bookmarks,
    /// Every `[[wikilink]]` in the vault that reaches no note. Reached from
    /// the palette, not from tab.
    Unresolved,
}

#[derive(PartialEq, Clone, Copy)]
pub enum Overlay {
    None,
    Palette,
    /// ^O: every note in the vault, recently opened first.
    QuickOpen,
    ConfirmDelete,
    RenameFile,
    /// Find and replace within the open note.
    Find,
    /// Move the open note to another folder under the session root.
    MoveFile,
    /// Every heading of the open note: ⏎ goes there, ⌥⏎ folds it.
    Outline,
    /// Re-root the session in another folder: recent vaults, or a typed path.
    OpenVault,
    Help,
}

/// The completion popup: the token it answers, its rows and the one the
/// arrows are on.
#[derive(Clone, Debug)]
pub struct Completion {
    pub token: crate::complete::Token,
    pub items: Vec<crate::complete::Candidate>,
    pub selected: usize,
}

#[derive(Clone, PartialEq)]
pub enum Command {
    /// A palette row for something a key can also do: one path, either way.
    Act(Action),
    MoveFile,
    /// Every unresolved `[[wikilink]]` in the vault, in ^O.
    Unresolved,
    /// Bookmark the open note, or take its bookmark away.
    Bookmark,
    /// The bookmarked notes, in ^O.
    Bookmarks,
    /// Re-root the session in another folder.
    OpenVault,
    InsertTable,
    Table(crate::table::Op),
    TableSource,
    InsertCallout,
    InsertMath,
    InsertFootnote,
}

/// The palette's row and column commands, in the order they are listed.
const TABLE_OPS: [crate::table::Op; 13] = {
    use crate::table::Op;
    [
        Op::RowAbove,
        Op::RowBelow,
        Op::RowDelete,
        Op::RowDuplicate,
        Op::ColLeft,
        Op::ColRight,
        Op::ColDelete,
        Op::ColMoveLeft,
        Op::ColMoveRight,
        Op::ColDuplicate,
        Op::AlignLeft,
        Op::AlignCenter,
        Op::AlignRight,
    ]
};

const COMMANDS: [Command; 39] = [
    Command::Act(Action::NewNote),
    Command::Act(Action::DailyNote),
    Command::Act(Action::QuickOpen),
    Command::Act(Action::SearchAll),
    Command::Act(Action::Outline),
    Command::Act(Action::Tags),
    Command::Act(Action::ToggleProperties),
    Command::Act(Action::HideProperties),
    Command::Act(Action::ToggleOpener),
    Command::Act(Action::DeleteNote),
    Command::Act(Action::RenameFile),
    Command::MoveFile,
    Command::Unresolved,
    Command::Bookmark,
    Command::Bookmarks,
    Command::OpenVault,
    Command::Act(Action::Find),
    Command::Act(Action::TogglePreview),
    Command::Act(Action::Help),
    Command::Act(Action::Settings),
    Command::Act(Action::FoldSection),
    Command::Act(Action::UnfoldSection),
    Command::Act(Action::FoldAll),
    Command::Act(Action::UnfoldAll),
    Command::Act(Action::Quit),
    Command::Act(Action::ToggleCheckbox),
    Command::Act(Action::MoveLineUp),
    Command::Act(Action::MoveLineDown),
    Command::Act(Action::ToggleHeading),
    Command::Act(Action::InsertDate),
    Command::Act(Action::CopyPath),
    Command::Act(Action::RevealFile),
    Command::Act(Action::OpenSplitRight),
    Command::Act(Action::OpenSplitDown),
    Command::Act(Action::OpenTab),
    Command::InsertTable,
    Command::InsertCallout,
    Command::InsertMath,
    Command::InsertFootnote,
];

impl Command {
    /// The action this command runs, when it is one a key can be bound to —
    /// which is how the palette knows what key to show beside it.
    pub fn action(&self) -> Option<Action> {
        match self {
            Command::Act(a) => Some(*a),
            // palette-only: a move is rare enough that it earns no key
            _ => None,
        }
    }

    pub fn label(&self) -> (&'static str, &'static str) {
        match self {
            Command::Act(a) => match a {
                Action::NewNote => ("New note", "an empty note, ready to type"),
                Action::DailyNote => ("Today's note", "one note a day, made if missing"),
                Action::QuickOpen => ("Open note", "any folder, recent first"),
                Action::SearchAll => ("Search in all files", "type to search note contents"),
                Action::DeleteNote => ("Delete note", "delete the file on disk"),
                Action::RenameFile => ("Rename file", "change the name on disk"),
                Action::Find => ("Find in note", "step through matches in this note, or replace them"),
                Action::TogglePreview => ("Reading view", "the page, rendered"),
                Action::Help => ("Help", "every key, on one card"),
                Action::Settings => ("Settings", "edit them here, as a note"),
                Action::FoldSection => ("Fold section", "hide what is under this heading"),
                Action::UnfoldSection => ("Unfold section", "show it again"),
                Action::FoldAll => ("Fold all", "every section, headings only"),
                Action::UnfoldAll => ("Unfold all", "open every fold in the note"),
                Action::Quit => ("Quit", "save and exit"),
                Action::ToggleCheckbox => ("Toggle checkbox", "item → [ ] → [x] → item"),
                Action::MoveLineUp => ("Move line up", "the line or selection, one up"),
                Action::MoveLineDown => ("Move line down", "the line or selection, one down"),
                Action::ToggleHeading => ("Toggle heading", "#, ##, ###, then none"),
                Action::InsertDate => ("Insert today's date", "2026-09-01, at the cursor"),
                Action::CopyPath => ("Copy path", "the note's path, to the clipboard"),
                Action::RevealFile => ("Reveal in Finder", "show the file on disk"),
                Action::OpenSplitRight => ("Open in split right", "this note again, beside this one"),
                Action::OpenSplitDown => ("Open in split down", "this note again, below this one"),
                Action::OpenTab => ("Open in new tab", "this note again, in a terminal tab"),
                Action::Outline => ("Outline", "every heading in this note; ⏎ goes there, ⌥⏎ folds"),
                Action::Tags => ("Tags", "every tag in the vault with its note count; ⏎ lists the notes"),
                Action::ToggleProperties => ("Toggle properties (hide / show)", "the front matter: box, line or hidden on the page; dim or hidden in the editor"),
                Action::HideProperties => ("Hide properties", "the front matter off the page entirely; Toggle properties brings it back"),
                Action::ToggleOpener => ("Toggle opener", "the decode animation when catcher starts: on or off"),
                // the rest have no palette row; COMMANDS never names them
                _ => ("", ""),
            },
            Command::MoveFile => ("Move to folder", "another folder under this one"),
            Command::Unresolved => ("Unresolved links", "every [[link]] to a note that is not there; ⏎ goes to it"),
            Command::Bookmark => ("Bookmark note", "keep this note in the bookmarks list, or take it out"),
            Command::Bookmarks => ("Bookmarks", "the bookmarked notes; ⏎ opens one"),
            Command::OpenVault => ("Open vault…", "another folder as the notes folder for this session"),
            Command::InsertTable => ("Table: Insert table", "a 2×2 grid at the cursor"),
            Command::InsertCallout => ("Insert callout", "> [!note] with a title and a body"),
            Command::InsertMath => ("Insert math block", "$$ … $$ on lines of their own"),
            Command::InsertFootnote => ("Insert footnote", "[^n] here, its text at the end of the note"),
            Command::TableSource => ("Table: Edit source", "the pipes, until the cursor leaves"),
            Command::Table(op) => {
                use crate::table::Op;
                match op {
                    Op::RowAbove => ("Table: Add row above", "an empty row over this one"),
                    Op::RowBelow => ("Table: Add row below", "an empty row under this one"),
                    Op::RowDelete => ("Table: Delete row", "this row"),
                    Op::RowDuplicate => ("Table: Duplicate row", "this row again, under it"),
                    Op::ColLeft => ("Table: Add column to the left", "an empty column before this one"),
                    Op::ColRight => ("Table: Add column to the right", "an empty column after this one"),
                    Op::ColDelete => ("Table: Delete column", "this column"),
                    Op::ColMoveLeft => ("Table: Move column left", "swap with the column before"),
                    Op::ColMoveRight => ("Table: Move column right", "swap with the column after"),
                    Op::ColDuplicate => ("Table: Duplicate column", "this column again, after it"),
                    Op::AlignLeft => ("Table: Align left", "this column"),
                    Op::AlignCenter => ("Table: Align center", "this column"),
                    Op::AlignRight => ("Table: Align right", "this column"),
                }
            }
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
            (
                "tab / ⇧tab",
                "in a table: next / previous cell; past the last, a new row",
            ),
            ("↵", "in a table: a row below"),
            ("esc", "in a table: show its source, and back"),
            (
                "⇧↑↓←→ / drag",
                "in a table: select cells; the grips beside and above select rows and columns",
            ),
            ("⌥↑↓←→", "in a table: move the selected rows or columns"),
            (
                "⌫ ⌘C ⌘X ⌘V",
                "on selected cells: clear, copy, cut, paste (tabs and newlines)",
            ),
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
            ("tab", "in ^O, next tab: recent · tree · contents · tags"),
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
    /// A heading of the open note, by its line, from the outline picker.
    /// Choosing it puts the cursor there; with ⌥ it folds the section.
    Heading(usize),
    /// A tag from the tags tab, in [`md::tag_key`] form. Choosing it lists
    /// the notes carrying it, the way following a `#tag` does.
    Tag(String),
    /// A note by path with a line in it, from the unresolved-links list.
    /// Choosing it opens the note there.
    At(PathBuf, usize),
    /// A folder to re-root the session in, from the vault picker.
    Vault(PathBuf),
    Command(Command),
}

/// One row the reading view drew: where it sits on screen, which page row it
/// is, and what it drew. `pan` is the display column the first drawn cell
/// stands for — the pan for a row of a scrolling table and zero for everything
/// else — and is what keeps a click landing on the character under the
/// pointer. `src_line` is `None` for a row the renderer invented — a blank
/// line between paragraphs, a linked-mentions row — which can be selected and
/// copied but is nowhere in the buffer to click into.
#[derive(Clone, Debug)]
pub struct PreviewRow {
    pub page_row: usize,
    pub rect: Rect,
    pub pan: usize,
    pub src_line: Option<usize>,
    pub cells: Vec<crate::render::PCell>,
}

/// One screen row of the reading view, after wrapping and image expansion.
#[derive(Clone, Debug)]
pub struct PageRow {
    pub cells: Vec<crate::render::PCell>,
    pub checkbox: Option<usize>,
    pub src_line: Option<usize>,
    /// A row of a scrolling table: never soft-wrapped, panned instead.
    pub wide: bool,
}

/// The reading view's page as laid out before the scroll window is cut from
/// it: parsed, folded, wrapped, with the rows its pictures take. Rebuilt only
/// when something it was made from changes — the draw runs ten times a second
/// whether anything happened or not, and a parse of the whole note each time
/// is work nobody asked for.
#[derive(Clone, Debug, Default)]
pub struct PreviewPage {
    /// A hash of every input the layout reads; `None` before the first draw.
    pub key: Option<u64>,
    pub rows: Vec<PageRow>,
    /// (first page row, rows, image index) of each picture drawn as a band.
    pub bands: Vec<(usize, u16, usize)>,
    /// Link targets by index, what a cell's `link` points into.
    pub urls: Vec<String>,
    pub images: Vec<crate::render::ImageSpec>,
    /// The widest table row, which bounds the sideways pan.
    pub widest: usize,
}

/// What a tag walk answers with: the fresh index, and the hits in it.
type TagScan = (Vec<index::Entry>, Vec<usize>);

pub struct App {
    pub config: Config,
    /// Bumped whenever `config` is replaced, so anything laid out under the
    /// old one — table style, theme colours — knows to start over.
    pub config_gen: u64,
    /// The reading view's laid-out page, kept between frames.
    pub preview_page: PreviewPage,
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
    /// The system-appearance check in flight, if one is; see
    /// [`App::follow_system_theme`].
    theme_rx: Option<std::sync::mpsc::Receiver<Option<crate::theme::Mode>>>,
    pub status: Option<(String, Instant)>,
    /// The start-up decode animation, while it runs: when it began and the
    /// seed its scatter is drawn from. See `opener`.
    pub opener: Option<(Instant, u64)>,
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
    /// The table block (by its first line) showing its raw pipes while the
    /// cursor is in it, rather than the grid.
    pub table_source: Option<usize>,
    /// The table (by its first line) whose right or bottom edge the pointer
    /// is at in the editor, which is when its add handle shows.
    pub table_hover: Option<(usize, TableEdge)>,
    /// The handles the last draw put beside a hovered table.
    pub table_handles: Vec<(Rect, TableHandle)>,
    /// Selected cells of a table, if any.
    pub cell_sel: Option<CellSel>,
    /// A press in a grid cell that may become a drag across cells: the
    /// table, the cell, and the position pressed.
    table_drag: Option<(usize, (usize, usize), Pos)>,
    /// Every row the last draw put on screen, in order.
    pub preview_rows: Vec<PreviewRow>,
    pub images: Images,
    dragging: bool,
    /// Every `.md` file quick-open can reach, rebuilt each time ^O opens.
    pub open_index: Vec<index::Entry>,
    /// A walk started on a thread and not yet collected — the launch one,
    /// which must not hold up the first frame.
    index_rx: Option<std::sync::mpsc::Receiver<Vec<index::Entry>>>,
    /// The vault-wide walk of non-note files by name, started with every
    /// index scan and taken here when it lands: the root it was walked from,
    /// and the answer on its way.
    file_index_rx: Option<(PathBuf, std::sync::mpsc::Receiver<FileIndex>)>,
    /// The tag scan in flight for `tag_filter`, if one is: the tag it is
    /// for, and the fresh index plus the hits in it that the worker answers
    /// with. `None` once the answer has landed, so the overlay's hint can
    /// tell "scanning" from "no query yet".
    tag_rx: Option<(String, std::sync::mpsc::Receiver<TagScan>)>,
    /// The ^O tab on screen: recent, tree, contents, or tags.
    pub tab: QuickTab,
    /// Every unresolved link the last scan found, for the unresolved tab.
    unresolved: Vec<crate::unresolved::Broken>,
    /// That scan in flight; `poll_unresolved_scan` takes the answer.
    unresolved_rx: Option<crate::unresolved::Pending>,
    /// The bookmarked notes, root-relative, read when the tab is entered.
    bookmarks: Vec<String>,
    /// Every tag the tags tab lists, with how many notes carry each, most
    /// used first. Gathered on a thread when the tab is entered.
    tag_list: Vec<(String, usize)>,
    /// That gathering in flight; `poll_tags_scan` takes the answer.
    tags_rx: Option<std::sync::mpsc::Receiver<Vec<(String, usize)>>>,
    /// Every body the contents tab searches, one per `open_index` entry, read
    /// once when the tab is entered so a keystroke never touches the disk.
    contents_bodies: Vec<Option<crate::contents::Body>>,
    /// The read of those bodies in flight: entering the tab hands the disk
    /// work to a thread, and `poll_contents_scan` takes the answer.
    contents_rx: Option<std::sync::mpsc::Receiver<Vec<Option<crate::contents::Body>>>>,
    /// The last query's rows, so a redraw with the query unchanged scans
    /// nothing. Cleared whenever the bodies change under it.
    contents_cache: std::cell::RefCell<Option<(String, Vec<crate::contents::Row>)>>,
    /// Every folder the move picker offers, walked once when it opens rather
    /// than on every frame it is on screen.
    move_targets: Vec<(PathBuf, usize)>,
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
    /// Buffer for the inline rename prompt.
    pub rename_input: String,
    /// The find prompt's two fields, and which of them the typing goes to.
    pub find_input: String,
    pub replace_input: String,
    pub find_replacing: bool,
    /// Every match of `find_input` in the open note, kept while the prompt
    /// is up, and which of them is the current one.
    find_matches: Vec<crate::find::Match>,
    pub find_at: Option<usize>,
    /// Where you have been, for ^B and ^F.
    pub history: crate::history::History,
    /// What has been typed into the shortcuts card, which filters its rows.
    pub help_query: String,
    /// The wikilink the pointer is resting on in the reading view, and when
    /// it arrived there. A peek opens once it has stayed put for a moment.
    hover: Option<(String, Rect, Instant)>,
    /// The peek popup on screen, if any: the note a link points at, loaded
    /// once when the link under the pointer changed and not touched again
    /// for every pointer twitch after.
    pub peek: Option<Peek>,
    /// The completion popup, while one is up.
    pub complete: Option<Completion>,
    /// The screen cell of the editor's cursor as last drawn: where the
    /// completion popup hangs from.
    pub complete_anchor: Option<(u16, u16)>,
    /// The token esc put away, so it stays away until the cursor leaves it.
    complete_dismissed: Option<(usize, usize)>,
    /// Every tag in the vault, read once per index and dropped when the
    /// index is rebuilt.
    tag_cache: Option<Vec<String>>,
    /// Which headings are folded, per note, for as long as the app runs.
    folds: crate::fold::Folds,
    /// The open note's lines as they stand on screen, rebuilt whenever the
    /// buffer or its folds change. Everything that walks the note by line —
    /// the draw, ↑↓, the wheel — asks this rather than the buffer.
    pub visible: crate::fold::Visible,
}

/// The most of the screen the peek may take, in percent of its height.
pub const PEEK_MAX_HEIGHT_PCT: u16 = 40;

impl App {
    /// Build the app for one of the CLI's launch shapes.
    pub fn launch(launch: crate::cli::Launch) -> Result<Self> {
        use crate::cli::Launch;
        let (config, config_warning) = Config::load_reporting()?;
        config.ensure_dirs()?;
        // before anything is rendered: every style resolves against this
        config.apply();

        // where this session is rooted, and which note it should open on
        let (dir, want): (PathBuf, Option<Want>) = match &launch {
            Launch::Default => (config.notes_dir.clone(), None),
            Launch::Name(n) => (config.notes_dir.clone(), Some(Want::Title(n.clone()))),
            Launch::New(n) => (config.notes_dir.clone(), Some(Want::New(n.clone()))),
            Launch::Today => {
                let path = crate::daily::ensure(
                    &config.daily_dir(),
                    &config.daily_format,
                    &config.daily_template(),
                    crate::dates::now(),
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
                None => anyhow::bail!(
                    "no note matching \u{201c}{name}\u{201d} \u{b7} catcher new {name} creates one"
                ),
            },
            Some(Want::New(name)) => {
                if let Some(n) = all.iter().find(|n| n.title().eq_ignore_ascii_case(&name)) {
                    anyhow::bail!(
                        "a note called \u{201c}{}\u{201d} already exists: {}",
                        n.title(),
                        n.path.display()
                    );
                }
                all.insert(0, notes::create_with(&dir, format!("# {name}\n"))?);
                active = 0;
            }
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
            find_input: String::new(),
            replace_input: String::new(),
            find_replacing: false,
            find_matches: Vec::new(),
            find_at: None,
            help_query: String::new(),
            images: Images::new(Lookup::new(
                config.attachments_dir.clone(),
                config.attachment_subfolder.clone(),
            )),
            config,
            config_gen: 0,
            preview_page: PreviewPage::default(),
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
            theme_rx: None,
            status: None,
            opener: None,
            quit: false,
            last_title: None,
            dirty: false,
            disk_checked: Instant::now(),
            last_edit: Instant::now(),
            editor_area: Rect::default(),
            complete: None,
            complete_anchor: None,
            complete_dismissed: None,
            tag_cache: None,
            edit_rows: Vec::new(),
            palette_rows: Vec::new(),
            preview_links: Vec::new(),
            preview_images: Vec::new(),
            preview_image_urls: Vec::new(),
            zoom: None,
            preview_checkboxes: Vec::new(),
            table_source: None,
            table_hover: None,
            table_handles: Vec::new(),
            cell_sel: None,
            table_drag: None,
            preview_rows: Vec::new(),
            dragging: false,
            open_index: Vec::new(),
            index_rx: None,
            file_index_rx: None,
            tag_rx: None,
            tab: QuickTab::Recent,
            unresolved: Vec::new(),
            unresolved_rx: None,
            bookmarks: Vec::new(),
            tag_list: Vec::new(),
            tags_rx: None,
            tag_filter: None,
            tree_open: BTreeSet::new(),
            contents_bodies: Vec::new(),
            contents_rx: None,
            contents_cache: std::cell::RefCell::new(None),
            move_targets: Vec::new(),
            preview_goto: None,
            overlay_rect: Rect::default(),
            hover: None,
            peek: None,
            mentions: crate::mentions::Backlinks::default(),
            recents,
            preview_sel: None,
            preview_dragging: false,
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
        if let Some(w) = config_warning {
            app.flash(w);
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
        // a `[!kind]-` callout starts folded, the first time the note is seen
        self.folds
            .seed(&self.notes[self.active].path, self.editor.lines(), &blocks);
        self.refresh_visible();
        self.sync_title();
        self.refresh_file_resolver();
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

    /// Is `row` the title line of a callout card?
    fn callout_title_at(&self, row: usize) -> bool {
        crate::fold::callout_at(self.editor.lines(), &self.blocks(), row).is_some()
    }

    /// The folded headings of the open note, in order.
    pub fn folded_lines(&self) -> Vec<usize> {
        self.folds.of(&self.notes[self.active].path)
    }

    /// What a folded heading says at its right edge. A folded callout title
    /// says it inside its top edge instead, so it has no label here.
    pub fn fold_label(&self, row: usize) -> Option<String> {
        if !self.folded_here(row) || self.callout_title_at(row) {
            return None;
        }
        Some(match self.visible.hidden_under(row) {
            1 => "1 line folded".to_string(),
            n => format!("{n} lines folded"),
        })
    }

    /// Is the cursor on a heading line, or a callout's title — the places
    /// the fold keys apply?
    fn on_heading(&self) -> bool {
        crate::fold::foldable_at(self.editor.lines(), &self.blocks(), self.editor.cursor.0)
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
                let heading =
                    |line: usize| crate::fold::foldable_at(self.editor.lines(), &blocks, line);
                let at_sel = self.preview_span().and_then(|((row, _), _)| {
                    self.preview_rows
                        .iter()
                        .find(|r| r.page_row == row)?
                        .src_line
                        .filter(|&l| heading(l))
                });
                at_sel.or_else(|| {
                    self.preview_rows
                        .iter()
                        .filter_map(|r| r.src_line)
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
        let is_callout = crate::fold::callout_at(self.editor.lines(), &blocks, row).is_some();
        match self.folds.fold(&path, self.editor.lines(), &blocks, row) {
            Some(_) => self.folds_changed(),
            None if is_heading => self.flash("nothing under this heading".to_string()),
            None if is_callout => self.flash("nothing under this callout".to_string()),
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
        if notes::sync_disk(&mut self.notes[self.active]) == notes::Disk::Changed {
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
        // the headings as they stood on disk, so a renamed one can be told
        let before = self.notes[self.active].saved.clone();
        match notes::save(&dir, &mut self.notes[self.active], allow_rename) {
            Ok(now) => {
                self.dirty = false;
                if let Some(done) = self.update_heading_links(&before, &now) {
                    self.flash(done);
                }
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
        self.adopt_rewritten(&report);
        report.describe()
    }

    /// The note at `path` was just saved: when exactly one of its headings
    /// was renamed since the last save, point the `[[note#heading]]` links
    /// under the roots at the new name. `updated K links` when any were.
    fn update_heading_links(&mut self, before: &str, path: &Path) -> Option<String> {
        if !self.config.update_links || before.is_empty() {
            return None;
        }
        let after = self.notes[self.active].saved.clone();
        let (old, new) = crate::links::heading_change(before, &after)?;
        let report = crate::links::retarget_heading(path, &old, &new, &self.index_roots());
        if report.links == 0 {
            return None;
        }
        self.adopt_rewritten(&report);
        Some(match report.links {
            1 => "updated 1 link".to_string(),
            n => format!("updated {n} links"),
        })
    }

    /// Refresh any note this session holds a copy of that a rewrite wrote
    /// back, so switching to one does not show the old text.
    fn adopt_rewritten(&mut self, report: &crate::links::Report) {
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
    }

    pub fn maybe_autosave(&mut self) -> bool {
        let after = Duration::from_millis(self.config.autosave_ms);
        if self.dirty && self.last_edit.elapsed() >= after {
            self.save_now();
            return true;
        }
        false
    }

    pub fn flash(&mut self, msg: String) {
        self.status = Some((msg, Instant::now()));
    }

    /// Every half second, ask whether another program has touched the open
    /// note's file. A change is taken as it stands; a deletion is announced
    /// once and the buffer kept, so the next save puts the file back.
    fn watch_disk(&mut self) -> bool {
        if self.disk_checked.elapsed() < Duration::from_millis(500) {
            return false;
        }
        self.disk_checked = Instant::now();
        match notes::sync_disk(&mut self.notes[self.active]) {
            notes::Disk::Unchanged => false,
            notes::Disk::Changed => {
                self.reload_from_disk();
                true
            }
            notes::Disk::Gone => {
                // forget the stamp so this is said once, and so the file
                // coming back reads as a change
                self.notes[self.active].stamp = None;
                self.flash("deleted on disk".to_string());
                true
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

    /// Housekeeping between frames. Returns whether anything on screen may
    /// have changed, so the loop can skip the draw while idle; anything
    /// unsure says yes.
    pub fn tick(&mut self) -> bool {
        let mut changed = self.watch_disk();
        if let Some((at, _)) = self.opener {
            if at.elapsed() >= crate::opener::total() {
                self.opener = None;
            }
            changed = true;
        }
        changed |= self.maybe_autosave();
        changed |= self.follow_system_theme();
        changed |= self.poll_index_scan();
        changed |= self.poll_file_index_scan();
        changed |= self.poll_tag_scan();
        changed |= self.poll_tags_scan();
        changed |= self.poll_contents_scan();
        changed |= self.poll_unresolved_scan();
        changed |= self.maybe_peek();
        // a filename that followed its title on save; the title is the
        // terminal's, not the frame's
        self.sync_title();
        if let Some((_, at)) = self.status {
            if at.elapsed() > Duration::from_secs(3) {
                self.status = None;
                changed = true;
            }
        }
        changed
    }

    /// With `theme: auto` on a terminal that was found to track the system
    /// appearance, flip the palette when the system does — the terminal has
    /// already repainted itself by then, and the old palette reads wrong on
    /// it. Checked every couple of seconds; a change reloads the settings so
    /// colour overrides still sit on top of the new base.
    fn follow_system_theme(&mut self) -> bool {
        if self.config.theme != crate::config::Theme::Auto
            || !crate::theme::follows_system()
            || self.theme_checked.elapsed() < Duration::from_secs(2)
        {
            return false;
        }
        // one `defaults read` at a time, off the draw loop: the answer is
        // taken on a later tick, the way an index walk is
        let mode = match self.theme_rx.as_ref().map(|rx| rx.try_recv()) {
            None => {
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(crate::theme::system_mode());
                });
                self.theme_rx = Some(rx);
                return false;
            }
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) => return false,
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                self.theme_rx = None;
                return false;
            }
            Some(Ok(mode)) => {
                self.theme_rx = None;
                self.theme_checked = Instant::now();
                let Some(mode) = mode else {
                    return false;
                };
                mode
            }
        };
        if mode != crate::theme::detected() {
            crate::theme::set_detected(mode);
            if let Ok(config) = Config::load() {
                config.apply();
                self.config = config;
                self.config_gen += 1;
            }
            return true;
        }
        false
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

    /// How the reading view draws this note's front matter: the setting.
    pub fn properties_mode(&self) -> crate::config::Properties {
        self.config.properties
    }

    /// *Hide properties*: straight to nothing on the page, or to a hidden
    /// block in the editor, without cycling through the line.
    fn hide_properties(&mut self) {
        use crate::config::{FrontMatter, Properties};
        let (key, value) = match self.view {
            View::Preview => {
                self.config.properties = Properties::Hide;
                ("properties", "hide")
            }
            View::Edit => {
                self.config.front_matter = FrontMatter::Hide;
                ("front_matter", "hide")
            }
        };
        self.save_setting(key, value);
    }

    /// Begin the start-up animation, if the settings want one. Called once,
    /// from `main`, on a cold start; opening another note never replays it.
    pub fn start_opener(&mut self) {
        if self.config.opener {
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(1);
            self.opener = Some((Instant::now(), seed));
        }
    }

    pub fn opener_running(&self) -> bool {
        self.opener.is_some()
    }

    /// Write one flipped setting to the settings note and say so.
    fn save_setting(&mut self, key: &str, value: &str) {
        match crate::config::set_value(key, value) {
            Ok(()) => self.flash(format!("{key}: {value}")),
            Err(e) => self.flash(format!("{key}: {value} (not saved: {e})")),
        }
    }

    /// *Toggle properties* on the page cycles box → line → hide → box; a
    /// click on the box's edge folds it to the line and a click on the line
    /// opens the box (`click` — a click never hides the lot, since there
    /// would be nothing left to click). Written to the settings so every note
    /// follows. In the editor the command flips `front_matter` dim ⇄ hide.
    fn toggle_properties(&mut self, click: bool) {
        use crate::config::{FrontMatter, Properties, Words};
        let (key, value) = match self.view {
            View::Preview => {
                let next = match self.config.properties {
                    Properties::Box => Properties::Line,
                    Properties::Line if click => Properties::Box,
                    Properties::Line => Properties::Hide,
                    Properties::Hide => Properties::Box,
                };
                self.config.properties = next;
                ("properties", next.name())
            }
            View::Edit => {
                let next = match self.config.front_matter {
                    FrontMatter::Hide => FrontMatter::Dim,
                    FrontMatter::Dim | FrontMatter::Show => FrontMatter::Hide,
                };
                self.config.front_matter = next;
                (
                    "front_matter",
                    match next {
                        FrontMatter::Dim => "dim",
                        FrontMatter::Show => "show",
                        FrontMatter::Hide => "hide",
                    },
                )
            }
        };
        self.save_setting(key, value);
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
        self.tag_cache = None;
        self.start_file_index_scan();
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
        self.start_file_index_scan();
    }

    /// Walk the vault for every file that is not a note, on a thread, so a
    /// `[[report.pdf]]` finds its file wherever Obsidian put it. Run alongside
    /// every index scan; the answer is taken by [`App::poll_file_index_scan`].
    fn start_file_index_scan(&mut self) {
        let root = self.file_index_root();
        let (tx, rx) = std::sync::mpsc::channel();
        let walk_root = root.clone();
        std::thread::spawn(move || {
            let _ = tx.send(FileIndex::scan(&walk_root));
        });
        self.file_index_rx = Some((root, rx));
    }

    /// The vault the file index covers: the notes dir, unless this session
    /// is rooted somewhere outside it.
    fn file_index_root(&self) -> PathBuf {
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let dir = canon(&self.dir);
        let vault = canon(&self.config.notes_dir);
        if dir.starts_with(&vault) {
            vault
        } else {
            dir
        }
    }

    /// Take a finished file walk as the lookup's index, if it is for the root
    /// the app is still on.
    fn poll_file_index_scan(&mut self) -> bool {
        let Some((root, rx)) = self.file_index_rx.as_ref() else {
            return false;
        };
        match rx.try_recv() {
            Ok(index) => {
                if *root == self.file_index_root() {
                    let mut lookup = self.images.lookup().clone();
                    lookup.index = std::sync::Arc::new(index);
                    self.images.set_lookup(lookup);
                    self.refresh_file_resolver();
                }
                self.file_index_rx = None;
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.file_index_rx = None;
                false
            }
        }
    }

    /// Take a walk started by [`App::start_index_scan`] if it has finished.
    fn poll_index_scan(&mut self) -> bool {
        let Some(rx) = self.index_rx.as_ref() else {
            return false;
        };
        match rx.try_recv() {
            Ok(entries) => {
                self.adopt_index(entries);
                self.index_rx = None;
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            // only a panic in the walk can do this; there is nothing to wait
            // for any more, and ^O will walk again itself
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.index_rx = None;
                true
            }
        }
    }

    /// Take a fresh walk as the index. A tag's list is indices into the old
    /// one, so it is carried across by path — the same notes, wherever the
    /// new walk ranks them — and the selection is kept inside the rows.
    fn adopt_index(&mut self, entries: Vec<index::Entry>) {
        if let Some((_, hits)) = self.tag_filter.as_mut() {
            let paths: Vec<PathBuf> = hits
                .iter()
                .filter_map(|&i| self.open_index.get(i).map(|e| e.path.clone()))
                .collect();
            *hits = entries
                .iter()
                .enumerate()
                .filter(|(_, e)| paths.contains(&e.path))
                .map(|(i, _)| i)
                .collect();
        }
        self.open_index = entries;
        self.tag_cache = None;
        self.refresh_links();
        if self.overlay == Overlay::QuickOpen && self.tab == QuickTab::Tags {
            self.count_tags();
        }
        if self.overlay == Overlay::QuickOpen {
            let rows = match self.tab {
                QuickTab::Tree => self.browse_rows().len(),
                QuickTab::Contents => self.contents_rows().len(),
                QuickTab::Tags => self.tag_items().len(),
                QuickTab::Bookmarks => self.bookmark_items().len(),
                QuickTab::Unresolved => self.unresolved_items().len(),
                QuickTab::Recent => self.open_items().len(),
            };
            self.selected = self.selected.min(rows.saturating_sub(1));
        }
    }

    /// Whether the tag list on screen is still being gathered.
    pub fn tag_scanning(&self) -> bool {
        self.tag_rx.is_some()
    }

    /// Whether the contents tab is still reading its bodies.
    pub fn contents_indexing(&self) -> bool {
        self.contents_rx.is_some()
    }

    /// Take a body read started by [`App::enter_contents`] if it has finished.
    fn poll_contents_scan(&mut self) -> bool {
        let Some(rx) = self.contents_rx.as_ref() else {
            return false;
        };
        match rx.try_recv() {
            Ok(bodies) => {
                self.contents_rx = None;
                self.set_contents_bodies(bodies);
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.contents_rx = None;
                true
            }
        }
    }

    /// Take the tag count started by [`App::enter_tags`] if it has finished.
    fn poll_tags_scan(&mut self) -> bool {
        let Some(rx) = self.tags_rx.as_ref() else {
            return false;
        };
        match rx.try_recv() {
            Ok(tags) => {
                self.tags_rx = None;
                self.tag_list = tags;
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.tags_rx = None;
                true
            }
        }
    }

    /// Whether the tags tab's list is still being gathered.
    pub fn tags_scanning(&self) -> bool {
        self.tags_rx.is_some()
    }

    /// Adopt a fresh set of bodies; rows cached for the old set are stale.
    fn set_contents_bodies(&mut self, bodies: Vec<Option<crate::contents::Body>>) {
        self.contents_bodies = bodies;
        self.contents_cache.borrow_mut().take();
    }

    /// Take a tag scan started by [`App::open_tag`] if it has finished.
    fn poll_tag_scan(&mut self) -> bool {
        let Some((tag, rx)) = self.tag_rx.as_ref() else {
            return false;
        };
        match rx.try_recv() {
            Ok((entries, hits)) => {
                let tag = tag.clone();
                self.tag_rx = None;
                // the walk is a fresh index whatever the tag turned up
                self.tag_filter = None;
                self.adopt_index(entries);
                // the overlay may have moved on to something else meanwhile
                if self.overlay != Overlay::QuickOpen {
                    return true;
                }
                if hits.is_empty() {
                    self.overlay = Overlay::None;
                    self.flash(format!("no notes tagged #{tag}"));
                } else {
                    self.tag_filter = Some((tag, hits));
                    self.selected = 0;
                }
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.tag_rx = None;
                self.tag_filter = None;
                if self.overlay == Overlay::QuickOpen {
                    self.overlay = Overlay::None;
                }
                true
            }
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
        // an embedded note is found the way a followed link is: by the same
        // index, ranked the same way
        let resolver = index::Resolver::new(self.open_index.clone());
        crate::md::embeds::set_resolver(Box::new(move |target| {
            resolver.resolve(target).map(|e| e.path.clone())
        }));
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
    /// [`Self::maybe_peek`] once the pointer has rested for [`peek::PEEK_DWELL`].
    fn on_hover(&mut self, x: u16, y: u16) {
        if self.view == View::Edit && self.overlay == Overlay::None {
            self.table_hover = self.table_edge_at(x, y);
            return;
        }
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

    /// Select whole rows `r0..=r1` (matrix rows) of the table at `start`.
    fn select_rows(&mut self, start: usize, r0: usize, r1: usize) {
        self.cell_sel = Some(CellSel {
            start,
            anchor: (r0, 0),
            head: (r1, 0),
            kind: SelKind::Rows,
        });
    }

    /// Select whole columns `c0..=c1` of the table at `start`.
    fn select_cols(&mut self, start: usize, c0: usize, c1: usize) {
        self.cell_sel = Some(CellSel {
            start,
            anchor: (0, c0),
            head: (0, c1),
            kind: SelKind::Cols,
        });
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
        let (name, fragment) = md::split_fragment(target);
        // `[[#Heading]]`: a place in the note already on screen
        if name.is_empty() {
            if let Some(f) = fragment {
                self.goto_fragment(f);
            }
            return;
        }
        if let Some(path) = self.resolve_link(target) {
            self.open_path_at(&path, fragment);
            return;
        }
        // a note written since the last walk is the ordinary miss, and one
        // vault walk to be sure is cheap next to telling someone their link is
        // broken when it is not
        self.refresh_index();
        if let Some(path) = self.resolve_link(target) {
            self.open_path_at(&path, fragment);
            return;
        }
        // still nothing: the link is a note that has not been written yet,
        // and following it is how it gets written
        self.create_from_link(target);
    }

    /// The note `target` names, against the index as it stands. A target
    /// spelled as a file — `other.md`, the shape a `[text](other.md)` in the
    /// body arrives in — is a relative path, so it is tried beside the note on
    /// screen first and only then anywhere in the vault; a bare `[[name]]` is
    /// a name, and goes straight to the vault-wide resolver.
    fn resolve_link(&self, target: &str) -> Option<PathBuf> {
        let name = md::split_fragment(target).0;
        if name.to_lowercase().ends_with(".md") {
            if let Some(folder) = self.active_note_folder() {
                let near = format!("{folder}/{name}");
                if let Some(e) = index::resolve(&self.open_index, &near) {
                    return Some(e.path.clone());
                }
            }
        }
        index::resolve(&self.open_index, target).map(|e| e.path.clone())
    }

    /// The folder of the note on screen, relative to the root it sits under:
    /// `stories` for `<notes>/stories/spec.md`, `None` at a root's top level
    /// or outside every root.
    fn active_note_folder(&self) -> Option<String> {
        let path = &self.active_note().path;
        let parent = path.parent()?;
        self.index_roots()
            .iter()
            .find_map(|root| parent.strip_prefix(root).ok())
            .map(|rel| rel.to_string_lossy().replace('\\', "/"))
            .filter(|rel| !rel.is_empty())
    }

    /// Open `path` and, when the link named a place in it, go there.
    fn open_path_at(&mut self, path: &Path, fragment: Option<&str>) {
        self.open_path(path);
        let Some(fragment) = fragment else {
            return;
        };
        // an open that failed flashed and left the old note up, and a heading
        // of another note must not be looked for in this one
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        if canon(&self.active_note().path) != canon(path) {
            return;
        }
        self.goto_fragment(fragment);
    }

    /// Land on the heading or block a link's `#fragment` names, in the note
    /// on screen: the cursor goes to its line and the reading view scrolls it
    /// to the top, the way a contents-tab hit does.
    fn goto_fragment(&mut self, fragment: &str) {
        match crate::links::find_anchor(self.editor.lines(), fragment) {
            Some(line) => {
                self.editor.set_cursor((line, 0));
                self.reveal_cursor();
                self.preview_goto = Some(line);
            }
            None => {
                let what = if fragment.starts_with('^') {
                    "block"
                } else {
                    "heading"
                };
                self.flash(format!("no {what} \u{201c}{fragment}\u{201d} here"));
            }
        }
    }

    /// The folder and filename a link target names, relative to the note the
    /// link was written in. `None` for a target that would write outside the
    /// vault: a link target is note text, and note text must never be able to
    /// name `/etc/passwd` or climb out with `..`.
    fn link_note_path(target: &str) -> Option<(PathBuf, String)> {
        // the `#heading` is a place inside the note, not part of its name
        let t = md::split_fragment(target).0.trim_end_matches(".md");
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
                // the link that made this note stops being red at once —
                // one entry pushed, not a walk of the whole vault
                self.index_add(&path, &name);
                self.flash(format!("created \u{201c}{name}\u{201d}"));
            }
            Err(e) => self.flash(format!("create failed: {e}")),
        }
    }

    /// Put a note this session just made into the index, where a walk would
    /// have put it: at the front, as the most recently opened. The title is
    /// the name it was made with, since that is all the file holds yet.
    fn index_add(&mut self, path: &Path, name: &str) {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if self.open_index.iter().any(|e| e.path == path) {
            return;
        }
        let roots = self.index_roots();
        let home_root = roots.first().cloned().unwrap_or_default();
        let home_root = std::fs::canonicalize(&home_root).unwrap_or(home_root);
        let rel = roots
            .iter()
            .filter_map(|r| std::fs::canonicalize(r).ok())
            .find_map(|r| {
                path.strip_prefix(&r)
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| index::short(&path));
        let entry = index::Entry {
            title: name.to_string(),
            rel,
            folder: index::folder_of(&path, &home_root),
            modified: std::time::SystemTime::now(),
            aliases: Vec::new(),
            name: index::Entry::name_of(&path),
            path,
        };
        self.open_index.insert(0, entry);
        self.refresh_links();
    }

    /// A typed path — `~/vault/spec.md`, `/tmp/x.md` — as an openable file.
    /// The escape hatch for a note in a folder catcher has never been shown:
    /// you can always say where it is.
    fn typed_path(&self) -> Option<PathBuf> {
        let q = self.query.trim();
        if !(q.starts_with('/') || q.starts_with("~/")) {
            return None;
        }
        let home = std::env::home_dir().unwrap_or_else(|| PathBuf::from("."));
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
        self.tag_rx = None;
        // the list already known is shown at once; a fresh walk lands through
        // `poll_index_scan`, and the rows re-rank when it does
        self.start_index_scan();
        self.overlay = Overlay::QuickOpen;
        self.tab = QuickTab::Recent;
        if self.config.quick_open_browse {
            self.enter_browse();
        }
    }

    /// ⇧^F: ^O, opened straight on the contents tab.
    fn open_search_all(&mut self) {
        self.open_quick_open();
        self.tag_filter = None;
        self.enter_contents();
    }

    /// The palette's Tags: ^O, opened straight on the tags tab.
    fn open_tags(&mut self) {
        self.open_quick_open();
        self.tag_filter = None;
        self.enter_tags();
    }

    /// Entering the tags tab counts every tag over the index. Every note is
    /// read, so the count runs on a thread the way the contents read does;
    /// the tab is on screen at once saying it is gathering.
    fn enter_tags(&mut self) {
        self.tab = QuickTab::Tags;
        self.selected = 0;
        self.count_tags();
    }

    /// Count the tags over the index as it stands, on a thread. Run again
    /// when a walk lands while the tab is up, so the list is the vault's.
    fn count_tags(&mut self) {
        let entries = self.open_index.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(index::tag_counts(&entries));
        });
        self.tags_rx = Some(rx);
    }

    /// The tags tab's rows for the current query: every tag that matches,
    /// most used first.
    pub fn tag_items(&self) -> Vec<Item> {
        self.tag_list
            .iter()
            .filter(|(t, _)| self.query.is_empty() || search::fuzzy(&self.query, t).is_some())
            .map(|(t, _)| Item::Tag(t.clone()))
            .collect()
    }

    /// How many notes carry `tag`, as the tags tab counted them.
    pub fn tag_count(&self, tag: &str) -> usize {
        self.tag_list
            .iter()
            .find(|(t, _)| t == tag)
            .map(|(_, n)| *n)
            .unwrap_or(0)
    }

    /// Entering the contents tab reads every body the index reaches, once.
    /// A vault of a few thousand notes is a moment's work, done on a thread
    /// so the tab is on screen at once; after it lands each keystroke is
    /// string search over memory.
    fn enter_contents(&mut self) {
        self.tab = QuickTab::Contents;
        self.selected = 0;
        // a note this session holds is searched as it stands in memory: the
        // file may be an autosave behind, and a hit's line number has to be
        // right for the buffer it opens into. Index paths are already
        // canonical, so only the open notes are canonicalized, once each.
        let open: std::collections::HashMap<PathBuf, &str> = self
            .notes
            .iter()
            .map(|n| {
                let p = std::fs::canonicalize(&n.path).unwrap_or_else(|_| n.path.clone());
                (p, n.content.as_str())
            })
            .collect();
        // each body beside the path and name the `path:` and `file:` terms ask
        let work: Vec<(Result<crate::contents::Body, PathBuf>, String, String)> = self
            .open_index
            .iter()
            .map(|e| {
                let body = match open.get(&e.path) {
                    Some(body) => Ok(crate::contents::body(body)),
                    None => Err(e.path.clone()),
                };
                (body, e.rel.clone(), e.name())
            })
            .collect();
        self.set_contents_bodies(Vec::new());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let bodies = work
                .into_iter()
                .map(|(w, rel, name)| {
                    let body = match w {
                        Ok(body) => Some(body),
                        Err(path) => crate::contents::body_of(&path)
                            .as_deref()
                            .map(crate::contents::body),
                    };
                    body.map(|b| b.at(&rel, &name))
                })
                .collect();
            let _ = tx.send(bodies);
        });
        // an older read still in flight loses its receiver here, and its
        // answer goes nowhere
        self.contents_rx = Some(rx);
    }

    /// The contents rows for the current query: a header per note, its
    /// matching lines under it, and a count of what the cap left out.
    /// Cached by query, so a redraw that changed nothing scans nothing.
    pub fn contents_rows(&self) -> Vec<crate::contents::Row> {
        if let Some((q, rows)) = self.contents_cache.borrow().as_ref() {
            if *q == self.query {
                return rows.clone();
            }
        }
        let (hits, more) = crate::contents::search(
            &self.contents_bodies,
            &self.query,
            crate::contents::MAX_HITS,
        );
        let rows = crate::contents::rows(&hits, more);
        *self.contents_cache.borrow_mut() = Some((self.query.clone(), rows.clone()));
        rows
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

    /// Open the note at `path` with line `line` in view, the way a contents
    /// hit opens — for a row that carries its path rather than an index slot.
    fn open_path_line(&mut self, path: &Path, line: usize) {
        self.open_path(path);
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        if canon(&self.active_note().path) != canon(path) {
            return;
        }
        let line = line.min(self.editor.lines().len().saturating_sub(1));
        self.editor.set_cursor((line, 0));
        self.reveal_cursor();
        self.preview_goto = Some(line);
    }

    /// The palette's Unresolved links: ^O on a tab listing every `[[link]]`
    /// in the vault that reaches no note, gathered on a thread.
    fn open_unresolved(&mut self) {
        self.open_quick_open();
        self.tag_filter = None;
        self.tab = QuickTab::Unresolved;
        self.selected = 0;
        self.unresolved.clear();
        self.unresolved_rx = Some(crate::unresolved::spawn(self.index_roots()));
    }

    /// Is the unresolved scan still running?
    pub fn unresolved_scanning(&self) -> bool {
        self.unresolved_rx.is_some()
    }

    fn poll_unresolved_scan(&mut self) -> bool {
        let Some(rows) = self.unresolved_rx.as_ref().and_then(|p| p.poll()) else {
            return false;
        };
        self.unresolved = rows;
        self.unresolved_rx = None;
        true
    }

    /// The unresolved rows for the current query: every broken link whose
    /// note or target matches, in walk order.
    pub fn unresolved_rows(&self) -> Vec<&crate::unresolved::Broken> {
        self.unresolved
            .iter()
            .filter(|b| {
                self.query.is_empty()
                    || search::fuzzy(&self.query, &b.target).is_some()
                    || search::fuzzy(&self.query, &b.name).is_some()
            })
            .collect()
    }

    pub fn unresolved_items(&self) -> Vec<Item> {
        self.unresolved_rows()
            .into_iter()
            .map(|b| Item::At(b.path.clone(), b.line))
            .collect()
    }

    /// The palette's Bookmarks: ^O on the bookmarks tab.
    fn open_bookmarks(&mut self) {
        self.open_quick_open();
        self.tag_filter = None;
        self.enter_bookmarks();
    }

    fn enter_bookmarks(&mut self) {
        self.tab = QuickTab::Bookmarks;
        self.selected = 0;
        self.bookmarks = crate::bookmarks::load(&self.dir);
    }

    /// The bookmarks tab's rows for the current query, as paths: only the
    /// ones still on disk, so a bookmark to a deleted note is not a row that
    /// fails when chosen.
    pub fn bookmark_items(&self) -> Vec<Item> {
        self.bookmarks
            .iter()
            .filter(|b| self.query.is_empty() || search::fuzzy(&self.query, b).is_some())
            .map(|b| {
                let p = Path::new(b);
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    self.dir.join(p)
                }
            })
            .filter(|p| p.is_file())
            .map(Item::Path)
            .collect()
    }

    /// *Bookmark note*: the open note into the bookmarks, or out of them.
    fn toggle_bookmark(&mut self) {
        let path = self.active_note().path.clone();
        let added = crate::bookmarks::toggle(&self.dir, &path);
        self.flash(
            if added {
                "bookmarked"
            } else {
                "bookmark removed"
            }
            .to_string(),
        );
    }

    /// *Open vault…*: the recent vaults above a line for a typed path.
    fn open_vault_picker(&mut self) {
        self.query.clear();
        self.selected = 0;
        self.overlay = Overlay::OpenVault;
    }

    /// The vault picker's rows: the recent vaults that match what is typed,
    /// then the typed text itself as a folder, when it is one.
    pub fn vault_items(&self) -> Vec<Item> {
        let mut out: Vec<Item> = crate::bookmarks::vaults()
            .into_iter()
            .filter(|p| {
                self.query.is_empty() || search::fuzzy(&self.query, &index::short(p)).is_some()
            })
            .map(Item::Vault)
            .collect();
        let typed = self.query.trim();
        if !typed.is_empty() {
            let path = crate::config::expand_home(typed);
            if path.is_dir()
                && !out
                    .iter()
                    .any(|i| matches!(i, Item::Vault(p) if *p == path))
            {
                out.push(Item::Vault(path));
            }
        }
        out
    }

    /// Re-root the session at `root`: the settings read again with it as the
    /// notes folder, the index rebuilt, the open notes saved and let go, and
    /// the note last opened there — or the newest — on screen.
    fn open_vault(&mut self, root: &Path) {
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        if !root.is_dir() {
            self.flash(format!("not a folder: {}", root.display()));
            return;
        }
        self.sync_editor_to_note();
        self.save_now();
        let config = match Config::load_for(&root) {
            Ok(c) => c,
            Err(e) => {
                self.flash(format!("open vault failed: {e}"));
                return;
            }
        };
        let mut all = match notes::load_all(&root) {
            Ok(all) => all,
            Err(e) => {
                self.flash(format!("open vault failed: {e}"));
                return;
            }
        };
        if all.is_empty() {
            match notes::create(&root) {
                Ok(note) => all.push(note),
                Err(e) => {
                    self.flash(format!("open vault failed: {e}"));
                    return;
                }
            }
        }
        let mut lookup = self.images.lookup().clone();
        lookup.attachments = config.attachments_dir.clone();
        lookup.subfolder = config.attachment_subfolder.clone();
        self.images.set_lookup(lookup);
        config.apply();
        self.editor.tab_width = config.tab_width;
        self.config = config;
        self.config_gen += 1;
        self.dir = root.clone();
        self.notes = all;
        self.active = 0;
        self.dirty = false;
        self.tag_filter = None;
        self.tag_cache = None;
        self.set_contents_bodies(Vec::new());
        self.unresolved.clear();
        self.unresolved_rx = None;
        self.load_active_into_editor();
        // the note last opened in this vault, wherever it sits under it
        let last = self.recents.iter().find(|p| p.starts_with(&root)).cloned();
        match last {
            Some(path) if path.exists() => self.open_path(&path),
            _ => self.remember_active(),
        }
        self.reindex();
        crate::bookmarks::push_vault(&root);
        self.flash(format!("vault: {}", index::short(&root)));
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
        match self.tab {
            QuickTab::Tree => self.enter_contents(),
            QuickTab::Contents => self.enter_tags(),
            QuickTab::Tags => self.enter_bookmarks(),
            QuickTab::Bookmarks | QuickTab::Unresolved => {
                self.tab = QuickTab::Recent;
                self.selected = 0;
            }
            QuickTab::Recent => self.enter_browse(),
        }
    }

    /// Entering browse mode unfolds the folder you are already in and selects
    /// the note you have open, so the first thing the tree tells you is where
    /// you are rather than where the vault starts.
    fn enter_browse(&mut self) {
        self.tab = QuickTab::Tree;
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
            Overlay::QuickOpen if self.tab == QuickTab::Tree => self
                .browse_rows()
                .iter()
                .map(|r| match &r.kind {
                    crate::tree::RowKind::Folder { key, .. } => Item::Folder(key.clone()),
                    crate::tree::RowKind::Note { entry, .. } => Item::Entry(*entry),
                })
                .collect(),
            Overlay::QuickOpen if self.tab == QuickTab::Contents => self
                .contents_rows()
                .into_iter()
                .map(|r| match r {
                    // a header opens the note at its top, like any note row
                    crate::contents::Row::Note(entry) => Item::Entry(entry),
                    crate::contents::Row::Hit(h) => Item::Line(h.entry, h.line),
                    crate::contents::Row::More(_) => Item::Notice,
                })
                .collect(),
            Overlay::QuickOpen if self.tab == QuickTab::Tags => self.tag_items(),
            Overlay::QuickOpen if self.tab == QuickTab::Bookmarks => self.bookmark_items(),
            Overlay::QuickOpen if self.tab == QuickTab::Unresolved => self.unresolved_items(),
            Overlay::QuickOpen => self.open_items(),
            Overlay::MoveFile => self.move_items(),
            Overlay::OpenVault => self.vault_items(),
            Overlay::Outline => self.outline_items(),
            _ => Vec::new(),
        }
    }

    /// The open note's headings, as the outline picker lists them.
    pub fn outline_headings(&self) -> Vec<crate::outline::Heading> {
        crate::outline::headings(self.editor.lines(), &self.blocks())
    }

    /// Outline rows for the current query: the headings that match, in the
    /// order they stand in the note.
    pub fn outline_items(&self) -> Vec<Item> {
        crate::outline::filter(&self.outline_headings(), &self.query)
            .into_iter()
            .map(|h| Item::Heading(h.line))
            .collect()
    }

    /// The palette's Outline: every heading of this note, the one the cursor
    /// is under already selected. In the reading view, where there is no
    /// cursor to speak of, the first line on screen stands in for it.
    fn open_outline(&mut self) {
        let headings = self.outline_headings();
        if headings.is_empty() {
            self.flash("no headings in this note".to_string());
            return;
        }
        self.query.clear();
        let here = match self.view {
            View::Edit => self.editor.cursor.0,
            View::Preview => self
                .preview_rows
                .iter()
                .find_map(|r| r.src_line)
                .unwrap_or(self.editor.cursor.0),
        };
        self.selected = crate::outline::containing(&headings, here).unwrap_or(0);
        self.overlay = Overlay::Outline;
    }

    /// Put the cursor on heading `line` with the heading near the top of the
    /// page: a couple of rows down, so the section reads with its title in
    /// place rather than flush against the edge. The reading view scrolls
    /// there the same way.
    fn goto_heading(&mut self, line: usize) {
        let line = line.min(self.editor.lines().len().saturating_sub(1));
        self.editor.set_cursor((line, 0));
        // a heading under a folded parent is one you asked to see
        self.reveal_cursor();
        let row = self.visible.line_to_row(line).saturating_sub(2);
        self.editor.scroll = self.visible.row_to_line(row);
        self.preview_goto = Some(line);
    }

    /// ⌥⏎ on an outline row: close the section under that heading, or open
    /// it again. The picker stays, so a note can be shaped in one visit.
    fn toggle_outline_fold(&mut self, line: usize) {
        if self.folded_here(line) {
            self.unfold_line(line);
        } else {
            self.fold_line(line);
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
        let format = self.config.daily_format.clone();
        let now = crate::dates::now();
        let made = !crate::daily::path(&dir, &format, now).exists();
        match crate::daily::ensure(&dir, &format, &template, now) {
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
        let mut commands: Vec<Command> = COMMANDS.to_vec();
        // the row and column commands only mean something with the cursor
        // in a table, so that is the only time they are offered
        if self.view == View::Edit && self.table_cell().is_some() {
            commands.extend(TABLE_OPS.iter().map(|op| Command::Table(*op)));
            commands.push(Command::TableSource);
        }
        for c in commands {
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
        for (dir, _) in &self.move_targets {
            if let Some(s) = search::fuzzy(&self.query, &self.move_label(dir)) {
                scored.push((s, Item::MoveTo(dir.clone())));
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
            Item::Heading(line) => self.goto_heading(line),
            Item::Tag(tag) => self.open_tag(&tag),
            Item::At(path, line) => self.open_path_line(&path, line),
            Item::Vault(root) => self.open_vault(&root),
            // handled above, before the overlay was closed
            Item::Folder(_) | Item::Notice => {}
            // palette-only: the one command without a key
            Item::Command(Command::MoveFile) => self.open_move(),
            Item::Command(Command::Unresolved) => self.open_unresolved(),
            Item::Command(Command::Bookmark) => self.toggle_bookmark(),
            Item::Command(Command::Bookmarks) => self.open_bookmarks(),
            Item::Command(Command::OpenVault) => self.open_vault_picker(),
            Item::Command(Command::InsertTable) => self.insert_table(),
            Item::Command(Command::InsertCallout) => {
                self.insert_block(vec!["> [!note] ".to_string(), "> ".to_string()], 0, 10);
            }
            Item::Command(Command::InsertMath) => {
                self.insert_block(
                    vec!["$$".to_string(), String::new(), "$$".to_string()],
                    1,
                    0,
                );
            }
            Item::Command(Command::InsertFootnote) => self.insert_footnote(),
            Item::Command(Command::Table(op)) => self.table_op(op),
            Item::Command(Command::TableSource) => self.toggle_table_source(),
            // the rest are plain actions: one path, whether by key or palette;
            // the overlay is already closed, so the toggling ones just open
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
        self.move_targets = self.move_targets();
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

    /// Open the find prompt over the editor. The selection, if there is one,
    /// is the first thing looked for.
    fn open_find(&mut self) {
        self.enter_edit_view();
        if let Some(sel) = self.editor.selected_text() {
            if !sel.contains('\n') && !sel.is_empty() {
                self.find_input = sel;
            }
        }
        self.find_replacing = false;
        self.overlay = Overlay::Find;
        self.refind(true);
    }

    /// The matches of `find_input`, recomputed; the current one is the first
    /// at or after the cursor when `from_cursor`, else the one nearest the
    /// old index. The current match is selected, which lights it and brings
    /// it onto the page.
    fn refind(&mut self, from_cursor: bool) {
        self.find_matches = crate::find::matches(self.editor.lines(), &self.find_input);
        let at = if from_cursor || self.find_at.is_none() {
            let (row, col) = self.editor.anchor.unwrap_or(self.editor.cursor);
            crate::find::next_from(&self.find_matches, (row, col))
        } else {
            self.find_at
                .map(|i| i.min(self.find_matches.len().saturating_sub(1)))
                .filter(|_| !self.find_matches.is_empty())
        };
        self.find_at = at;
        self.select_find_match();
    }

    fn select_find_match(&mut self) {
        let Some(&(row, s, e)) = self.find_at.and_then(|i| self.find_matches.get(i)) else {
            self.editor.clear_selection();
            return;
        };
        // Drop any earlier anchor first: set_cursor keeps it, and the
        // selecting move below would then run from there to the match.
        self.editor.clear_selection();
        self.editor.set_cursor((row, s));
        self.editor.move_cursor((row, e), true);
    }

    /// Step to the next match, or the previous one, wrapping.
    fn find_step(&mut self, back: bool) {
        let n = self.find_matches.len();
        if n == 0 {
            return;
        }
        let i = self.find_at.unwrap_or(0);
        self.find_at = Some(if back { (i + n - 1) % n } else { (i + 1) % n });
        self.select_find_match();
    }

    /// The char ranges of `row` the find prompt lights up.
    pub fn find_marks_on(&self, row: usize) -> Vec<(usize, usize)> {
        if self.overlay != Overlay::Find {
            return Vec::new();
        }
        self.find_matches
            .iter()
            .filter(|&&(r, _, _)| r == row)
            .map(|&(_, s, e)| (s, e))
            .collect()
    }

    pub fn find_count(&self) -> usize {
        self.find_matches.len()
    }

    /// Swap the current match for the replace field, then move to the next.
    fn replace_current(&mut self) {
        let Some(&(row, s, e)) = self.find_at.and_then(|i| self.find_matches.get(i)) else {
            self.flash("nothing to replace".to_string());
            return;
        };
        let line = crate::find::replace_span(&self.editor.lines()[row], s, e, &self.replace_input);
        let end = s + self.replace_input.chars().count();
        self.editor.replace_lines(row, row, vec![line], (row, end));
        self.sync_editor_to_note();
        self.refind(true);
    }

    /// Swap every match at once, as one undo step.
    fn replace_every(&mut self) {
        let (lines, n) =
            crate::find::replace_all(self.editor.lines(), &self.find_input, &self.replace_input);
        if n == 0 {
            self.flash("nothing to replace".to_string());
            return;
        }
        let last = lines.len().saturating_sub(1);
        let cursor = self.editor.cursor;
        self.editor.replace_lines(0, last, lines, cursor);
        self.sync_editor_to_note();
        self.refind(true);
        self.flash(format!("replaced {n}"));
    }

    fn close_find(&mut self) {
        self.overlay = Overlay::None;
        self.find_matches.clear();
        self.find_at = None;
        // leave the cursor on the match, not a selection of it
        let at = self.editor.cursor;
        self.editor.clear_selection();
        self.editor.set_cursor(at);
    }

    fn on_find_key(&mut self, key: KeyEvent) {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let all = key
            .modifiers
            .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL | KeyModifiers::SUPER);
        match key.code {
            KeyCode::Esc => self.close_find(),
            KeyCode::Tab | KeyCode::BackTab => self.find_replacing = !self.find_replacing,
            KeyCode::Enter if self.find_replacing && all => self.replace_every(),
            KeyCode::Enter if self.find_replacing => self.replace_current(),
            KeyCode::Enter => self.find_step(shift),
            KeyCode::Up => self.find_step(true),
            KeyCode::Down => self.find_step(false),
            _ => {
                if self.find_replacing {
                    edit_line(&mut self.replace_input, &key);
                } else if edit_line(&mut self.find_input, &key) {
                    self.refind(false);
                }
            }
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
        // the page opens where the editor was, not at its top
        self.preview_goto = match self.view {
            View::Preview => Some(self.editor.scroll),
            View::Edit => None,
        };
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
        // a key cuts the opener short and is then what it always was
        self.opener = None;
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
        if self.complete_key(key) {
            return;
        }
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
        // must get there before the keymap, where ⌥⏎ is follow-link; and
        // ⌥⏎ on an outline row folds it, for the same reason
        if matches!(self.overlay, Overlay::QuickOpen | Overlay::Outline)
            && key.code == KeyCode::Enter
        {
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

        self.on_mode_key(key);
    }

    /// A key no binding claimed, given to whatever is on screen: the overlay
    /// if one is open, otherwise the preview or the editor.
    fn on_mode_key(&mut self, key: KeyEvent) {
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
            Overlay::Palette
            | Overlay::QuickOpen
            | Overlay::MoveFile
            | Overlay::Outline
            | Overlay::OpenVault => self.on_palette_key(key),
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
            Overlay::Find => self.on_find_key(key),
            Overlay::None => match self.view {
                View::Preview => self.on_preview_key(key),
                View::Edit => self.on_edit_key(key),
            },
        }
    }

    /// A key in the reading view: scrolling, panning and leaving it.
    fn on_preview_key(&mut self, key: KeyEvent) {
        match key.code {
            // ← and → pan a table too wide for the page; with nothing
            // to pan they do nothing rather than something surprising
            KeyCode::Left => self.pan(-4),
            KeyCode::Right => self.pan(4),
            KeyCode::Home => self.preview_hscroll = 0,
            KeyCode::Up => self.preview_scroll = self.preview_scroll.saturating_sub(1),
            KeyCode::Down => self.preview_scroll = self.preview_scroll.saturating_add(1),
            KeyCode::PageUp => self.preview_scroll = self.preview_scroll.saturating_sub(10),
            KeyCode::PageDown => self.preview_scroll = self.preview_scroll.saturating_add(10),
            // esc drops a selection before it drops the preview, the
            // same order it takes in the editor
            KeyCode::Esc if self.preview_sel.is_some() => self.preview_sel = None,
            KeyCode::Esc if self.preview_hscroll > 0 => self.preview_hscroll = 0,
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('e') => self.view = View::Edit,
            _ => {}
        }
    }

    /// A key in the editor. Up/Down move by display row, which only the view
    /// knows, so they are handled here rather than in the buffer; ⌘↑/⌘↓
    /// (document ends) stay with the buffer.
    fn on_edit_key(&mut self, key: KeyEvent) {
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::SUPER | KeyModifiers::CONTROL | KeyModifiers::ALT);
        let select = key.modifiers.contains(KeyModifiers::SHIFT);
        let before = self.editor.cursor;
        if self.table_key(key) {
            self.settle_table_cursor(before);
            self.leave_folds(before.0);
            return;
        }
        match key.code {
            KeyCode::Up if plain => self.move_vertical(false, select),
            KeyCode::Down if plain => self.move_vertical(true, select),
            _ => {
                if self.editor.on_key(key) {
                    self.sync_editor_to_note();
                }
            }
        }
        self.settle_table_cursor(before);
        self.leave_folds(before.0);
        self.refresh_complete();
    }

    /// A key while the completion popup is up: the arrows move on it, ⏎ and
    /// ⇥ take the row, esc puts it away. Anything else falls through to the
    /// editor, which re-filters after. `true` when the key was taken.
    fn complete_key(&mut self, key: KeyEvent) -> bool {
        let Some(c) = self.complete.as_mut() else {
            return false;
        };
        if self.overlay != Overlay::None || self.view != View::Edit {
            self.complete = None;
            return false;
        }
        match key.code {
            KeyCode::Up => c.selected = c.selected.saturating_sub(1),
            KeyCode::Down => c.selected = (c.selected + 1).min(c.items.len().saturating_sub(1)),
            KeyCode::Enter | KeyCode::Tab => self.accept_complete(),
            KeyCode::Esc => {
                self.complete_dismissed = Some((self.editor.cursor.0, c.token.start));
                self.complete = None;
            }
            _ => return false,
        }
        true
    }

    /// Put the selected row's text into the line in place of the query.
    fn accept_complete(&mut self) {
        let Some(c) = self.complete.take() else {
            return;
        };
        let Some(item) = c.items.get(c.selected) else {
            return;
        };
        if let Some(stamp_row) = item.stamp {
            if let crate::complete::Kind::Anchor { note } = &c.token.kind {
                if !self.stamp_block(note, stamp_row, &item.insert) {
                    return;
                }
            }
        }
        let (row, col) = self.editor.cursor;
        let line = self.editor.lines()[row].clone();
        let (text, cursor) = crate::complete::accept(&line, col, &c.token, &item.insert);
        self.editor.set_line(row, text);
        self.editor.set_cursor((row, cursor));
        self.sync_editor_to_note();
    }

    /// Write ` ^id` onto line `row` of `note` — the open buffer when the
    /// link points at this note, else the file on disk — so the block link
    /// being inserted has something to land on. False when it could not.
    fn stamp_block(&mut self, note: &str, row: usize, insert: &str) -> bool {
        let here = self
            .notes
            .get(self.active)
            .map(|n| std::fs::canonicalize(&n.path).unwrap_or_else(|_| n.path.clone()));
        let target = if note.is_empty() {
            None
        } else {
            index::resolve(&self.open_index, note).map(|e| e.path.clone())
        };
        let in_buffer = match &target {
            None => true,
            Some(p) => here.as_ref() == Some(p),
        };
        if in_buffer {
            let Some(line) = self.editor.lines().get(row).cloned() else {
                return false;
            };
            self.editor
                .set_line(row, crate::complete::stamp_line(&line, insert));
            return true;
        }
        let Some(path) = target else {
            self.flash(format!("no note called {note}"));
            return false;
        };
        let Ok(body) = std::fs::read_to_string(&path) else {
            self.flash("could not read the note to mark the block".to_string());
            return false;
        };
        let mut lines: Vec<String> = body.lines().map(String::from).collect();
        let Some(line) = lines.get(row).cloned() else {
            return false;
        };
        lines[row] = crate::complete::stamp_line(&line, insert);
        let mut out = lines.join("\n");
        if body.ends_with('\n') {
            out.push('\n');
        }
        if notes::write_atomic(&path, &out).is_err() {
            self.flash("could not mark the block".to_string());
            return false;
        }
        let report = crate::links::Report {
            notes: vec![path],
            ..Default::default()
        };
        self.adopt_rewritten(&report);
        true
    }

    /// Work out what the popup should offer for the cursor as it now
    /// stands: nothing when the cursor is not in a token, in code or in the
    /// front matter, or when esc put this very token away.
    fn refresh_complete(&mut self) {
        let (row, col) = self.editor.cursor;
        let token = if self.config.autocomplete && self.editor.anchor.is_none() {
            self.editor
                .lines()
                .get(row)
                .and_then(|l| crate::complete::token_at(l, col))
        } else {
            None
        };
        let Some(token) = token else {
            self.complete = None;
            self.complete_dismissed = None;
            return;
        };
        if self.complete_dismissed == Some((row, token.start)) {
            self.complete = None;
            return;
        }
        self.complete_dismissed = None;
        let blocks = self.blocks();
        if let Some(b) = md::block_at(&blocks, row) {
            if matches!(
                b.kind,
                md::BlockKind::Fence
                    | md::BlockKind::Mermaid
                    | md::BlockKind::Math
                    | md::BlockKind::FrontMatter
                    | md::BlockKind::Comment
                    | md::BlockKind::IndentedCode
            ) {
                self.complete = None;
                return;
            }
        }
        let wikilinks = self.config.wikilinks;
        let items = match &token.kind {
            crate::complete::Kind::Link if wikilinks => {
                crate::complete::link_candidates(&token.query, &self.open_index)
            }
            crate::complete::Kind::Anchor { note } if wikilinks => {
                let lines: Vec<String> = if note.is_empty() {
                    self.editor.lines().to_vec()
                } else {
                    index::resolve(&self.open_index, note)
                        .and_then(|e| std::fs::read_to_string(&e.path).ok())
                        .map(|c| c.lines().map(String::from).collect())
                        .unwrap_or_default()
                };
                crate::complete::anchor_candidates(&token.query, &lines)
            }
            crate::complete::Kind::Tag if self.config.tags => {
                let tags = self.tag_cache.get_or_insert_with(|| {
                    index::tag_counts(&self.open_index)
                        .into_iter()
                        .map(|(t, _)| t)
                        .collect()
                });
                crate::complete::tag_candidates(&token.query, tags)
            }
            _ => Vec::new(),
        };
        if items.is_empty() {
            self.complete = None;
            return;
        }
        // the same token, re-filtered: keep the row if it is still there
        let selected = self
            .complete
            .as_ref()
            .filter(|c| c.token.kind == token.kind && c.token.start == token.start)
            .and_then(|c| {
                let was = &c.items.get(c.selected)?.insert;
                items.iter().position(|i| &i.insert == was)
            })
            .unwrap_or(0);
        self.complete = Some(Completion {
            token,
            items,
            selected,
        });
    }

    /// The bottom edges drawn under `row`: one for every callout card whose
    /// last line on screen it is — the block's own card, a card nested in
    /// it, or one folded down to its title.
    pub fn callout_close_rows(
        &self,
        blocks: &[md::Block],
        row: usize,
        width: usize,
    ) -> Vec<md::RLine> {
        let Some(block) = md::block_at(blocks, row).filter(|b| b.kind == md::BlockKind::Callout)
        else {
            return Vec::new();
        };
        md::callout_closes(self.editor.lines(), block, row, width, &|l| {
            self.visible.is_hidden(l)
        })
    }

    /// The rows hung under `row` when it is a `![[note]]` embed drawn as a
    /// card: the first lines of the embedded note. None while the cursor is
    /// on it, which is when the line shows its syntax instead.
    pub fn embed_rows(&self, blocks: &[md::Block], row: usize, width: usize) -> Vec<md::RLine> {
        let Some(block) = md::block_at(blocks, row) else {
            return Vec::new();
        };
        if block.kind != md::BlockKind::Embed || self.revealed(block) {
            return Vec::new();
        }
        let src = self
            .editor
            .lines()
            .get(row)
            .map(String::as_str)
            .unwrap_or("");
        md::embed_rows(src, width, &self.config.keys.label(Action::FollowLink))
    }

    /// Put `with` at the cursor as a paragraph of its own: an empty line
    /// takes it, a line with text gets it after, and a blank line is added
    /// beneath when the next line has text. The cursor lands on line
    /// `line` of the block at column `col`.
    fn insert_block(&mut self, mut with: Vec<String>, line: usize, col: usize) {
        self.enter_edit_view();
        let (row, _) = self.editor.cursor;
        let lines = self.editor.lines();
        let here_blank = lines[row].trim().is_empty();
        let at = if here_blank { row } else { row + 1 };
        if !here_blank {
            with.insert(0, String::new());
        }
        let below_blank = lines.get(at).is_none_or(|l| l.trim().is_empty());
        if !below_blank {
            with.push(String::new());
        }
        let first = at + usize::from(!here_blank) + line;
        if here_blank {
            self.editor.replace_lines(row, row, with, (first, col));
        } else {
            self.editor.insert_lines(at, with, (first, col));
        }
        self.sync_editor_to_note();
    }

    /// `[^n]` at the cursor, `n` one past the highest numbered footnote in
    /// the note, and its definition at the end of the note with the cursor
    /// on it, ready for the text.
    fn insert_footnote(&mut self) {
        self.enter_edit_view();
        let n = self
            .editor
            .lines()
            .iter()
            .filter_map(|l| {
                let rest = l.trim_start().strip_prefix("[^")?;
                let close = rest.find("]:")?;
                rest[..close].parse::<u64>().ok()
            })
            .max()
            .unwrap_or(0)
            + 1;
        self.editor.insert_str(&format!("[^{n}]"));
        let end = self.editor.lines().len();
        let mut with = vec![format!("[^{n}]: ")];
        // a blank line before it unless the note already ends with one
        if self
            .editor
            .lines()
            .last()
            .is_some_and(|l| !l.trim().is_empty())
        {
            with.insert(0, String::new());
        }
        let last = end + with.len() - 1;
        let col = with.last().map(|l| l.chars().count()).unwrap_or(0);
        self.editor.insert_lines(end, with, (last, col));
        self.sync_editor_to_note();
        self.flash(format!("footnote {n} — type its text"));
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
            Action::Outline => {
                if self.overlay == Overlay::Outline {
                    self.overlay = Overlay::None;
                } else {
                    self.open_outline();
                }
            }
            Action::Tags => {
                if self.overlay == Overlay::QuickOpen && self.tab == QuickTab::Tags {
                    self.overlay = Overlay::None;
                } else {
                    self.open_tags();
                }
            }
            Action::ToggleProperties => {
                self.overlay = Overlay::None;
                self.toggle_properties(false);
            }
            Action::HideProperties => {
                self.overlay = Overlay::None;
                self.hide_properties();
            }
            Action::ToggleOpener => {
                self.overlay = Overlay::None;
                self.config.opener = !self.config.opener;
                let word = if self.config.opener { "yes" } else { "no" };
                self.save_setting("opener", word);
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
                } else if !self.copy_cells() {
                    self.copy_selection();
                }
            }
            Action::Cut => {
                if self.view != View::Edit || !self.cut_cells() {
                    self.cut_selection();
                }
            }
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
            Action::Find => self.open_find(),
            Action::FollowLink => self.follow_link_at_cursor(),
            Action::NavBack => self.nav_history(true),
            Action::NavForward => self.nav_history(false),
            Action::Peek => self.peek_at_cursor(),
            Action::SearchAll => {
                if self.overlay == Overlay::QuickOpen && self.tab == QuickTab::Contents {
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
        // in a grid a row moves within its part of the table: never onto the
        // separator, out of the block, or across the header line
        if let Some((block, table, r, _)) = self.table_cell() {
            let (from, to) = self.editor.selected_rows();
            let other = if down { to + 1 } else { from.wrapping_sub(1) };
            let ok = block.contains(other)
                && table
                    .row_of(other - block.start)
                    .is_some_and(|o| (o < table.head) == (r < table.head));
            if !ok {
                self.flash("nowhere to move".to_string());
                return;
            }
        }
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
        let tree = self.tab == QuickTab::Tree && self.overlay == Overlay::QuickOpen;
        match key.code {
            KeyCode::Esc => self.overlay = Overlay::None,
            KeyCode::Right if tree => self.browse_right(),
            KeyCode::Left if tree => self.browse_left(),
            KeyCode::Up => self.select_step(-1),
            KeyCode::Down => self.select_step(1),
            KeyCode::Enter => {
                if let Some(item) = self.overlay_items().get(self.selected).cloned() {
                    self.activate_item(item, key.modifiers);
                }
            }
            _ => {}
        }
    }

    /// Move the overlay's selection by `delta` rows, clamped to the list.
    fn select_step(&mut self, delta: isize) {
        if delta < 0 {
            self.selected = self.selected.saturating_sub(1);
        } else {
            // the row count is only ever wanted here, and asking for it
            // builds every row of the overlay — in browse mode the whole
            // tree — so a typed character must not pay for one
            let count = self.overlay_items().len();
            if count > 0 && self.selected + 1 < count {
                self.selected += 1;
            }
        }
    }

    /// ⏎ or a click on an overlay row: with a beside modifier a ^O row opens
    /// the note in a new split or tab, and an outline heading folds and
    /// leaves the picker up; otherwise the row runs.
    fn activate_item(&mut self, item: Item, modifiers: KeyModifiers) {
        match beside_place(modifiers) {
            Some(place) if self.overlay == Overlay::QuickOpen => self.run_item_beside(item, place),
            Some(_) if self.overlay == Overlay::Outline => {
                if let Item::Heading(line) = item {
                    self.toggle_outline_fold(line);
                }
            }
            _ => self.run_item(item),
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

    /// The source line whose drawn checkbox sits at screen cell (x, y), if
    /// that is what is there. A line showing its raw `[ ]` has no box to hit.
    fn checkbox_at(&self, x: u16, y: u16) -> Option<usize> {
        let hit = self
            .edit_rows
            .iter()
            .find(|r| r.rect.height == 1 && y == r.rect.y && r.seg == 0)?;
        let row = hit.line;
        let (start, _) = md::task_prefix(self.editor.lines().get(row)?)?;
        let blocks = self.blocks();
        let width = self.editor_area.width.max(1) as usize;
        let segs = self.wrapped(row, &blocks, width);
        let dcol = x.checked_sub(self.editor_area.x)? as usize;
        let cell = segs.first()?.cell_at_display(dcol)?;
        let glyph = cell.ch.to_string();
        (cell.src == start && crate::theme::TASK_GLYPHS.contains(&glyph.as_str())).then_some(row)
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
        let grip_row = match self.table_hover {
            Some((_, TableEdge::Left(r))) => Some(r),
            _ => None,
        };
        // a callout draws its own fold: the marker inside the card's top
        // edge, and the count with it
        if let Some(block) = md::block_at(blocks, row).filter(|b| b.kind == md::BlockKind::Callout)
        {
            let hidden = self
                .folded_here(row)
                .then(|| self.visible.hidden_under(row))
                .filter(|&n| n > 0);
            return md::callout_line_folded(
                self.editor.lines(),
                block,
                row,
                width,
                row == self.editor.cursor.0,
                hidden,
            );
        }
        let line = view_line(
            self.editor.lines(),
            blocks,
            row,
            width,
            self.editor.cursor,
            self.editor.selection(),
            self.table_source,
            grip_row,
            self.cell_sel,
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
            // the properties box's own edge, and the line it folds to
            if url == crate::render::PROPERTIES_HREF {
                self.toggle_properties(true);
                return;
            }
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
            .find(|r| r.rect.contains(at))
            .and_then(|r| r.src_line)
            .filter(|&l| crate::fold::foldable_at(self.editor.lines(), &self.blocks(), l));
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
            .find(|r| r.rect.contains(ratatui::layout::Position { x, y }))
            .cloned();
        self.view = View::Edit;
        if let Some(row) = hit {
            let dcol = x.saturating_sub(row.rect.x) as usize;
            // a row the renderer invented is nowhere in the buffer: better to
            // leave the cursor where it was than to answer a click on the
            // footer with the top of the note
            if let Some(pos) =
                cell_source(&row.cells, dcol).or_else(|| row.src_line.map(|r| (r, 0)))
            {
                self.editor.clear_selection();
                self.editor.set_cursor(pos);
            }
        }
    }

    /// Where a screen point lands in the rendered page. Points above or below
    /// the drawn rows clamp to the first or last one, so dragging off the top
    /// or bottom of the window extends the selection rather than stalling.
    fn preview_point(&self, x: u16, y: u16) -> Option<PSel> {
        let first = self.preview_rows.first()?;
        let last = self.preview_rows.last()?;
        let row = if y < first.rect.y {
            first
        } else if y > last.rect.y {
            last
        } else {
            self.preview_rows
                .iter()
                .find(|r| y >= r.rect.y && y < r.rect.y + r.rect.height)
                .unwrap_or(last)
        };
        Some((
            row.page_row,
            x.saturating_sub(row.rect.x) as usize + row.pan,
        ))
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
        Some(selected_text(&self.preview_rows, self.preview_span()?))
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
            md::LinkTarget::File(f) => self.open_attachment(&f),
        }
    }

    /// Following `[[report.pdf]]`: the file is looked for the way a picture
    /// is — beside the note, in its attachments folder, in the configured one
    /// — and handed to the desktop. Never made when it is not there: a PDF
    /// is not a note waiting to be written.
    fn open_attachment(&mut self, name: &str) {
        match self.images.resolve(name, &self.note_dir()) {
            Some(path) => self.open_url(&path.to_string_lossy()),
            None => self.flash(format!("no such file: {name}")),
        }
    }

    /// Tell the embed cards where attachments are found from the note on
    /// screen: the same places a picture is looked for. Done when the note
    /// changes, never per frame.
    fn refresh_file_resolver(&self) {
        let note_dir = self.note_dir();
        let lookup = self.images.lookup().clone();
        crate::md::embeds::set_file_resolver(Box::new(move |name| {
            crate::images::resolve_in(name, &note_dir, &lookup)
        }));
    }

    /// Following a `#tag`: ^O, cut to the notes that carry it. Saved first,
    /// because the tag scan reads files and the note on screen may have just
    /// gained the tag being followed.
    ///
    /// The scan reads every note, so it runs on a thread the way the mentions
    /// scan does: the overlay opens at once saying it is looking, and
    /// [`App::poll_tag_scan`] fills it in — or shuts it, if nothing carries
    /// the tag.
    fn open_tag(&mut self, tag: &str) {
        self.save_now();
        let roots = self.index_roots();
        let recents = self.recents.clone();
        let want = tag.to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let entries = index::scan(&roots, &recents);
            let hits = index::with_tag(&entries, &want);
            let _ = tx.send((entries, hits));
        });
        // an ordinary walk in flight would answer with an index the hits do
        // not belong to; the tag walk is a fresh one anyway
        self.index_rx = None;
        self.tag_rx = Some((tag.to_string(), rx));
        self.query.clear();
        self.selected = 0;
        self.tag_filter = Some((tag.to_string(), Vec::new()));
        self.tab = QuickTab::Recent;
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
                if self.paste_cells(&text) {
                    return;
                }
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
                let mut lookup = self.images.lookup().clone();
                lookup.attachments = config.attachments_dir.clone();
                lookup.subfolder = config.attachment_subfolder.clone();
                self.images.set_lookup(lookup);
                config.apply();
                self.editor.tab_width = config.tab_width;
                self.config = config;
                self.refresh_file_resolver();
                self.config_gen += 1;
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
        if !matches!(ev.kind, MouseEventKind::Moved) {
            self.complete = None;
            self.opener = None;
        }
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
            MouseEventKind::ScrollUp => self.on_wheel(-2),
            MouseEventKind::ScrollDown => self.on_wheel(2),
            MouseEventKind::ScrollLeft if self.view == View::Preview => self.pan(-4),
            MouseEventKind::ScrollRight if self.view == View::Preview => self.pan(4),
            MouseEventKind::Moved => self.on_hover(ev.column, ev.row),
            MouseEventKind::Down(MouseButton::Left) => {
                self.on_click(ev.column, ev.row, ev.modifiers)
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
                if self.table_drag.is_some() {
                    self.drag_cells(ev.column, ev.row);
                } else {
                    let pos = self.pos_at(ev.column, ev.row);
                    self.editor.set_cursor(pos);
                }
            }
            MouseEventKind::Up(MouseButton::Left) if self.dragging => {
                self.dragging = false;
                self.table_drag = None;
                if self.cell_sel.is_some() {
                    // a block of cells stays selected, and is not copied
                    // until asked for
                    self.editor.clear_selection();
                } else if self.editor.selection().is_some() {
                    self.copy_selection();
                } else {
                    self.editor.clear_selection();
                }
            }
            _ => {}
        }
    }

    /// The wheel by `delta` rows, over the list overlay if one is open,
    /// otherwise the reading view or the editor.
    fn on_wheel(&mut self, delta: isize) {
        match (self.overlay, self.view) {
            (Overlay::Palette | Overlay::QuickOpen | Overlay::MoveFile | Overlay::Outline, _) => {
                self.select_step(delta)
            }
            (_, View::Preview) => {
                let n = delta.unsigned_abs() as u16;
                self.preview_scroll = if delta < 0 {
                    self.preview_scroll.saturating_sub(n)
                } else {
                    self.preview_scroll.saturating_add(n)
                };
            }
            (_, View::Edit) => self.scroll_edit(delta),
        }
    }

    /// A left click at (x, y): on the peek, an overlay, the reading view or
    /// the editor, in that order.
    fn on_click(&mut self, x: u16, y: u16, modifiers: KeyModifiers) {
        // a click on the popup opens the note it shows
        if self.peek.as_ref().is_some_and(|p| p.contains(x, y)) {
            self.open_peek();
            return;
        }
        self.peek = None;
        self.hover = None;
        if matches!(
            self.overlay,
            Overlay::Palette | Overlay::QuickOpen | Overlay::MoveFile | Overlay::Outline
        ) {
            if let Some((_, item)) = self
                .palette_rows
                .iter()
                .find(|(r, _)| r.contains(ratatui::layout::Position { x, y }))
                .cloned()
            {
                self.activate_item(item, modifiers);
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
            // a click on a drawn checkbox flips it, the way it does in the
            // reading view; the cursor stays where it was
            if let Some(row) = self.checkbox_at(x, y) {
                self.toggle_checkbox(row);
                return;
            }
            let at = ratatui::layout::Position { x, y };
            if let Some((_, handle)) = self.table_handles.iter().find(|(r, _)| r.contains(at)) {
                self.table_handle(*handle);
                return;
            }
            // under everything drawn, with a table ending the note: the
            // click means "below the table", so make that line
            let below = self
                .edit_rows
                .last()
                .is_some_and(|r| y >= r.rect.y + r.rect.height)
                && self
                    .edit_rows
                    .iter()
                    .any(|r| r.line + 1 == self.editor.lines().len());
            if below && self.step_below_table() {
                return;
            }
            let pos = self.pos_at(x, y);
            // modifier-click follows a link instead of moving the cursor
            if follows_link(modifiers) {
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
            self.cell_sel = None;
            let before = self.editor.cursor;
            self.editor.set_cursor(pos);
            self.settle_table_cursor(before);
            self.editor.anchor = Some(self.editor.cursor);
            self.dragging = true;
            // a press in a grid cell may become a drag across cells
            self.table_drag = self
                .table_cell()
                .map(|(b, _, r, c)| (b.start, (r, c), self.editor.cursor));
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
#[allow(clippy::too_many_arguments)]
fn view_line(
    lines: &[String],
    blocks: &[md::Block],
    row: usize,
    width: usize,
    cursor: Pos,
    selection: Option<(Pos, Pos)>,
    table_source: Option<usize>,
    grip_row: Option<usize>,
    cell_sel: Option<CellSel>,
) -> md::RLine {
    let cursor_row = cursor.0;
    let src = lines.get(row).map(String::as_str).unwrap_or("");
    if let Some(block) = md::block_at(blocks, row) {
        // a table stays a grid with the cursor in it — the cursor moves cell
        // to cell — unless its source was asked for
        // a callout keeps its card with the cursor in it; the cursor's own
        // line shows its text as typed
        if block.kind == md::BlockKind::Callout {
            return md::callout_line(lines, block, row, width, row == cursor_row);
        }
        if block.kind == md::BlockKind::Table && table_source != Some(block.start) {
            let inner = width.saturating_sub(TABLE_GUTTER);
            let mut line = if block.contains(cursor_row) {
                md::table_line_editing(lines, block, row, inner, cursor_row - block.start)
            } else {
                md::style_block_line(lines, block, row, inner)
            };
            if let Some(sel) = cell_sel.filter(|s| s.start == block.start) {
                if let Some(table) = crate::table::Table::parse(&lines[block.start..=block.end]) {
                    let rect = sel.rect(table.rows.len(), table.cols());
                    if let Some(r) = table.row_of(row - block.start) {
                        if r >= rect.r0 && r <= rect.r1 {
                            md::tint_table_cells(
                                &mut line,
                                src,
                                &|c| c >= rect.c0 && c <= rect.c1,
                                crate::theme::row(),
                            );
                        }
                    }
                }
            }
            return with_gutter(line, grip_row == Some(row));
        }
        return if revealed_by(block, cursor_row, selection) {
            md::RLine::raw(src)
        } else {
            md::style_block_line(lines, block, row, width)
        };
    }
    if row == cursor_row {
        md::raw_with_task(src, cursor.1)
    } else {
        md::style_line_in(lines, row)
    }
}

/// Does a fold key fold here, or fall through to the editor as the word
/// motion it also is? A heading takes it; a heading with shift held does not,
/// because ⇧⌥← is extending a selection and that is a motion anywhere.
fn fold_key_takes(key: &KeyEvent, on_heading: bool) -> bool {
    on_heading && !key.modifiers.contains(KeyModifiers::SHIFT)
}

/// A table row with its gutter in front: blank, or the grip when the
/// pointer is beside this row. The gutter stands for source column 0, so a
/// click on it lands at the row's start and settles into the first cell.
fn with_gutter(mut line: md::RLine, grip: bool) -> md::RLine {
    let text = if grip { "⠿ " } else { "  " };
    let cells = text.chars().map(|ch| md::Cell {
        ch,
        style: crate::theme::state(),
        src: 0,
    });
    line.cells.splice(0..0, cells);
    line
}

/// A folded heading's line with the `▸ ` marker in front. The marker stands
/// for source column 0 — the first `#` — so a click on it lands at the start
/// of the line and the cursor's own column mapping is untouched.
pub fn fold_marked(mut line: md::RLine) -> md::RLine {
    let marker = crate::theme::FOLDED.chars().map(|ch| md::Cell {
        ch,
        style: crate::theme::fold(),
        src: 0,
    });
    line.cells.splice(0..0, marker);
    line
}

/// Flip the `[ ]`/`[x]` box of a task line, keeping everything else intact.
/// A box in one of the other states (`[/]`, `[-]`, `[>]`, `[?]`) is done
/// with, so it becomes `[x]`; a numbered `1. [ ]` flips like a bullet.
pub fn toggle_task(line: &str) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    // only the leading list marker counts, so text that merely looks like a
    // second checkbox later on the line is left alone
    let mut start = 0;
    while matches!(chars.get(start), Some(' ') | Some('\t')) {
        start += 1;
    }
    if matches!(chars.get(start), Some(c) if c.is_ascii_digit()) {
        while matches!(chars.get(start), Some(c) if c.is_ascii_digit()) {
            start += 1;
        }
        if !matches!(chars.get(start), Some('.') | Some(')')) {
            return None;
        }
    } else if !matches!(chars.get(start), Some('-') | Some('*') | Some('+')) {
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
        c if md::task_state(c).is_some() => 'x',
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

/// The text a preview selection covers, from the rows the last draw recorded.
///
/// Free-standing so the rule for a row that has no cells can be tested without
/// a terminal behind it: such a row contributes its newline and nothing else,
/// rather than abandoning the whole copy. Blank lines between paragraphs are
/// already like that, and the linked-mentions footer makes them easy to drag
/// across.
fn selected_text(rows: &[PreviewRow], ((sr, sc), (er, ec)): (PSel, PSel)) -> String {
    let mut out = String::new();
    for r in rows {
        let row = r.page_row;
        if row < sr || row > er {
            continue;
        }
        let from = if row == sr { sc } else { r.pan };
        let to = if row == er { ec } else { usize::MAX };
        out.push_str(&slice_cells(&r.cells, r.pan, from, to));
        if row < er {
            out.push('\n');
        }
    }
    out
}

/// Does this click's modifier set mean "follow the link" rather than "put the
/// cursor here"? SGR mouse reporting only carries shift/alt/ctrl, so Cmd-click
/// is not something a terminal can report — ctrl or alt is the working path.
/// SUPER is accepted anyway, for a terminal that ever does report it.
fn follows_link(m: KeyModifiers) -> bool {
    m.intersects(KeyModifiers::SUPER | KeyModifiers::CONTROL | KeyModifiers::ALT)
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
    fn palette_order_and_labels_are_unchanged() {
        // the palette lists COMMANDS in this order, under these names; a
        // reshuffle or rename here is a change users notice
        let labels: Vec<&str> = super::COMMANDS.iter().map(|c| c.label().0).collect();
        assert_eq!(
            labels,
            [
                "New note",
                "Today's note",
                "Open note",
                "Search in all files",
                "Outline",
                "Tags",
                "Toggle properties (hide / show)",
                "Hide properties",
                "Toggle opener",
                "Delete note",
                "Rename file",
                "Move to folder",
                "Unresolved links",
                "Bookmark note",
                "Bookmarks",
                "Open vault…",
                "Find in note",
                "Reading view",
                "Help",
                "Settings",
                "Fold section",
                "Unfold section",
                "Fold all",
                "Unfold all",
                "Quit",
                "Toggle checkbox",
                "Move line up",
                "Move line down",
                "Toggle heading",
                "Insert today's date",
                "Copy path",
                "Reveal in Finder",
                "Open in split right",
                "Open in split down",
                "Open in new tab",
                "Table: Insert table",
                "Insert callout",
                "Insert math block",
                "Insert footnote",
            ]
        );
        // every Act row names an action with a label; the palette-only ones
        // have none and are dispatched by run_item directly
        for c in super::COMMANDS {
            assert!(!c.label().0.is_empty());
            assert_eq!(c.action().is_none(), !matches!(c, super::Command::Act(_)));
        }
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
    fn numbered_tasks_and_the_other_states_toggle_too() {
        assert_eq!(toggle_task("1. [ ] a").as_deref(), Some("1. [x] a"));
        assert_eq!(toggle_task("  12) [x] a").as_deref(), Some("  12) [ ] a"));
        assert_eq!(toggle_task("1. a"), None);
        assert_eq!(toggle_task("1.[ ] a"), None);
        // in progress, cancelled, forwarded, question: a click finishes them
        assert_eq!(toggle_task("- [/] a").as_deref(), Some("- [x] a"));
        assert_eq!(toggle_task("- [-] a").as_deref(), Some("- [x] a"));
        assert_eq!(toggle_task("- [>] a").as_deref(), Some("- [x] a"));
        assert_eq!(toggle_task("3. [?] a").as_deref(), Some("3. [x] a"));
        // an unknown state is text, not a box
        assert_eq!(toggle_task("- [z] a"), None);
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
    fn a_block_reveals_its_source_only_while_the_cursor_is_inside_it() {
        let lines: Vec<String> = "text\n```\nlet x = 1;\n```\nafter"
            .lines()
            .map(String::from)
            .collect();
        let blocks = md::blocks(&lines);
        let view = |row, cursor| {
            view_line(
                &lines,
                &blocks,
                row,
                20,
                (cursor, 0),
                None,
                None,
                None,
                None,
            )
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
            view_line(&table, &bs, row, 20, (cursor, 0), None, None, None, None)
                .cells
                .iter()
                .map(|c| c.ch)
                .collect::<String>()
        };
        // a grid carries its two-column gutter, where the row grip goes
        assert_eq!(raw(0, 9), "  a │ bbbb"); // cursor elsewhere: laid out
                                             // a table is the exception: the grid stays with the cursor in it,
                                             // and its source shows only when asked for
        assert_eq!(raw(0, 2), "  a │ bbbb");
        assert_eq!(raw(1, 2), "  ──┼─────");
        let source = |row, cursor| {
            view_line(&table, &bs, row, 20, (cursor, 0), None, Some(0), None, None)
                .cells
                .iter()
                .map(|c| c.ch)
                .collect::<String>()
        };
        assert_eq!(source(0, 2), "| a | bbbb |");
        assert_eq!(source(1, 2), "| --- | --- |");
        // the cursor's own row keeps its markup as typed so every column is
        // somewhere on screen; the others style theirs
        let marked: Vec<String> = "| a | **b** |
| --- | --- |
| 1 | 2 |"
            .lines()
            .map(String::from)
            .collect();
        let mbs = md::blocks(&marked);
        let text = |row, cursor| {
            view_line(&marked, &mbs, row, 20, (cursor, 0), None, None, None, None)
                .cells
                .iter()
                .map(|c| c.ch)
                .collect::<String>()
        };
        assert_eq!(text(0, 0), "  a │ **b**");
        assert_eq!(text(0, 2), "  a │ b");
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

        // show leaves it to the markdown scanner, which reads the file the
        // way CommonMark would: a rule, then `tags: work` over its closing
        // `---` as a setext heading, then the rule further down
        let shown = blocks_with(&lines, FrontMatter::Show);
        assert_eq!(
            shown.iter().map(|b| (b.kind, b.start)).collect::<Vec<_>>(),
            vec![
                (md::BlockKind::Rule, 0),
                (md::BlockKind::Setext, 1),
                (md::BlockKind::Rule, 6),
            ]
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
            view_line(
                &lines,
                &blocks,
                row,
                20,
                (cursor, 0),
                None,
                None,
                None,
                None,
            )
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
        assert_eq!(line.cells[0].style, crate::theme::fold());
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
            Command::Act(Action::FoldSection),
            Command::Act(Action::UnfoldSection),
            Command::Act(Action::FoldAll),
            Command::Act(Action::UnfoldAll),
        ] {
            assert!(COMMANDS.contains(&c));
            assert!(c.action().is_some());
            assert!(!c.label().0.is_empty());
        }
    }

    #[test]
    fn toggle_properties_is_a_palette_command_with_a_rebindable_key() {
        assert!(COMMANDS.contains(&Command::Act(Action::ToggleProperties)));
        assert_eq!(
            Command::Act(Action::ToggleProperties).action(),
            Some(Action::ToggleProperties)
        );
        assert_eq!(
            Command::Act(Action::ToggleProperties).label().0,
            "Toggle properties (hide / show)"
        );
        for q in ["toggle", "hide", "show", "prop"] {
            assert!(
                crate::search::fuzzy(q, Command::Act(Action::ToggleProperties).label().0).is_some(),
                "{q}"
            );
        }
        assert!(COMMANDS.contains(&Command::Act(Action::HideProperties)));
        assert_eq!(
            Command::Act(Action::HideProperties).action(),
            Some(Action::HideProperties)
        );
        assert_eq!(
            Command::Act(Action::HideProperties).label().0,
            "Hide properties"
        );
        let map = crate::keys::Keymap::default();
        assert!(map
            .settings_rows()
            .iter()
            .any(|(k, v, _)| *k == "key_hide_properties" && v == "none"));
        assert_eq!(map.label(Action::ToggleProperties), "");
        assert!(map
            .settings_rows()
            .iter()
            .any(|(k, v, _)| *k == "key_properties" && v == "none"));
    }

    #[test]
    fn the_outline_is_a_palette_command_with_a_rebindable_key() {
        assert!(COMMANDS.contains(&Command::Act(Action::Outline)));
        assert_eq!(
            Command::Act(Action::Outline).action(),
            Some(Action::Outline)
        );
        assert_eq!(Command::Act(Action::Outline).label().0, "Outline");
        // unbound out of the box, and settable as key_outline
        let map = crate::keys::Keymap::default();
        assert_eq!(map.label(Action::Outline), "");
        assert!(map
            .settings_rows()
            .iter()
            .any(|(k, v, _)| *k == "key_outline" && v == "none"));
    }

    #[test]
    fn the_tags_list_is_a_palette_command_with_a_rebindable_key() {
        assert!(COMMANDS.contains(&Command::Act(Action::Tags)));
        assert_eq!(Command::Act(Action::Tags).action(), Some(Action::Tags));
        assert_eq!(Command::Act(Action::Tags).label().0, "Tags");
        let map = crate::keys::Keymap::default();
        assert_eq!(map.label(Action::Tags), "");
        assert!(map
            .settings_rows()
            .iter()
            .any(|(k, v, _)| *k == "key_tags" && v == "none"));
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
    fn a_preview_selection_across_a_row_with_no_source_line_still_copies() {
        use crate::render::PCell;
        let cells = |text: &str, src: Option<usize>| -> Vec<PCell> {
            text.chars()
                .enumerate()
                .map(|(i, ch)| PCell {
                    ch,
                    style: crate::theme::PLAIN,
                    link: None,
                    src: src.map(|l| (l, i)),
                })
                .collect()
        };
        let row = |page_row: usize, src_line: Option<usize>, cells: Vec<PCell>| PreviewRow {
            page_row,
            rect: Rect::new(0, page_row as u16, 20, 1),
            pan: 0,
            src_line,
            cells,
        };
        let span = ((0, 0), (2, 4));

        // the middle row is a blank line or a footer row: drawn, selectable,
        // and nowhere in the buffer
        let rows = vec![
            row(0, Some(3), cells("alpha", Some(3))),
            row(1, None, Vec::new()),
            row(2, Some(5), cells("beta", Some(5))),
        ];
        assert_eq!(selected_text(&rows, span), "alpha\n\nbeta");
    }
}

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
    /// A wikilink that resolves to nothing: enter makes the note it names.
    ConfirmCreate,
    RenameFile,
    Help,
}

#[derive(Clone, PartialEq)]
pub enum Command {
    NewNote,
    QuickOpen,
    DeleteNote,
    RenameFile,
    TogglePreview,
    Shortcuts,
    OpenSettings,
    Quit,
}

const COMMANDS: [Command; 8] = [
    Command::NewNote,
    Command::QuickOpen,
    Command::DeleteNote,
    Command::RenameFile,
    Command::TogglePreview,
    Command::Shortcuts,
    Command::OpenSettings,
    Command::Quit,
];

impl Command {
    /// The action this command runs, when it is one a key can be bound to —
    /// which is how the palette knows what key to show beside it.
    pub fn action(&self) -> Option<Action> {
        Some(match self {
            Command::NewNote => Action::NewNote,
            Command::QuickOpen => Action::QuickOpen,
            Command::DeleteNote => Action::DeleteNote,
            Command::RenameFile => Action::RenameFile,
            Command::TogglePreview => Action::TogglePreview,
            Command::Shortcuts => Action::Help,
            Command::OpenSettings => Action::Settings,
            Command::Quit => Action::Quit,
        })
    }

    pub fn label(&self) -> (&'static str, &'static str) {
        match self {
            Command::NewNote => ("New note", "an empty note, ready to type"),
            Command::QuickOpen => ("Open note", "any folder, recent first"),
            Command::DeleteNote => ("Delete note", "delete the file on disk"),
            Command::RenameFile => ("Rename file", "change the name on disk"),
            Command::TogglePreview => ("Reading view", "the page, rendered"),
            Command::Shortcuts => ("Help", "every key, on one card"),
            Command::OpenSettings => ("Settings", "edit them here, as a note"),
            Command::Quit => ("Quit", "save and exit"),
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
            ("⌥← ⌥→", "by word"),
            ("⌘← ⌘→", "start / end of line"),
            ("⌘↑ ⌘↓", "start / end of note"),
            ("⇧ + any motion", "extend the selection"),
            ("click, drag", "place the cursor, select (drag copies)"),
            (
                "⌥click  ^click",
                "open the link or [[wikilink]] under the pointer",
            ),
            ("wheel", "scroll without moving the cursor"),
        ],
    ),
    (
        "palette",
        &[
            ("type", "fuzzy-search titles and bodies"),
            ("↑ ↓", "move  ·  ⏎ open or run  ·  esc close"),
            ("tab", "in ^O, swap the ranked list for the folder tree"),
            ("← →", "in the tree, fold and unfold a folder"),
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
    Note(usize),
    /// A file from the quick-open index, which may live in another folder and
    /// may not be loaded into this session at all yet.
    Entry(usize),
    /// A path typed into quick-open that turned out to exist.
    Path(PathBuf),
    /// A folder row in the ^O tree, by the key `tree` gave it. Choosing one
    /// folds or unfolds it — a folder is not somewhere to go, so the overlay
    /// stays exactly where it was.
    Folder(String),
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
    /// How far the page has panned sideways, in columns. Only lines a table
    /// marked `wide` move; prose stays where it is, so the note never slides
    /// out from under you.
    pub preview_hscroll: u16,
    /// The furthest right the page can pan, worked out by the last draw from
    /// the widest table on it. Zero when nothing overflows.
    pub preview_hmax: u16,
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
    /// The wikilink target the create prompt is asking about.
    pub pending_link: Option<String>,
    /// What has been typed into the shortcuts card, which filters its rows.
    pub help_query: String,
    /// True when the session is rooted outside the configured notes dir (a
    /// `tinynote <file>` / `<dir>` invocation). Renaming and image paste are
    /// decided per note now — quick-open can reach anywhere — but the flag
    /// still says what kind of session this is.
    #[allow(dead_code)]
    pub foreign_root: bool,
}

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

        let recents = index::load_recent();
        // a plain `tinynote` picks up where you left off: the note you had
        // open when you closed it, wherever it lives. Only a launch that names
        // nothing gets this — `tinynote <file>` asked for something else.
        let restore = matches!(launch, Launch::Default)
            .then(|| recents.first().cloned())
            .flatten();

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
            pending_link: None,
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
            open_index: Vec::new(),
            index_rx: None,
            browse: false,
            tree_open: BTreeSet::new(),
            overlay_rect: Rect::default(),
            mentions: crate::mentions::Backlinks::default(),
            recents,
            preview_sel: None,
            preview_dragging: false,
            preview_page_rows: Vec::new(),
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
        self.preview_sel = None;
    }

    fn sync_editor_to_note(&mut self) {
        let content = self.editor.text();
        if content != self.notes[self.active].content {
            self.notes[self.active].content = content;
            self.dirty = true;
            self.last_edit = Instant::now();
        }
    }

    /// Is `path` a file tinynote may rename to follow its title? Only inside
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
            Ok(_) => {
                self.dirty = false;
                // a save is the only way a body under the roots changes from
                // inside tinynote, and it is what makes a mention you have
                // just typed turn up in the footer of the note it names
                self.mentions.invalidate();
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

    pub fn maybe_autosave(&mut self) {
        let after = Duration::from_millis(self.config.autosave_ms);
        if self.dirty && self.last_edit.elapsed() >= after {
            self.save_now();
        }
    }

    pub fn flash(&mut self, msg: String) {
        self.status = Some((msg, Instant::now()));
    }

    pub fn tick(&mut self) {
        self.maybe_autosave();
        self.poll_index_scan();
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
        self.remember_active();
    }

    /// Put the note on screen at the front of the recents list, which is what
    /// quick-open ranks by.
    fn remember_active(&mut self) {
        // the settings note has its own key and its own palette row; putting
        // it at the top of "recently opened" would only push notes down
        if self.editing_settings() {
            return;
        }
        let path = self.notes[self.active].path.clone();
        index::push_recent(&mut self.recents, &path);
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
                self.view = View::Edit;
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
        self.pending_link = Some(target.to_string());
        self.overlay = Overlay::ConfirmCreate;
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

    /// Make the note an unresolved wikilink named, and open it. Confirmed
    /// first through [`Overlay::ConfirmCreate`] — a mistyped link should not
    /// quietly leave a file behind.
    fn create_from_link(&mut self) {
        let Some(target) = self.pending_link.take() else {
            return;
        };
        let Some((folder, name)) = Self::link_note_path(&target) else {
            self.flash(format!("“{target}” is not a name a note can have"));
            return;
        };
        let dir = self.note_dir().join(folder);
        match notes::create_named(&dir, &name, format!("# {name}\n\n")) {
            Ok(note) => {
                let path = note.path.clone();
                self.open_path(&path);
                // the link that made this note stops being red at once
                self.refresh_index();
                self.flash(format!("created \u{201c}{name}\u{201d}"));
            }
            Err(e) => self.flash(format!("create failed: {e}")),
        }
    }

    /// A typed path — `~/vault/spec.md`, `/tmp/x.md` — as an openable file.
    /// The escape hatch for a note in a folder tinynote has never been shown:
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
        // before `enter_browse`, which reads the index it builds
        self.refresh_index();
        self.overlay = Overlay::QuickOpen;
        self.browse = self.config.quick_open_browse;
        if self.browse {
            self.enter_browse();
        }
    }

    /// Quick-open rows for the current query. With no query this is simply the
    /// index order — most recently opened first, then most recently modified —
    /// which is the whole point of having a second list beside the palette.
    pub fn open_items(&self) -> Vec<Item> {
        if self.query.is_empty() {
            return (0..self.open_index.len()).map(Item::Entry).collect();
        }
        // a path that exists is not a guess, so it leads the list outright
        if let Some(path) = self.typed_path() {
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
        for (i, e) in self.open_index.iter().enumerate() {
            // the filename is what the list shows, so it is what people
            // search by; the title is a second chance, and the folder path a
            // weaker third, so "applications/log" finds it too
            let by_name = search::fuzzy(&self.query, &e.name()).map(|s| s * 10 + 100);
            let by_title = search::fuzzy(&self.query, &e.title).map(|s| s * 10 + 50);
            let by_path = search::fuzzy(&self.query, &e.rel);
            let Some(base) = by_name.into_iter().chain(by_title).chain(by_path).max() else {
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

    /// tab: the same overlay, the other way of looking at it. The query
    /// survives the swap both ways — typing `log` and then wanting to see
    /// *where* the log notes live is the whole reason to have this.
    fn toggle_browse(&mut self) {
        self.browse = !self.browse;
        if self.browse {
            self.enter_browse();
        } else {
            self.selected = 0;
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
            Overlay::QuickOpen => self.open_items(),
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
        // the file is gone, and an index that still lists it is worse than no
        // index at all: `follow_wikilink` would resolve a `[[link]]` against
        // the entry, open nothing, and never reach the offer to create it
        self.reindex();
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
        // a folder is not somewhere to go: folding it leaves the overlay open
        // and the tree exactly where it was, which is also what makes clicking
        // a folder row work, since a click routes through here too
        if let Item::Folder(key) = item {
            self.toggle_folder(&key);
            return;
        }
        self.overlay = Overlay::None;
        match item {
            Item::Note(i) => self.switch_to(i),
            Item::Entry(i) => {
                if let Some(path) = self.open_index.get(i).map(|e| e.path.clone()) {
                    self.open_path(&path);
                }
            }
            Item::Path(path) => self.open_path(&path),
            // handled above, before the overlay was closed
            Item::Folder(_) => {}
            Item::Command(Command::NewNote) => self.new_note(),
            Item::Command(Command::QuickOpen) => self.open_quick_open(),
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
                // the filename is one of the names a `[[wikilink]]` reaches a
                // note by, so a rename changes what resolves and what does not
                self.reindex();
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
        self.preview_hscroll = 0;
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
        // whatever the settings say this key does, if anything
        if let Some(action) = self.config.keys.action(&key) {
            self.run_action(action);
            return;
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
            Overlay::Palette | Overlay::QuickOpen => self.on_palette_key(key),
            Overlay::ConfirmDelete => match key.code {
                KeyCode::Enter => {
                    self.overlay = Overlay::None;
                    self.delete_active();
                }
                KeyCode::Esc => self.overlay = Overlay::None,
                _ => {}
            },
            Overlay::ConfirmCreate => match key.code {
                KeyCode::Enter => {
                    self.overlay = Overlay::None;
                    self.create_from_link();
                }
                KeyCode::Esc => {
                    self.overlay = Overlay::None;
                    self.pending_link = None;
                }
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
        }
    }

    fn on_palette_key(&mut self, key: KeyEvent) {
        // tab flips ^O between the ranked list and the tree, both ways. The
        // command palette has no second view of itself, so it never sees this.
        if key.code == KeyCode::Tab && self.overlay == Overlay::QuickOpen {
            self.toggle_browse();
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
                    self.run_item(item);
                }
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
        hidden_by(
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
    /// What the create-a-note prompt should say: the filename the note will
    /// be given, and the folder it will land in, `~/`-shortened. Worked out
    /// from the same two calls `create_from_link` makes, so the prompt cannot
    /// describe a different file from the one that then appears —
    /// `[[stories/story-matrix]]` makes `story-matrix.md` inside `stories/`,
    /// and saying "in ~/notes" would be a surprise on both counts.
    ///
    /// `None` for a target that names no note it is allowed to write.
    pub fn pending_create(&self) -> Option<(String, String)> {
        let (folder, name) = Self::link_note_path(self.pending_link.as_deref()?)?;
        Some((name, index::short(&self.note_dir().join(folder))))
    }

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
        if self.config.preview_click == PreviewClick::Edit {
            self.edit_at_preview(x, y);
            return;
        }
        self.preview_sel = self.preview_point(x, y).map(|p| (p, p));
        self.preview_dragging = true;
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
        }
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
                // the roots to walk, or the setting itself, may have moved
                self.mentions.invalidate();
                if moved {
                    self.flash("notes_dir changed — restart tinynote".to_string());
                } else {
                    self.flash("settings applied".to_string());
                }
            }
            Err(e) => self.flash(format!("settings reload failed: {e}")),
        }
    }

    pub fn on_mouse(&mut self, ev: MouseEvent) {
        match ev.kind {
            MouseEventKind::ScrollUp => match (self.overlay, self.view) {
                (Overlay::Palette | Overlay::QuickOpen, _) => {
                    self.selected = self.selected.saturating_sub(1)
                }
                (_, View::Preview) => self.preview_scroll = self.preview_scroll.saturating_sub(2),
                (_, View::Edit) => self.editor.scroll_by(-2),
            },
            MouseEventKind::ScrollDown => match (self.overlay, self.view) {
                (Overlay::Palette | Overlay::QuickOpen, _) => {
                    let count = self.overlay_items().len();
                    if count > 0 && self.selected + 1 < count {
                        self.selected += 1;
                    }
                }
                (_, View::Preview) => self.preview_scroll = self.preview_scroll.saturating_add(2),
                (_, View::Edit) => self.editor.scroll_by(2),
            },
            MouseEventKind::ScrollLeft if self.view == View::Preview => self.pan(-4),
            MouseEventKind::ScrollRight if self.view == View::Preview => self.pan(4),
            MouseEventKind::Down(MouseButton::Left) => {
                let (x, y) = (ev.column, ev.row);
                if matches!(self.overlay, Overlay::Palette | Overlay::QuickOpen) {
                    if let Some((_, item)) = self
                        .palette_rows
                        .iter()
                        .find(|(r, _)| r.contains(ratatui::layout::Position { x, y }))
                        .cloned()
                    {
                        self.run_item(item);
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
                } else if matches!(
                    self.overlay,
                    Overlay::ConfirmDelete | Overlay::ConfirmCreate | Overlay::RenameFile
                ) {
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
        let only: Vec<String> = "---\ntags: work\n---"
            .lines()
            .map(String::from)
            .collect();
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

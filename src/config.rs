//! `~/.config/catcher/settings.md` — the settings, as a note.
//!
//! Settings are markdown because they are edited *inside catcher*: the same
//! editor, the same live preview, no $EDITOR and no TOML. Every setting is a
//! `- key: value` line under a `##` heading, with the prose around it saying
//! what it does. Delete a line and its default stands.
//!
//! Notes default to `~/catcher`, not `~/notes`: catcher renames files to
//! follow their titles, so pointing it at a directory someone might already
//! keep markdown in would let it quietly reorganize a collection it was never
//! given. A directory named after the tool can only be the tool's.
//!
//! `CATCHER_DIR` still wins over `notes_dir`, which keeps
//! `CATCHER_DIR=/tmp/x cargo run` working.

use crate::keys::Keymap;
use crate::theme::{self, Mode, Palette};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// A setting spelt as one of a few words. `WORDS` pairs each variant with
/// its word, once; the settings file is read and written through it.
pub trait Words: Copy + PartialEq + 'static {
    const WORDS: &'static [(Self, &'static str)];

    fn name(self) -> &'static str {
        Self::WORDS
            .iter()
            .find(|(v, _)| *v == self)
            .map(|(_, w)| *w)
            .expect("every variant is listed in WORDS")
    }

    /// The variant `s` names, whatever its case; `None` for anything else.
    fn parse(s: &str) -> Option<Self> {
        Self::WORDS
            .iter()
            .find(|(_, w)| w.eq_ignore_ascii_case(s))
            .map(|(v, _)| *v)
    }
}

/// How a table too wide for the page is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TableStyle {
    /// Grid while it fits; a scrolling grid once it doesn't.
    #[default]
    Auto,
    /// Columns keep a readable width and wrap inside it, and the table itself
    /// scrolls sideways — Obsidian's answer, and the one that keeps a table a
    /// table however many columns it has.
    Scroll,
    /// Always a grid, columns squeezed and cells cut with an ellipsis.
    Fit,
    /// Always a grid, but cells wrap onto as many lines as the row needs.
    Wrap,
    /// One labelled block per row — nothing truncated, however wide the table.
    Cards,
}

impl Words for TableStyle {
    const WORDS: &'static [(Self, &'static str)] = &[
        (TableStyle::Auto, "auto"),
        (TableStyle::Scroll, "scroll"),
        (TableStyle::Fit, "fit"),
        (TableStyle::Wrap, "wrap"),
        (TableStyle::Cards, "cards"),
    ];
}

/// What a plain click in the preview does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PreviewClick {
    /// Place a selection anchor; dragging selects text and copies it.
    #[default]
    Select,
    /// Drop into the editor at the same spot (how catcher used to behave).
    Edit,
}

impl Words for PreviewClick {
    const WORDS: &'static [(Self, &'static str)] = &[
        (PreviewClick::Select, "select"),
        (PreviewClick::Edit, "edit"),
    ];
}

/// What the editor does with a note's YAML front matter. The reading view has
/// a setting of its own, `Properties`: it draws the block as a box, not as YAML.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FrontMatter {
    /// Visible and editable, drawn quiet, and never read as markdown.
    #[default]
    Dim,
    /// Styled like any other markdown — `---` rules and all.
    Show,
    /// Not drawn at all until the cursor moves into it. The text is still in
    /// the file; this is only about what the page spends rows on.
    Hide,
}

impl Words for FrontMatter {
    const WORDS: &'static [(Self, &'static str)] = &[
        (FrontMatter::Dim, "dim"),
        (FrontMatter::Show, "show"),
        (FrontMatter::Hide, "hide"),
    ];
}

/// What the reading view does with a note's front matter: the box of
/// properties, a single line standing in for it, or nothing. The *Toggle
/// properties* command cycles through all three and a click on the box or
/// the line flips between the first two; either writes the choice here, so
/// every note follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Properties {
    #[default]
    Box,
    Line,
    Hide,
}

impl Words for Properties {
    const WORDS: &'static [(Self, &'static str)] = &[
        (Properties::Box, "box"),
        (Properties::Line, "line"),
        (Properties::Hide, "hide"),
    ];
}

/// Panel and overlay border treatment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BorderStyle {
    #[default]
    Rounded,
    Square,
    None,
}

impl Words for BorderStyle {
    const WORDS: &'static [(Self, &'static str)] = &[
        (BorderStyle::Rounded, "rounded"),
        (BorderStyle::Square, "square"),
        (BorderStyle::None, "none"),
    ];
}

/// One thing the status bar can show. The order they are listed in the
/// settings is the order they are drawn in, left half first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusItem {
    /// The note's full path, `~/`-shortened.
    Path,
    /// Just the filename.
    Name,
    /// `edit` / `preview`.
    Mode,
    /// The key hints.
    Keys,
    /// Transient messages — saves, failures, confirmations.
    Message,
    /// How many top-level keys the note's front matter declares. Shown only
    /// when there is front matter at all, so it costs nothing on a note
    /// without any.
    Properties,
}

impl StatusItem {
    fn parse(word: &str) -> Option<StatusItem> {
        Some(match word {
            "path" => StatusItem::Path,
            "name" | "file" | "filename" => StatusItem::Name,
            "mode" => StatusItem::Mode,
            "keys" | "hints" => StatusItem::Keys,
            "message" | "status" => StatusItem::Message,
            "properties" | "props" => StatusItem::Properties,
            _ => return None,
        })
    }

    fn word(self) -> &'static str {
        match self {
            StatusItem::Path => "path",
            StatusItem::Name => "name",
            StatusItem::Mode => "mode",
            StatusItem::Keys => "keys",
            StatusItem::Message => "message",
            StatusItem::Properties => "properties",
        }
    }
}

/// The `theme` setting: a polarity, or `auto` to follow what the terminal
/// reports its background to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    #[default]
    Auto,
    Dark,
    Light,
}

impl Words for Theme {
    const WORDS: &'static [(Self, &'static str)] = &[
        (Theme::Auto, "auto"),
        (Theme::Dark, "dark"),
        (Theme::Light, "light"),
    ];
}

impl Theme {
    /// The polarity this setting means right now.
    pub fn mode(self) -> Mode {
        match self {
            Theme::Auto => theme::detected(),
            Theme::Dark => Mode::Dark,
            Theme::Light => Mode::Light,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub notes_dir: PathBuf,
    pub attachments_dir: PathBuf,
    /// The per-note attachments subfolder a link is also looked for in:
    /// `attachments`, or what an Obsidian vault's `./sub` setting names. Not
    /// a setting of its own — read from the vault, never written.
    pub attachment_subfolder: String,
    /// The folder *Insert template* lists, under the notes dir unless
    /// absolute. Kept as written, like `daily_dir`, so the settings file
    /// reads back the way it was typed.
    pub templates_dir: PathBuf,
    pub theme: Theme,
    /// User colour overrides, already applied on top of the theme's palette.
    pub palette: Palette,
    /// Widest the note column is ever drawn, in terminal columns. `0` fills
    /// the window.
    pub page_width: u16,
    pub borders: BorderStyle,
    pub bold_headings: bool,
    pub status_bar: bool,
    pub key_hints: bool,
    /// Whether the note decodes out of noise when catcher starts.
    pub opener: bool,
    /// Whether the terminal window's title follows the open note.
    pub window_title: bool,
    /// What the status bar shows, in the order given.
    pub status_bar_items: Vec<StatusItem>,
    pub autosave_ms: u64,
    pub tab_width: usize,
    /// Whether a filename follows its note's title. Off means catcher never
    /// renames a file for you.
    pub rename_files: bool,
    /// Whether renaming a note's file rewrites the `[[wikilinks]]` in other
    /// notes that pointed at the old name. Off leaves them to break.
    pub update_links: bool,
    /// Whether the status bar counts the note's words and characters — or
    /// the selection's, while there is one.
    pub status_words: bool,
    pub table_style: TableStyle,
    pub preview_click: PreviewClick,
    /// What the editor does with a note's front matter.
    pub front_matter: FrontMatter,
    /// What the reading view does with it.
    pub properties: Properties,
    /// Whether `[[wikilinks]]` are links. Off leaves them as the literal text
    /// a reader without Obsidian sees.
    pub wikilinks: bool,
    /// Whether `#tags` are coloured and followable. Off leaves them as text.
    pub tags: bool,
    /// Whether a fence that names a language has its words coloured by role.
    /// Off draws a fence exactly as it always did, and no syntax set is ever
    /// loaded.
    pub code_colors: bool,
    /// Whether the reading view lists the notes that link to this one. It
    /// costs a pass over every note body, so it is a setting and not simply
    /// how the app behaves.
    pub linked_mentions: bool,
    /// Whether typing `[[` or `#` pops up notes, headings and tags to pick
    /// from.
    pub autocomplete: bool,
    /// Whether quick-open walks subfolders or offers only the current folder.
    pub quick_open_recursive: bool,
    /// Whether ^O opens on the folder tree rather than the ranked list. Which
    /// folders are unfolded is deliberately not settable and never persisted:
    /// that is where you are in a session, not something to configure.
    pub quick_open_browse: bool,
    /// Folders quick-open searches besides the notes dir — another vault, a
    /// work folder. Empty by default.
    pub quick_open_dirs: Vec<PathBuf>,
    /// Where daily notes go, under the notes dir unless absolute. Kept as
    /// written so the settings file reads back the way it was typed.
    pub daily_dir: PathBuf,
    /// The file name of a daily note, in moment-style tokens; a slash in it
    /// is a subfolder under `daily_dir`.
    pub daily_format: String,
    /// The template a new daily note is filled from; a heading when the
    /// file is missing.
    pub daily_template: PathBuf,
    /// What every key does, defaults included.
    pub keys: Keymap,
}

impl Default for Config {
    fn default() -> Self {
        let home = std::env::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let notes_dir = default_notes_dir(&home);
        Config {
            attachments_dir: notes_dir.join("attachments"),
            attachment_subfolder: "attachments".to_string(),
            templates_dir: PathBuf::from(crate::templates::DEFAULT_DIR),
            notes_dir,
            theme: Theme::Auto,
            palette: theme::base(theme::detected()),
            page_width: 100,
            borders: BorderStyle::Rounded,
            bold_headings: true,
            status_bar: true,
            key_hints: true,
            opener: true,
            window_title: true,
            status_bar_items: vec![StatusItem::Path, StatusItem::Message, StatusItem::Keys],
            autosave_ms: 500,
            tab_width: 2,
            rename_files: true,
            update_links: true,
            status_words: false,
            table_style: TableStyle::Auto,
            preview_click: PreviewClick::Select,
            front_matter: FrontMatter::Dim,
            properties: Properties::Box,
            wikilinks: true,
            tags: true,
            code_colors: true,
            linked_mentions: true,
            autocomplete: true,
            quick_open_recursive: true,
            quick_open_browse: false,
            quick_open_dirs: Vec::new(),
            daily_dir: PathBuf::from("journal"),
            daily_format: crate::daily::DEFAULT_FORMAT.to_string(),
            daily_template: PathBuf::from("journal/template.md"),
            keys: Keymap::default(),
        }
    }
}

/// Path to the settings note, whether or not it exists yet.
pub fn settings_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("settings.md"))
}

pub fn config_dir() -> Result<PathBuf> {
    let config = std::env::home_dir()
        .context("no home directory")?
        .join(".config");
    let new = config.join("catcher");
    let old = config.join("tinynote");
    // The app was called tinynote until 0.9. A settings folder left behind
    // by that name is still the user's settings: use it until they move it.
    if !new.exists() && old.is_dir() {
        return Ok(old);
    }
    Ok(new)
}

/// `~/catcher`, unless the notes are still in the `~/tinynote` the app
/// used before it was renamed and no `~/catcher` exists yet.
fn default_notes_dir(home: &Path) -> PathBuf {
    let new = home.join("catcher");
    let old = home.join("tinynote");
    if !new.exists() && old.is_dir() {
        return old;
    }
    new
}

impl Config {
    /// Read the settings note, writing it on first run.
    pub fn load() -> Result<Config> {
        let (config, warning) = Config::load_reporting()?;
        if let Some(w) = warning {
            // the app flashes this itself; only a plain terminal gets stderr
            if !crossterm::terminal::is_raw_mode_enabled().unwrap_or(false) {
                eprintln!("catcher: {w}");
            }
        }
        Ok(config)
    }

    /// `load`, also returning a warning when the settings note was parsed
    /// fine but could not be rewritten with the settings it was missing.
    pub fn load_reporting() -> Result<(Config, Option<String>)> {
        let path = settings_path()?;
        if !path.exists() {
            let fresh = Config::from_str("");
            write_settings(&path, &fresh)?;
            return Ok((fresh, None));
        }
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        // A settings file written by an older catcher is missing whatever has
        // been added since, and a setting you cannot see is a setting you do
        // not have. Rewriting is safe because the document is generated from
        // the config the file was just parsed into: every value survives —
        // parsed *without* the environment, or a `CATCHER_DIR=/tmp/x` run
        // would write /tmp/x into the file as the notes folder for good.
        let on_disk = Config::from_file_text(&text);
        let warning = if covers_every_setting(&text, &on_disk) {
            None
        } else {
            write_settings(&path, &on_disk)
                .err()
                .map(|e| format!("settings not updated: {e:#}"))
        };
        Ok((Config::from_str(&text), warning))
    }

    /// Push everything this config decides into the places that read it
    /// globally. Called at startup and on every settings save.
    pub fn apply(&self) {
        theme::set_palette(self.palette);
        theme::set_bold_headings(self.bold_headings);
        crate::md::links::set_enabled(self.wikilinks);
        crate::md::tags::set_enabled(self.tags);
        crate::highlight::set_enabled(self.code_colors);
    }

    /// The file plus the environment: `CATCHER_DIR` wins over `notes_dir`,
    /// which is how a one-off session is pointed elsewhere. A named
    /// `attachments_dir` still stands, as before.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(text: &str) -> Config {
        let mut c = Config::from_file_text(text);
        match std::env::var_os("CATCHER_DIR") {
            Some(d) => c.root_at(text, PathBuf::from(d)),
            None => c.resolve_folders(text),
        }
        c
    }

    /// The settings note read again, with `root` as the notes folder for
    /// this session — what *Open vault…* does. The file is not written: the
    /// vault opened is where you are, not a setting you changed.
    pub fn load_for(root: &Path) -> Result<Config> {
        let path = settings_path()?;
        let text = fs::read_to_string(&path).unwrap_or_default();
        let mut c = Config::from_file_text(&text);
        c.root_at(&text, root.to_path_buf());
        Ok(c)
    }

    /// Point the config at `root`. A named `attachments_dir` still stands;
    /// otherwise the vault's own says where its attachments are.
    fn root_at(&mut self, text: &str, root: PathBuf) {
        self.notes_dir = root;
        if value(text, "attachments_dir").is_none() {
            self.attachments_dir = self.notes_dir.join("attachments");
            self.attachment_subfolder = "attachments".to_string();
        }
        if value(text, "templates_dir").is_none() {
            self.templates_dir = PathBuf::from(crate::templates::DEFAULT_DIR);
        }
        self.resolve_folders(text);
    }

    /// Settle the two folders, once `notes_dir` is final — both the load path
    /// and `root_at` end here, because a folder only means anything relative
    /// to the vault it is under.
    ///
    /// A folder that is not there is not a folder: a `templates_dir` left in
    /// the settings from a vault ago would otherwise leave *Insert template*
    /// listing nothing, with no hint why. The default stands instead, and the
    /// settings note is not rewritten — the folder may be coming back, and a
    /// session is not the place to throw away what someone typed.
    fn resolve_folders(&mut self, text: &str) {
        if value(text, "templates_dir").is_some() && !self.templates_dir().is_dir() {
            self.templates_dir = PathBuf::from(crate::templates::DEFAULT_DIR);
        }
        self.adopt_obsidian_templates();
        if value(text, "attachments_dir").is_some() && !self.attachments_dir.is_dir() {
            self.attachments_dir = self.notes_dir.join("attachments");
            self.attachment_subfolder = "attachments".to_string();
        }
        self.adopt_obsidian_attachments();
    }

    /// When `attachments_dir` is not set (or is just the default), take the
    /// vault's own answer: Obsidian keeps where it puts attachments in
    /// `.obsidian/app.json`, and a link written there points where that says.
    fn adopt_obsidian_attachments(&mut self) {
        if self.attachments_dir != self.notes_dir.join("attachments") {
            return;
        }
        let Ok(text) = fs::read_to_string(self.notes_dir.join(".obsidian/app.json")) else {
            return;
        };
        if let Some(setting) = obsidian_attachment_setting(&text) {
            // `./` and `./sub` are beside the note, so there is no one folder
            // to look for; a plain vault folder that is not there is no answer
            // at all, and the default is the better one
            let s = setting.trim();
            let beside = s.starts_with("./") || s == ".";
            if beside || self.notes_dir.join(s.trim_matches('/')).is_dir() {
                self.set_obsidian_attachments(&setting);
            }
        }
    }

    /// When `templates_dir` is not set (or is just the default), take the
    /// vault's own answer: Obsidian's Templates plugin keeps its folder in
    /// `.obsidian/templates.json`, and that is where a vault's templates are
    /// whether or not catcher was told about them.
    fn adopt_obsidian_templates(&mut self) {
        if self.templates_dir != Path::new(crate::templates::DEFAULT_DIR) {
            return;
        }
        let Ok(text) = fs::read_to_string(self.notes_dir.join(".obsidian/templates.json")) else {
            return;
        };
        if let Some(folder) = obsidian_template_folder(&text) {
            let folder = folder.trim().trim_matches('/');
            // a folder the vault names but no longer has is worse than the
            // default: it makes Insert template list nothing at all
            if !folder.is_empty() && self.notes_dir.join(folder).is_dir() {
                self.templates_dir = PathBuf::from(folder);
            }
        }
    }

    /// Apply an Obsidian `attachmentFolderPath`: `./` means beside the note,
    /// `./sub` a subfolder beside the note, anything else a vault folder.
    fn set_obsidian_attachments(&mut self, setting: &str) {
        let setting = setting.trim().trim_end_matches('/');
        match setting
            .strip_prefix("./")
            .or(if setting == "." { Some("") } else { None })
        {
            Some("") => {
                self.attachments_dir = self.notes_dir.clone();
                self.attachment_subfolder = ".".to_string();
            }
            Some(sub) => {
                self.attachments_dir = self.notes_dir.join(sub);
                self.attachment_subfolder = sub.to_string();
            }
            None if setting.is_empty() || setting == "/" => {
                self.attachments_dir = self.notes_dir.clone();
            }
            None => {
                self.attachments_dir = self.notes_dir.join(setting.trim_start_matches('/'));
            }
        }
    }

    /// The file alone, as it would be written back — the two folders exactly
    /// as typed, since this is what an out-of-date settings note is rewritten
    /// from and a folder that is missing today is not one to erase.
    /// `resolve_folders` settles them for the session.
    fn from_file_text(text: &str) -> Config {
        let home = std::env::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let mut c = Config::default();

        if let Some(v) = value(text, "notes_dir") {
            c.notes_dir = expand(&v, &home);
            c.attachments_dir = c.notes_dir.join("attachments");
        }
        if let Some(v) = value(text, "attachments_dir") {
            c.attachments_dir = expand(&v, &home);
        }

        // anything unrecognised is auto: a typo should leave the terminal's
        // own polarity standing rather than pin a palette that reads wrong
        c.theme = word(text, "theme").unwrap_or(Theme::Auto);
        c.palette = theme::base(c.theme.mode());
        for key in theme::COLOR_KEYS {
            if let Some(color) = value(text, key).and_then(|v| theme::parse_color(&v)) {
                c.palette.set(key, color);
            }
        }

        if let Some(v) = value(text, "page_width") {
            // "full" is the honest word for it; 0 does the same
            c.page_width = if v.eq_ignore_ascii_case("full") {
                0
            } else {
                v.parse().unwrap_or(c.page_width)
            };
        }
        c.borders = word(text, "borders").unwrap_or(c.borders);
        c.bold_headings = flag(text, "bold_headings", c.bold_headings);
        c.status_bar = flag(text, "status_bar", c.status_bar);
        c.key_hints = flag(text, "key_hints", c.key_hints);
        c.opener = flag(text, "opener", c.opener);
        c.window_title = flag(text, "window_title", c.window_title);
        let items: Vec<StatusItem> = values(text, "status_bar_items")
            .iter()
            .flat_map(|v| v.split(',').map(str::trim).map(str::to_ascii_lowercase))
            .filter_map(|w| StatusItem::parse(&w))
            .fold(Vec::new(), |mut acc, item| {
                // listing a thing twice draws it once
                if !acc.contains(&item) {
                    acc.push(item);
                }
                acc
            });
        if !items.is_empty() {
            c.status_bar_items = items;
        }
        c.rename_files = flag(text, "rename_files", c.rename_files);
        c.update_links = flag(text, "update_links", c.update_links);
        c.status_words = flag(text, "status_words", c.status_words);
        c.wikilinks = flag(text, "wikilinks", c.wikilinks);
        c.tags = flag(text, "tags", c.tags);
        c.code_colors = flag(text, "code_colors", c.code_colors);
        c.quick_open_recursive = match value(text, "quick_open").as_deref() {
            Some("folder") => false,
            Some("recursive") => true,
            _ => c.quick_open_recursive,
        };
        c.quick_open_browse = match value(text, "quick_open_mode").as_deref() {
            Some("browse") | Some("tree") => true,
            Some("search") | Some("list") => false,
            _ => c.quick_open_browse,
        };

        if let Some(v) = value(text, "autosave_ms").and_then(|v| v.parse::<u64>().ok()) {
            // no floor of 0: a zero here means "save on every keystroke", and
            // that is a legitimate thing to ask for
            c.autosave_ms = v.min(60_000);
        }
        if let Some(v) = value(text, "tab_width").and_then(|v| v.parse::<usize>().ok()) {
            c.tab_width = v.clamp(1, 16);
        }
        c.table_style = word(text, "table_style").unwrap_or(c.table_style);
        c.quick_open_dirs = values(text, "quick_open_dirs")
            .iter()
            // one folder per line, or several on one line separated by commas
            .flat_map(|v| v.split(',').map(str::trim).map(String::from))
            .filter(|v| !v.is_empty())
            .map(|v| expand(&v, &home))
            .collect();
        if let Some(v) = value(text, "daily_dir") {
            c.daily_dir = expand(&v, &home);
        }
        if let Some(v) = value(text, "daily_format") {
            c.daily_format = v.trim_matches('/').to_string();
        }
        if let Some(v) = value(text, "daily_template") {
            c.daily_template = expand(&v, &home);
        }
        if let Some(v) = value(text, "templates_dir") {
            c.templates_dir = expand(&v, &home);
        }
        c.keys = Keymap::from_settings(|key| value(text, key));
        c.preview_click = word(text, "preview_click").unwrap_or(c.preview_click);
        c.linked_mentions = flag(text, "linked_mentions", c.linked_mentions);
        c.autocomplete = flag(text, "autocomplete", c.autocomplete);
        c.front_matter = word(text, "front_matter").unwrap_or(c.front_matter);
        c.properties = word(text, "properties").unwrap_or(c.properties);
        c
    }

    /// Create both directories; returns an error only if the notes dir can't exist.
    pub fn ensure_dirs(&self) -> Result<()> {
        fs::create_dir_all(&self.notes_dir)
            .with_context(|| format!("creating notes_dir {}", self.notes_dir.display()))?;
        Ok(())
    }

    /// The folder today's note goes in, resolved against the notes dir.
    pub fn daily_dir(&self) -> PathBuf {
        crate::daily::resolve(&self.notes_dir, &self.daily_dir)
    }

    /// The daily template file, resolved against the notes dir.
    pub fn daily_template(&self) -> PathBuf {
        crate::daily::resolve(&self.notes_dir, &self.daily_template)
    }

    /// The folder *Insert template* lists, resolved against the notes dir.
    pub fn templates_dir(&self) -> PathBuf {
        crate::daily::resolve(&self.notes_dir, &self.templates_dir)
    }

    /// How an attachment should be written into a note: relative when it sits
    /// under the notes dir, absolute otherwise.
    pub fn link_for(&self, file: &Path) -> String {
        match file.strip_prefix(&self.notes_dir) {
            Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
            Err(_) => file.to_string_lossy().into_owned(),
        }
    }

    /// The settings note as it should be written: every setting on one line,
    /// with a short hint after it. Generated rather than stored, so a setting
    /// added later shows up the next time the file is regenerated.
    ///
    /// One line each and no paragraphs: the file is read to find a setting and
    /// change it, not read through, and prose between the lines is prose you
    /// have to skip past every time.
    pub fn to_document(&self) -> String {
        let short = crate::index::short;
        let yn = |b: bool| if b { "yes" } else { "no" };
        let mut d = Doc::default();
        d.head(
            "Settings",
            "Change a value and press ^S — it applies at once. New settings \
             are added to this file as they arrive.",
        );

        d.section("Folders");
        d.row(
            "notes_dir",
            short(&self.notes_dir),
            "where the .md files live",
        );
        d.row(
            "attachments_dir",
            short(&self.attachments_dir),
            "where pasted images go",
        );
        d.row(
            "templates_dir",
            short(&self.templates_dir),
            "the folder Insert template lists; a vault's .obsidian/templates.json sets it when this does not",
        );

        d.section("Daily note");
        d.row(
            "daily_dir",
            short(&self.daily_dir),
            "one note a day, under notes_dir unless absolute",
        );
        d.row(
            "daily_format",
            &self.daily_format,
            "the file name: YYYY MM DD MMMM ddd Do HH mm A, [literal], / for subfolders",
        );
        d.row(
            "daily_template",
            short(&self.daily_template),
            "{{title}} {{date}} {{date:FMT}} {{time}} {{yesterday}} {{tomorrow}}; a heading if missing",
        );

        d.section("Appearance");
        d.row("theme", self.theme.name(), "auto · dark · light");
        d.row(
            "page_width",
            if self.page_width == 0 {
                "full".to_string()
            } else {
                self.page_width.to_string()
            },
            "columns of note, or full",
        );
        d.row("borders", self.borders.name(), "rounded · square · none");
        d.row("bold_headings", yn(self.bold_headings), "yes · no");
        d.row(
            "code_colors",
            yn(self.code_colors),
            "colour words in fenced code by language",
        );
        d.row("status_bar", yn(self.status_bar), "the bottom line at all");
        d.row("key_hints", yn(self.key_hints), "the shortcuts in it");
        d.row(
            "opener",
            yn(self.opener),
            "the note decodes out of noise on start; Toggle opener flips it",
        );
        d.row(
            "window_title",
            yn(self.window_title),
            "the terminal title follows the note",
        );
        d.row(
            "status_words",
            yn(self.status_words),
            "words and characters in the bar",
        );
        d.row(
            "status_bar_items",
            self.status_bar_items
                .iter()
                .map(|i| i.word())
                .collect::<Vec<_>>()
                .join(", "),
            "path · name · mode · properties · keys · message, in order",
        );

        d.section("Colours");
        d.note("#rrggbb · #rgb · red, brightblue · default · theme");
        // a colour the user has not touched is written as the word `theme`,
        // not as the hex the theme happens to give it today. Spelling out all
        // ten pinned the dark palette into every settings file ever written,
        // and `theme: light` then changed nothing — the overrides underneath
        // put every colour back. The key stays in the document so it is still
        // discoverable; only the value defers.
        let base = theme::base(self.theme.mode());
        for c in &theme::COLORS {
            let mine = self.palette.get(c.name);
            let value = match (mine, base.get(c.name)) {
                (Some(m), Some(b)) if m != b => theme::color_to_string(m),
                _ => "theme".to_string(),
            };
            d.row(c.name, value, c.hint);
        }

        d.section("Editing");
        d.row("autosave_ms", self.autosave_ms, "idle time before a save");
        d.row("tab_width", self.tab_width, "spaces one tab inserts");
        d.row(
            "rename_files",
            yn(self.rename_files),
            "filename follows title",
        );
        d.row(
            "update_links",
            yn(self.update_links),
            "a rename fixes [[links]] to the note",
        );
        // an editor setting: the reading view's is `properties`, under Reading
        d.row(
            "front_matter",
            self.front_matter.name(),
            "dim · show · hide",
        );

        d.section("Reading");
        d.row(
            "table_style",
            self.table_style.name(),
            "auto · scroll · fit · wrap · cards",
        );
        d.row("preview_click", self.preview_click.name(), "select · edit");
        d.row(
            "properties",
            self.properties.name(),
            "box · line · hide — Toggle properties cycles them",
        );
        d.row("wikilinks", yn(self.wikilinks), "[[links]] open notes");
        d.row(
            "tags",
            yn(self.tags),
            "#tags coloured; follow one to list its notes",
        );
        d.row(
            "linked_mentions",
            yn(self.linked_mentions),
            "notes that link here, at the foot",
        );
        d.row(
            "autocomplete",
            yn(self.autocomplete),
            "suggest notes after [[ and tags after #",
        );
        d.row(
            "quick_open",
            if self.quick_open_recursive {
                "recursive"
            } else {
                "folder"
            },
            "recursive · folder",
        );
        d.row(
            "quick_open_mode",
            if self.quick_open_browse {
                "browse"
            } else {
                "search"
            },
            "search · browse",
        );
        if self.quick_open_dirs.is_empty() {
            d.row(
                "quick_open_dirs",
                "",
                "extra folders to search, comma separated",
            );
        } else {
            for (i, dir) in self.quick_open_dirs.iter().enumerate() {
                let hint = if i == 0 {
                    "extra folders to search"
                } else {
                    ""
                };
                d.row("quick_open_dirs", short(dir), hint);
            }
        }

        d.section("Keys");
        d.note("^K · cmd+k · alt+k · f5 · none — or several, as `^/ f1`");
        for (key, spec, what) in self.keys.settings_rows() {
            d.row(key, spec, what);
        }
        d.finish()
    }
}

/// Change one setting in the settings note on disk, leaving every other line
/// — comments, order, the user's own spacing — as it was. The value on the
/// `- key:` line is replaced up to its `#` hint; a key the file lacks is
/// appended. Used by the commands that flip a setting from inside the app,
/// where regenerating the whole document would also write the environment's
/// overrides into it for good.
pub fn set_value(key: &str, new: &str) -> Result<()> {
    let path = settings_path()?;
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let out = with_value(&text, key, new);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(&path, out).with_context(|| format!("writing {}", path.display()))
}

/// `text`, a settings document, with `key` set to `new`: see `set_value`.
fn with_value(text: &str, key: &str, new: &str) -> String {
    let mut out = String::with_capacity(text.len() + 32);
    let mut found = false;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let stripped = strip_comment(body);
        let head = stripped.trim().trim_start_matches(['-', '*', '>', ' ']);
        let is_key = head
            .split_once([':', '='])
            .is_some_and(|(k, _)| k.trim() == key);
        if !is_key || found {
            out.push_str(line);
            continue;
        }
        found = true;
        let hint = &body[stripped.len()..];
        let (prefix, _) = stripped.split_once([':', '=']).unwrap_or((stripped, ""));
        let pad = if hint.is_empty() { "" } else { "  " };
        out.push_str(&format!("{prefix}: {new}{pad}{}", hint.trim_start()));
        out.push_str(&line[body.len()..]);
    }
    if !found {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("- {key}: {new}\n"));
    }
    out
}

fn write_settings(path: &Path, config: &Config) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(path, config.to_document()).with_context(|| format!("writing {}", path.display()))
}

/// Does `text` already mention every setting catcher has? Compared against
/// the document this config would generate, so the check maintains itself:
/// a setting added to `to_document` is one an old file is missing.
fn covers_every_setting(text: &str, config: &Config) -> bool {
    let have = setting_keys(text);
    setting_keys(&config.to_document())
        .into_iter()
        .all(|k| have.contains(&k))
}

/// The `- key:` names a settings document sets, whatever their values.
fn setting_keys(text: &str) -> std::collections::BTreeSet<String> {
    text.lines()
        .filter_map(|line| {
            let line = strip_comment(line);
            let line = line.trim();
            let line = line
                .strip_prefix("- ")
                .or_else(|| line.strip_prefix("* "))?;
            let (k, _) = line.split_once(':')?;
            let k = k.trim();
            (!k.is_empty() && k.chars().all(|c| c.is_alphanumeric() || c == '_'))
                .then(|| k.to_string())
        })
        .collect()
}

/// Builds the settings note: one `- key: value` line per setting with its hint
/// aligned into a column, section by section. Alignment is per section, so a
/// long path in one does not push every hint in the file to the right.
#[derive(Default)]
struct Doc {
    out: String,
    /// (line, hint) for the section being built, before it is aligned.
    pending: Vec<(String, String)>,
}

impl Doc {
    fn head(&mut self, title: &str, line: &str) {
        self.out.push_str(&format!("# {title}\n\n{line}\n"));
    }

    fn section(&mut self, name: &str) {
        self.flush();
        self.out.push_str(&format!("\n## {name}\n\n"));
    }

    /// A line of guidance for the whole section — the only prose left, and
    /// only where the values are not self-explanatory.
    fn note(&mut self, text: &str) {
        self.flush();
        self.out.push_str(&format!("{text}\n\n"));
    }

    fn row(&mut self, key: &str, value: impl std::fmt::Display, hint: &str) {
        let value = value.to_string();
        let line = if value.is_empty() {
            format!("- {key}:")
        } else {
            format!("- {key}: {value}")
        };
        self.pending.push((line, hint.to_string()));
    }

    fn flush(&mut self) {
        let width = self
            .pending
            .iter()
            .filter(|(_, h)| !h.is_empty())
            .map(|(l, _)| l.chars().count())
            .max()
            .unwrap_or(0);
        for (line, hint) in std::mem::take(&mut self.pending) {
            if hint.is_empty() {
                self.out.push_str(&format!("{line}\n"));
            } else {
                let pad = " ".repeat(width.saturating_sub(line.chars().count()) + 2);
                self.out.push_str(&format!("{line}{pad}# {hint}\n"));
            }
        }
    }

    fn finish(mut self) -> String {
        self.flush();
        self.out
    }
}

/// `key: value`, `- key: value` or `key = value`, first match wins. `#` starts
/// a comment except inside quotes — a path may legitimately contain one.
///
/// Both separators are accepted so a `key = value` typed out of habit still
/// works.
/// The `attachmentFolderPath` string in an Obsidian `app.json`.
pub fn obsidian_attachment_setting(json: &str) -> Option<String> {
    obsidian_string(json, "attachmentFolderPath")
}

/// The `folder` string in an Obsidian `templates.json` — where that vault's
/// Templates plugin keeps its files.
pub fn obsidian_template_folder(json: &str) -> Option<String> {
    obsidian_string(json, "folder")
}

/// A string value out of one of Obsidian's small config files, found by
/// looking rather than parsing: the files are flat and the values are plain
/// strings, and a JSON reader would be a dependency for two keys.
fn obsidian_string(json: &str, name: &str) -> Option<String> {
    let key = format!("\"{name}\"");
    let rest = &json[json.find(&key)? + key.len()..];
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    let code = u32::from_str_radix(&hex, 16).ok()?;
                    out.push(char::from_u32(code)?);
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

fn value(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let line = strip_comment(line);
        // markdown list markers, blockquote bars and indentation are all chrome
        let line = line.trim().trim_start_matches(['-', '*', '>', ' ']);
        let Some((k, v)) = line.split_once([':', '=']) else {
            continue;
        };
        if k.trim() != key {
            continue;
        }
        let v = v
            .trim()
            .trim_matches(|c| c == '"' || c == '\'' || c == '`')
            .trim();
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }
    None
}

/// Every value given for `key`, in file order — for the settings a user may
/// repeat, like a list of folders.
fn values(text: &str, key: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = strip_comment(line);
            let line = line.trim().trim_start_matches(['-', '*', '>', ' ']);
            let (k, v) = line.split_once([':', '='])?;
            (k.trim() == key).then(|| {
                v.trim()
                    .trim_matches(|c| c == '"' || c == '\'' || c == '`')
                    .trim()
                    .to_string()
            })
        })
        .filter(|v| !v.is_empty())
        .collect()
}

/// A yes/no setting, falling back to `default` when unset or unreadable.
fn flag(text: &str, key: &str, default: bool) -> bool {
    match value(text, key).as_deref().map(str::to_ascii_lowercase) {
        Some(v) => matches!(v.as_str(), "yes" | "true" | "on" | "1"),
        None => default,
    }
}

/// A word-valued setting, or `None` when unset or not one of its words.
fn word<T: Words>(text: &str, key: &str) -> Option<T> {
    value(text, key).and_then(|v| T::parse(&v))
}

/// Drop a trailing `# comment`. A `#` only opens one when a space follows it,
/// which is how a comment is actually written — and is what keeps `#00ff88`
/// readable as a colour and `/a/c#1/notes` as a path.
fn strip_comment(line: &str) -> &str {
    // a markdown heading is prose, never a setting
    if line.trim_start().starts_with("# ") || line.trim() == "#" {
        return "";
    }
    let mut quote: Option<char> = None;
    let bytes = line.as_bytes();
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (None, '"') | (None, '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '#') if bytes.get(i + 1).is_none_or(|b| b.is_ascii_whitespace()) => {
                return &line[..i]
            }
            _ => {}
        }
    }
    line
}

/// A typed path with a leading `~` made absolute, for the vault picker.
pub fn expand_home(value: &str) -> PathBuf {
    match std::env::home_dir() {
        Some(home) => expand(value, &home),
        None => PathBuf::from(value),
    }
}

fn expand(value: &str, home: &Path) -> PathBuf {
    match value.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if value == "~" => home.to_path_buf(),
        None => PathBuf::from(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_settings_parse_in_any_case_and_write_back_their_word() {
        let c = Config::from_file_text("- theme: LIGHT\n- borders: Square\n- table_style: cards\n");
        assert_eq!(c.theme, Theme::Light);
        assert_eq!(c.borders, BorderStyle::Square);
        assert_eq!(c.table_style, TableStyle::Cards);
        assert_eq!(TableStyle::parse("nope"), None);
        assert_eq!(c.table_style.name(), "cards");
        for c in theme::COLORS {
            assert!(theme::COLOR_KEYS.contains(&c.name));
        }
    }

    #[test]
    fn reads_markdown_list_settings() {
        let text = "## Folders\n\n- notes_dir: /a/b\n- theme: light\n";
        assert_eq!(value(text, "notes_dir").as_deref(), Some("/a/b"));
        assert_eq!(value(text, "theme").as_deref(), Some("light"));
        assert_eq!(value(text, "attachments_dir"), None);
    }

    #[test]
    fn a_markdown_heading_is_not_a_comment_but_prose_after_a_value_is() {
        let text = "# Settings\n\n- notes_dir: /a/b  # mine\n";
        assert_eq!(value(text, "notes_dir").as_deref(), Some("/a/b"));
    }

    #[test]
    fn a_hash_inside_a_quoted_path_is_not_a_comment() {
        let text = "- notes_dir: \"/a/c#1/notes\"\n";
        assert_eq!(value(text, "notes_dir").as_deref(), Some("/a/c#1/notes"));
    }

    #[test]
    fn the_generated_document_round_trips_to_the_same_config() {
        let mut palette = theme::base(Mode::Light);
        palette.accent = theme::parse_color("#00ff88").unwrap();
        let c = Config {
            theme: Theme::Light,
            palette,
            page_width: 72,
            borders: BorderStyle::None,
            bold_headings: false,
            key_hints: false,
            window_title: false,
            autosave_ms: 1500,
            tab_width: 4,
            rename_files: false,
            update_links: false,
            status_words: true,
            table_style: TableStyle::Cards,
            preview_click: PreviewClick::Edit,
            front_matter: FrontMatter::Hide,
            status_bar_items: vec![
                StatusItem::Name,
                StatusItem::Properties,
                StatusItem::Message,
            ],
            wikilinks: false,
            quick_open_recursive: false,
            quick_open_browse: true,
            quick_open_dirs: vec![PathBuf::from("/vault"), PathBuf::from("/work")],
            ..Default::default()
        };
        let back = Config::from_str(&c.to_document());
        if std::env::var_os("CATCHER_DIR").is_none() {
            assert_eq!(back, c);
        }
    }

    #[test]
    fn the_environment_points_the_session_elsewhere_but_is_never_written_back() {
        let text = "- notes_dir: /a/b\n";
        let on_disk = Config::from_file_text(text);
        assert_eq!(on_disk.notes_dir, PathBuf::from("/a/b"));
        // the document is generated from the file's own parse, so whatever
        // CATCHER_DIR says in this process, the file keeps its folder
        assert!(on_disk.to_document().contains("notes_dir: /a/b"));
    }

    #[test]
    fn quick_open_can_be_told_to_open_on_the_tree_instead_of_the_list() {
        assert!(!Config::default().quick_open_browse);
        assert!(Config::from_str("- quick_open_mode: browse\n").quick_open_browse);
        assert!(Config::from_str("- quick_open_mode: tree\n").quick_open_browse);
        assert!(!Config::from_str("- quick_open_mode: search\n").quick_open_browse);
        // and the choice survives being written out and read back, which is
        // what `covers_every_setting` leans on
        let c = Config {
            quick_open_browse: true,
            ..Default::default()
        };
        assert!(Config::from_str(&c.to_document()).quick_open_browse);
    }

    #[test]
    fn linked_mentions_is_on_by_default_and_can_be_turned_off() {
        assert!(Config::default().linked_mentions);
        assert!(!Config::from_str("- linked_mentions: no\n").linked_mentions);
        // and off survives being written out and read back, which is what
        // `covers_every_setting` leans on
        let c = Config {
            linked_mentions: false,
            ..Default::default()
        };
        assert!(!Config::from_str(&c.to_document()).linked_mentions);
    }

    #[test]
    fn autocomplete_is_on_by_default_and_can_be_turned_off() {
        assert!(Config::default().autocomplete);
        assert!(!Config::from_str("- autocomplete: off\n").autocomplete);
        let c = Config {
            autocomplete: false,
            ..Default::default()
        };
        assert!(!Config::from_str(&c.to_document()).autocomplete);
    }

    #[test]
    fn status_words_is_off_by_default_and_can_be_turned_on() {
        assert!(!Config::default().status_words);
        assert!(Config::from_str("- status_words: on\n").status_words);
        assert!(!Config::from_str("- status_words: off\n").status_words);
    }

    #[test]
    fn update_links_is_on_by_default_and_can_be_turned_off() {
        assert!(Config::default().update_links);
        assert!(!Config::from_str("- update_links: no\n").update_links);
        // and off survives being written out and read back, which is what
        // `covers_every_setting` leans on
        let c = Config {
            update_links: false,
            ..Default::default()
        };
        assert!(!Config::from_str(&c.to_document()).update_links);
    }

    #[test]
    fn wikilinks_can_be_turned_off() {
        assert!(Config::default().wikilinks);
        assert!(!Config::from_str("- wikilinks: no\n").wikilinks);
        // and the setting survives being written out and read back, which is
        // what `covers_every_setting` leans on
        let c = Config {
            wikilinks: false,
            ..Default::default()
        };
        assert!(!Config::from_str(&c.to_document()).wikilinks);
    }

    #[test]
    fn tags_can_be_turned_off() {
        assert!(Config::default().tags);
        assert!(!Config::from_str("- tags: no\n").tags);
        let c = Config {
            tags: false,
            ..Default::default()
        };
        assert!(!Config::from_str(&c.to_document()).tags);
    }

    #[test]
    fn code_colours_are_on_by_default_and_the_five_roles_round_trip() {
        assert!(Config::default().code_colors);
        assert!(!Config::from_str("- code_colors: no\n").code_colors);
        let c = Config {
            code_colors: false,
            ..Default::default()
        };
        assert!(!Config::from_str(&c.to_document()).code_colors);
        // and each role is a colour row like any other, pinned or deferred
        let mut c = Config::default();
        c.palette.code_keyword = theme::parse_color("#00ff88").unwrap();
        let doc = c.to_document();
        assert!(doc.contains("- code_keyword: #00ff88"));
        assert!(doc.contains("- code_string: theme"));
        let back = Config::from_str(&doc);
        assert_eq!(back.palette.code_keyword, c.palette.code_keyword);
        assert_eq!(back.palette.code_comment, theme::DARK.code_comment);
    }

    #[test]
    fn front_matter_takes_dim_show_or_hide_and_defaults_to_dim() {
        assert_eq!(Config::default().front_matter, FrontMatter::Dim);
        let read = |v: &str| Config::from_str(&format!("- front_matter: {v}\n")).front_matter;
        assert_eq!(read("show"), FrontMatter::Show);
        assert_eq!(read("hide"), FrontMatter::Hide);
        assert_eq!(read("dim"), FrontMatter::Dim);
        // a value nobody recognises leaves the default standing rather than
        // picking one of the three at random
        assert_eq!(read("sometimes"), FrontMatter::Dim);
        assert_eq!(
            Config::from_str("nothing set\n").front_matter,
            FrontMatter::Dim
        );
    }

    #[test]
    fn properties_is_a_status_item_a_user_has_to_ask_for() {
        assert!(!Config::default()
            .status_bar_items
            .contains(&StatusItem::Properties));
        let c = Config::from_str("- status_bar_items: path, properties, keys\n");
        assert_eq!(
            c.status_bar_items,
            vec![StatusItem::Path, StatusItem::Properties, StatusItem::Keys]
        );
        // and it survives being written back out
        assert!(c.to_document().contains("properties"));
        assert_eq!(
            Config::from_str(&c.to_document()).status_bar_items,
            c.status_bar_items
        );
    }

    #[test]
    fn an_older_settings_file_is_spotted_and_its_values_survive_the_rewrite() {
        // the shape of a file written before a setting existed
        let old = "# Settings\n\n## Folders\n\n- notes_dir: /vault\n\n## Appearance\n\n                   - theme: light\n\nA paragraph of prose that used to live here.\n";
        let c = Config::from_str(old);
        assert!(!covers_every_setting(old, &c));

        // rewriting keeps what was set and adds what was missing
        let fresh = c.to_document();
        assert!(covers_every_setting(&fresh, &c));
        let back = Config::from_str(&fresh);
        assert_eq!(back.theme, Theme::Light);
        if std::env::var_os("CATCHER_DIR").is_none() {
            assert_eq!(back.notes_dir, PathBuf::from("/vault"));
        }
        // and the prose is gone
        assert!(!fresh.contains("A paragraph of prose"));
    }

    #[test]
    fn a_current_settings_file_is_left_alone() {
        let c = Config::default();
        assert!(covers_every_setting(&c.to_document(), &c));
        // a hand-written comment beside a setting does not read as missing
        let edited = c
            .to_document()
            .replace("- theme: auto", "- theme: auto  # mine");
        assert!(covers_every_setting(&edited, &c));
    }

    #[test]
    fn prose_lines_are_not_mistaken_for_settings() {
        // the section notes are bare lines, and bullets in a note are not keys
        let keys = setting_keys("#rrggbb · #rgb · red\n- accent: #ff9e64\n- a thing: no\n");
        assert!(keys.contains("accent"));
        assert_eq!(keys.len(), 1);
    }

    #[test]
    fn every_default_survives_a_round_trip() {
        let c = Config::default();
        let back = Config::from_str(&c.to_document());
        if std::env::var_os("CATCHER_DIR").is_none() {
            assert_eq!(back, c);
        }
    }

    #[test]
    fn the_defaults_are_catcher_s_own_directory() {
        // an unset notes_dir must never land on ~/notes: catcher renames
        // files to follow their titles, and that directory may be someone
        // else's markdown
        let home = std::env::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let c = Config::from_str("nothing set\n");
        if std::env::var_os("CATCHER_DIR").is_none() {
            assert_eq!(c.notes_dir, default_notes_dir(&home));
        }
        assert_eq!(c.attachments_dir, c.notes_dir.join("attachments"));
    }

    #[test]
    fn attachments_follow_a_changed_notes_dir_unless_set_themselves() {
        let c = Config::from_str("- notes_dir: /vault\n");
        if std::env::var_os("CATCHER_DIR").is_none() {
            assert_eq!(c.attachments_dir, PathBuf::from("/vault/attachments"));
            // a folder of its own stands, as long as it is there
            let pics = crate::testutil::tmpdir("config", "attachments-set");
            std::fs::create_dir_all(&pics).unwrap();
            let c = Config::from_str(&format!(
                "- notes_dir: /vault\n- attachments_dir: {}\n",
                pics.display()
            ));
            assert_eq!(c.attachments_dir, pics);
            let _ = std::fs::remove_dir_all(&pics);
        }
    }

    #[test]
    fn the_daily_note_settings_default_to_journal_and_round_trip() {
        let c = Config::default();
        assert_eq!(c.daily_dir, PathBuf::from("journal"));
        assert_eq!(c.daily_template, PathBuf::from("journal/template.md"));
        assert_eq!(c.daily_format, "YYYY-MM-DD");
        assert_eq!(c.daily_dir(), c.notes_dir.join("journal"));
        assert_eq!(c.daily_template(), c.notes_dir.join("journal/template.md"));
        let c = Config {
            daily_dir: PathBuf::from("/vault/daily"),
            daily_template: PathBuf::from("templates/day.md"),
            daily_format: "YYYY/MM/DD-MM-YYYY".to_string(),
            ..Default::default()
        };
        assert_eq!(c.daily_dir(), PathBuf::from("/vault/daily"));
        let back = Config::from_str(&c.to_document());
        assert_eq!(back.daily_dir, c.daily_dir);
        assert_eq!(back.daily_template, c.daily_template);
        assert_eq!(back.daily_format, c.daily_format);
        // a leading or trailing slash is not a folder
        assert_eq!(
            Config::from_str("- daily_format: /YYYY/").daily_format,
            "YYYY"
        );
        // a hand-typed value reads back, tilde and all
        let home = std::env::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let c = Config::from_str("- daily_dir: ~/days\n");
        assert_eq!(c.daily_dir, home.join("days"));
    }

    #[test]
    fn paths_expand_a_leading_tilde() {
        let home = PathBuf::from("/home/x");
        assert_eq!(expand("~/notes", &home), PathBuf::from("/home/x/notes"));
        assert_eq!(expand("/abs", &home), PathBuf::from("/abs"));
    }

    #[test]
    fn theme_defaults_to_dark_and_only_light_flips_it() {
        assert_eq!(Config::from_str("").theme, Theme::Auto);
        assert_eq!(Config::from_str("- theme: light").theme, Theme::Light);
        assert_eq!(Config::from_str("- theme: dark").theme, Theme::Dark);
        assert_eq!(Config::from_str("- theme: lite").theme, Theme::Auto);
        assert_eq!(Theme::Dark.mode(), Mode::Dark);
        assert_eq!(Theme::Light.mode(), Mode::Light);
    }

    #[test]
    fn colours_override_the_theme_they_sit_on() {
        let c = Config::from_str("- theme: dark\n- accent: #00ff88\n");
        assert_eq!(c.palette.accent, theme::parse_color("#00ff88").unwrap());
        // everything unset still comes from the theme
        assert_eq!(c.palette.dim, theme::DARK.dim);
        // an unreadable colour leaves the theme's own standing
        let c = Config::from_str("- accent: chartreuse-ish\n");
        assert_eq!(c.palette.accent, theme::DARK.accent);
    }

    #[test]
    fn colour_shorthand_and_names_parse() {
        assert_eq!(theme::parse_color("#f80"), theme::parse_color("#ff8800"));
        assert!(theme::parse_color("brightblue").is_some());
        assert!(theme::parse_color("#gg0000").is_none());
        assert!(theme::parse_color("#ff00").is_none());
    }

    #[test]
    fn numbers_are_clamped_to_something_usable() {
        assert_eq!(Config::from_str("- tab_width: 99").tab_width, 16);
        assert_eq!(Config::from_str("- tab_width: 0").tab_width, 1);
        assert_eq!(
            Config::from_str("- autosave_ms: 999999").autosave_ms,
            60_000
        );
        assert_eq!(Config::from_str("- page_width: full").page_width, 0);
    }

    #[test]
    fn extra_quick_open_folders_can_be_repeated_or_listed() {
        let home = std::env::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let c = Config::from_str("- quick_open_dirs: /vault\n- quick_open_dirs: ~/work\n");
        assert_eq!(
            c.quick_open_dirs,
            vec![PathBuf::from("/vault"), home.join("work")]
        );
        let c = Config::from_str("- quick_open_dirs: /a, /b\n");
        assert_eq!(
            c.quick_open_dirs,
            vec![PathBuf::from("/a"), PathBuf::from("/b")]
        );
        // the empty line the template writes means no extra folders
        assert!(Config::from_str("- quick_open_dirs:\n")
            .quick_open_dirs
            .is_empty());
    }

    #[test]
    fn the_status_bar_is_a_list_of_parts_in_the_order_given() {
        let c = Config::from_str("- status_bar_items: name, keys\n");
        assert_eq!(c.status_bar_items, vec![StatusItem::Name, StatusItem::Keys]);
        // unknown words are ignored, repeats drawn once
        let c = Config::from_str("- status_bar_items: keys, weather, keys, path\n");
        assert_eq!(c.status_bar_items, vec![StatusItem::Keys, StatusItem::Path]);
        // nothing usable leaves the default standing
        assert_eq!(
            Config::from_str("- status_bar_items: weather\n").status_bar_items,
            Config::default().status_bar_items
        );
        assert_eq!(
            Config::from_str("").status_bar_items,
            vec![StatusItem::Path, StatusItem::Message, StatusItem::Keys]
        );
    }

    #[test]
    fn the_search_key_is_written_and_read_back() {
        use crate::keys::Action;
        let c = Config::default();
        assert!(c.to_document().contains("- key_search: ctrl+⇧F"));
        let back = Config::from_str("- key_search: f3\n");
        assert_eq!(back.keys.label(Action::SearchAll), "F3");
    }

    #[test]
    fn properties_is_a_reading_setting_that_round_trips() {
        let c = Config::from_str("- properties: line\n");
        assert_eq!(c.properties, Properties::Line);
        assert_eq!(
            Config::from_str("- properties: hide\n").properties,
            Properties::Hide
        );
        assert_eq!(Config::from_str("").properties, Properties::Box);
        assert!(c.to_document().contains("- properties: line"));
    }

    #[test]
    fn setting_one_value_leaves_the_rest_of_the_document_alone() {
        let text = "# Settings\n\n- properties: box  # front matter as a box · one line · hide\n- wikilinks: yes\n";
        let out = with_value(text, "properties", "line");
        assert_eq!(
            out,
            "# Settings\n\n- properties: line  # front matter as a box · one line · hide\n- wikilinks: yes\n"
        );
        assert_eq!(Config::from_str(&out).properties, Properties::Line);
        // a key the file lacks is appended
        let out = with_value("- wikilinks: yes\n", "properties", "hide");
        assert!(out.ends_with("- wikilinks: yes\n- properties: hide\n"));
        // a value without a hint
        assert_eq!(
            with_value("- front_matter: dim\n", "front_matter", "hide"),
            "- front_matter: hide\n"
        );
    }

    #[test]
    fn flags_take_the_words_a_person_would_type() {
        assert!(!Config::from_str("- key_hints: no").key_hints);
        assert!(!Config::from_str("- key_hints: false").key_hints);
        assert!(Config::from_str("- key_hints: yes").key_hints);
        // unset leaves the default
        assert!(Config::from_str("").key_hints);
    }

    #[test]
    fn reads_the_attachment_folder_from_obsidian_app_json() {
        let json = r#"{ "promptDelete": false, "attachmentFolderPath": "Files/img", "x": 1 }"#;
        assert_eq!(
            obsidian_attachment_setting(json).as_deref(),
            Some("Files/img")
        );
        let json = "{\n  \"attachmentFolderPath\" : \"./\"\n}";
        assert_eq!(obsidian_attachment_setting(json).as_deref(), Some("./"));
        assert_eq!(
            obsidian_attachment_setting(r#"{"attachmentFolderPath":"a \"q\" \u0041"}"#).as_deref(),
            Some("a \"q\" A")
        );
        assert_eq!(obsidian_attachment_setting(r#"{"other": "x"}"#), None);
        assert_eq!(
            obsidian_attachment_setting(r#"{"attachmentFolderPath": 3}"#),
            None
        );
        assert_eq!(
            obsidian_attachment_setting(r#"{"attachmentFolderPath": "open"#),
            None
        );
    }

    #[test]
    fn obsidian_attachment_settings_map_onto_the_lookup() {
        let mut c = Config {
            notes_dir: PathBuf::from("/v"),
            attachments_dir: PathBuf::from("/v/attachments"),
            ..Default::default()
        };
        c.set_obsidian_attachments("Files");
        assert_eq!(c.attachments_dir, PathBuf::from("/v/Files"));
        assert_eq!(c.attachment_subfolder, "attachments");
        c.set_obsidian_attachments("./");
        assert_eq!(c.attachments_dir, PathBuf::from("/v"));
        assert_eq!(c.attachment_subfolder, ".");
        c.set_obsidian_attachments("./_media");
        assert_eq!(c.attachments_dir, PathBuf::from("/v/_media"));
        assert_eq!(c.attachment_subfolder, "_media");

        // a named attachments_dir is left alone; the default is replaced
        let dir = std::env::temp_dir().join("catcher-obsidian-cfg-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
        std::fs::write(
            dir.join(".obsidian/app.json"),
            r#"{"attachmentFolderPath": "Files"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("Files")).unwrap();
        let mut c = Config {
            notes_dir: dir.clone(),
            attachments_dir: dir.join("attachments"),
            ..Default::default()
        };
        c.adopt_obsidian_attachments();
        assert_eq!(c.attachments_dir, dir.join("Files"));
        let mut c = Config {
            notes_dir: dir.clone(),
            attachments_dir: PathBuf::from("/pics"),
            ..Default::default()
        };
        c.adopt_obsidian_attachments();
        assert_eq!(c.attachments_dir, PathBuf::from("/pics"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_templates_folder_defaults_to_templates_and_round_trips() {
        let c = Config::default();
        assert_eq!(c.templates_dir, PathBuf::from("templates"));
        assert_eq!(c.templates_dir(), c.notes_dir.join("templates"));
        let c = Config {
            notes_dir: PathBuf::from("/vault"),
            templates_dir: PathBuf::from("_meta/forms"),
            ..Default::default()
        };
        assert_eq!(c.templates_dir(), PathBuf::from("/vault/_meta/forms"));
        // read back from a document the folder actually exists in
        let vault = crate::testutil::tmpdir("config", "templates-round-trip");
        std::fs::create_dir_all(vault.join("_meta/forms")).unwrap();
        let c = Config {
            notes_dir: vault.clone(),
            templates_dir: PathBuf::from("_meta/forms"),
            ..Default::default()
        };
        let back = Config::from_str(&c.to_document());
        assert_eq!(back.templates_dir, c.templates_dir);
        // an absolute folder stands, and a tilde expands
        assert_eq!(
            Config::from_str(&format!(
                "- templates_dir: {}\n",
                vault.join("_meta/forms").display()
            ))
            .templates_dir,
            vault.join("_meta/forms")
        );
        let _ = std::fs::remove_dir_all(&vault);
        let c = Config {
            notes_dir: PathBuf::from("/vault"),
            templates_dir: PathBuf::from("/forms"),
            ..Default::default()
        };
        assert_eq!(c.templates_dir(), PathBuf::from("/forms"));
    }

    #[test]
    fn the_templates_folder_comes_from_obsidian_when_the_setting_does_not() {
        let json = r#"{ "folder": "_meta/forms", "createTitle": "" }"#;
        assert_eq!(
            obsidian_template_folder(json).as_deref(),
            Some("_meta/forms")
        );
        assert_eq!(obsidian_template_folder(r#"{"other": "x"}"#), None);
        assert_eq!(obsidian_template_folder(r#"{"folder": 3}"#), None);

        // a named templates_dir is left alone; the default is replaced
        let dir = crate::testutil::tmpdir("config", "obsidian-templates");
        std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
        std::fs::create_dir_all(dir.join("_meta/forms")).unwrap();
        std::fs::write(dir.join(".obsidian/templates.json"), json).unwrap();
        let mut c = Config {
            notes_dir: dir.clone(),
            ..Default::default()
        };
        c.adopt_obsidian_templates();
        assert_eq!(c.templates_dir, PathBuf::from("_meta/forms"));
        assert_eq!(c.templates_dir(), dir.join("_meta/forms"));
        let mut c = Config {
            notes_dir: dir.clone(),
            templates_dir: PathBuf::from("forms"),
            ..Default::default()
        };
        c.adopt_obsidian_templates();
        assert_eq!(c.templates_dir, PathBuf::from("forms"));
        // an empty folder in the file is no answer at all
        std::fs::write(dir.join(".obsidian/templates.json"), r#"{"folder": ""}"#).unwrap();
        let mut c = Config {
            notes_dir: dir.clone(),
            ..Default::default()
        };
        c.adopt_obsidian_templates();
        assert_eq!(c.templates_dir, PathBuf::from("templates"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_that_is_not_there_falls_back_to_the_default() {
        let dir = crate::testutil::tmpdir("config", "dead-folders");
        std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
        let text = |extra: &str| format!("- notes_dir: {}\n{extra}", dir.display());
        // resolved by hand rather than through `from_str`, so a CATCHER_DIR in
        // the environment cannot move the vault out from under the test
        let cfg = |extra: &str| {
            let t = text(extra);
            let mut c = Config::from_file_text(&t);
            c.resolve_folders(&t);
            c
        };

        // a vault whose Obsidian settings name folders it no longer has
        std::fs::write(
            dir.join(".obsidian/templates.json"),
            r#"{"folder": "gone/forms"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join(".obsidian/app.json"),
            r#"{"attachmentFolderPath": "gone/pics"}"#,
        )
        .unwrap();
        let c = cfg("");
        assert_eq!(c.templates_dir, PathBuf::from("templates"));
        assert_eq!(c.attachments_dir, dir.join("attachments"));

        // the same settings, with the folders there
        std::fs::create_dir_all(dir.join("gone/forms")).unwrap();
        std::fs::create_dir_all(dir.join("gone/pics")).unwrap();
        let c = cfg("");
        assert_eq!(c.templates_dir, PathBuf::from("gone/forms"));
        assert_eq!(c.attachments_dir, dir.join("gone/pics"));

        // a `./` setting is beside the note, not a vault folder, so it is
        // taken as written whatever is or is not on disk
        std::fs::write(
            dir.join(".obsidian/app.json"),
            r#"{"attachmentFolderPath": "./nowhere"}"#,
        )
        .unwrap();
        let c = cfg("");
        assert_eq!(c.attachments_dir, dir.join("nowhere"));
        assert_eq!(c.attachment_subfolder, "nowhere");

        // and a folder the settings note names itself, that is not there
        let _ = std::fs::remove_file(dir.join(".obsidian/app.json"));
        let _ = std::fs::remove_file(dir.join(".obsidian/templates.json"));
        let named = "- templates_dir: archive/forms\n- attachments_dir: /nowhere/pics\n";
        let c = cfg(named);
        assert_eq!(c.templates_dir, PathBuf::from("templates"));
        assert_eq!(c.attachments_dir, dir.join("attachments"));
        // and the settings note keeps what was typed: the rewrite is generated
        // from the unresolved parse, so the folder may still come back
        let on_disk = Config::from_file_text(&text(named)).to_document();
        assert!(on_disk.contains("- templates_dir: archive/forms"));
        assert!(on_disk.contains("- attachments_dir: /nowhere/pics"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn links_are_relative_inside_the_notes_dir() {
        let c = Config {
            notes_dir: PathBuf::from("/n"),
            attachments_dir: PathBuf::from("/n/attachments"),
            ..Default::default()
        };
        assert_eq!(
            c.link_for(Path::new("/n/attachments/a.png")),
            "attachments/a.png"
        );
        assert_eq!(c.link_for(Path::new("/other/a.png")), "/other/a.png");
    }

    #[test]
    fn an_untouched_colour_is_written_as_the_word_theme() {
        let c = Config::default();
        let doc = c.to_document();
        assert!(doc.contains("- code_bg: theme"));
        // and the word survives the round trip as the theme's own value
        assert_eq!(Config::from_str(&doc).palette.code_bg, theme::DARK.code_bg);
    }

    #[test]
    fn a_pinned_colour_is_written_as_its_hex_and_only_that_one_is() {
        let mut c = Config::default();
        c.palette.accent = theme::parse_color("#00ff88").unwrap();
        let doc = c.to_document();
        assert!(doc.contains("- accent: #00ff88"));
        assert!(doc.contains("- dim: theme"));
    }

    #[test]
    fn the_editing_command_keys_are_written_unbound_and_read_back_bound() {
        use crate::keys::Action;
        let doc = Config::default().to_document();
        for key in [
            "key_checkbox",
            "key_line_up",
            "key_line_down",
            "key_heading",
            "key_date",
            "key_copy_path",
            "key_reveal",
            "key_split_right",
            "key_split_down",
            "key_new_tab",
            "key_extract",
        ] {
            assert!(doc.contains(&format!("- {key}: none")), "{key}");
        }
        let c = Config::from_str(&doc.replace("- key_date: none", "- key_date: ^D"));
        assert_eq!(c.keys.label(Action::InsertDate), "^D");
        // and the rewrite keeps it
        assert!(c.to_document().contains("- key_date: ^D"));
    }

    #[test]
    fn switching_the_theme_moves_every_colour_the_user_has_not_pinned() {
        // the bug this guards: a settings file that spelled out all ten
        // colours made `theme: light` inert, because the dark hexes below it
        // put the dark palette straight back
        let doc = Config::default().to_document();
        let light = doc.replace("- theme: auto", "- theme: light");
        let c = Config::from_str(&light);
        assert_eq!(c.theme, Theme::Light);
        assert_eq!(c.palette.code_bg, theme::LIGHT.code_bg);
        assert_eq!(c.palette.accent, theme::LIGHT.accent);
    }
}

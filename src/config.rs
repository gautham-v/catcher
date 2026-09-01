//! `~/.config/tinynote/settings.md` — the settings, as a note.
//!
//! Settings are markdown because they are edited *inside tinynote*: the same
//! editor, the same live preview, no $EDITOR and no TOML. Every setting is a
//! `- key: value` line under a `##` heading, with the prose around it saying
//! what it does. Delete a line and its default stands.
//!
//! Notes default to `~/tinynote`, not `~/notes`: tinynote renames files to
//! follow their titles, so pointing it at a directory someone might already
//! keep markdown in would let it quietly reorganize a collection it was never
//! given. A directory named after the tool can only be the tool's.
//!
//! `TINYNOTE_DIR` still wins over `notes_dir`, which keeps
//! `TINYNOTE_DIR=/tmp/x cargo run` working. An old `config.toml` is read once,
//! to seed `settings.md` the first time, and never again.

use crate::keys::Keymap;
use crate::md::theme::{self, Mode, Palette};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

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

/// What a plain click in the preview does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PreviewClick {
    /// Place a selection anchor; dragging selects text and copies it.
    #[default]
    Select,
    /// Drop into the editor at the same spot (how tinynote used to behave).
    Edit,
}

/// Panel and overlay border treatment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BorderStyle {
    #[default]
    Rounded,
    Square,
    None,
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
}

impl StatusItem {
    fn parse(word: &str) -> Option<StatusItem> {
        Some(match word {
            "path" => StatusItem::Path,
            "name" | "file" | "filename" => StatusItem::Name,
            "mode" => StatusItem::Mode,
            "keys" | "hints" => StatusItem::Keys,
            "message" | "status" => StatusItem::Message,
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
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub notes_dir: PathBuf,
    pub attachments_dir: PathBuf,
    pub theme: Mode,
    /// User colour overrides, already applied on top of the theme's palette.
    pub palette: Palette,
    /// Widest the note column is ever drawn, in terminal columns. `0` fills
    /// the window.
    pub page_width: u16,
    pub borders: BorderStyle,
    pub bold_headings: bool,
    pub status_bar: bool,
    pub key_hints: bool,
    /// What the status bar shows, in the order given.
    pub status_bar_items: Vec<StatusItem>,
    pub autosave_ms: u64,
    pub tab_width: usize,
    /// Whether a filename follows its note's title. Off means tinynote never
    /// renames a file for you.
    pub rename_files: bool,
    pub table_style: TableStyle,
    pub preview_click: PreviewClick,
    /// Whether quick-open walks subfolders or offers only the current folder.
    pub quick_open_recursive: bool,
    /// Folders quick-open searches besides the notes dir — another vault, a
    /// work folder. Empty by default.
    pub quick_open_dirs: Vec<PathBuf>,
    /// What every key does, defaults included.
    pub keys: Keymap,
}

impl Default for Config {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let notes_dir = home.join("tinynote");
        Config {
            attachments_dir: notes_dir.join("attachments"),
            notes_dir,
            theme: Mode::Dark,
            palette: theme::DARK,
            page_width: 100,
            borders: BorderStyle::Rounded,
            bold_headings: true,
            status_bar: true,
            key_hints: true,
            status_bar_items: vec![StatusItem::Path, StatusItem::Message, StatusItem::Keys],
            autosave_ms: 500,
            tab_width: 2,
            rename_files: true,
            table_style: TableStyle::Auto,
            preview_click: PreviewClick::Select,
            quick_open_recursive: true,
            quick_open_dirs: Vec::new(),
            keys: Keymap::default(),
        }
    }
}

/// Path to the settings note, whether or not it exists yet.
pub fn settings_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("settings.md"))
}

pub fn config_dir() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("no home directory")?
        .join(".config")
        .join("tinynote"))
}

/// The pre-markdown config file. Only ever read to seed `settings.md`.
pub fn legacy_config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

impl Config {
    /// Read the settings note, writing it on first run. Values found in an old
    /// `config.toml` are carried across once, so an upgrade keeps its notes
    /// dir and theme without the user doing anything.
    pub fn load() -> Result<Config> {
        let path = settings_path()?;
        if !path.exists() {
            let seed = legacy_config_path()
                .ok()
                .and_then(|p| fs::read_to_string(p).ok())
                .unwrap_or_default();
            let migrated = Config::from_str(&seed);
            write_settings(&path, &migrated)?;
            return Ok(migrated);
        }
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let config = Config::from_str(&text);
        // A settings file written by an older tinynote is missing whatever has
        // been added since, and a setting you cannot see is a setting you do
        // not have. Rewriting is safe because the document is generated from
        // the config the file was just parsed into: every value survives.
        if !covers_every_setting(&text, &config) {
            let _ = write_settings(&path, &config);
        }
        Ok(config)
    }

    /// Push everything this config decides into the places that read it
    /// globally. Called at startup and on every settings save.
    pub fn apply(&self) {
        theme::set_palette(self.palette);
        theme::set_bold_headings(self.bold_headings);
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(text: &str) -> Config {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let mut c = Config::default();

        if let Some(v) = value(text, "notes_dir") {
            c.notes_dir = expand(&v, &home);
            c.attachments_dir = c.notes_dir.join("attachments");
        }
        // the environment wins: it is how a one-off session is pointed elsewhere
        if let Some(d) = std::env::var_os("TINYNOTE_DIR") {
            c.notes_dir = PathBuf::from(d);
            c.attachments_dir = c.notes_dir.join("attachments");
        }
        if let Some(v) = value(text, "attachments_dir") {
            c.attachments_dir = expand(&v, &home);
        }

        // anything but "light" is dark: a typo should leave the default
        // palette standing rather than flip it to the one that reads wrong on
        // the overwhelmingly more common dark terminal
        c.theme = match value(text, "theme").as_deref() {
            Some("light") => Mode::Light,
            _ => Mode::Dark,
        };
        c.palette = theme::base(c.theme);
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
        c.borders = match value(text, "borders").as_deref() {
            Some("square") => BorderStyle::Square,
            Some("none") => BorderStyle::None,
            Some("rounded") => BorderStyle::Rounded,
            _ => c.borders,
        };
        c.bold_headings = flag(text, "bold_headings", c.bold_headings);
        c.status_bar = flag(text, "status_bar", c.status_bar);
        c.key_hints = flag(text, "key_hints", c.key_hints);
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
        c.quick_open_recursive = match value(text, "quick_open").as_deref() {
            Some("folder") => false,
            Some("recursive") => true,
            _ => c.quick_open_recursive,
        };

        if let Some(v) = value(text, "autosave_ms").and_then(|v| v.parse::<u64>().ok()) {
            // no floor of 0: a zero here means "save on every keystroke", and
            // that is a legitimate thing to ask for
            c.autosave_ms = v.min(60_000);
        }
        if let Some(v) = value(text, "tab_width").and_then(|v| v.parse::<usize>().ok()) {
            c.tab_width = v.clamp(1, 16);
        }
        c.table_style = match value(text, "table_style").as_deref() {
            Some("scroll") => TableStyle::Scroll,
            Some("fit") => TableStyle::Fit,
            Some("wrap") => TableStyle::Wrap,
            Some("cards") => TableStyle::Cards,
            Some("auto") => TableStyle::Auto,
            _ => c.table_style,
        };
        c.quick_open_dirs = values(text, "quick_open_dirs")
            .iter()
            // one folder per line, or several on one line separated by commas
            .flat_map(|v| v.split(',').map(str::trim).map(String::from))
            .filter(|v| !v.is_empty())
            .map(|v| expand(&v, &home))
            .collect();
        c.keys = Keymap::from_settings(|key| value(text, key));
        c.preview_click = match value(text, "preview_click").as_deref() {
            Some("edit") => PreviewClick::Edit,
            Some("select") => PreviewClick::Select,
            _ => c.preview_click,
        };
        c
    }

    /// Create both directories; returns an error only if the notes dir can't exist.
    pub fn ensure_dirs(&self) -> Result<()> {
        fs::create_dir_all(&self.notes_dir)
            .with_context(|| format!("creating notes_dir {}", self.notes_dir.display()))?;
        Ok(())
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

        d.section("Appearance");
        d.row(
            "theme",
            match self.theme {
                Mode::Light => "light",
                Mode::Dark => "dark",
            },
            "dark · light",
        );
        d.row(
            "page_width",
            if self.page_width == 0 {
                "full".to_string()
            } else {
                self.page_width.to_string()
            },
            "columns of note, or full",
        );
        d.row(
            "borders",
            match self.borders {
                BorderStyle::Rounded => "rounded",
                BorderStyle::Square => "square",
                BorderStyle::None => "none",
            },
            "rounded · square · none",
        );
        d.row("bold_headings", yn(self.bold_headings), "yes · no");
        d.row("status_bar", yn(self.status_bar), "the bottom line at all");
        d.row("key_hints", yn(self.key_hints), "the shortcuts in it");
        d.row(
            "status_bar_items",
            self.status_bar_items
                .iter()
                .map(|i| i.word())
                .collect::<Vec<_>>()
                .join(", "),
            "path · name · mode · keys · message, in order",
        );

        d.section("Colours");
        d.note("#rrggbb · #rgb · red, brightblue · default");
        for (key, hint) in COLOUR_HINTS {
            let c = self.palette.get(key).unwrap_or(theme::DARK.accent);
            d.row(key, theme::color_to_string(c), hint);
        }

        d.section("Editing");
        d.row("autosave_ms", self.autosave_ms, "idle time before a save");
        d.row("tab_width", self.tab_width, "spaces one tab inserts");
        d.row(
            "rename_files",
            yn(self.rename_files),
            "filename follows title",
        );

        d.section("Reading");
        d.row(
            "table_style",
            match self.table_style {
                TableStyle::Auto => "auto",
                TableStyle::Scroll => "scroll",
                TableStyle::Fit => "fit",
                TableStyle::Wrap => "wrap",
                TableStyle::Cards => "cards",
            },
            "auto · scroll · fit · wrap · cards",
        );
        d.row(
            "preview_click",
            match self.preview_click {
                PreviewClick::Select => "select",
                PreviewClick::Edit => "edit",
            },
            "select · edit",
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
        d.note("^K · cmd+k · alt+k · f5 · none");
        for (key, spec, what) in self.keys.settings_rows() {
            d.row(key, spec, what);
        }
        d.finish()
    }
}

fn write_settings(path: &Path, config: &Config) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(path, config.to_document()).with_context(|| format!("writing {}", path.display()))
}

/// Does `text` already mention every setting tinynote has? Compared against
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

/// The one-line hint beside each colour, in the order the file lists them.
const COLOUR_HINTS: [(&str, &str); 9] = [
    ("accent", "h1, ticked boxes, the status bar"),
    ("bright", "the step that leads"),
    ("grey", "h2 and other structure"),
    ("dim", "markers, rules, quotes"),
    ("link", "links, which also underline"),
    ("code_bg", "behind code"),
    ("border", "panel borders"),
    ("danger", "the delete confirmation"),
    ("ground", "under a highlight"),
];

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
/// Both separators are accepted so the old TOML file can be read by the same
/// parser during the one-time migration, and so a `key = value` typed out of
/// habit still works.
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
    fn reads_markdown_list_settings() {
        let text = "## Folders\n\n- notes_dir: /a/b\n- theme: light\n";
        assert_eq!(value(text, "notes_dir").as_deref(), Some("/a/b"));
        assert_eq!(value(text, "theme").as_deref(), Some("light"));
        assert_eq!(value(text, "attachments_dir"), None);
    }

    #[test]
    fn the_old_toml_spelling_still_parses_for_the_migration() {
        let text = "# tinynote configuration.\nnotes_dir = \"/a/b\"  # trailing\n";
        assert_eq!(value(text, "notes_dir").as_deref(), Some("/a/b"));
        // a commented-out key is not a value
        assert_eq!(value("# notes_dir = \"/x\"\n", "notes_dir"), None);
        // …but a colour is not a comment
        assert_eq!(
            value("- accent: #00ff88\n", "accent").as_deref(),
            Some("#00ff88")
        );
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
            theme: Mode::Light,
            palette,
            page_width: 72,
            borders: BorderStyle::None,
            bold_headings: false,
            key_hints: false,
            autosave_ms: 1500,
            tab_width: 4,
            rename_files: false,
            table_style: TableStyle::Cards,
            preview_click: PreviewClick::Edit,
            quick_open_recursive: false,
            quick_open_dirs: vec![PathBuf::from("/vault"), PathBuf::from("/work")],
            ..Default::default()
        };
        let back = Config::from_str(&c.to_document());
        if std::env::var_os("TINYNOTE_DIR").is_none() {
            assert_eq!(back, c);
        }
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
        assert_eq!(back.theme, Mode::Light);
        if std::env::var_os("TINYNOTE_DIR").is_none() {
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
            .replace("- theme: dark", "- theme: dark  # mine");
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
        if std::env::var_os("TINYNOTE_DIR").is_none() {
            assert_eq!(back, c);
        }
    }

    #[test]
    fn the_defaults_are_tinynote_s_own_directory() {
        // an unset notes_dir must never land on ~/notes: tinynote renames
        // files to follow their titles, and that directory may be someone
        // else's markdown
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let c = Config::from_str("nothing set\n");
        if std::env::var_os("TINYNOTE_DIR").is_none() {
            assert_eq!(c.notes_dir, home.join("tinynote"));
        }
        assert_eq!(c.attachments_dir, c.notes_dir.join("attachments"));
    }

    #[test]
    fn attachments_follow_a_changed_notes_dir_unless_set_themselves() {
        let c = Config::from_str("- notes_dir: /vault\n");
        if std::env::var_os("TINYNOTE_DIR").is_none() {
            assert_eq!(c.attachments_dir, PathBuf::from("/vault/attachments"));
            let c = Config::from_str("- notes_dir: /vault\n- attachments_dir: /pics\n");
            assert_eq!(c.attachments_dir, PathBuf::from("/pics"));
        }
    }

    #[test]
    fn paths_expand_a_leading_tilde() {
        let home = PathBuf::from("/home/x");
        assert_eq!(expand("~/notes", &home), PathBuf::from("/home/x/notes"));
        assert_eq!(expand("/abs", &home), PathBuf::from("/abs"));
    }

    #[test]
    fn theme_defaults_to_dark_and_only_light_flips_it() {
        assert_eq!(Config::from_str("").theme, Mode::Dark);
        assert_eq!(Config::from_str("- theme: light").theme, Mode::Light);
        assert_eq!(Config::from_str("- theme: lite").theme, Mode::Dark);
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
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
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
    fn flags_take_the_words_a_person_would_type() {
        assert!(!Config::from_str("- key_hints: no").key_hints);
        assert!(!Config::from_str("- key_hints: false").key_hints);
        assert!(Config::from_str("- key_hints: yes").key_hints);
        // unset leaves the default
        assert!(Config::from_str("").key_hints);
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
}

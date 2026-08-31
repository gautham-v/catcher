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
            autosave_ms: 500,
            tab_width: 2,
            rename_files: true,
            table_style: TableStyle::Auto,
            preview_click: PreviewClick::Select,
            quick_open_recursive: true,
            quick_open_dirs: Vec::new(),
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
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            fs::write(&path, migrated.to_document())
                .with_context(|| format!("writing {}", path.display()))?;
            return Ok(migrated);
        }
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        Ok(Config::from_str(&text))
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

    /// The settings note as it should be written: every setting present, set
    /// to this config's value, with the prose that explains it. Generated
    /// rather than stored so a new setting appears in an existing install's
    /// file the first time it is regenerated.
    pub fn to_document(&self) -> String {
        let short = crate::index::short;
        let yn = |b: bool| if b { "yes" } else { "no" };
        let mut s = String::new();
        s.push_str(
            "# Settings\n\
             \n\
             This is tinynote's settings file, and it is just a note. Edit it here, \
             press ^S, and the change applies at once — no restart. Every setting is a \
             `- key: value` line. Delete a line to go back to its default.\n\
             \n\
             ## Folders\n\
             \n",
        );
        s.push_str(&format!("- notes_dir: {}\n", short(&self.notes_dir)));
        s.push_str(&format!(
            "- attachments_dir: {}\n",
            short(&self.attachments_dir)
        ));
        s.push_str(
            "\nWhere the `.md` files live, and where a pasted image is written. \
             `TINYNOTE_DIR` in the environment overrides `notes_dir`. Changing the notes \
             folder takes effect next time tinynote starts.\n\
             \n\
             ## Appearance\n\
             \n",
        );
        s.push_str(&format!(
            "- theme: {}\n",
            match self.theme {
                Mode::Light => "light",
                Mode::Dark => "dark",
            }
        ));
        s.push_str(&format!(
            "- page_width: {}\n",
            if self.page_width == 0 {
                "full".to_string()
            } else {
                self.page_width.to_string()
            }
        ));
        s.push_str(&format!(
            "- borders: {}\n",
            match self.borders {
                BorderStyle::Rounded => "rounded",
                BorderStyle::Square => "square",
                BorderStyle::None => "none",
            }
        ));
        s.push_str(&format!("- bold_headings: {}\n", yn(self.bold_headings)));
        s.push_str(&format!("- status_bar: {}\n", yn(self.status_bar)));
        s.push_str(&format!("- key_hints: {}\n", yn(self.key_hints)));
        s.push_str(
            "\n`theme` is `dark` or `light` — tinynote never paints a background, so this \
             only says which way your terminal's own background runs. `page_width` is the \
             widest the note column is ever drawn, in columns; `full` uses the whole \
             window. `borders` is `rounded`, `square` or `none`. `key_hints` is the row of \
             shortcuts along the bottom right.\n\
             \n\
             ## Colours\n\
             \n",
        );
        for key in theme::COLOR_KEYS {
            let c = self.palette.get(key).unwrap_or(theme::DARK.accent);
            s.push_str(&format!("- {key}: {}\n", theme::color_to_string(c)));
        }
        s.push_str(
            "\nEach takes `#rrggbb`, `#rgb`, an ANSI colour name (`red`, `brightblue`, …), \
             or `default` for your terminal's own. `accent` is the one hue — h1, a ticked \
             box, the status bar; `bright` leads, `grey` and `dim` recede; `ground` is what \
             a highlight sits on. Setting `theme` reloads all nine, so change the theme \
             first and your colours second.\n\
             \n\
             ## Editing\n\
             \n",
        );
        s.push_str(&format!("- autosave_ms: {}\n", self.autosave_ms));
        s.push_str(&format!("- tab_width: {}\n", self.tab_width));
        s.push_str(&format!("- rename_files: {}\n", yn(self.rename_files)));
        s.push_str(
            "\nHow long after you stop typing a note saves, how many spaces `tab` inserts, \
             and whether a filename follows its note's title. Turn `rename_files` off and \
             tinynote never renames a file for you — the same rule it already follows for \
             folders outside your notes dir.\n\
             \n\
             ## Reading\n\
             \n",
        );
        s.push_str(&format!(
            "- table_style: {}\n",
            match self.table_style {
                TableStyle::Auto => "auto",
                TableStyle::Scroll => "scroll",
                TableStyle::Fit => "fit",
                TableStyle::Wrap => "wrap",
                TableStyle::Cards => "cards",
            }
        ));
        s.push_str(&format!(
            "- preview_click: {}\n",
            match self.preview_click {
                PreviewClick::Select => "select",
                PreviewClick::Edit => "edit",
            }
        ));
        s.push_str(&format!(
            "- quick_open: {}\n",
            if self.quick_open_recursive {
                "recursive"
            } else {
                "folder"
            }
        ));
        if self.quick_open_dirs.is_empty() {
            s.push_str("- quick_open_dirs:\n");
        } else {
            for dir in &self.quick_open_dirs {
                s.push_str(&format!("- quick_open_dirs: {}\n", short(dir)));
            }
        }
        s.push_str(
            "\n`table_style` decides what happens to a table wider than the page. `scroll` \
             keeps every column readable and lets the table itself pan sideways — ← and → \
             in the preview, or a sideways scroll; `fit` squeezes the columns and cuts \
             cells with an ellipsis; `wrap` keeps the whole table on the page and lets a \
             cell run onto as many lines as it needs; `cards` gives every row its own \
             labelled block; and `auto` leaves a table that fits alone and scrolls one \
             that doesn't. `preview_click` is what a plain click in the preview does — \
             `select` starts a selection you can drag and copy, `edit` jumps into the \
             editor. `quick_open` (^O) either walks subfolders or offers only this \
             folder.\n",
        );
        s
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

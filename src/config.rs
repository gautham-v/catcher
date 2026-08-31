//! `~/.config/tinynote/config.toml` — two keys, a tiny hand-rolled parser.
//!
//! The file is written with commented defaults the first time tinynote runs, so
//! there is always something to open and edit. `TINYNOTE_DIR` still wins over
//! `notes_dir`, which keeps `TINYNOTE_DIR=/tmp/x cargo run` working.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

const TEMPLATE: &str = "\
# tinynote configuration.
# Paths may start with ~/. Delete a line to fall back to its default.

# Where the .md notes live. Overridden by the TINYNOTE_DIR environment variable.
# notes_dir = \"~/notes\"

# Where pasted images are written. Defaults to <notes_dir>/attachments.
# attachments_dir = \"~/notes/attachments\"
";

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub notes_dir: PathBuf,
    pub attachments_dir: PathBuf,
}

/// Path to the config file, whether or not it exists yet.
pub fn config_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("no home directory")?
        .join(".config")
        .join("tinynote")
        .join("config.toml"))
}

impl Config {
    /// Read the config file, writing the commented template on first run.
    /// A missing or unreadable file is not an error — the defaults stand.
    pub fn load() -> Result<Config> {
        let path = config_path()?;
        if !path.exists() {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            fs::write(&path, TEMPLATE).with_context(|| format!("writing {}", path.display()))?;
        }
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        Ok(Config::from_str(&text))
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(text: &str) -> Config {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let notes_dir = match std::env::var_os("TINYNOTE_DIR") {
            Some(d) => PathBuf::from(d),
            None => value(text, "notes_dir")
                .map(|v| expand(&v, &home))
                .unwrap_or_else(|| home.join("notes")),
        };
        let attachments_dir = value(text, "attachments_dir")
            .map(|v| expand(&v, &home))
            .unwrap_or_else(|| notes_dir.join("attachments"));
        Config {
            notes_dir,
            attachments_dir,
        }
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
}

/// `key = "value"` (or bare value), first match wins. `#` starts a comment,
/// except inside quotes — a path may legitimately contain one.
fn value(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let line = strip_comment(line);
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.trim() != key {
            continue;
        }
        let v = v.trim().trim_matches(|c| c == '"' || c == '\'').trim();
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }
    None
}

/// Drop a trailing `# comment`, ignoring `#` inside a quoted value.
fn strip_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (None, '"') | (None, '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '#') => return &line[..i],
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
    fn reads_keys_and_ignores_comments() {
        let text = "# notes_dir = \"/commented/out\"\nnotes_dir = \"/a/b\"  # trailing\n";
        assert_eq!(value(text, "notes_dir").as_deref(), Some("/a/b"));
        assert_eq!(value(text, "attachments_dir"), None);
    }

    #[test]
    fn a_hash_inside_a_quoted_path_is_not_a_comment() {
        let text = "notes_dir = \"/a/c#1/notes\"  # mine\n";
        assert_eq!(value(text, "notes_dir").as_deref(), Some("/a/c#1/notes"));
    }

    #[test]
    fn the_default_template_parses_to_the_defaults() {
        assert_eq!(value(TEMPLATE, "notes_dir"), None);
        assert_eq!(value(TEMPLATE, "attachments_dir"), None);
    }

    #[test]
    fn attachments_default_under_notes_dir() {
        let home = PathBuf::from("/home/x");
        assert_eq!(expand("~/notes", &home), PathBuf::from("/home/x/notes"));
        assert_eq!(expand("/abs", &home), PathBuf::from("/abs"));
    }

    #[test]
    fn links_are_relative_inside_the_notes_dir() {
        let c = Config {
            notes_dir: PathBuf::from("/n"),
            attachments_dir: PathBuf::from("/n/attachments"),
        };
        assert_eq!(
            c.link_for(Path::new("/n/attachments/a.png")),
            "attachments/a.png"
        );
        assert_eq!(c.link_for(Path::new("/other/a.png")), "/other/a.png");
    }
}

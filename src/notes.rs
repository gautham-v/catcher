use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct Note {
    pub path: PathBuf,
    pub content: String,
    pub modified: SystemTime,
}

impl Note {
    pub fn title(&self) -> String {
        title_of(&self.content)
    }

    pub fn snippet(&self) -> String {
        let mut lines = self.content.lines().filter(|l| !l.trim().is_empty());
        lines.next(); // skip the title line
        lines
            .next()
            .map(|l| l.trim_start_matches(['#', '>', '-', ' ']).to_string())
            .unwrap_or_default()
    }
}

pub fn title_of(content: &str) -> String {
    content
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim().trim_start_matches('#').trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "Untitled".to_string())
}

pub fn slug(title: &str) -> String {
    let s: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let s = s
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let s: String = s.chars().take(60).collect();
    if s.is_empty() {
        "untitled".to_string()
    } else {
        s
    }
}

pub fn notes_dir() -> Result<PathBuf> {
    let dir = match std::env::var_os("TINYNOTE_DIR") {
        Some(d) => PathBuf::from(d),
        None => dirs::home_dir().context("no home directory")?.join("notes"),
    };
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// All .md files in the notes dir, newest modification first.
pub fn load_all(dir: &Path) -> Result<Vec<Note>> {
    let mut notes = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") || !path.is_file() {
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue, // not UTF-8 markdown; leave it alone
        };
        let modified = fs::metadata(&path)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        notes.push(Note {
            path,
            content,
            modified,
        });
    }
    notes.sort_by_key(|n| std::cmp::Reverse(n.modified));
    Ok(notes)
}

/// A fresh path for a new note, never clobbering an existing file.
pub fn unique_path(dir: &Path, title: &str, keep: Option<&Path>) -> PathBuf {
    let base = slug(title);
    let mut n = 1;
    loop {
        let name = if n == 1 {
            format!("{base}.md")
        } else {
            format!("{base}-{n}.md")
        };
        let candidate = dir.join(name);
        if !candidate.exists() || Some(candidate.as_path()) == keep {
            return candidate;
        }
        n += 1;
    }
}

/// Write the note's content, renaming the file to follow its title.
/// Returns the (possibly new) path.
pub fn save(dir: &Path, note: &Note) -> Result<PathBuf> {
    let target = unique_path(dir, &note.title(), Some(&note.path));
    fs::write(&note.path, &note.content)?;
    if target != note.path {
        fs::rename(&note.path, &target)?;
        return Ok(target);
    }
    Ok(note.path.clone())
}

pub fn delete(note: &Note) -> Result<()> {
    fs::remove_file(&note.path)?;
    Ok(())
}

pub fn create(dir: &Path) -> Result<Note> {
    let path = unique_path(dir, "untitled", None);
    fs::write(&path, "")?;
    Ok(Note {
        path,
        content: String::new(),
        modified: SystemTime::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles() {
        assert_eq!(title_of("# Hello world\nbody"), "Hello world");
        assert_eq!(title_of("\n\nplain line"), "plain line");
        assert_eq!(title_of(""), "Untitled");
        assert_eq!(title_of("###\n"), "Untitled");
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Hello, World!"), "hello-world");
        assert_eq!(slug("  "), "untitled");
        assert_eq!(slug("café ☕ notes"), "café-notes");
    }
}

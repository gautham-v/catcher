use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct Note {
    pub path: PathBuf,
    pub content: String,
    pub modified: SystemTime,
    /// The note's title as it stood on disk when the file was last read or
    /// written. The filename is considered to be *tracking* the title while it
    /// still equals the slug of this; once the user renames the file by hand
    /// the two diverge and the automatic rename stops for good. Nothing is
    /// stored beside the file: the check is the filename against the content.
    pub disk_title: String,
}

impl Note {
    pub fn title(&self) -> String {
        title_of(&self.content)
    }

    /// The file's own name, when it is no longer tracking the title — that is,
    /// when the user renamed the file by hand and the two have diverged.
    /// `None` while the filename still follows the title, where showing it
    /// would only repeat the title back in slug form.
    pub fn detached_name(&self) -> Option<String> {
        let name = self.path.file_name()?.to_str()?;
        let stem = self.path.file_stem()?.to_str()?;
        // measured against the title as it stands on disk, the same thing
        // `save` decides tracking against, so editing a heading doesn't read
        // as a detachment for the half second before the autosave renames
        (!tracks(stem, &self.disk_title)).then(|| name.to_string())
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

/// Is `stem` the name this title would have been given? `unique_path` appends
/// `-2`, `-3` … on a collision, so those count as tracking too.
pub fn tracks(stem: &str, title: &str) -> bool {
    let base = slug(title);
    if stem == base {
        return true;
    }
    match stem.strip_prefix(&base).and_then(|r| r.strip_prefix('-')) {
        Some(n) => !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

/// Write PNG bytes into `attachments_dir` under a name derived from the note's
/// title, never clobbering an existing file. Returns the file's path.
pub fn write_attachment(attachments_dir: &Path, title: &str, png: &[u8]) -> Result<PathBuf> {
    fs::create_dir_all(attachments_dir)
        .with_context(|| format!("creating {}", attachments_dir.display()))?;
    let base = slug(title);
    let mut n = 1;
    let path = loop {
        let candidate = attachments_dir.join(format!("{base}-{n}.png"));
        if !candidate.exists() {
            break candidate;
        }
        n += 1;
    };
    fs::write(&path, png).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
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
            disk_title: title_of(&content),
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

/// Write the note's content, and rename the file to follow its title *only*
/// while the filename is still tracking it (see [`Note::disk_title`]).
/// `allow_rename` is false for sessions rooted outside the notes dir, where
/// foreign filenames must never move. Updates `note.path`/`disk_title`.
pub fn save(dir: &Path, note: &mut Note, allow_rename: bool) -> Result<PathBuf> {
    fs::write(&note.path, &note.content)?;
    let tracking = note
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|stem| tracks(stem, &note.disk_title));
    note.disk_title = note.title();
    note.modified = SystemTime::now();
    if allow_rename && tracking {
        let target = unique_path(dir, &note.title(), Some(&note.path));
        if target != note.path {
            fs::rename(&note.path, &target)?;
            note.path = target;
        }
    }
    Ok(note.path.clone())
}

/// Rename the file behind a note to `stem`.md, never clobbering. After this the
/// filename no longer tracks the title, so the automatic rename stays off.
pub fn rename_file(note: &mut Note, stem: &str) -> Result<PathBuf> {
    let dir = note
        .path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let stem = stem.trim();
    let stem = if stem.is_empty() { "untitled" } else { stem };
    let stem = stem.trim_end_matches(".md");
    let mut n = 1;
    let target = loop {
        let name = if n == 1 {
            format!("{stem}.md")
        } else {
            format!("{stem}-{n}.md")
        };
        let candidate = dir.join(name);
        if !candidate.exists() || candidate == note.path {
            break candidate;
        }
        n += 1;
    };
    if target != note.path {
        fs::rename(&note.path, &target)
            .with_context(|| format!("renaming to {}", target.display()))?;
        note.path = target;
    }
    Ok(note.path.clone())
}

pub fn delete(note: &Note) -> Result<()> {
    fs::remove_file(&note.path)?;
    Ok(())
}

pub fn create(dir: &Path) -> Result<Note> {
    create_with(dir, String::new())
}

/// A new note holding `content`, named after its first line.
pub fn create_with(dir: &Path, content: String) -> Result<Note> {
    let title = title_of(&content);
    let path = unique_path(dir, &title, None);
    fs::write(&path, &content).with_context(|| format!("writing {}", path.display()))?;
    Ok(Note {
        path,
        content,
        modified: SystemTime::now(),
        disk_title: title,
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
    fn a_filename_tracks_its_title_until_it_is_renamed_by_hand() {
        assert!(tracks("groceries", "Groceries"));
        assert!(tracks("groceries", "groceries!"));
        // unique_path's collision suffix still counts as tracking
        assert!(tracks("groceries-2", "Groceries"));
        assert!(tracks("groceries-10", "Groceries"));
        // an explicit rename detaches the two
        assert!(!tracks("shopping", "Groceries"));
        assert!(!tracks("groceries-final", "Groceries"));
        assert!(!tracks("groceries-", "Groceries"));
    }

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tinynote-test-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn note_at(dir: &Path, name: &str, content: &str) -> Note {
        let path = dir.join(name);
        fs::write(&path, content).unwrap();
        Note {
            path,
            content: content.to_string(),
            modified: SystemTime::now(),
            disk_title: title_of(content),
        }
    }

    #[test]
    fn a_tracking_file_follows_the_title() {
        let dir = tmpdir("tracking");
        let mut n = note_at(&dir, "groceries.md", "# Groceries\n");
        n.content = "# Shopping\n".into();
        save(&dir, &mut n, true).unwrap();
        assert_eq!(n.path, dir.join("shopping.md"));
        assert!(!dir.join("groceries.md").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_detached_file_never_follows_the_title_again() {
        let dir = tmpdir("detached");
        // the filename does not match the title: the user renamed it
        let mut n = note_at(&dir, "keep-this-name.md", "# Groceries\n");
        n.content = "# Shopping\n".into();
        save(&dir, &mut n, true).unwrap();
        assert_eq!(n.path, dir.join("keep-this-name.md"));
        // and it stays detached on the next edit too
        n.content = "# Anything Else\n".into();
        save(&dir, &mut n, true).unwrap();
        assert_eq!(n.path, dir.join("keep-this-name.md"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_explicit_rename_detaches_a_tracking_file() {
        let dir = tmpdir("rename");
        let mut n = note_at(&dir, "groceries.md", "# Groceries\n");
        rename_file(&mut n, "market list").unwrap();
        assert_eq!(n.path, dir.join("market list.md"));
        n.content = "# Shopping\n".into();
        save(&dir, &mut n, true).unwrap();
        assert_eq!(n.path, dir.join("market list.md"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn renames_are_collision_safe_and_never_double_the_extension() {
        let dir = tmpdir("collide");
        fs::write(dir.join("taken.md"), "x").unwrap();
        let mut n = note_at(&dir, "groceries.md", "# Groceries\n");
        rename_file(&mut n, "taken.md").unwrap();
        assert_eq!(n.path, dir.join("taken-2.md"));
        // renaming to its own name is a no-op, not a -2
        rename_file(&mut n, "taken-2").unwrap();
        assert_eq!(n.path, dir.join("taken-2.md"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_is_skipped_entirely_outside_the_notes_dir() {
        let dir = tmpdir("foreign");
        // this filename *is* tracking its title, so only the flag stops it
        let mut n = note_at(&dir, "some-obsidian-note.md", "# Some Obsidian Note\n");
        n.content = "# Renamed Heading\n".into();
        save(&dir, &mut n, false).unwrap();
        assert_eq!(n.path, dir.join("some-obsidian-note.md"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_detached_filename_is_shown_beside_the_title() {
        let dir = PathBuf::from("/notes");
        let mut n = Note {
            path: dir.join("groceries.md"),
            content: "# Groceries\n".into(),
            modified: SystemTime::now(),
            disk_title: "Groceries".into(),
        };
        assert_eq!(n.detached_name(), None);
        // the collision suffix is still tracking
        n.path = dir.join("groceries-2.md");
        assert_eq!(n.detached_name(), None);
        // renamed by hand: the filename is worth showing
        n.path = dir.join("hello.md");
        assert_eq!(n.detached_name().as_deref(), Some("hello.md"));
    }

    #[test]
    fn editing_the_heading_does_not_read_as_a_detachment() {
        let dir = PathBuf::from("/notes");
        let n = Note {
            path: dir.join("groceries.md"),
            // typed but not yet saved: the file still tracks its disk title
            content: "# Groceries and more\n".into(),
            modified: SystemTime::now(),
            disk_title: "Groceries".into(),
        };
        assert_eq!(n.detached_name(), None);
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Hello, World!"), "hello-world");
        assert_eq!(slug("  "), "untitled");
        assert_eq!(slug("café ☕ notes"), "café-notes");
    }
}

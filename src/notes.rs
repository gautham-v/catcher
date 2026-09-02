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
    #[cfg(test)]
    pub fn detached_name(&self) -> Option<String> {
        let name = self.path.file_name()?.to_str()?;
        let stem = self.path.file_stem()?.to_str()?;
        // measured against the title as it stands on disk, the same thing
        // `save` decides tracking against, so editing a heading doesn't read
        // as a detachment for the half second before the autosave renames
        (!tracks(stem, &self.disk_title)).then(|| name.to_string())
    }
}

/// The note's title: its first line of prose. YAML front matter is stepped
/// over — a note that opens with `---\ntype: log\n---` is not called “---”,
/// and the heading under it is what a person would read as the title.
pub fn title_of(content: &str) -> String {
    body_after_front_matter(content)
        .lines()
        .find(|l| !l.trim().is_empty() && !is_rule(l))
        .map(|l| l.trim().trim_start_matches('#').trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "Untitled".to_string())
}

/// A line of nothing but `-`, `*`, `_` or `=`: a horizontal rule, which is
/// never a title however high up the note it sits.
fn is_rule(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 3 && t.chars().all(|c| matches!(c, '-' | '*' | '_' | '='))
}

/// The index of the line that *closes* a leading `---` … `---` block, both
/// fences counted as part of it. `None` when the note doesn't open with a
/// fence, or opens with one that never closes — an unterminated fence was
/// never front matter, it was a rule and then some prose.
///
/// It takes an iterator of lines rather than a `&str` so the editor, which
/// already holds the buffer split into a `Vec<String>`, can ask without
/// joining it back up; and so `content.lines()` deals with CRLF for free,
/// since `str::lines` drops the `\r` on its own.
///
/// The close is `starts_with("---")` and not `== "---"` because that is what
/// the older scan did: a `----` closes a block, and tightening it now would
/// silently retitle notes people already have on disk.
pub fn front_matter_end<'a>(mut lines: impl Iterator<Item = &'a str>) -> Option<usize> {
    if lines.next()? != "---" {
        return None;
    }
    lines.position(|l| l.starts_with("---")).map(|i| i + 1)
}

/// The byte range a leading front matter block occupies, fences included, so
/// a caller can slice it off and know exactly how much it cut.
pub fn front_matter_range(content: &str) -> Option<std::ops::Range<usize>> {
    let last = front_matter_end(content.lines())?;
    // split_inclusive, not lines(): each line's own ending has to be counted
    // as it is in the file, `\r\n` included, or the body starts a byte early
    // on every line of a CRLF note
    let end = content
        .split_inclusive('\n')
        .take(last + 1)
        .map(str::len)
        .sum();
    Some(0..end)
}

/// How many top-level keys the front matter declares — `2 properties`, the
/// thing the status bar can be asked to show. Zero when there is no block.
///
/// An indented line belongs to the value above it (a list item, a nested map)
/// and a line with no colon is not a key at all, so neither is counted; a
/// comment line isn't a key either.
pub fn property_count(content: &str) -> usize {
    let Some(last) = front_matter_end(content.lines()) else {
        return 0;
    };
    content
        .lines()
        .take(last)
        .skip(1)
        .filter(|l| !l.starts_with([' ', '\t', '-', '#']))
        .filter_map(|l| l.split_once(':'))
        .filter(|(k, _)| !k.trim().is_empty())
        .count()
}

/// Everything after a leading `---` … `---` block, or the whole thing when
/// there isn't one. Only a `---` on the very first line opens front matter; a
/// horizontal rule further down is just a rule.
pub fn body_after_front_matter(content: &str) -> &str {
    match front_matter_range(content) {
        Some(r) => &content[r.end..],
        None => content,
    }
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

/// One note, read from a path that may be anywhere at all. Quick-open uses
/// this to pull a file in from another folder, and the settings note is loaded
/// the same way — it is a note like any other.
pub fn load_one(path: &Path) -> Result<Note> {
    let content =
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let modified = fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    Ok(Note {
        disk_title: title_of(&content),
        path: path.to_path_buf(),
        content,
        modified,
    })
}

/// The first `base.md`, `base-2.md`, `base-3.md` … that is free, treating
/// `keep` as free so a note can be "renamed" to the name it already has.
fn free_path(dir: &Path, base: &str, keep: Option<&Path>) -> PathBuf {
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

/// A fresh path for a new note, never clobbering an existing file.
pub fn unique_path(dir: &Path, title: &str, keep: Option<&Path>) -> PathBuf {
    free_path(dir, &slug(title), keep)
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
    let target = free_path(&dir, stem, Some(&note.path));
    if target != note.path {
        fs::rename(&note.path, &target)
            .with_context(|| format!("renaming to {}", target.display()))?;
        note.path = target;
    }
    Ok(note.path.clone())
}

/// Move a note into another folder, keeping its filename — or the nearest
/// free one, if that folder already has a note by the name.
pub fn move_file(note: &mut Note, dir: &Path) -> Result<PathBuf> {
    let stem = note
        .path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".to_string());
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let target = free_path(dir, &stem, Some(&note.path));
    if target != note.path {
        fs::rename(&note.path, &target)
            .with_context(|| format!("moving to {}", target.display()))?;
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

/// A new note at an exact filename, never clobbering. A `[[wikilink]]` creates
/// a note this way because the link target *is* the filename: slugging
/// `[[Story Matrix]]` down to `story-matrix.md` would leave the very link that
/// made the note pointing at nothing. `save` then leaves the name alone too —
/// `tracks` compares the stem against `slug(title)`, so a filename that is not
/// a slug reads as one the user chose and the automatic rename stays off.
pub fn create_named(dir: &Path, stem: &str, content: String) -> Result<Note> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let stem = stem.trim().trim_end_matches(".md").trim();
    let stem = if stem.is_empty() { "untitled" } else { stem };
    let path = free_path(dir, stem, None);
    fs::write(&path, &content).with_context(|| format!("writing {}", path.display()))?;
    Ok(Note {
        disk_title: title_of(&content),
        path,
        content,
        modified: SystemTime::now(),
    })
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
    fn front_matter_is_stepped_over_not_read_as_the_title() {
        let md = "---\ntype: log\nupdated: 2026-08-25\n---\n\n# Job Application Log\nbody\n";
        assert_eq!(title_of(md), "Job Application Log");

        // a rule further down is a rule, not front matter
        assert_eq!(title_of("# Title\n\n---\n\nmore"), "Title");
        // an unterminated fence is not front matter — but a bare rule is
        // still not a title, so the first real line wins
        assert_eq!(title_of("---\nnot closed\n"), "not closed");
        assert_eq!(title_of("***\n\n# Real\n"), "Real");
    }

    #[test]
    fn front_matter_is_found_only_when_it_opens_the_note_and_closes() {
        let md = "---\ntype: log\n---\n# Title\n";
        assert_eq!(front_matter_end(md.lines()), Some(2));
        assert_eq!(front_matter_range(md), Some(0..18));
        assert_eq!(&md[18..], "# Title\n");
        // a blank line before the fence means the note did not open with one
        assert_eq!(front_matter_end("\n---\na: b\n---\n".lines()), None);
        assert_eq!(front_matter_range("# Title\nbody\n"), None);
    }

    #[test]
    fn an_unterminated_front_matter_fence_is_not_front_matter_at_all() {
        assert_eq!(front_matter_end("---\ntype: log\nbody\n".lines()), None);
        assert_eq!(front_matter_range("---\ntype: log\n"), None);
        // and the whole thing is still the body, as it always was
        assert_eq!(
            body_after_front_matter("---\nnot closed\n"),
            "---\nnot closed\n"
        );
    }

    #[test]
    fn a_rule_further_down_the_note_is_never_front_matter() {
        let md = "# Title\n\nsome prose\n\n---\n\nmore\n";
        assert_eq!(front_matter_range(md), None);
        assert_eq!(body_after_front_matter(md), md);
    }

    #[test]
    fn an_empty_front_matter_block_is_still_a_block() {
        // the older scan never looked at the first line after the opening
        // fence, so it could not close this; the line scan can, and `title_of`
        // answers "body" either way
        let md = "---\n---\nbody\n";
        assert_eq!(front_matter_end(md.lines()), Some(1));
        assert_eq!(body_after_front_matter(md), "body\n");
        assert_eq!(title_of(md), "body");
        assert_eq!(property_count(md), 0);
    }

    #[test]
    fn the_front_matter_byte_range_counts_crlf_endings_as_the_file_has_them() {
        let md = "---\r\ntype: log\r\n---\r\nbody\r\n";
        assert_eq!(front_matter_end(md.lines()), Some(2));
        let r = front_matter_range(md).unwrap();
        assert_eq!(&md[r.end..], "body\r\n");
        // one byte per `\r` more than the same note with unix endings
        assert_eq!(r.end, "---\ntype: log\n---\n".len() + 3);
    }

    #[test]
    fn properties_are_the_top_level_keys_and_nothing_indented_under_them() {
        let md = "---\ntype: log\ntags:\n  - work\n  - notes\nnested:\n  key: value\n# a comment\n---\nbody\n";
        // type, tags and nested; the list items, the nested key and the
        // comment all belong to something above them
        assert_eq!(property_count(md), 3);
        assert_eq!(property_count("---\na: 1\nb: 2\n---\n"), 2);
        // a line with no colon is not a key
        assert_eq!(property_count("---\njust prose\na: 1\n---\n"), 1);
    }

    #[test]
    fn a_note_without_front_matter_has_no_properties_to_count() {
        assert_eq!(property_count("# Title\n\na: not a property\n"), 0);
        assert_eq!(property_count("---\nunterminated: yes\n"), 0);
        assert_eq!(property_count(""), 0);
    }

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
        let dir = std::env::temp_dir().join(format!("catcher-test-{name}"));
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
    fn a_move_keeps_the_filename_and_dodges_a_collision() {
        let dir = tmpdir("move");
        let work = dir.join("work");
        let mut n = note_at(&dir, "spec.md", "# Spec\n");
        // the folder need not exist yet
        move_file(&mut n, &work).unwrap();
        assert_eq!(n.path, work.join("spec.md"));
        assert!(!dir.join("spec.md").exists());
        assert_eq!(fs::read_to_string(&n.path).unwrap(), "# Spec\n");
        // moving back beside a note of the same name takes the next free name
        fs::write(dir.join("spec.md"), "other").unwrap();
        move_file(&mut n, &dir).unwrap();
        assert_eq!(n.path, dir.join("spec-2.md"));
        // moving into the folder it is already in is a no-op
        move_file(&mut n, &dir).unwrap();
        assert_eq!(n.path, dir.join("spec-2.md"));
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
    fn a_note_created_from_a_wikilink_keeps_the_links_filename() {
        let dir = tmpdir("named");
        let mut n = create_named(&dir, "Story Matrix", "# Story Matrix\n".into()).unwrap();
        assert_eq!(n.path, dir.join("Story Matrix.md"));
        // and a save that is allowed to rename leaves it alone: the name is
        // not the slug of the title, so it reads as one the user chose — which
        // is what keeps the `[[Story Matrix]]` that made it pointing at it
        save(&dir, &mut n, true).unwrap();
        assert_eq!(n.path, dir.join("Story Matrix.md"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_notes_created_from_the_same_link_name_do_not_clobber() {
        let dir = tmpdir("named-twice");
        let a = create_named(&dir, "Story Matrix", "# One\n".into()).unwrap();
        let b = create_named(&dir, "Story Matrix.md", "# Two\n".into()).unwrap();
        assert_eq!(a.path, dir.join("Story Matrix.md"));
        assert_eq!(b.path, dir.join("Story Matrix-2.md"));
        assert_eq!(fs::read_to_string(&a.path).unwrap(), "# One\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Hello, World!"), "hello-world");
        assert_eq!(slug("  "), "untitled");
        assert_eq!(slug("café ☕ notes"), "café-notes");
    }
}

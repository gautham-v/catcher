//! The quick-open index: every `.md` file under the notes dir (and the
//! session's own root, when it is somewhere else), with the recently opened
//! ones first. Unlike the palette — which searches the notes *loaded* for this
//! session, all of them from one flat folder — this walks subfolders, so a
//! vault organised into directories is reachable from anywhere in it.

use crate::notes::title_of;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A file the quick-open list can offer. Titles are read from the first few
/// hundred bytes of the file, never the whole thing: a vault is walked on
/// every open and must stay instant.
pub struct Entry {
    pub path: PathBuf,
    pub title: String,
    /// Path relative to the root it was found under — what a query is matched
    /// against besides the title, so `applications/log` finds it.
    pub rel: String,
    /// The folder to show beside the title, so two notes with the same name
    /// are told apart. Relative for a note under a root, `~/`-shortened for
    /// one reached through the recents list.
    pub folder: String,
    pub modified: SystemTime,
}

/// Stop conditions for the walk. A vault of a few thousand notes is scanned in
/// milliseconds; past that the list stops being something to eyeball anyway.
const MAX_FILES: usize = 8000;
const MAX_DEPTH: usize = 10;
/// Enough for a title line under any reasonable front matter.
const TITLE_BYTES: usize = 4096;

/// Directories never worth walking into.
fn skip_dir(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "node_modules" | "target" | "attachments")
}

/// The title of the note at `path`, without reading the whole file.
fn title_at(path: &Path) -> String {
    let mut buf = vec![0u8; TITLE_BYTES];
    let read = fs::File::open(path)
        .and_then(|mut f| f.read(&mut buf))
        .unwrap_or(0);
    let head = String::from_utf8_lossy(&buf[..read]);
    // front matter is not a title: skip a leading `---` block if the head has one
    let body = match head.strip_prefix("---\n") {
        Some(rest) => match rest.split_once("\n---") {
            Some((_, after)) => after,
            None => rest,
        },
        None => &head,
    };
    title_of(body)
}

/// Shorten a path for display, `~/` for the home directory.
pub fn short(path: &Path) -> String {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    match path.strip_prefix(&home) {
        Ok(rel) => format!("~/{}", rel.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Every `.md` file under `roots`, plus every note in `recent` wherever it
/// lives, ordered with the recent ones first (in the order given) and the rest
/// by modification time, newest first.
///
/// Recents are entries and not merely a sort key: a note you opened last week
/// from a folder that is not in `roots` is exactly the note you are most
/// likely to reach for, and having it rank well in a list it was never in is
/// no use at all.
pub fn scan(roots: &[PathBuf], recent: &[PathBuf]) -> Vec<Entry> {
    let mut seen: HashMap<PathBuf, ()> = HashMap::new();
    let mut entries: Vec<Entry> = Vec::new();
    for root in roots {
        let root = fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        let mut stack = vec![(root.clone(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            if depth > MAX_DEPTH || entries.len() >= MAX_FILES {
                continue;
            }
            let Ok(read) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in read.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if is_dir {
                    if !skip_dir(name) {
                        stack.push((path, depth + 1));
                    }
                    continue;
                }
                if name.starts_with('.') || !name.ends_with(".md") {
                    continue;
                }
                if entries.len() >= MAX_FILES || seen.insert(path.clone(), ()).is_some() {
                    continue;
                }
                let modified = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                let rel = path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned();
                let folder = std::path::Path::new(&rel)
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
                    .filter(|p| !p.is_empty())
                    .unwrap_or_else(|| ".".to_string());
                entries.push(Entry {
                    title: title_at(&path),
                    path,
                    rel,
                    folder,
                    modified,
                });
            }
        }
    }

    // anything opened before that the walk did not reach — another vault, a
    // file passed on the command line once — is still worth offering
    for path in recent {
        if entries.len() >= MAX_FILES {
            break;
        }
        let path = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        if !path.is_file() || seen.insert(path.clone(), ()).is_some() {
            continue;
        }
        let modified = fs::metadata(&path)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        entries.push(Entry {
            title: title_at(&path),
            // outside every root, so the whole path is what a query has to
            // match against, and the folder is what is worth showing
            rel: short(&path),
            folder: short(path.parent().unwrap_or(&path)),
            path,
            modified,
        });
    }

    let rank: HashMap<&Path, usize> = recent
        .iter()
        .enumerate()
        .map(|(i, p)| (p.as_path(), i))
        .collect();
    entries.sort_by(|a, b| {
        let ra = rank.get(a.path.as_path()).copied();
        let rb = rank.get(b.path.as_path()).copied();
        match (ra, rb) {
            // recently opened first, in the order they were opened
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => b.modified.cmp(&a.modified),
        }
    });
    entries
}

/// How many paths the recents file keeps. Long enough to cover a week of
/// hopping about, short enough to stay a file you could read yourself.
const MAX_RECENT: usize = 100;

fn recent_path() -> Option<PathBuf> {
    crate::config::config_dir().ok().map(|d| d.join("recent"))
}

/// The recently opened notes, most recent first. Paths that no longer exist
/// are dropped as they are read, so a deleted note leaves nothing behind.
pub fn load_recent() -> Vec<PathBuf> {
    let Some(path) = recent_path() else {
        return Vec::new();
    };
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .map(|l| PathBuf::from(l.trim()))
        .filter(|p| !p.as_os_str().is_empty() && p.exists())
        .take(MAX_RECENT)
        .collect()
}

/// Move `path` to the front of `recent` and write the file back. Failures are
/// silent: the recents list is a convenience, never something to interrupt a
/// note-taking session over.
pub fn push_recent(recent: &mut Vec<PathBuf>, path: &Path) {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    recent.retain(|p| p != &path);
    recent.insert(0, path);
    recent.truncate(MAX_RECENT);
    if let Some(file) = recent_path() {
        if let Some(dir) = file.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let body: String = recent
            .iter()
            .map(|p| format!("{}\n", p.display()))
            .collect();
        let _ = fs::write(file, body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tinynote-index-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_walk_reaches_subfolders_and_skips_dotted_ones() {
        let dir = tmpdir("walk");
        fs::create_dir_all(dir.join("applications")).unwrap();
        fs::create_dir_all(dir.join(".obsidian")).unwrap();
        fs::write(dir.join("top.md"), "# Top\n").unwrap();
        fs::write(dir.join("applications/log.md"), "# Job Log\nbody").unwrap();
        fs::write(dir.join(".obsidian/hidden.md"), "# Hidden\n").unwrap();
        fs::write(dir.join("not-a-note.txt"), "x").unwrap();

        let found = scan(std::slice::from_ref(&dir), &[]);
        let titles: Vec<&str> = found.iter().map(|e| e.title.as_str()).collect();
        assert!(titles.contains(&"Top"));
        assert!(titles.contains(&"Job Log"));
        assert!(!titles.contains(&"Hidden"));
        let log = found.iter().find(|e| e.title == "Job Log").unwrap();
        assert_eq!(log.rel, "applications/log.md");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn recently_opened_notes_come_first() {
        let dir = tmpdir("recents");
        fs::write(dir.join("a.md"), "# A\n").unwrap();
        fs::write(dir.join("b.md"), "# B\n").unwrap();
        fs::write(dir.join("c.md"), "# C\n").unwrap();
        let recent = vec![
            fs::canonicalize(dir.join("c.md")).unwrap(),
            fs::canonicalize(dir.join("a.md")).unwrap(),
        ];
        let found = scan(std::slice::from_ref(&dir), &recent);
        let titles: Vec<&str> = found.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(&titles[..2], &["C", "A"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_recently_opened_note_is_offered_even_from_a_folder_that_is_not_scanned() {
        // the case that made this necessary: log.md lives in a vault that is
        // not the notes dir, and was opened last week
        let dir = tmpdir("elsewhere");
        fs::create_dir_all(dir.join("notes")).unwrap();
        fs::create_dir_all(dir.join("vault/applications")).unwrap();
        fs::write(dir.join("notes/a.md"), "# A\n").unwrap();
        let far = dir.join("vault/applications/log.md");
        fs::write(&far, "# Job Application Log\n").unwrap();

        // scanning only the notes dir does not reach it
        let found = scan(std::slice::from_ref(&dir.join("notes")), &[]);
        assert!(!found.iter().any(|e| e.title == "Job Application Log"));

        // …but having opened it before is enough, and it comes first
        let recent = vec![fs::canonicalize(&far).unwrap()];
        let found = scan(&[dir.join("notes")], &recent);
        assert_eq!(found[0].title, "Job Application Log");
        assert_eq!(found[0].path, fs::canonicalize(&far).unwrap());
        // and its folder is shown, since there is no root to be relative to
        assert!(
            found[0].folder.ends_with("vault/applications"),
            "{}",
            found[0].folder
        );
        // the whole path is searchable, so "applications/log" finds it
        assert!(crate::search::fuzzy("applications/log", &found[0].rel).is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_recent_note_that_has_been_deleted_is_dropped_rather_than_offered() {
        let dir = tmpdir("gone");
        fs::write(dir.join("a.md"), "# A\n").unwrap();
        let recent = vec![dir.join("deleted.md")];
        let found = scan(std::slice::from_ref(&dir), &recent);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "A");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_note_reached_by_both_the_walk_and_the_recents_appears_once() {
        let dir = tmpdir("dedup");
        let path = dir.join("a.md");
        fs::write(&path, "# A\n").unwrap();
        let recent = vec![fs::canonicalize(&path).unwrap()];
        let found = scan(std::slice::from_ref(&dir), &recent);
        assert_eq!(found.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn front_matter_is_not_mistaken_for_a_title() {
        let dir = tmpdir("frontmatter");
        let path = dir.join("log.md");
        fs::write(
            &path,
            "---\ntype: log\nupdated: 2026-08-25\n---\n\n# Job Application Log\n",
        )
        .unwrap();
        assert_eq!(title_at(&path), "Job Application Log");
        let _ = fs::remove_dir_all(&dir);
    }
}

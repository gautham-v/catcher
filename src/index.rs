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
#[derive(Clone)]
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

impl Entry {
    /// The file's own name, without the `.md`: what the open picker and the
    /// tree show, so a note is found under the name it has on disk.
    pub fn name(&self) -> String {
        self.path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// Stop conditions for the walk. A vault of a few thousand notes is scanned in
/// milliseconds; past that the list stops being something to eyeball anyway.
pub(crate) const MAX_FILES: usize = 8000;
pub(crate) const MAX_DEPTH: usize = 10;
/// Enough for a title line under any reasonable front matter.
const TITLE_BYTES: usize = 4096;

/// Directories never worth walking into. The linked-mentions scan walks with
/// this too: a directory not worth offering in quick-open is not worth reading
/// bodies out of either, and the two walks disagreeing about that would be a
/// footer citing a note you cannot open.
pub(crate) fn skip_dir(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "node_modules" | "target" | "attachments")
}

/// The title of the note at `path`, without reading the whole file. Shared
/// with the linked-mentions walk, which needs the same title for a file it
/// decided not to read whole — a note both walks must rank the same way.
pub(crate) fn title_at(path: &Path) -> String {
    let mut buf = vec![0u8; TITLE_BYTES];
    let read = fs::File::open(path)
        .and_then(|mut f| f.read(&mut buf))
        .unwrap_or(0);
    // `title_of` steps over front matter itself, so this is the same title
    // the palette shows for a note that is already loaded
    title_of(&String::from_utf8_lossy(&buf[..read]))
}

/// What to show beside a note's title: its folder relative to the notes dir,
/// empty when it sits directly in it, and a `~/`-shortened path when it lives
/// somewhere else entirely.
pub fn folder_of(path: &Path, notes_dir: &Path) -> String {
    let parent = path.parent().unwrap_or(path);
    match parent.strip_prefix(notes_dir) {
        Ok(rel) => rel.to_string_lossy().into_owned(),
        Err(_) => short(parent),
    }
}

/// Shorten a path for display, `~/` for the home directory.
pub fn short(path: &Path) -> String {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    match path.strip_prefix(&home) {
        Ok(rel) => format!("~/{}", rel.display()),
        Err(_) => path.display().to_string(),
    }
}

/// How long ago, in the fewest characters that still say it: `today`, `3d`,
/// `2w`, `5mo`, `2y`. `now` is passed in rather than read here, so a whole
/// list can be dated against one clock and a test is not a race with the
/// second hand.
///
/// There is no date library behind this on purpose. A calendar date would be
/// UTC-only without one, and how long ago is what the eye is actually asking
/// when it scans a folder of notes.
pub fn age(modified: SystemTime, now: SystemTime) -> String {
    // a file dated in the future is what clock skew and rsync both produce,
    // and it is not worth a panic or a negative number
    let Ok(since) = now.duration_since(modified) else {
        return "today".to_string();
    };
    let days = since.as_secs() / 86_400;
    match days {
        0 => "today".to_string(),
        1..=6 => format!("{days}d"),
        7..=29 => format!("{}w", days / 7),
        30..=364 => format!("{}mo", days / 30),
        _ => format!("{}y", days / 365),
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
    // the first root is the notes dir: a note under it is placed relative to
    // it, and anything else gets a `~/`-shortened path, so no row ever shows a
    // bare "." and no row repeats the notes dir on every line
    let home_root = roots.first().cloned().unwrap_or_default();
    let home_root = fs::canonicalize(&home_root).unwrap_or(home_root);
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
                let folder = folder_of(&path, &home_root);
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
            folder: folder_of(&path, &home_root),
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

/// Every name a `[[wikilink]]` could reach this entry by: its filename stem,
/// its title, and every `/`-boundary suffix of its path relative to the root
/// it was found under. All of them go through [`crate::md::link_key`], so what
/// the styling calls resolvable and what [`resolve`] actually finds are the
/// same set by construction.
///
/// An entry reached through the recents list carries a `~/`-shortened absolute
/// path in `rel`, so its suffixes still work as names and its stem still
/// matches — a note in another vault is linkable by name like any other.
pub fn link_keys(entry: &Entry) -> Vec<String> {
    let mut keys = vec![crate::md::link_key(&entry.title)];
    if let Some(stem) = entry.path.file_stem().and_then(|s| s.to_str()) {
        keys.push(crate::md::link_key(stem));
    }
    let rel = crate::md::link_key(&entry.rel);
    keys.push(rel.clone());
    // "interviews/stories/story-matrix" also answers to "stories/story-matrix"
    for (i, _) in rel.match_indices('/') {
        keys.push(rel[i + 1..].to_string());
    }
    keys.retain(|k| !k.is_empty());
    keys
}

/// The note a `[[wikilink]]` target names, or `None` when the vault has no
/// such note. Tried in order — filename stem, title, then a path suffix — and
/// ties broken by the shortest path, because a link that could mean two notes
/// most likely means the one nearer the top of the vault.
pub fn resolve<'a>(entries: &'a [Entry], target: &str) -> Option<&'a Entry> {
    let want = crate::md::link_key(target);
    if want.is_empty() {
        return None;
    }
    entries
        .iter()
        .filter_map(|e| rank(e, &want).map(|r| (r, e)))
        // `scan` returns entries in filesystem order, which is not stable
        // between runs, so the path is the last word rather than the order
        // they happened to arrive in
        .min_by_key(|(r, e)| (*r, e.rel.chars().count(), e.rel.clone()))
        .map(|(_, e)| e)
}

/// How well `entry` answers to `want`, lower being better, `None` for not at
/// all. Free-standing and taking a plain `&Entry` so the ordering can be
/// tested against hand-built entries with no filesystem behind them.
fn rank(entry: &Entry, want: &str) -> Option<u8> {
    // the filename is what an Obsidian link names first and foremost
    if entry
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| crate::md::link_key(s) == want)
    {
        return Some(0);
    }
    if crate::md::link_key(&entry.title) == want {
        return Some(1);
    }
    // a path suffix, but only at a folder boundary: without that check
    // `matrix` would match `story-matrix.md`, which is a different note
    let rel = crate::md::link_key(&entry.rel);
    if rel == want || rel.ends_with(&format!("/{want}")) {
        return Some(2);
    }
    None
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
    let settings = crate::config::settings_path().ok();
    text.lines()
        .map(|l| PathBuf::from(l.trim()))
        .filter(|p| !p.as_os_str().is_empty() && p.exists())
        // it has its own key and its own palette row; a recents file written
        // before that was true may still list it
        .filter(|p| settings.as_ref() != Some(p))
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
    use std::time::Duration;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("catcher-index-{name}"));
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
        assert_eq!(log.folder, "applications");
        // a note sitting directly in the notes dir has no folder worth showing
        let top = found.iter().find(|e| e.title == "Top").unwrap();
        assert_eq!(top.folder, "");
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
        assert!(found[0].folder.starts_with('~') || found[0].folder.starts_with('/'));
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

    /// A hand-built entry: `Entry`'s fields are all public, so the ranking can
    /// be tested without writing a single file.
    fn entry(rel: &str, title: &str) -> Entry {
        Entry {
            path: PathBuf::from("/vault").join(rel),
            title: title.to_string(),
            rel: rel.to_string(),
            folder: String::new(),
            modified: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn a_wikilink_resolves_by_filename_stem_before_title() {
        // one note is *called* "spec"; another is *filed* as spec.md
        let entries = vec![
            entry("notes/spec.md", "The Deploy Spec"),
            entry("a.md", "Spec"),
        ];
        assert_eq!(resolve(&entries, "spec").unwrap().rel, "notes/spec.md");
    }

    #[test]
    fn a_wikilink_resolves_a_path_suffix_at_a_folder_boundary() {
        let entries = vec![entry("interviews/stories/story-matrix.md", "Story Matrix")];
        assert_eq!(
            resolve(&entries, "stories/story-matrix").unwrap().rel,
            "interviews/stories/story-matrix.md"
        );
        // "matrix" is not a suffix of "story-matrix" at a boundary, so it
        // names nothing rather than the note that merely ends in it
        assert!(resolve(&entries, "matrix").is_none());
    }

    #[test]
    fn an_ambiguous_wikilink_resolves_to_the_shortest_path() {
        let entries = vec![
            entry("archive/2024/old/spec.md", "Old Spec"),
            entry("spec.md", "Spec"),
            entry("work/spec.md", "Work Spec"),
        ];
        assert_eq!(resolve(&entries, "spec").unwrap().rel, "spec.md");
    }

    #[test]
    fn a_wikilink_target_is_matched_without_its_case_or_its_md_suffix() {
        let entries = vec![entry("Story-Matrix.md", "Story Matrix")];
        for target in ["story-matrix", "Story-Matrix.md", "story-matrix#Method"] {
            assert!(resolve(&entries, target).is_some(), "{target}");
        }
    }

    #[test]
    fn a_wikilink_with_no_note_behind_it_resolves_to_nothing() {
        let entries = vec![entry("a.md", "A")];
        assert!(resolve(&entries, "nowhere").is_none());
        assert!(resolve(&entries, "  ").is_none());
    }

    #[test]
    fn every_name_a_note_answers_to_is_in_its_link_keys() {
        let keys = link_keys(&entry("interviews/stories/story-matrix.md", "Story Matrix"));
        for want in [
            "story-matrix",
            "story matrix",
            "stories/story-matrix",
            "interviews/stories/story-matrix",
        ] {
            assert!(
                keys.contains(&want.to_string()),
                "{want} missing from {keys:?}"
            );
        }
        // whatever the key set says is resolvable, `resolve` has to find —
        // otherwise a link is drawn as a link and opens nothing
        let entries = vec![entry("interviews/stories/story-matrix.md", "Story Matrix")];
        for k in &keys {
            assert!(resolve(&entries, k).is_some(), "{k}");
        }
    }

    #[test]
    fn an_age_reads_as_the_fewest_characters_that_still_say_it() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(400 * 86_400);
        let ago = |days: u64| age(now - Duration::from_secs(days * 86_400), now);
        assert_eq!(ago(0), "today");
        assert_eq!(ago(3), "3d");
        assert_eq!(ago(14), "2w");
        assert_eq!(ago(60), "2mo");
        assert_eq!(ago(400), "1y");
    }

    #[test]
    fn a_file_dated_in_the_future_reads_as_today_rather_than_panicking() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(86_400);
        assert_eq!(age(now + Duration::from_secs(3600), now), "today");
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

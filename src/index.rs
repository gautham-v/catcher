//! The quick-open index: every `.md` file under the notes dir (and the
//! session's own root, when it is somewhere else), with the recently opened
//! ones first. Unlike the palette — which searches the notes *loaded* for this
//! session, all of them from one flat folder — this walks subfolders, so a
//! vault organised into directories is reachable from anywhere in it.

use crate::notes::title_of;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// The other names the note answers to — its front matter `aliases`,
    /// each already put through [`crate::md::link_key`] — so `[[launch]]`
    /// reaches a note filed under some longer name.
    pub aliases: Vec<String>,
    /// The file's own name, without the `.md`: what the open picker and the
    /// tree show, so a note is found under the name it has on disk. Computed
    /// once at scan time; see [`Entry::name_of`].
    pub name: String,
}

impl Entry {
    /// The file's own name, without the `.md`.
    pub fn name(&self) -> String {
        self.name.clone()
    }

    /// The name an entry at `path` carries: its file stem.
    pub fn name_of(path: &Path) -> String {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
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

/// Walk `roots` and call `f` once for every `.md` file under them, with the
/// canonical root it was found under. This is the one walk rule: the depth and
/// file caps, the directories skipped, the dot-file and `.md` filter, and the
/// dedupe across roots that overlap (a vault named in the settings that sits
/// inside the notes dir). The quick-open index, the linked-mentions scan and
/// the rename rewrite all walk with it, because a footer citing a note you
/// cannot open from ^O, or a rename that misses one, is the walks disagreeing.
///
/// `cancel` is watched per directory entry rather than per root: one root is
/// the whole vault, and stopping "at the next root" is not stopping. Returns
/// the set of files visited, or `None` when cancelled.
pub(crate) fn walk_notes(
    roots: &[PathBuf],
    cancel: Option<&AtomicBool>,
    mut f: impl FnMut(&Path, PathBuf, &fs::DirEntry),
) -> Option<HashSet<PathBuf>> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for root in roots {
        let root = fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        let mut stack = vec![(root.clone(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            if depth > MAX_DEPTH || seen.len() >= MAX_FILES {
                continue;
            }
            let Ok(read) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in read.flatten() {
                if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                    return None;
                }
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    if !skip_dir(name) {
                        stack.push((path, depth + 1));
                    }
                    continue;
                }
                if name.starts_with('.') || !name.ends_with(".md") {
                    continue;
                }
                if seen.len() >= MAX_FILES || !seen.insert(path.clone()) {
                    continue;
                }
                f(&root, path, &entry);
            }
        }
    }
    Some(seen)
}

/// The title of the note at `path`, without reading the whole file. Shared
/// with the linked-mentions walk, which needs the same title for a file it
/// decided not to read whole — a note both walks must rank the same way.
#[cfg(test)]
pub(crate) fn title_at(path: &Path) -> String {
    head_at(path).0
}

/// The title and the front matter aliases of the note at `path`, from one
/// read of its head: both live at the top of the file, and a walk that opened
/// every note twice would be a walk you could feel.
pub(crate) fn head_at(path: &Path) -> (String, Vec<String>) {
    head_into(path, &mut Vec::new())
}

/// [`head_at`] with the read buffer supplied, so a walk over thousands of
/// notes fills one 4 KiB buffer over and over instead of zeroing a fresh one
/// per file.
fn head_into(path: &Path, buf: &mut Vec<u8>) -> (String, Vec<String>) {
    buf.clear();
    buf.resize(TITLE_BYTES, 0);
    let read = fs::File::open(path)
        .and_then(|mut f| f.read(buf))
        .unwrap_or(0);
    let head = String::from_utf8_lossy(&buf[..read]);
    // `title_of` steps over front matter itself, so this is the same title
    // the palette shows for a note that is already loaded
    (title_of(&head), crate::md::front_matter_aliases(&head))
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
    let home = std::env::home_dir().unwrap_or_else(|| PathBuf::from("/"));
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
    let mut entries: Vec<Entry> = Vec::new();
    let mut buf = Vec::new();
    let mut seen = walk_notes(roots, None, |root, path, entry| {
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        let folder = folder_of(&path, &home_root);
        let (title, aliases) = head_into(&path, &mut buf);
        entries.push(Entry {
            title,
            name: Entry::name_of(&path),
            path,
            rel,
            folder,
            modified,
            aliases,
        });
    })
    .unwrap_or_default();

    // anything opened before that the walk did not reach — another vault, a
    // file passed on the command line once — is still worth offering
    for path in recent {
        if entries.len() >= MAX_FILES {
            break;
        }
        let path = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        if !path.is_file() || !seen.insert(path.clone()) {
            continue;
        }
        let modified = fs::metadata(&path)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let (title, aliases) = head_into(&path, &mut buf);
        entries.push(Entry {
            title,
            // outside every root, so the whole path is what a query has to
            // match against, and the folder is what is worth showing
            rel: short(&path),
            folder: folder_of(&path, &home_root),
            name: Entry::name_of(&path),
            path,
            modified,
            aliases,
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
    let mut keys = vec![
        crate::md::link_key(&entry.title),
        crate::md::link_key(&entry.name),
    ];
    let rel = crate::md::link_key(&entry.rel);
    keys.push(rel.clone());
    // "interviews/stories/story-matrix" also answers to "stories/story-matrix"
    for (i, _) in rel.match_indices('/') {
        keys.push(rel[i + 1..].to_string());
    }
    // and the names its front matter says it also goes by
    keys.extend(entry.aliases.iter().cloned());
    keys.retain(|k| !k.is_empty());
    keys
}

/// The note a `[[wikilink]]` target names, or `None` when the vault has no
/// such note. Tried in order — filename stem, title, then a path suffix — and
/// ties broken by the shortest path, because a link that could mean two notes
/// most likely means the one nearer the top of the vault.
pub fn resolve<'a>(entries: &'a [Entry], target: &str) -> Option<&'a Entry> {
    resolve_with(entries, target, true)
}

/// [`resolve`] without the alias fallback: only a note's own names count.
/// The rename rewrite asks this one, because a link written to an alias
/// still reaches the note after the file is renamed and must be left alone.
pub fn resolve_by_name<'a>(entries: &'a [Entry], target: &str) -> Option<&'a Entry> {
    resolve_with(entries, target, false)
}

fn resolve_with<'a>(entries: &'a [Entry], target: &str, aliases: bool) -> Option<&'a Entry> {
    let want = crate::md::link_key(target);
    if want.is_empty() {
        return None;
    }
    best(entries.iter(), &want, aliases)
}

/// [`resolve`] over a prebuilt lookup table, for the caller that answers many
/// targets against one index — the embed resolver runs once per rendered
/// embed. Every [`link_keys`] name of every entry maps to the entries that
/// carry it, so a target is a hash lookup and ranking runs over the few hits
/// rather than the whole vault. The ranking and tie-break are [`resolve`]'s.
pub struct Resolver {
    entries: Vec<Entry>,
    by_key: HashMap<String, Vec<usize>>,
}

impl Resolver {
    pub fn new(entries: Vec<Entry>) -> Self {
        let mut by_key: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, e) in entries.iter().enumerate() {
            for k in link_keys(e) {
                let hits = by_key.entry(k).or_default();
                if hits.last() != Some(&i) {
                    hits.push(i);
                }
            }
        }
        Self { entries, by_key }
    }

    pub fn resolve(&self, target: &str) -> Option<&Entry> {
        let want = crate::md::link_key(target);
        if want.is_empty() {
            return None;
        }
        let hits = self.by_key.get(&want)?;
        best(hits.iter().map(|&i| &self.entries[i]), &want, true)
    }
}

/// The best of `candidates` for `want`, by [`rank`] then shortest path — the
/// one ordering both [`resolve`] and [`Resolver`] use.
fn best<'a>(
    candidates: impl Iterator<Item = &'a Entry>,
    want: &str,
    aliases: bool,
) -> Option<&'a Entry> {
    candidates
        .filter_map(|e| {
            rank(e, want)
                .or_else(|| (aliases && e.aliases.iter().any(|a| a == want)).then_some(3))
                .map(|r| (r, e))
        })
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
    if crate::md::link_key(&entry.name) == want {
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

/// The tags a note carries: its front matter `tags:` and every `#tag` in
/// its body, each once, in the form [`crate::md::tag_key`] matches on.
/// Fenced code is stepped over, as the styling steps over it.
pub fn tags_of(content: &str) -> Vec<String> {
    let mut tags = front_matter_tags(content);
    for (_, line) in crate::notes::prose_lines(content) {
        let chars: Vec<char> = line.chars().collect();
        for (s, e) in crate::md::tags_in(line) {
            tags.push(crate::md::tag_key(&chars[s..e].iter().collect::<String>()));
        }
    }
    let mut seen = std::collections::HashSet::new();
    tags.retain(|t| seen.insert(t.clone()));
    tags
}

/// The `tags:` a note's front matter declares, either inline — `tags: a, b`,
/// with or without brackets — or as a YAML list on the lines under it.
/// Only a top-level `tags:` counts; an indented one belongs to some other key.
/// The singular `tag:` is read too, as Obsidian reads it.
pub fn front_matter_tags(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for key in ["tags", "tag"] {
        for v in crate::md::front_matter_values(content, key) {
            push_tags(&mut out, &v);
        }
    }
    out
}

/// Split a comma- or space-separated run of tags, shedding the quotes and
/// the `#` people write out of habit.
fn push_tags(out: &mut Vec<String>, text: &str) {
    for t in text.split(|c: char| c == ',' || c.is_whitespace()) {
        let key = crate::md::tag_key(t.trim_matches(|c| c == '"' || c == '\''));
        if !key.is_empty() {
            out.push(key);
        }
    }
}

/// The tags of the note on disk at `path`. The whole file, unlike
/// `title_at`: a tag can sit on the last line.
pub fn tags_at(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .map(|c| tags_of(&c))
        .unwrap_or_default()
}

/// Which entries carry `tag`, as indices into `entries` in their order.
/// Reads every file, which is what the linked-mentions scan does too, and
/// only when a tag is actually followed. A tag covers the ones nested under
/// it: `#work` lists the `#work/projects` notes too.
pub fn with_tag(entries: &[Entry], tag: &str) -> Vec<usize> {
    let want = crate::md::tag_key(tag);
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| tags_at(&e.path).iter().any(|t| tag_under(t, &want)))
        .map(|(i, _)| i)
        .collect()
}

/// Whether `tag` is `want` itself or nested under it, both in `tag_key`
/// form: `work/projects` is under `work`, `workshop` is not.
pub fn tag_under(tag: &str, want: &str) -> bool {
    tag == want
        || tag
            .strip_prefix(want)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Every tag the notes at `entries` carry, with how many notes carry each,
/// most-used first and by name among equals. Reads every file, like
/// [`with_tag`]; meant for a worker thread.
pub fn tag_counts(entries: &[Entry]) -> Vec<(String, usize)> {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for e in entries {
        for t in tags_at(&e.path) {
            *counts.entry(t).or_default() += 1;
        }
    }
    let mut out: Vec<(String, usize)> = counts.into_iter().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
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
        crate::testutil::tmpdir("index", name)
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
        let recent = vec![dir.join("c.md"), dir.join("a.md")];
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
        let recent = vec![far.clone()];
        let found = scan(&[dir.join("notes")], &recent);
        assert_eq!(found[0].title, "Job Application Log");
        assert_eq!(found[0].path, far);
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
        let recent = vec![path.clone()];
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
            aliases: Vec::new(),
            name: Entry::name_of(&PathBuf::from(rel)),
        }
    }

    #[test]
    fn the_resolver_ranks_hits_the_way_resolve_does() {
        let entries = vec![
            entry("notes/launch.md", "Something Else"),
            entry("plan.md", "Launch"),
            aliased("deep/other.md", "Other", &["launch"]),
            entry("stories/story-matrix.md", "Story Matrix"),
        ];
        let r = Resolver::new(entries.clone());
        // a stem hit beats a title hit beats an alias
        assert_eq!(r.resolve("launch").unwrap().rel, "notes/launch.md");
        assert_eq!(r.resolve("Launch").unwrap().rel, "notes/launch.md");
        assert_eq!(
            r.resolve("stories/story-matrix").unwrap().rel,
            "stories/story-matrix.md"
        );
        assert_eq!(r.resolve("Something Else").unwrap().rel, "notes/launch.md");
        // a path suffix only counts at a folder boundary
        assert!(r.resolve("matrix").is_none());
        assert!(r.resolve("").is_none());
        for t in ["launch", "plan", "Other", "story-matrix", "nowhere"] {
            assert_eq!(
                r.resolve(t).map(|e| &e.rel),
                resolve(&entries, t).map(|e| &e.rel),
                "{t}"
            );
        }
    }

    /// An entry whose front matter says it also goes by `aliases`.
    fn aliased(rel: &str, title: &str, aliases: &[&str]) -> Entry {
        let mut e = entry(rel, title);
        e.aliases = aliases.iter().map(|a| crate::md::link_key(a)).collect();
        e
    }

    #[test]
    fn a_wikilink_falls_back_to_a_front_matter_alias_after_every_real_name() {
        let entries = vec![
            aliased(
                "projects/big-launch.md",
                "The Big Launch",
                &["launch", "Go Live"],
            ),
            entry("launch.md", "Launch"),
        ];
        // a note actually called launch.md wins over an alias
        assert_eq!(resolve(&entries, "launch").unwrap().rel, "launch.md");
        // an alias reaches the note when nothing is named that, whatever the case
        assert_eq!(
            resolve(&entries, "go live").unwrap().rel,
            "projects/big-launch.md"
        );
        assert_eq!(
            resolve(&entries, "GO LIVE").unwrap().rel,
            "projects/big-launch.md"
        );
        assert!(resolve(&entries, "nothing").is_none());
        // the by-name resolver does not know aliases at all
        assert!(resolve_by_name(&entries, "go live").is_none());
        // and the alias is one of the names the styling calls resolvable
        assert!(link_keys(&entries[0]).contains(&"go live".to_string()));
    }

    #[test]
    fn a_scanned_note_carries_the_aliases_its_front_matter_declares() {
        let dir = crate::testutil::tmpdir("index", "aliases");
        fs::write(
            dir.join("big-launch.md"),
            "---
aliases: [launch, \"Go Live\"]
---
# The Big Launch
",
        )
        .unwrap();
        let entries = scan(std::slice::from_ref(&dir), &[]);
        let e = entries.iter().find(|e| e.rel == "big-launch.md").unwrap();
        assert_eq!(e.title, "The Big Launch");
        assert_eq!(e.aliases, vec!["launch", "go live"]);
        assert_eq!(resolve(&entries, "Launch").unwrap().rel, "big-launch.md");
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
    fn front_matter_tags_come_inline_bracketed_or_as_a_list() {
        assert_eq!(front_matter_tags("---\ntags: a, b\n---\n"), vec!["a", "b"]);
        assert_eq!(
            front_matter_tags("---\ntags: [A, \"b\"]\n---\n"),
            vec!["a", "b"]
        );
        assert_eq!(
            front_matter_tags("---\ntags:\n  - a\n  - '#b'\nother: x\n---\n"),
            vec!["a", "b"]
        );
        assert_eq!(front_matter_tags("---\ntags:\n- a\n---\n"), vec!["a"]);
        // an indented `tags:` is some other key's, and no block means none
        assert!(front_matter_tags("---\nmeta:\n  tags: a\n---\n").is_empty());
        assert!(front_matter_tags("tags: a\n").is_empty());
        assert!(front_matter_tags("---\ntags:\n---\n").is_empty());
        // the singular key too, and both when a note has both
        assert_eq!(front_matter_tags("---\ntag: solo\n---\n"), vec!["solo"]);
        assert_eq!(
            front_matter_tags("---\ntags: a\ntag: b\n---\n"),
            vec!["a", "b"]
        );
    }

    #[test]
    fn a_tag_covers_the_tags_nested_under_it_but_not_its_lookalikes() {
        assert!(tag_under("work", "work"));
        assert!(tag_under("work/projects", "work"));
        assert!(tag_under("work/projects/x", "work"));
        assert!(!tag_under("workshop", "work"));
        assert!(!tag_under("work", "work/projects"));
    }

    #[test]
    fn tags_are_counted_across_the_index_most_used_first() {
        let dir = tmpdir("tag-counts");
        fs::write(dir.join("a.md"), "# A\n#work #home\n").unwrap();
        fs::write(dir.join("b.md"), "---\ntags: [work]\n---\n# B\n#work\n").unwrap();
        fs::write(dir.join("c.md"), "# C\n#alpha\n").unwrap();
        let found = scan(std::slice::from_ref(&dir), &[]);
        assert_eq!(
            tag_counts(&found),
            vec![
                ("work".to_string(), 2),
                ("alpha".to_string(), 1),
                ("home".to_string(), 1)
            ]
        );
    }

    #[test]
    fn a_notes_tags_are_its_front_matter_and_its_body_each_once() {
        let tags = tags_of(
            "---\ntags: work\n---\n# T #Work\n\n```\n#fenced\n```\n~~~\n#tilde\n~~~\nsee #home and `#code`\n",
        );
        assert_eq!(tags, vec!["work", "home"]);
    }

    #[test]
    fn the_notes_carrying_a_tag_are_picked_out_of_the_index() {
        let dir = tmpdir("tags");
        fs::write(dir.join("a.md"), "# A\n#work\n").unwrap();
        fs::write(dir.join("b.md"), "---\ntags: [home, work]\n---\n# B\n").unwrap();
        fs::write(dir.join("c.md"), "# C\nnothing\n").unwrap();
        let mut found = scan(std::slice::from_ref(&dir), &[]);
        found.sort_by(|a, b| a.rel.cmp(&b.rel));
        let names =
            |idx: Vec<usize>| -> Vec<String> { idx.iter().map(|i| found[*i].name()).collect() };
        assert_eq!(names(with_tag(&found, "Work")), vec!["a", "b"]);
        assert_eq!(names(with_tag(&found, "#home")), vec!["b"]);
        // a nested tag is listed under its parent, a lookalike is not
        fs::write(dir.join("d.md"), "# D\n#work/projects #workshop\n").unwrap();
        let mut found = scan(std::slice::from_ref(&dir), &[]);
        found.sort_by(|a, b| a.rel.cmp(&b.rel));
        let names =
            |idx: Vec<usize>| -> Vec<String> { idx.iter().map(|i| found[*i].name()).collect() };
        assert_eq!(names(with_tag(&found, "work")), vec!["a", "b", "d"]);
        assert_eq!(names(with_tag(&found, "work/projects")), vec!["d"]);
        assert_eq!(names(with_tag(&found, "workshop")), vec!["d"]);
        assert!(with_tag(&found, "none").is_empty());
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

    #[test]
    fn head_buffer_is_reused_without_bleeding_between_files() {
        let dir = tmpdir("headbuf");
        let long = dir.join("long.md");
        let short = dir.join("short.md");
        fs::write(
            &long,
            format!("# A much longer title here\n{}", "x".repeat(3000)),
        )
        .unwrap();
        fs::write(&short, "# S\n").unwrap();
        let mut buf = Vec::new();
        assert_eq!(head_into(&long, &mut buf).0, "A much longer title here");
        // the second read must not see the first file's tail
        assert_eq!(head_into(&short, &mut buf).0, "S");
        assert_eq!(buf.len(), TITLE_BYTES);
        let _ = fs::remove_dir_all(&dir);
    }
}

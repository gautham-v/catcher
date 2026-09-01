//! The folder tree behind ^O's browse mode. A fuzzy matcher can only help
//! with a name you can half remember; sometimes what you actually want is to
//! see what is there, because the note you are after is "the one in the
//! interviews folder from a couple of weeks ago" and nothing about that is a
//! string to type.
//!
//! Everything here is pure over the `index::Entry` values the quick-open scan
//! already produced, so browse and search are two views of one index rather
//! than two indexes that can drift apart — a note the ranked list can find is
//! a note the tree can show, by construction. It also means the whole module
//! tests without a terminal, an `App`, or a single file on disk.
//!
//! Nothing here runs while the overlay is shut: the fold set is empty and no
//! rows are built until someone asks for them.

use crate::index::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::SystemTime;

/// What one drawn row of the tree stands for.
pub enum RowKind {
    /// A folder, by the key `index::Entry.folder` gave it. `notes` counts
    /// every note beneath it, subfolders included — that is the only thing
    /// "6 notes" can honestly mean on a folder that is closed.
    Folder {
        key: String,
        name: String,
        notes: usize,
        open: bool,
    },
    /// A note, by its index into the `&[Entry]` slice the rows were built
    /// from. That index is what becomes `Item::Entry`, so opening a note from
    /// the tree goes down the same path quick-open already uses.
    Note {
        entry: usize,
        /// The filename stem, which is what the row shows.
        name: String,
        modified: SystemTime,
    },
}

pub struct Row {
    pub depth: usize,
    /// The folder key this row sits under, or `None` at the top. Indent alone
    /// cannot say: a folder's own notes are drawn *after* its subfolders and
    /// at the same depth as those subfolders' notes, so walking backwards for
    /// a shallower row would find the wrong parent.
    pub parent: Option<String>,
    pub kind: RowKind,
}

/// The folder keys an entry nests under, outermost first: `a/b` gives
/// `["a", "a/b"]`, and a note sitting in the vault root gives nothing at all.
///
/// A key that starts with `~/` or `/` is kept whole as one row rather than
/// split into a chain. A folder in another vault is one place, and splitting
/// it would spend six levels of indent on `~ / Code / tinycomputer / …` before
/// reaching a single note.
///
/// The absolute case is not hypothetical: `index::folder_of` hands back a bare
/// `/Volumes/work/notes` for anything outside both the notes dir and the home
/// directory. Split, that would draw a top-level `Volumes` — and worse, a
/// `/tmp/scratch` would arrive as `tmp` and share a row, and one fold state,
/// with a real `tmp/` folder inside the vault.
fn components(folder: &str) -> Vec<String> {
    let trimmed = folder.trim_matches('/');
    if trimmed.is_empty() {
        return Vec::new();
    }
    if folder.starts_with('~') || folder.starts_with('/') {
        return vec![folder.trim_end_matches('/').to_string()];
    }
    let mut out = Vec::new();
    let mut acc = String::new();
    for seg in trimmed.split('/').filter(|s| !s.is_empty()) {
        if !acc.is_empty() {
            acc.push('/');
        }
        acc.push_str(seg);
        out.push(acc.clone());
    }
    out
}

/// What to call a folder on screen: its last segment inside the vault, and the
/// whole path for one outside it, where the last segment on its own would be a
/// bare `applications` with no hint of which vault it belongs to.
fn name_of(key: &str) -> String {
    if key.starts_with('~') || key.starts_with('/') {
        return key.to_string();
    }
    key.rsplit('/').next().unwrap_or(key).to_string()
}

/// Whether the query lets this entry through. The haystacks are the ones
/// `App::open_items` scores on, so the tree and the ranked list never disagree
/// about what the word in the prompt matched.
fn matches(query: &str, entry: &Entry) -> bool {
    query.is_empty()
        || crate::search::fuzzy(query, &entry.name()).is_some()
        || crate::search::fuzzy(query, &entry.title).is_some()
        || crate::search::fuzzy(query, &entry.rel).is_some()
}

struct Node {
    parent: Option<String>,
    name: String,
    notes: usize,
    kids: Vec<usize>,
}

/// Every row the tree shows right now, in drawing order: at each level the
/// folders first and then the notes, both by name.
///
/// A non-empty query unfolds the ancestors of every match regardless of what
/// has been folded by hand — a filter that leaves you folders still to click
/// is not a filter. That does mean the fold state you see while filtering is
/// not the one you left behind: clearing the query snaps the folders shut
/// again, which is intended and not a bug to chase later.
pub fn rows(entries: &[Entry], open: &BTreeSet<String>, query: &str) -> Vec<Row> {
    let unfold_all = !query.is_empty();
    let mut nodes: BTreeMap<String, Node> = BTreeMap::new();
    let mut loose: Vec<usize> = Vec::new();

    for (i, entry) in entries.iter().enumerate() {
        if !matches(query, entry) {
            continue;
        }
        let comps = components(&entry.folder);
        let Some(last) = comps.last().cloned() else {
            loose.push(i);
            continue;
        };
        for (d, key) in comps.iter().enumerate() {
            let node = nodes.entry(key.clone()).or_insert_with(|| Node {
                parent: if d == 0 {
                    None
                } else {
                    Some(comps[d - 1].clone())
                },
                name: name_of(key),
                notes: 0,
                kids: Vec::new(),
            });
            // the count is of everything beneath, so every ancestor on the way
            // down takes a tick, not just the folder the note is filed in
            node.notes += 1;
        }
        nodes.get_mut(&last).expect("just inserted").kids.push(i);
    }

    // who sits under whom, worked out once. `emit` used to look for a folder's
    // children by scanning the whole node map, which is quadratic in folders
    // exactly when a query is typed — that is the path where every folder is
    // forced open and so every one of them is visited.
    let mut kids_by_parent: BTreeMap<Option<&str>, Vec<&str>> = BTreeMap::new();
    for (key, node) in &nodes {
        kids_by_parent
            .entry(node.parent.as_deref())
            .or_default()
            .push(key.as_str());
    }
    // by name, lowercased, with the key as the last word so two folders that
    // differ only in case still come out in the same order every time
    for group in kids_by_parent.values_mut() {
        group.sort_by(|a, b| {
            nodes[*a]
                .name
                .to_lowercase()
                .cmp(&nodes[*b].name.to_lowercase())
                .then_with(|| a.cmp(b))
        });
    }

    let mut out = Vec::new();
    emit(
        &mut out,
        &nodes,
        &kids_by_parent,
        entries,
        None,
        0,
        open,
        unfold_all,
    );
    push_notes(&mut out, entries, &loose, 0, None);
    out
}

#[allow(clippy::too_many_arguments)]
fn emit(
    out: &mut Vec<Row>,
    nodes: &BTreeMap<String, Node>,
    kids_by_parent: &BTreeMap<Option<&str>, Vec<&str>>,
    entries: &[Entry],
    parent: Option<&str>,
    depth: usize,
    open: &BTreeSet<String>,
    unfold_all: bool,
) {
    let Some(kids) = kids_by_parent.get(&parent) else {
        return;
    };
    for key in kids {
        let node = &nodes[*key];
        let is_open = unfold_all || open.contains(*key);
        out.push(Row {
            depth,
            parent: parent.map(str::to_string),
            kind: RowKind::Folder {
                key: (*key).to_string(),
                name: node.name.clone(),
                notes: node.notes,
                open: is_open,
            },
        });
        if is_open {
            emit(
                out,
                nodes,
                kids_by_parent,
                entries,
                Some(key),
                depth + 1,
                open,
                unfold_all,
            );
            push_notes(out, entries, &node.kids, depth + 1, Some(key));
        }
    }
}

fn push_notes(
    out: &mut Vec<Row>,
    entries: &[Entry],
    kids: &[usize],
    depth: usize,
    parent: Option<&str>,
) {
    let mut kids = kids.to_vec();
    // the path is the tiebreak so the order is total: two notes with the
    // same name in different cases would otherwise swap places between draws
    kids.sort_by(|&a, &b| {
        entries[a]
            .name()
            .to_lowercase()
            .cmp(&entries[b].name().to_lowercase())
            .then_with(|| entries[a].rel.cmp(&entries[b].rel))
    });
    for i in kids {
        out.push(Row {
            depth,
            parent: parent.map(str::to_string),
            kind: RowKind::Note {
                entry: i,
                name: entries[i].name(),
                modified: entries[i].modified,
            },
        });
    }
}

/// Fold or unfold one folder, and say which row the selection should land on
/// afterwards — that same folder, wherever folding moved it to.
pub fn toggle(entries: &[Entry], open: &mut BTreeSet<String>, key: &str, query: &str) -> usize {
    if !open.remove(key) {
        open.insert(key.to_string());
    }
    rows(entries, open, query)
        .iter()
        .position(|r| matches!(&r.kind, RowKind::Folder { key: k, .. } if k == key))
        .unwrap_or(0)
}

/// Unfold the folder the open note lives in, and every folder above it, and
/// say which row that note ended up on. This is what makes browse mode open
/// showing you where you already are rather than where the vault starts.
///
/// `query` is the one the tree is about to be drawn with, and it has to be:
/// tab flips ^O into browse mode with whatever is typed still in the prompt,
/// and a row counted against the whole vault means nothing in a tree filtered
/// down to three rows. A note the filter hides has no row to land on, so the
/// selection goes to the top of what is showing.
pub fn reveal(
    entries: &[Entry],
    open: &mut BTreeSet<String>,
    active: Option<&Path>,
    query: &str,
) -> usize {
    let Some(active) = active else {
        return 0;
    };
    let Some(i) = entries.iter().position(|e| same(&e.path, active)) else {
        // a note the walk never saw — one deleted underneath the session, or
        // one outside every root and never opened before — is no reason to
        // fail; the tree simply opens at the top
        return 0;
    };
    // the fold set is opened either way: the query may be cleared a keystroke
    // later, and when it is, the tree should still be open on where you are
    for key in components(&entries[i].folder) {
        open.insert(key);
    }
    rows(entries, open, query)
        .iter()
        .position(|r| matches!(r.kind, RowKind::Note { entry, .. } if entry == i))
        .unwrap_or(0)
}

/// Whether two paths name the same file. The index holds canonical paths and a
/// loaded note may not, so the cheap comparison is tried first and the
/// filesystem only asked when it fails.
fn same(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// The row for the folder this row sits in — where ← goes when there is
/// nothing left to collapse.
pub fn parent_of(rows: &[Row], at: usize) -> Option<usize> {
    let parent = rows.get(at)?.parent.as_ref()?;
    rows[..at]
        .iter()
        .rposition(|r| matches!(&r.kind, RowKind::Folder { key, .. } if key == parent))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A hand-built entry. `Entry`'s fields are all public, so a whole vault
    /// can be described in a few lines with nothing on disk behind it.
    fn entry(rel: &str, title: &str) -> Entry {
        let folder = match rel.rfind('/') {
            Some(i) => rel[..i].to_string(),
            None => String::new(),
        };
        Entry {
            path: PathBuf::from("/vault").join(rel),
            title: title.to_string(),
            rel: rel.to_string(),
            folder,
            modified: SystemTime::UNIX_EPOCH,
        }
    }

    /// An entry filed under a folder that is not under the notes dir, the way
    /// the recents pass in `index::scan` produces one.
    fn far(folder: &str, rel: &str, title: &str) -> Entry {
        Entry {
            path: PathBuf::from(rel),
            title: title.to_string(),
            rel: rel.to_string(),
            folder: folder.to_string(),
            modified: SystemTime::UNIX_EPOCH,
        }
    }

    fn open_set(keys: &[&str]) -> BTreeSet<String> {
        keys.iter().map(|k| k.to_string()).collect()
    }

    /// The drawn shape of the tree: depth, and either `▸key` / `▾key` for a
    /// folder or the note's filename.
    fn shape(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| {
                let body = match &r.kind {
                    RowKind::Folder { key, open, .. } => {
                        format!("{}{key}", if *open { '▾' } else { '▸' })
                    }
                    RowKind::Note { name, .. } => name.clone(),
                };
                format!("{}{body}", "  ".repeat(r.depth))
            })
            .collect()
    }

    #[test]
    fn folders_nest_and_notes_sit_under_them() {
        let entries = vec![
            entry("interviews/stories/matrix.md", "Story Matrix"),
            entry("interviews/prep.md", "Prep"),
        ];
        let rows = rows(
            &entries,
            &open_set(&["interviews", "interviews/stories"]),
            "",
        );
        assert_eq!(
            shape(&rows),
            vec![
                "▾interviews",
                "  ▾interviews/stories",
                "    matrix",
                "  prep",
            ]
        );
    }

    #[test]
    fn folders_come_before_notes_and_both_sort_by_name() {
        let entries = vec![
            entry("zebra.md", "Zebra"),
            entry("apple.md", "Apple"),
            entry("work/a.md", "A"),
            entry("archive/b.md", "B"),
        ];
        let rows = rows(&entries, &open_set(&[]), "");
        assert_eq!(shape(&rows), vec!["▸archive", "▸work", "apple", "zebra"]);
    }

    #[test]
    fn a_collapsed_folder_counts_every_note_beneath_it_not_just_its_own() {
        let entries = vec![
            entry("interviews/prep.md", "Prep"),
            entry("interviews/stories/one.md", "One"),
            entry("interviews/stories/two.md", "Two"),
        ];
        let rows = rows(&entries, &open_set(&[]), "");
        match &rows[0].kind {
            RowKind::Folder { key, notes, .. } => {
                assert_eq!(key, "interviews");
                assert_eq!(*notes, 3);
            }
            _ => panic!("expected a folder row"),
        }
    }

    #[test]
    fn a_note_in_the_vault_root_has_no_folder_row_above_it() {
        let entries = vec![entry("scratch.md", "Scratch")];
        let rows = rows(&entries, &open_set(&[]), "");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].depth, 0);
        assert!(rows[0].parent.is_none());
        assert!(matches!(rows[0].kind, RowKind::Note { .. }));
    }

    #[test]
    fn a_folder_outside_the_vault_is_one_row_named_by_its_whole_path() {
        let entries = vec![far("~/Code/tinycomputer/notes", "/x/log.md", "Log")];
        let rows = rows(&entries, &open_set(&["~/Code/tinycomputer/notes"]), "");
        // one row, not four levels of indent spent getting to it
        assert_eq!(shape(&rows), vec!["▾~/Code/tinycomputer/notes", "  log"]);
    }

    #[test]
    fn unfolding_a_folder_shows_its_children_and_folding_it_hides_them_again() {
        let entries = vec![entry("work/a.md", "A")];
        let mut open = open_set(&[]);
        assert_eq!(shape(&rows(&entries, &open, "")), vec!["▸work"]);
        toggle(&entries, &mut open, "work", "");
        assert_eq!(shape(&rows(&entries, &open, "")), vec!["▾work", "  a"]);
        toggle(&entries, &mut open, "work", "");
        assert_eq!(shape(&rows(&entries, &open, "")), vec!["▸work"]);
    }

    #[test]
    fn folding_a_folder_leaves_the_selection_on_that_folder_wherever_it_moved_to() {
        let entries = vec![
            entry("archive/a.md", "A"),
            entry("archive/b.md", "B"),
            entry("work/c.md", "C"),
        ];
        let mut open = open_set(&["archive"]);
        // "work" is row 3 while archive is open, and row 1 once it is shut
        assert_eq!(toggle(&entries, &mut open, "archive", ""), 0);
        assert_eq!(toggle(&entries, &mut open, "work", ""), 1);
        assert_eq!(toggle(&entries, &mut open, "archive", ""), 0);
        assert_eq!(
            shape(&rows(&entries, &open, "")),
            vec!["▾archive", "  a", "  b", "▾work", "  c"]
        );
    }

    #[test]
    fn filtering_keeps_the_folders_a_matching_note_lives_in() {
        let entries = vec![
            entry("interviews/stories/matrix.md", "Story Matrix"),
            entry("interviews/prep.md", "Prep"),
            entry("work/invoice.md", "Invoice"),
        ];
        let rows = rows(&entries, &open_set(&[]), "matrix");
        assert_eq!(
            shape(&rows),
            vec!["▾interviews", "  ▾interviews/stories", "    matrix"]
        );
    }

    #[test]
    fn a_filtered_tree_unfolds_its_matches_without_anything_being_clicked() {
        let entries = vec![entry("a/b/c/deep.md", "Deep")];
        // nothing at all is in the fold set, and every level is still open
        let rows = rows(&entries, &open_set(&[]), "deep");
        assert!(rows.iter().all(|r| match &r.kind {
            RowKind::Folder { open, .. } => *open,
            RowKind::Note { .. } => true,
        }));
        assert_eq!(rows.len(), 4);
    }

    #[test]
    fn a_query_that_matches_nothing_leaves_an_empty_tree_rather_than_the_whole_vault() {
        let entries = vec![entry("work/a.md", "A")];
        assert!(rows(&entries, &open_set(&[]), "zzzz").is_empty());
    }

    #[test]
    fn entering_browse_unfolds_the_folder_of_the_note_you_are_in_and_selects_that_note() {
        let entries = vec![
            entry("archive/old.md", "Old"),
            entry("interviews/stories/matrix.md", "Story Matrix"),
        ];
        let mut open = open_set(&[]);
        let at = reveal(&entries, &mut open, Some(&entries[1].path.clone()), "");
        // ▸archive, ▾interviews, ▾interviews/stories, Story Matrix
        assert_eq!(at, 3);
        assert!(open.contains("interviews") && open.contains("interviews/stories"));
        // and nothing else was opened on the way
        assert!(!open.contains("archive"));
        match &rows(&entries, &open, "")[at].kind {
            RowKind::Note { entry, .. } => assert_eq!(*entry, 1),
            _ => panic!("expected the note row"),
        }
    }

    #[test]
    fn entering_browse_on_a_note_the_index_never_saw_selects_the_first_row() {
        let entries = vec![entry("work/a.md", "A")];
        let mut open = open_set(&[]);
        assert_eq!(reveal(&entries, &mut open, None, ""), 0);
        assert_eq!(
            reveal(&entries, &mut open, Some(Path::new("/nowhere/gone.md")), ""),
            0
        );
        assert!(open.is_empty());
    }

    #[test]
    fn revealing_with_a_query_typed_counts_rows_in_the_tree_the_query_leaves() {
        let entries = vec![
            entry("archive/old.md", "Old"),
            entry("interviews/stories/matrix.md", "Story Matrix"),
        ];
        let mut open = open_set(&[]);
        // tab into browse with "matrix" typed: the drawn tree is three rows,
        // and the note you are in is not one of them
        let at = reveal(&entries, &mut open, Some(&entries[0].path.clone()), "matrix");
        assert_eq!(
            shape(&rows(&entries, &open, "matrix")),
            vec!["▾interviews", "  ▾interviews/stories", "    matrix"]
        );
        assert_eq!(at, 0);
        // and the note that *is* showing is found at its filtered row, not at
        // the one it would have had in the whole vault
        let at = reveal(&entries, &mut open, Some(&entries[1].path.clone()), "matrix");
        assert_eq!(at, 2);
    }

    #[test]
    fn a_folder_outside_the_home_directory_is_one_row_named_by_its_whole_path() {
        // `index::folder_of` hands back a bare absolute path for a note under
        // neither the notes dir nor `~`; split into segments it would draw a
        // top-level "tmp" sharing a row with the vault's own tmp/ folder
        let entries = vec![
            far("/tmp/scratch", "/tmp/scratch/x.md", "Outside"),
            entry("tmp/y.md", "Inside"),
        ];
        let rows = rows(&entries, &open_set(&["/tmp/scratch", "tmp"]), "");
        assert_eq!(
            shape(&rows),
            vec!["▾/tmp/scratch", "  x", "▾tmp", "  y"]
        );
    }

    #[test]
    fn the_left_key_finds_the_folder_a_row_lives_in() {
        let entries = vec![
            entry("interviews/stories/matrix.md", "Story Matrix"),
            entry("interviews/prep.md", "Prep"),
        ];
        let rows = rows(
            &entries,
            &open_set(&["interviews", "interviews/stories"]),
            "",
        );
        // ▾interviews / ▾interviews/stories / Story Matrix / Prep
        assert_eq!(parent_of(&rows, 0), None);
        assert_eq!(parent_of(&rows, 1), Some(0));
        assert_eq!(parent_of(&rows, 2), Some(1));
        // "Prep" is drawn after its sibling folder's subtree and at the same
        // depth as that folder's notes, so depth alone would find the wrong
        // parent — this is the case `Row::parent` exists for
        assert_eq!(parent_of(&rows, 3), Some(0));
    }
}

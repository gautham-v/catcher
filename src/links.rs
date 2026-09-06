//! Keeping `[[wikilinks]]` pointing at a note whose file has just been renamed.
//!
//! A wikilink names a file, so a rename — the automatic one that follows a
//! note's first line, or an explicit one — would leave every link to the old
//! name pointing at nothing. This walks the same roots quick-open walks and
//! rewrites the target of each link that *resolved* to the old file: by name a
//! `[[spec]]` could be any note called spec, and only the resolver can say
//! which one it meant, so the link is judged the way a click on it would be.
//! The alias and the `#heading` are left as they were typed.

use crate::index::{self, Entry};
use crate::notes::{self, write_atomic};
use std::fs;
use std::path::{Path, PathBuf};

/// What a pass over the vault did, for the status bar.
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    /// How many links were rewritten.
    pub links: usize,
    /// The notes written back, so a copy held in memory can be refreshed.
    pub notes: Vec<PathBuf>,
    /// Notes that could not be read or written, so their links still point
    /// at the old name.
    pub skipped: Vec<PathBuf>,
}

impl Report {
    /// `3 links updated in 2 notes`, or nothing when there was nothing to do
    /// — a rename that touched no other note has nothing worth saying.
    pub fn describe(&self) -> Option<String> {
        if self.links == 0 && self.skipped.is_empty() {
            return None;
        }
        let mut out = format!(
            "{} updated in {}",
            plural(self.links, "link"),
            plural(self.notes.len(), "note")
        );
        if !self.skipped.is_empty() {
            out.push_str(&format!(
                " · {} could not be updated",
                plural(self.skipped.len(), "note")
            ));
        }
        Some(out)
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// One rename, described the two ways the resolver has to see the vault: as
/// it was, with the note at `old`, and as it is now, with it at `new`. A link
/// is rewritten when it reached the note before and would not any more.
struct Rename {
    old: PathBuf,
    new: PathBuf,
    stem: String,
    before: Vec<Entry>,
    after: Vec<Entry>,
}

impl Rename {
    /// The target `raw` should become, or `None` to leave it alone. The folder
    /// part of a path-shaped target and a written-out `.md` both survive, so
    /// `[[stories/spec.md]]` becomes `[[stories/plan.md]]` and not `[[plan]]`.
    fn retarget(&self, raw: &str) -> Option<String> {
        let reached_old =
            index::resolve_by_name(&self.before, raw).is_some_and(|e| e.path == self.old);
        if !reached_old {
            return None;
        }
        let (head, tail) = match raw.rfind('/') {
            Some(i) => (&raw[..=i], &raw[i + 1..]),
            None => ("", raw),
        };
        let suffix = if tail.to_lowercase().ends_with(".md") {
            ".md"
        } else {
            ""
        };
        let target = format!("{head}{}{suffix}", self.stem);
        // another note may already own the new name; then the rewrite would
        // point somewhere else, and a link left broken is the honest outcome
        index::resolve_by_name(&self.after, &target)
            .is_some_and(|e| e.path == self.new)
            .then_some(target)
    }

    /// One line with every link to the old name rewritten, and how many were.
    /// Both spellings of a link are covered: `[[old]]` and `[text](old.md)`.
    fn rewrite_line(&self, line: &str) -> (String, usize) {
        let src: Vec<char> = line.chars().collect();
        // (from, to, replacement), in line order; the two scanners never
        // overlap, since neither reads a link the other recognises
        let mut edits: Vec<(usize, usize, String)> = Vec::new();
        for w in crate::md::wikilinks(line) {
            let (from, to) = target_span(&src, &w);
            let raw: String = src[from..to].iter().collect();
            if let Some(target) = self.retarget(&raw) {
                edits.push((from, to, target));
            }
        }
        for l in crate::md::md_links(line) {
            if let Some(href) = self.retarget_href(&l.href(&src)) {
                edits.push((l.href_start, l.href_end, href));
            }
        }
        edits.sort_by_key(|e| e.0);
        let mut out = String::new();
        let mut at = 0;
        for (from, to, target) in &edits {
            out.extend(&src[at..*from]);
            out.push_str(target);
            at = *to;
        }
        out.extend(&src[at..]);
        (out, edits.len())
    }

    /// The href a `[text](href)` should become, or `None` to leave it alone:
    /// only one that names a note (see `md::note_href`) and reached the old
    /// file. The `#fragment` survives, and spaces go back as `%20`.
    fn retarget_href(&self, href: &str) -> Option<String> {
        let path = crate::md::note_href(href)?;
        let (name, fragment) = crate::md::split_fragment(&path);
        let target = self.retarget(name)?;
        let target = crate::md::percent_encode_spaces(&target);
        Some(match fragment {
            Some(f) => format!("{target}#{f}"),
            None => target,
        })
    }

    /// A whole note rewritten, or `None` when no link in it needed to be.
    fn rewrite(&self, body: &str) -> Option<(String, usize)> {
        rewrite_body(body, |line| self.rewrite_line(line))
    }
}

/// `body` with `line` applied to every prose line, and how many edits that
/// made — or `None` when none. Front matter and fenced code are stepped over,
/// as the mentions scan steps over them: a link there is not a link the
/// reader could click.
fn rewrite_body(
    body: &str,
    mut line: impl FnMut(&str) -> (String, usize),
) -> Option<(String, usize)> {
    let front = notes::front_matter_range(body).map_or(0, |r| r.end);
    let mut out = body[..front].to_string();
    let mut done = 0;
    let mut fenced = false;
    for src in body[front..].split_inclusive('\n') {
        if crate::md::is_fence(src) {
            fenced = !fenced;
        }
        if fenced {
            out.push_str(src);
            continue;
        }
        let (rewritten, n) = line(src);
        out.push_str(&rewritten);
        done += n;
    }
    (done > 0).then_some((out, done))
}

/// The one heading whose text changed between two versions of a note, as
/// `(old, new)` — the case a save can follow up on by fixing the
/// `[[note#Old]]` links to it. `None` when the headings are the same, when
/// more than one changed, or when one was added or removed: a heading that
/// moved cannot be told from one deleted and another written, and a guess
/// there would rewrite links to the wrong place. Also `None` when `old` still
/// heads a section, since the links to it still land.
pub fn heading_change(before: &str, after: &str) -> Option<(String, String)> {
    let headings = |body: &str| -> Vec<String> {
        notes::prose_lines(body)
            .filter_map(|(_, l)| crate::md::heading_text(l).map(str::to_string))
            .collect()
    };
    let (a, b) = (headings(before), headings(after));
    if a.len() != b.len() {
        return None;
    }
    let mut changed = a.iter().zip(&b).filter(|(x, y)| x != y);
    let (old, new) = changed.next()?;
    if changed.next().is_some() || old.is_empty() || new.is_empty() {
        return None;
    }
    let same = |x: &String| x.to_lowercase() == old.to_lowercase();
    if b.iter().any(same) {
        return None;
    }
    Some((old.clone(), new.clone()))
}

/// The char span of the `#fragment` inside `[[…]]`, past the `#`: up to the
/// `|` or the closing brackets. `None` when the link names no place.
fn fragment_span(src: &[char], w: &crate::md::Wikilink) -> Option<(usize, usize)> {
    let body = w.start + 2;
    let close = w.end - 2;
    let hash = (body..close).find(|&k| matches!(src[k], '|' | '#'))?;
    if src[hash] != '#' {
        return None;
    }
    let end = (hash + 1..close).find(|&k| src[k] == '|').unwrap_or(close);
    Some((hash + 1, end))
}

/// One line with every `[[target#old]]` whose target `points_here` given
/// `new` as its fragment instead, and how many were. `[[#old]]` — a place
/// in the note being read — is left alone: the link is in another note, so
/// it names that note's heading. Block ids are never headings.
pub fn rewrite_fragment_line(
    line: &str,
    old: &str,
    new: &str,
    points_here: impl Fn(&str) -> bool,
) -> (String, usize) {
    let src: Vec<char> = line.chars().collect();
    let want = old.trim().to_lowercase();
    let mut edits: Vec<(usize, usize)> = Vec::new();
    for w in crate::md::wikilinks(line) {
        let Some(f) = w.fragment.as_deref() else {
            continue;
        };
        if f.starts_with('^') || f.trim().to_lowercase() != want {
            continue;
        }
        if w.target.is_empty() || !points_here(&w.target) {
            continue;
        }
        if let Some(span) = fragment_span(&src, &w) {
            edits.push(span);
        }
    }
    let mut out = String::new();
    let mut at = 0;
    for (from, to) in &edits {
        out.extend(&src[at..*from]);
        out.push_str(new);
        at = *to;
    }
    out.extend(&src[at..]);
    (out, edits.len())
}

/// The heading `old` of the note at `note` is now called `new`: rewrite the
/// `[[note#old]]` fragments under `roots` that pointed at it. The note itself
/// is left alone, for the reason [`retarget`] leaves the renamed note alone.
pub fn retarget_heading(note: &Path, old: &str, new: &str, roots: &[PathBuf]) -> Report {
    let note = fs::canonicalize(note).unwrap_or_else(|_| note.to_path_buf());
    let found = notes_under(roots);
    let (_, entries) = views(&found, &note, &note);
    let points_here =
        |target: &str| index::resolve(&entries, target).is_some_and(|e| e.path == note);
    let mut report = Report::default();
    for (path, _) in found {
        if path == note {
            continue;
        }
        let Ok(body) = fs::read_to_string(&path) else {
            report.skipped.push(path);
            continue;
        };
        let Some((rewritten, n)) = rewrite_body(&body, |line| {
            rewrite_fragment_line(line, old, new, points_here)
        }) else {
            continue;
        };
        if write_atomic(&path, &rewritten).is_ok() {
            report.links += n;
            report.notes.push(path);
        } else {
            report.skipped.push(path);
        }
    }
    report
}

/// The char span of the target inside `[[…]]`: past the brackets, before the
/// first `|`, before any `#`, and without the whitespace either side, so what
/// is put back sits exactly where what was typed sat.
fn target_span(src: &[char], w: &crate::md::Wikilink) -> (usize, usize) {
    let body = w.start + 2;
    let close = w.end - 2;
    let mut end = (body..close)
        .find(|&k| matches!(src[k], '|' | '#'))
        .unwrap_or(close);
    let mut start = body;
    while start < end && src[start].is_whitespace() {
        start += 1;
    }
    while end > start && src[end - 1].is_whitespace() {
        end -= 1;
    }
    (start, end)
}

/// Every `.md` file under `roots` with its path relative to the root it was
/// found under, walked the way quick-open walks: what cannot be opened from
/// ^O is not a note worth rewriting.
fn notes_under(roots: &[PathBuf]) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    index::walk_notes(roots, None, |root, path, _| {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        out.push((path, rel));
    });
    out
}

/// One walked file as the resolver sees it. The fields a walk of the vault
/// cannot answer cheaply — when the note was touched, which folder ^O files
/// it under — are not what resolving reads.
fn entry_at(path: &Path, rel: &str) -> Entry {
    let (title, aliases) = index::head_at(path);
    Entry {
        path: path.to_path_buf(),
        title,
        rel: rel.to_string(),
        folder: String::new(),
        modified: std::time::SystemTime::UNIX_EPOCH,
        aliases,
        name: Entry::name_of(path),
    }
}

/// The vault as the resolver has to see it, before and after the rename. The
/// note keeps its title across a rename, so the one entry that moves is the
/// same entry at a different path.
fn views(found: &[(PathBuf, String)], old: &Path, new: &Path) -> (Vec<Entry>, Vec<Entry>) {
    let mut before = Vec::new();
    let mut after = Vec::new();
    for (path, rel) in found {
        let entry = entry_at(path, rel);
        if path == new {
            let mut moved = entry.clone();
            moved.path = old.to_path_buf();
            moved.name = Entry::name_of(old);
            moved.rel = sibling_rel(rel, old);
            before.push(moved);
        } else {
            before.push(entry.clone());
        }
        after.push(entry);
    }
    (before, after)
}

/// `rel` with its filename swapped for `old`'s: the two paths share a folder,
/// which is what a rename (as opposed to a move) means.
fn sibling_rel(rel: &str, old: &Path) -> String {
    let name = old
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match rel.rfind('/') {
        Some(i) => format!("{}{name}", &rel[..=i]),
        None => name,
    }
}

/// The note at `old` now lives at `new`: rewrite every link under `roots`
/// that pointed at it. The renamed note itself is left alone — the app holds
/// it in the editor, and the next save would write the old text back over
/// anything done here.
pub fn retarget(old: &Path, new: &Path, roots: &[PathBuf]) -> Report {
    // the walk yields canonical paths, and the note has to be found among them
    let new = fs::canonicalize(new).unwrap_or_else(|_| new.to_path_buf());
    let old = match (old.parent(), old.file_name()) {
        (Some(dir), Some(name)) => fs::canonicalize(dir)
            .unwrap_or_else(|_| dir.to_path_buf())
            .join(name),
        _ => old.to_path_buf(),
    };
    let Some(stem) = new.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
        return Report::default();
    };
    let found = notes_under(roots);
    let (before, after) = views(&found, &old, &new);
    let rename = Rename {
        old,
        new,
        stem,
        before,
        after,
    };
    let mut report = Report::default();
    for (path, _) in found {
        if path == rename.new {
            continue;
        }
        let Ok(body) = fs::read_to_string(&path) else {
            report.skipped.push(path);
            continue;
        };
        let Some((rewritten, n)) = rename.rewrite(&body) else {
            continue;
        };
        if write_atomic(&path, &rewritten).is_ok() {
            report.links += n;
            report.notes.push(path);
        } else {
            report.skipped.push(path);
        }
    }
    report
}

/// One merge, as the resolver has to see it: the note at `old` folded into
/// the note at `into`, where it is now a section called `heading`.
///
/// A link that reached the old note has to reach that section instead, so the
/// target is rewritten and the heading becomes the fragment — unless the link
/// already named a place, which travelled into the target with the rest of
/// the note and is still called what it was called.
struct Merge {
    old: PathBuf,
    heading: String,
    /// What a link should say to reach the target: its stem, or its path when
    /// the stem alone would land somewhere else. `None` when nothing reaches
    /// it, and then there is nothing to rewrite.
    target: Option<String>,
    before: Vec<Entry>,
}

impl Merge {
    /// Did `raw` reach the merged note, the way a click on it would have?
    /// Read by name and not by alias, for the reason [`Rename::retarget`]
    /// leaves an alias alone: the target carries the aliases now, so a link
    /// written to one still lands.
    fn reached_old(&self, raw: &str) -> bool {
        index::resolve_by_name(&self.before, raw).is_some_and(|e| e.path == self.old)
    }

    /// One line with every link to the merged note pointed at its section,
    /// and how many were.
    fn rewrite_line(&self, line: &str) -> (String, usize) {
        let Some(target) = self.target.as_deref() else {
            return (line.to_string(), 0);
        };
        let src: Vec<char> = line.chars().collect();
        let mut edits: Vec<(usize, usize, String)> = Vec::new();
        for w in crate::md::wikilinks(line) {
            let (from, to) = target_span(&src, &w);
            let raw: String = src[from..to].iter().collect();
            if raw.is_empty() || !self.reached_old(&raw) {
                continue;
            }
            let text = match w.fragment {
                Some(_) => target.to_string(),
                None => format!("{target}#{}", self.heading),
            };
            edits.push((from, to, text));
        }
        for l in crate::md::md_links(line) {
            if let Some(href) = self.retarget_href(&l.href(&src)) {
                edits.push((l.href_start, l.href_end, href));
            }
        }
        edits.sort_by_key(|e| e.0);
        let mut out = String::new();
        let mut at = 0;
        for (from, to, text) in &edits {
            out.extend(&src[at..*from]);
            out.push_str(text);
            at = *to;
        }
        out.extend(&src[at..]);
        (out, edits.len())
    }

    /// The href a `[text](href)` should become: the target's file, and the
    /// section as its fragment when the link named no other place.
    fn retarget_href(&self, href: &str) -> Option<String> {
        let target = self.target.as_deref()?;
        let path = crate::md::note_href(href)?;
        let (name, fragment) = crate::md::split_fragment(&path);
        if !self.reached_old(name) {
            return None;
        }
        let fragment = fragment.unwrap_or(&self.heading);
        Some(crate::md::percent_encode_spaces(&format!(
            "{target}.md#{fragment}"
        )))
    }
}

/// The note at `old` has been folded into the note at `into` under a heading
/// called `heading`: point every link under `roots` that reached it at that
/// section. The merged note itself is left alone — it is on its way to the
/// trash, and its body is already in the target.
pub fn retarget_merged(old: &Path, into: &Path, heading: &str, roots: &[PathBuf]) -> Report {
    merge_pass(old, into, heading, roots, true)
}

/// What [`retarget_merged`] would do, without touching a file: how many links
/// stand in how many notes. What the confirmation counts before anything moves.
pub fn merged_links(old: &Path, into: &Path, heading: &str, roots: &[PathBuf]) -> (usize, usize) {
    let report = merge_pass(old, into, heading, roots, false);
    (report.links, report.notes.len())
}

/// The pass both of those are: walk the vault, rewrite what points at the
/// merged note, and write it back only when asked to.
fn merge_pass(old: &Path, into: &Path, heading: &str, roots: &[PathBuf], write: bool) -> Report {
    let into = fs::canonicalize(into).unwrap_or_else(|_| into.to_path_buf());
    let old = fs::canonicalize(old).unwrap_or_else(|_| old.to_path_buf());
    let found = notes_under(roots);
    let before: Vec<Entry> = found.iter().map(|(p, rel)| entry_at(p, rel)).collect();
    // the vault as it will stand: the merged note gone, and the target left
    // to answer for it
    let after: Vec<Entry> = before.iter().filter(|e| e.path != old).cloned().collect();
    let names = [
        into.file_stem().map(|s| s.to_string_lossy().into_owned()),
        found
            .iter()
            .find(|(p, _)| *p == into)
            .map(|(_, rel)| rel.trim_end_matches(".md").to_string()),
    ];
    let merge = Merge {
        target: names
            .into_iter()
            .flatten()
            .find(|t| index::resolve_by_name(&after, t).is_some_and(|e| e.path == into)),
        old,
        heading: heading.trim().to_string(),
        before,
    };
    let mut report = Report::default();
    for (path, _) in found {
        if path == merge.old {
            continue;
        }
        let Ok(body) = fs::read_to_string(&path) else {
            report.skipped.push(path);
            continue;
        };
        let Some((rewritten, n)) = rewrite_body(&body, |line| merge.rewrite_line(line)) else {
            continue;
        };
        if !write || write_atomic(&path, &rewritten).is_ok() {
            report.links += n;
            report.notes.push(path);
        } else {
            report.skipped.push(path);
        }
    }
    report
}

/// The line a link's fragment lands on: for `^id`, the line that carries
/// ` ^id` at its end; for anything else, the first heading whose text (the
/// `#`s and any block id dropped, trimmed) is the fragment, case-insensitively.
/// Fenced code is stepped over — a `# comment` in a shell snippet is not a
/// heading. `None` when the note has no such place.
pub fn find_anchor(lines: &[String], fragment: &str) -> Option<usize> {
    let fragment = fragment.trim();
    if let Some(id) = fragment.strip_prefix('^') {
        return lines.iter().position(|l| {
            crate::md::block_id_at(l).is_some_and(|(_, got)| got.eq_ignore_ascii_case(id))
        });
    }
    let want = fragment.to_lowercase();
    let mut fenced = false;
    lines.iter().position(|line| {
        if crate::md::is_fence(line) {
            fenced = !fenced;
            return false;
        }
        !fenced && crate::md::heading_text(line).is_some_and(|t| t.to_lowercase() == want)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::write;

    fn tmpdir(name: &str) -> PathBuf {
        crate::testutil::tmpdir("links", name)
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    /// Rename `from` to `to` on disk and run the pass, the way the app does.
    fn renamed(dir: &Path, from: &str, to: &str) -> Report {
        let (old, new) = (dir.join(from), dir.join(to));
        fs::rename(&old, &new).unwrap();
        retarget(&old, &new, &[dir.to_path_buf()])
    }

    #[test]
    fn a_bare_link_to_the_old_name_points_at_the_new_one() {
        let dir = tmpdir("bare");
        write(&dir, "groceries.md", "# Groceries\n");
        let other = write(&dir, "other.md", "see [[groceries]] later\n");
        let r = renamed(&dir, "groceries.md", "shopping.md");
        assert_eq!(read(&other), "see [[shopping]] later\n");
        assert_eq!(r.links, 1);
        assert_eq!(r.notes, vec![other]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_link_written_to_a_front_matter_alias_is_not_rewritten() {
        let dir = tmpdir("front-alias");
        write(
            &dir,
            "groceries.md",
            "---\naliases: [shopping list]\n---\n# Groceries\n",
        );
        let other = write(&dir, "other.md", "[[shopping list]] and [[groceries]]\n");
        let r = renamed(&dir, "groceries.md", "food.md");
        // the alias still reaches the note after the rename; only the filename link moved
        assert_eq!(read(&other), "[[shopping list]] and [[food]]\n");
        assert_eq!(r.links, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_alias_and_the_heading_survive_the_rewrite() {
        let dir = tmpdir("forms");
        write(&dir, "groceries.md", "# Groceries\n");
        let other = write(
            &dir,
            "other.md",
            "[[groceries|the list]] [[groceries#Fruit]] [[groceries#Fruit|fruit]] [[ groceries ]]\n",
        );
        renamed(&dir, "groceries.md", "shopping.md");
        assert_eq!(
            read(&other),
            "[[shopping|the list]] [[shopping#Fruit]] [[shopping#Fruit|fruit]] [[ shopping ]]\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_link_is_matched_without_its_case_or_its_md_suffix() {
        let dir = tmpdir("case");
        write(&dir, "Story-Matrix.md", "# Story Matrix\n");
        let other = write(
            &dir,
            "other.md",
            "[[story-matrix]] and [[Story-Matrix.md]]\n",
        );
        renamed(&dir, "Story-Matrix.md", "matrix.md");
        assert_eq!(read(&other), "[[matrix]] and [[matrix.md]]\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_markdown_link_to_the_file_is_rewritten_too() {
        let dir = tmpdir("md-link");
        write(&dir, "groceries.md", "# Groceries\n");
        write(&dir, "stories/old name.md", "# Old\n");
        let other = write(
            &dir,
            "other.md",
            "[G](groceries.md) [G](groceries.md#Fruit) [O](stories/old%20name.md) [x](https://groceries.md) [[groceries]]\n",
        );
        let r = renamed(&dir, "groceries.md", "shopping.md");
        assert_eq!(
            read(&other),
            "[G](shopping.md) [G](shopping.md#Fruit) [O](stories/old%20name.md) [x](https://groceries.md) [[shopping]]\n"
        );
        assert_eq!(r.links, 3);
        renamed(&dir, "stories/old name.md", "stories/new name.md");
        assert!(read(&other).contains("[O](stories/new%20name.md)"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_path_shaped_link_keeps_its_folder() {
        let dir = tmpdir("path");
        write(&dir, "stories/spec.md", "# Spec\n");
        let other = write(&dir, "other.md", "[[stories/spec]]\n");
        let r = renamed(&dir, "stories/spec.md", "stories/plan.md");
        assert_eq!(read(&other), "[[stories/plan]]\n");
        assert_eq!(r.links, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_link_that_resolved_to_another_note_of_the_same_name_is_left_alone() {
        let dir = tmpdir("shadow");
        write(&dir, "spec.md", "# Spec\n");
        write(&dir, "work/spec.md", "# Work Spec\n");
        // a bare [[spec]] means the one at the top, and always did
        let other = write(&dir, "other.md", "[[spec]] and [[work/spec]]\n");
        let r = renamed(&dir, "work/spec.md", "work/plan.md");
        assert_eq!(read(&other), "[[spec]] and [[work/plan]]\n");
        assert_eq!(r.links, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_link_by_title_follows_the_file_too() {
        let dir = tmpdir("title");
        write(&dir, "groceries.md", "# Groceries\n");
        // it still resolves by title, but a wikilink names a file, and the
        // file is what the reader renamed
        let other = write(&dir, "other.md", "[[Groceries]]\n");
        renamed(&dir, "groceries.md", "shopping.md");
        assert_eq!(read(&other), "[[shopping]]\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn links_are_counted_per_link_and_notes_per_note() {
        let dir = tmpdir("count");
        write(&dir, "groceries.md", "# Groceries\n");
        write(&dir, "a.md", "[[groceries]] twice [[groceries|x]]\n");
        write(&dir, "b.md", "once [[groceries#h]]\n");
        write(&dir, "c.md", "nothing here\n");
        let r = renamed(&dir, "groceries.md", "shopping.md");
        assert_eq!(r.links, 3);
        assert_eq!(r.notes.len(), 2);
        assert_eq!(r.describe().as_deref(), Some("3 links updated in 2 notes"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_report_of_nothing_says_nothing_and_one_of_one_is_singular() {
        assert_eq!(Report::default().describe(), None);
        let r = Report {
            links: 1,
            notes: vec![PathBuf::from("a.md")],
            skipped: vec![],
        };
        assert_eq!(r.describe().as_deref(), Some("1 link updated in 1 note"));
        let r = Report {
            links: 2,
            notes: vec![PathBuf::from("a.md")],
            skipped: vec![PathBuf::from("b.md"), PathBuf::from("c.md")],
        };
        assert_eq!(
            r.describe().as_deref(),
            Some("2 links updated in 1 note · 2 notes could not be updated")
        );
    }

    #[test]
    fn front_matter_and_fenced_code_are_stepped_over() {
        let dir = tmpdir("fence");
        write(&dir, "groceries.md", "# Groceries\n");
        let body = "---\nsee: \"[[groceries]]\"\n---\n[[groceries]]\n```\n[[groceries]]\n```\n~~~\n[[groceries]]\n~~~\n";
        let other = write(&dir, "other.md", body);
        let r = renamed(&dir, "groceries.md", "shopping.md");
        assert_eq!(
            read(&other),
            "---\nsee: \"[[groceries]]\"\n---\n[[shopping]]\n```\n[[groceries]]\n```\n~~~\n[[groceries]]\n~~~\n"
        );
        assert_eq!(r.links, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_renamed_note_itself_and_untouched_notes_are_not_written() {
        let dir = tmpdir("untouched");
        write(&dir, "groceries.md", "# Groceries\nme: [[groceries]]\n");
        let other = write(&dir, "other.md", "no links\n");
        let r = renamed(&dir, "groceries.md", "shopping.md");
        assert_eq!(
            read(&dir.join("shopping.md")),
            "# Groceries\nme: [[groceries]]\n"
        );
        assert_eq!(read(&other), "no links\n");
        assert_eq!(r, Report::default());
        assert!(!dir.join(".other.md.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rewrite_that_would_land_on_a_different_note_is_not_made() {
        let dir = tmpdir("taken");
        write(&dir, "plan.md", "# Plan\n");
        write(&dir, "work/spec.md", "# Spec\n");
        let other = write(&dir, "other.md", "[[spec]]\n");
        // a bare [[plan]] would open the note at the top, so the link is
        // left as it was rather than pointed at the wrong note
        let r = renamed(&dir, "work/spec.md", "work/plan.md");
        assert_eq!(read(&other), "[[spec]]\n");
        assert_eq!(r, Report::default());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_collision_suffix_is_followed() {
        let dir = tmpdir("suffix");
        write(&dir, "groceries.md", "# Groceries\n");
        write(&dir, "shopping.md", "# Shopping\n");
        let other = write(&dir, "other.md", "[[groceries]]\n");
        let r = renamed(&dir, "groceries.md", "shopping-2.md");
        assert_eq!(read(&other), "[[shopping-2]]\n");
        assert_eq!(r.links, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    fn lines(src: &str) -> Vec<String> {
        src.lines().map(str::to_string).collect()
    }

    #[test]
    fn a_heading_fragment_finds_the_first_heading_with_those_words() {
        let note = lines("# Spec\n\ntext\n\n## Method ^m1\n\n### method\n\n# Results");
        assert_eq!(find_anchor(&note, "Method"), Some(4));
        // case is not part of a heading's name, and neither is its block id
        assert_eq!(find_anchor(&note, "method"), Some(4));
        assert_eq!(find_anchor(&note, "RESULTS"), Some(8));
        assert_eq!(find_anchor(&note, " Spec "), Some(0));
        assert_eq!(find_anchor(&note, "Missing"), None);
        // a line that merely begins with the words is not the heading
        assert_eq!(find_anchor(&lines("Method\n# Methods"), "Method"), None);
    }

    #[test]
    fn a_heading_in_fenced_code_is_not_a_heading() {
        let note = lines("```sh\n# Install\n```\n\n# Install");
        assert_eq!(find_anchor(&note, "Install"), Some(4));
    }

    #[test]
    fn a_heading_with_a_block_id_and_closing_hashes_is_found_by_its_words() {
        let note = lines(
            "# Intro

## Setup ^abc ##
",
        );
        assert_eq!(find_anchor(&note, "setup"), Some(2));
    }

    #[test]
    fn a_caret_fragment_finds_the_line_carrying_that_block_id() {
        let note = lines("# Spec\n\nfirst ^abc\n\nsecond ^abc-2\n\n^solo");
        assert_eq!(find_anchor(&note, "^abc"), Some(2));
        assert_eq!(find_anchor(&note, "^ABC-2"), Some(4));
        assert_eq!(find_anchor(&note, "^solo"), Some(6));
        assert_eq!(find_anchor(&note, "^abc-"), None);
    }

    #[test]
    fn heading_change_finds_the_one_heading_that_was_renamed() {
        let before = "# Title\n\n## Old\ntext\n## Other\n";
        let after = "# Title\n\n## New\ntext\n## Other\n";
        assert_eq!(
            heading_change(before, after),
            Some(("Old".to_string(), "New".to_string()))
        );
        // nothing changed, two changed, one added: no single rename to follow
        assert_eq!(heading_change(before, before), None);
        assert_eq!(
            heading_change(before, "# T\n\n## N\ntext\n## Other\n"),
            None
        );
        assert_eq!(
            heading_change(before, "# Title\n\n## Old\n## New\n## Other\n"),
            None
        );
        // a `# comment` in a fence is not a heading
        let fenced = "# Title\n```sh\n# comment\n```\n";
        assert_eq!(
            heading_change(fenced, "# Title\n```sh\n# other\n```\n"),
            None
        );
    }

    #[test]
    fn heading_change_ignores_a_rename_that_leaves_the_old_heading_standing() {
        // a case-only change: `[[note#a]]` reached the heading before and
        // still does, since fragments match without case
        let before = "## A\n## B\n";
        let after = "## a\n## B\n";
        assert_eq!(heading_change(before, after), None);
    }

    #[test]
    fn fragments_are_rewritten_only_on_links_to_this_note() {
        let here = |t: &str| t.eq_ignore_ascii_case("spec");
        let line = "[[spec#Old]] ![[spec#old|see]] [[other#Old]] [[#Old]] [[spec#^abc]] [[spec]]";
        let (out, n) = rewrite_fragment_line(line, "Old", "New", here);
        assert_eq!(
            out,
            "[[spec#New]] ![[spec#New|see]] [[other#Old]] [[#Old]] [[spec#^abc]] [[spec]]"
        );
        assert_eq!(n, 2);
        let (same, none) = rewrite_fragment_line("plain text [[spec#Else]]", "Old", "New", here);
        assert_eq!(same, "plain text [[spec#Else]]");
        assert_eq!(none, 0);
    }

    #[test]
    fn a_merge_points_every_link_at_the_section_the_note_became() {
        let dir = tmpdir("merge");
        let spec = write(&dir, "spec.md", "# Spec\n");
        let plan = write(&dir, "plan.md", "# Plan\n");
        let other = write(
            &dir,
            "other.md",
            "[[spec]] [[spec|the spec]] [[spec#Method]] ![[spec]] [[spec#^abc]] [[plan]] [S](spec.md)\n",
        );
        let (links, notes) = merged_links(&spec, &plan, "Spec", std::slice::from_ref(&dir));
        assert_eq!((links, notes), (6, 1));
        // the count was a dry run: nothing moved until the merge did
        assert!(read(&other).starts_with("[[spec]] "));
        let r = retarget_merged(&spec, &plan, "Spec", std::slice::from_ref(&dir));
        assert_eq!(
            read(&other),
            "[[plan#Spec]] [[plan#Spec|the spec]] [[plan#Method]] ![[plan#Spec]] [[plan#^abc]] [[plan]] [S](plan.md#Spec)\n"
        );
        assert_eq!(r.links, 6);
        assert_eq!(r.notes, vec![other]);
        // the merged note is left alone; its body is already in the target
        assert_eq!(read(&spec), "# Spec\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_merge_across_folders_names_the_target_by_the_path_that_reaches_it() {
        let dir = tmpdir("merge-path");
        let spec = write(&dir, "spec.md", "# Spec\n");
        // a bare [[plan]] would open the one at the top, so the link has to
        // say where the target it means actually lives
        write(&dir, "plan.md", "# Plan\n");
        let target = write(&dir, "work/plan.md", "# Work Plan\n");
        let other = write(&dir, "other.md", "[[spec]]\n");
        retarget_merged(&spec, &target, "Spec", std::slice::from_ref(&dir));
        assert_eq!(read(&other), "[[work/plan#Spec]]\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_renamed_heading_updates_the_fragments_across_the_vault() {
        let dir = tmpdir("heading");
        let spec = write(&dir, "spec.md", "# Spec\n\n## New\n");
        let other = write(&dir, "other.md", "see [[spec#Old]] and [[Spec#old|it]]\n");
        let unrelated = write(&dir, "third.md", "[[plan#Old]]\n");
        let r = retarget_heading(&spec, "Old", "New", std::slice::from_ref(&dir));
        assert_eq!(read(&other), "see [[spec#New]] and [[Spec#New|it]]\n");
        assert_eq!(read(&unrelated), "[[plan#Old]]\n");
        assert_eq!(r.links, 2);
        assert_eq!(r.notes, vec![other]);
        let _ = fs::remove_dir_all(&dir);
    }
}

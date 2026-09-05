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
    fn rewrite_line(&self, line: &str) -> (String, usize) {
        let src: Vec<char> = line.chars().collect();
        let mut out = String::new();
        let mut done = 0;
        let mut at = 0;
        for w in crate::md::wikilinks(line) {
            let (from, to) = target_span(&src, &w);
            let raw: String = src[from..to].iter().collect();
            let Some(target) = self.retarget(&raw) else {
                continue;
            };
            out.extend(&src[at..from]);
            out.push_str(&target);
            at = to;
            done += 1;
        }
        out.extend(&src[at..]);
        (out, done)
    }

    /// A whole note rewritten, or `None` when no link in it needed to be.
    /// Front matter and fenced code are stepped over, as the mentions scan
    /// steps over them: a link there is not a link the reader could click.
    fn rewrite(&self, body: &str) -> Option<(String, usize)> {
        let front = notes::front_matter_range(body).map_or(0, |r| r.end);
        let mut out = body[..front].to_string();
        let mut done = 0;
        let mut fenced = false;
        for line in body[front..].split_inclusive('\n') {
            if crate::md::is_fence(line) {
                fenced = !fenced;
            }
            if fenced {
                out.push_str(line);
                continue;
            }
            let (rewritten, n) = self.rewrite_line(line);
            out.push_str(&rewritten);
            done += n;
        }
        (done > 0).then_some((out, done))
    }
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

/// The vault as the resolver has to see it, before and after the rename. The
/// note keeps its title across a rename, so the one entry that moves is the
/// same entry at a different path.
fn views(found: &[(PathBuf, String)], old: &Path, new: &Path) -> (Vec<Entry>, Vec<Entry>) {
    let mut before = Vec::new();
    let mut after = Vec::new();
    for (path, rel) in found {
        let (title, aliases) = index::head_at(path);
        let entry = Entry {
            path: path.clone(),
            title,
            rel: rel.clone(),
            folder: String::new(),
            modified: std::time::SystemTime::UNIX_EPOCH,
            aliases,
        };
        if path == new {
            let mut moved = entry.clone();
            moved.path = old.to_path_buf();
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
}

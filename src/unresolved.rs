//! Every `[[wikilink]]` in the vault that names a note the vault does not
//! have — the list Obsidian shows as unresolved links, and the fastest way to
//! find the typo or the note you meant to write.
//!
//! One walk over the roots, on a thread: the bodies are read once, the notes
//! met become the resolver, and each link is judged the way a click on it
//! would be. `[[#Heading]]` names a place in the note that holds it and is
//! never unresolved here; a heading that is not there is a different thing.

use crate::index::{self, Entry};
use crate::notes;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

/// One link to nowhere: where it stands, and what it was pointing at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Broken {
    pub path: PathBuf,
    /// The note's filename without `.md`, for the row.
    pub name: String,
    pub line: usize,
    pub target: String,
}

/// The `(line, target)` of every wikilink in `body` that `entries` cannot
/// resolve, in order, each target once per line. Front matter and fenced
/// code are stepped over, as they are everywhere a link is read.
pub fn broken_in(entries: &[Entry], body: &str) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    // prose lines are numbered from the top of the body; the row opens the
    // note at a line of the file
    let offset = notes::front_matter_range(body).map_or(0, |r| body[..r.end].lines().count());
    for (line_no, line) in notes::prose_lines(body) {
        let line_no = line_no + offset;
        for w in crate::md::wikilinks(line) {
            let target = w.target.trim();
            if target.is_empty() || index::resolve(entries, target).is_some() {
                continue;
            }
            // a link to a file that is not a note — a picture, a PDF — is
            // resolved elsewhere and is not a missing note
            if is_file_link(target) {
                continue;
            }
            let row = (line_no, target.to_string());
            if !out.contains(&row) {
                out.push(row);
            }
        }
    }
    out
}

/// A target with an extension other than `.md` names an attachment, not a
/// note: `[[report.pdf]]`, `[[photo.png]]`.
fn is_file_link(target: &str) -> bool {
    let name = target.rsplit('/').next().unwrap_or(target);
    match name.rfind('.') {
        Some(i) if i > 0 => {
            let ext = &name[i + 1..];
            !ext.is_empty()
                && ext.len() <= 5
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
                && !ext.eq_ignore_ascii_case("md")
        }
        _ => false,
    }
}

/// Walk `roots` and list every unresolved link, by note in walk order.
/// `cancel` set stops the walk with nothing to say.
pub fn scan(roots: &[PathBuf], cancel: &AtomicBool) -> Vec<Broken> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut bodies: Vec<(PathBuf, String)> = Vec::new();
    let walked = index::walk_notes(roots, Some(cancel), |root, path, _| {
        let body = fs::read_to_string(&path).ok();
        let (title, aliases) = match &body {
            Some(b) => (notes::title_of(b), crate::md::front_matter_aliases(b)),
            None => index::head_at(&path),
        };
        entries.push(Entry {
            path: path.clone(),
            title,
            rel: path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned(),
            folder: String::new(),
            modified: std::time::SystemTime::UNIX_EPOCH,
            aliases,
            name: Entry::name_of(&path),
        });
        if let Some(body) = body {
            bodies.push((path, body));
        }
    });
    if walked.is_none() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (path, body) in bodies {
        if cancel.load(Ordering::Relaxed) {
            return Vec::new();
        }
        let name = Entry::name_of(&path);
        for (line, target) in broken_in(&entries, &body) {
            out.push(Broken {
                path: path.clone(),
                name: name.clone(),
                line,
                target,
            });
        }
    }
    out
}

/// A scan running on a thread, the way `mentions::Pending` is: dropping it
/// asks the walk to give up at the next file.
pub struct Pending {
    rx: Receiver<Vec<Broken>>,
    cancel: Arc<AtomicBool>,
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Pending {
    /// The answer, if it has landed. `Some(empty)` when the worker died.
    pub fn poll(&self) -> Option<Vec<Broken>> {
        match self.rx.try_recv() {
            Ok(rows) => Some(rows),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Vec::new()),
        }
    }
}

/// Start a scan on a thread and hand back the handle to it. Never joined:
/// quitting mid-walk drops the handle and the thread finishes on its own.
pub fn spawn(roots: Vec<PathBuf>) -> Pending {
    let (tx, rx) = std::sync::mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    std::thread::spawn(move || {
        let _ = tx.send(scan(&roots, &flag));
    });
    Pending { rx, cancel }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::write;

    fn entry(name: &str) -> Entry {
        Entry {
            path: PathBuf::from(format!("/v/{name}.md")),
            title: name.to_string(),
            rel: format!("{name}.md"),
            folder: String::new(),
            modified: std::time::SystemTime::UNIX_EPOCH,
            aliases: Vec::new(),
            name: name.to_string(),
        }
    }

    #[test]
    fn links_the_index_cannot_resolve_are_listed_once_per_line() {
        let entries = vec![entry("spec"), entry("plan")];
        let body = "---\ntitle: x\n---\n# T\n[[spec]] [[nope]] [[nope|again]] [[plan#Part]]\n\n```\n[[in-code]]\n```\n[[#Here]] [[photo.png]] [[Missing/deep]]\n";
        assert_eq!(
            broken_in(&entries, body),
            vec![(4, "nope".to_string()), (9, "Missing/deep".to_string()),]
        );
    }

    #[test]
    fn the_scan_names_the_note_and_the_line() {
        let dir = crate::testutil::tmpdir("unresolved", "scan");
        write(&dir, "a.md", "# A\n[[b]] and [[c]]\n");
        write(&dir, "b.md", "# B\n");
        let rows = scan(std::slice::from_ref(&dir), &AtomicBool::new(false));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "a");
        assert_eq!(rows[0].line, 1);
        assert_eq!(rows[0].target, "c");
        let _ = fs::remove_dir_all(&dir);
    }
}

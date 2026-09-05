//! Linked mentions: the notes that link to the one you are reading.
//!
//! This is a scan, and deliberately not a graph. Answering "what links here"
//! means reading every `.md` body under the same roots quick-open walks — a
//! couple of thousand notes is a few tens of megabytes and something like a
//! tenth of a second of IO. That is far too much to spend on the way to a
//! frame, and nowhere near enough to justify an index file: a persistent link
//! graph would make the answer instant, but it would be a file that has to
//! stay in step with every other program that writes into the vault, and a
//! stale graph is worse than a slow one because it lies. catcher keeps
//! nothing beside the notes today and does not want to start here.
//!
//! So the cost is paid honestly and off the draw path: the reading view asks,
//! a worker thread walks, the first frame is drawn without a footer, and the
//! footer appears a moment later when the scan lands. The answer is then
//! cached against the note and a generation counter, so flipping ^P back and
//! forth is free and a save is what makes it look again.

use crate::index::{self, Entry};
use crate::notes;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::time::SystemTime;

/// One row of the footer: a note that links here, and the first thing it says
/// around the link.
#[derive(Clone, Debug, PartialEq, Hash)]
pub struct Mention {
    pub path: PathBuf,
    /// The file's stem, which is how every other list in the app names a note.
    pub name: String,
    /// Raw markdown around the link, centred on it; the renderer styles it.
    pub excerpt: String,
    /// Where the link sits in `excerpt`, in chars, so the renderer can keep it
    /// on screen when the excerpt is cut to the page.
    pub link: (usize, usize),
    /// How many times that note links here. Several mentions collapse to one
    /// row — the row names a note, and a note is named once.
    pub count: usize,
    /// `true` for a `[[link]]` to this note; `false` for an unlinked mention —
    /// the note's title or one of its aliases written as plain words. The
    /// footer draws the two under separate headings.
    pub linked: bool,
}

/// One `[[link]]` found pointing this way, before it has been confirmed.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    /// Line in the body, counted below the front matter — the footer has no
    /// use for it, but it is what makes "which line was that on" testable.
    pub line: usize,
    /// The target exactly as it was written, so the resolver gets the same
    /// string a click on the link in the body would have given it.
    pub target: String,
    pub excerpt: String,
    /// The link's span in `excerpt`, in chars.
    pub link: (usize, usize),
}

/// A body past this is not prose any more — a pasted log, a generated dump, a
/// file someone's tooling writes — and reading it costs far more than the one
/// row it could ever produce.
const MAX_BODY_BYTES: u64 = 256 * 1024;
/// The footer is a footer, not a search results page. Past this many notes the
/// answer to "what links here" is "lots", and the list stops being readable.
const MAX_MENTIONS: usize = 50;
/// Unlinked mentions are noisier than links — a note called `Notes` is named
/// everywhere — so fewer rows are drawn before the footer says `N more`.
pub const MAX_UNLINKED_ROWS: usize = 20;
/// Chars kept either side of the link. The link is the reason the row exists,
/// so the excerpt is cut around it rather than from the start; the real cut to
/// the page width happens at render time, which is the only place that knows
/// the width.
const BEFORE_LINK: usize = 60;
const AFTER_LINK: usize = 120;

/// The note on screen, described the way the index would describe it, so that
/// [`index::link_keys`] can say what a `[[wikilink]]` could reach it by and
/// [`index::resolve`] can rank it against everything else. Building it here
/// rather than looking it up in the quick-open index is deliberate: the note
/// you are reading may have been opened from somewhere the index never walked.
pub fn target_entry(path: &Path, title: &str, roots: &[PathBuf]) -> Entry {
    Entry {
        title: title.to_string(),
        rel: rel_under(path, roots),
        // nothing in this module shows a folder; the footer names notes
        folder: String::new(),
        modified: SystemTime::UNIX_EPOCH,
        // its aliases too, so a `[[launch]]` elsewhere counts as a link here
        aliases: index::head_at(path).1,
        name: Entry::name_of(path),
        path: path.to_path_buf(),
    }
}

/// A path as the index would spell it: relative to the first root it sits
/// under, and `~/`-shortened when it sits under none of them. Same rule as the
/// walk, so `[[stories/story-matrix]]` names the note here that it names
/// there.
fn rel_under(path: &Path, roots: &[PathBuf]) -> String {
    for root in roots {
        if let Ok(rel) = path.strip_prefix(root) {
            return rel.to_string_lossy().into_owned();
        }
    }
    index::short(path)
}

/// Every `[[link]]` in `body` whose target is one of `names` — the names the
/// note on screen answers to.
///
/// The match is by name and is only a candidate: two notes can both be called
/// `spec`, and only the resolver can say which one a link meant. [`scan`] is
/// where that is settled; doing it the other way round — resolving every link
/// in the vault — would be the same work with a resolver call per link
/// instead of per hit.
pub fn mentions_in(body: &str, names: &[String]) -> Vec<Hit> {
    let mut out = Vec::new();
    for (line_no, line) in notes::prose_lines(body) {
        let mut hits: Vec<(usize, usize, String)> = crate::md::wikilinks(line)
            .into_iter()
            .map(|w| (w.start, w.end, w.target))
            .collect();
        // a `[text](other.md)` is a link to a note as much as `[[other]]` is
        let src: Vec<char> = line.chars().collect();
        for l in crate::md::md_links(line) {
            if let Some(path) = crate::md::note_href(&l.href(&src)) {
                let name = crate::md::split_fragment(&path).0.to_string();
                hits.push((l.start, l.end, name));
            }
        }
        hits.sort_by_key(|h| h.0);
        for (start, end, target) in hits {
            if names.contains(&crate::md::link_key(&target)) {
                let (excerpt, link) = excerpt(line, start, end);
                out.push(Hit {
                    line: line_no,
                    target,
                    excerpt,
                    link,
                });
            }
        }
    }
    out
}

/// Every place `body` says one of `words` — the note's title or an alias — as
/// a whole word, whatever its case, outside any `[[wikilink]]`. A link is
/// already counted by [`mentions_in`]; this finds the mentions that could have
/// been links and are not.
///
/// Whole word means the match is not flanked by a letter, digit or `_`, so
/// `spec` is not found inside `specific` or `respect`. Fenced code and front
/// matter are stepped over as they are for links. The `link` span of each hit
/// is the matched words, so the renderer can keep them on screen and undim
/// them.
pub fn unlinked_in(body: &str, words: &[String]) -> Vec<Hit> {
    let wants: Vec<Vec<char>> = words
        .iter()
        .map(|w| fold_case(w.trim()))
        .filter(|w| !w.is_empty())
        .collect();
    if wants.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (line_no, line) in notes::prose_lines(body) {
        let chars: Vec<char> = line.chars().collect();
        // `fold_case` keeps the char count, so the two index the same columns
        let lowered = fold_case(line);
        let links: Vec<(usize, usize)> = crate::md::wikilinks(line)
            .into_iter()
            .map(|w| (w.start, w.end))
            .collect();
        let inside_link = |a: usize, b: usize| links.iter().any(|(s, e)| b > *s && a < *e);
        let mut i = 0;
        while i < lowered.len() {
            // the longest word that starts here, so an alias that extends the
            // title (`spec`, `spec sheet`) is matched whole
            let len = wants
                .iter()
                .filter(|w| lowered[i..].starts_with(w))
                .map(Vec::len)
                .max();
            let Some(len) = len else {
                i += 1;
                continue;
            };
            let end = i + len;
            let whole = (i == 0 || !is_word_char(lowered[i - 1]))
                && (end >= lowered.len() || !is_word_char(lowered[end]));
            if !whole || inside_link(i, end) {
                i += 1;
                continue;
            }
            let (excerpt, link) = excerpt(line, i, end);
            out.push(Hit {
                line: line_no,
                target: chars[i..end].iter().collect(),
                excerpt,
                link,
            });
            i = end;
        }
    }
    out
}

/// A letter, digit or underscore: what a word is made of, and what on either
/// side of a match says it is part of a longer word.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Lowercased one char at a time, so the result has as many chars as the input
/// and the columns line up. A char whose lowercase is several chars (`İ`) is
/// kept as it is rather than shifting every column after it.
fn fold_case(s: &str) -> Vec<char> {
    s.chars()
        .map(|c| {
            let mut lower = c.to_lowercase();
            match (lower.next(), lower.next()) {
                (Some(l), None) => l,
                _ => c,
            }
        })
        .collect()
}

/// The `aliases:` a note's front matter declares, inline (`aliases: a, b`,
/// with or without brackets) or as a YAML list under the key — the other
/// names the note answers to, so a note that says one of them is mentioning
/// this one. Only a top-level key counts, the way `tags:` is read.
pub fn aliases_of(content: &str) -> Vec<String> {
    let lines: Vec<&str> = content.lines().collect();
    let Some(end) = notes::front_matter_end(lines.iter().copied()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut i = 1;
    while i < end {
        let Some(rest) = lines[i]
            .strip_prefix("aliases:")
            .or_else(|| lines[i].strip_prefix("alias:"))
        else {
            i += 1;
            continue;
        };
        let rest = rest.trim();
        if !rest.is_empty() {
            for a in rest
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split(',')
            {
                push_alias(&mut out, a);
            }
            i += 1;
            continue;
        }
        // a list: `- a` rows, indented or not, until the next key
        i += 1;
        while i < end {
            let line = lines[i];
            match line.trim_start().strip_prefix('-') {
                Some(item) => push_alias(&mut out, item),
                None if line.starts_with([' ', '\t']) => {}
                None => break,
            }
            i += 1;
        }
    }
    out
}

/// One alias, shed of the quotes people write around one with a colon in it.
fn push_alias(out: &mut Vec<String>, text: &str) {
    let a = text.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    if !a.is_empty() {
        out.push(a.to_string());
    }
}

/// The text around a mention, as one line of markdown centred on the link.
///
/// The link is what the row is about, so the cut is made around it: up to
/// [`BEFORE_LINK`] chars before and [`AFTER_LINK`] after, with `…` on any side
/// that was trimmed. A table row is reduced to the cell holding the link,
/// because the pipes and the other cells are the table's business, and the
/// markers a line is drawn with (bullet, quote, heading hashes) go away. What
/// is left is still markdown — `**bold**`, `[[link]]` — and the renderer
/// styles it. Returns the excerpt and the link's span within it, in chars.
/// `at`/`end` are the link's own source columns, in chars.
pub fn excerpt(line: &str, at: usize, end: usize) -> (String, (usize, usize)) {
    let mut chars: Vec<char> = line.chars().collect();
    let mut at = at.min(chars.len());
    let mut end = end.min(chars.len());
    // a table row: keep only the cell the link is in
    if line.trim_start().starts_with('|') {
        let prev = chars[..at]
            .iter()
            .rposition(|c| *c == '|')
            .map_or(0, |p| p + 1);
        let next = chars[end..]
            .iter()
            .position(|c| *c == '|')
            .map_or(chars.len(), |p| end + p);
        chars = chars[prev..next].to_vec();
        at -= prev;
        end -= prev;
    } else {
        let text: String = chars.iter().collect();
        let stripped = strip_markers(&text);
        let removed = chars.len() - stripped.chars().count();
        if removed <= at {
            chars = stripped.chars().collect();
            at -= removed;
            end -= removed;
        }
    }
    // whatever the writing looked like, the footer row is one line: runs of
    // whitespace collapse, and the link's columns move with them
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    let (mut new_at, mut new_end) = (0, out.len());
    for (i, c) in chars.iter().enumerate() {
        if i == at {
            new_at = out.len();
        }
        if i == end {
            new_end = out.len();
        }
        if c.is_whitespace() {
            if !out.is_empty() && out.last() != Some(&' ') {
                out.push(' ');
            }
        } else {
            out.push(*c);
        }
    }
    if end >= chars.len() {
        new_end = out.len();
    }
    while out.last() == Some(&' ') {
        out.pop();
    }
    let new_end = new_end.min(out.len());
    let start = new_at.saturating_sub(BEFORE_LINK);
    let stop = (new_end + AFTER_LINK).min(out.len());
    let mut text = String::new();
    let mut shift = 0;
    if start > 0 {
        text.push('…');
        shift = 1;
    }
    text.extend(out[start..stop].iter());
    if stop < out.len() {
        text.push('…');
    }
    (text, (new_at - start + shift, new_end - start + shift))
}

/// The markers a line is drawn with rather than the words it says: a heading's
/// hashes, a bullet, a quote mark, a checkbox, a callout's `[!kind]`. Only a
/// prefix is stripped, so the columns after it move by a fixed amount.
fn strip_markers(s: &str) -> String {
    let t = s.trim_start();
    let t = t.trim_start_matches('#').trim_start();
    let t = ["- ", "* ", "+ ", "> "]
        .iter()
        .find_map(|m| t.strip_prefix(m))
        .unwrap_or(t);
    let t = ["[ ] ", "[x] ", "[X] "]
        .iter()
        .find_map(|m| t.strip_prefix(m))
        .unwrap_or(t);
    let t = match t.strip_prefix("[!") {
        Some(rest) => rest
            .split_once(']')
            .map(|(_, after)| after.trim_start())
            .unwrap_or(t),
        None => t,
    };
    t.to_string()
}

/// Walk `roots` and collect every note that links to `target`.
///
/// This is a second walk and not [`index::scan`]: that one builds `Entry`s and
/// reads the first few kilobytes of each file looking for a title, and this
/// one needs whole bodies. It builds its own `Entry` per file as it goes,
/// because the resolver has to be able to rank the whole vault to say whether
/// a `[[spec]]` somewhere means *this* note or the other one — and doing that
/// from the quick-open index would mean an answer that depends on whether ^O
/// has been opened yet. Both walks are [`index::walk_notes`], so they agree
/// about which files exist; `cancel` is watched per directory entry there.
pub fn scan(target: &Entry, roots: &[PathBuf], cancel: &AtomicBool) -> Vec<Mention> {
    let names = index::link_keys(target);
    // the words an unlinked mention is made of: the title, and the aliases the
    // note's front matter declares. Read from disk once, before the walk,
    // because the walk may meet the notes that say them before it meets this
    // one — and the note may live outside every root
    let mut words: Vec<String> = vec![target.title.clone()];
    if let Ok(body) = fs::read_to_string(&target.path) {
        words.extend(aliases_of(&body));
    }
    // a note with no title is not named by every other empty note
    words.retain(|w| !w.trim().is_empty() && w != "Untitled");
    let mut entries: Vec<Entry> = Vec::new();
    let mut found: Vec<(PathBuf, SystemTime, Vec<Hit>)> = Vec::new();
    let mut unlinked: Vec<(SystemTime, Mention)> = Vec::new();
    let walked = index::walk_notes(roots, Some(cancel), |root, path, entry| {
        let meta = entry.metadata().ok();
        let modified = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        // a body too big to read is a body this scan will not read;
        // it is not a note that stopped existing. The resolver has to
        // rank it like any other or the footer starts claiming
        // mentions a click on the same link would not honour — a
        // `[[spec]]` beside a 300KB `spec.md` resolves one way here
        // and the other way in `App::follow_wikilink`. The title is
        // read the way the quick-open walk reads one, from the head of
        // the file, for the same reason.
        let too_big = meta.as_ref().is_some_and(|m| m.len() > MAX_BODY_BYTES);
        let body = if too_big {
            None
        } else {
            // a file that is not text simply is not read; there is
            // nothing to say about it and nothing to fix
            fs::read_to_string(&path).ok()
        };
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
            modified,
            aliases,
            name: Entry::name_of(&path),
        });
        let Some(body) = body else {
            return;
        };
        // a note linking to itself is not a mention of it: the footer
        // answers "who else", and the reader is already here
        if path == target.path {
            return;
        }
        let hits = mentions_in(&body, &names);
        // unlinked mentions need no resolver: the words either are there or
        // are not, so the row is made on the spot
        // a namesake — another note with this one's title — is not a note
        // that mentions it; the title alone would put every `spec.md` in the
        // vault in each other's footer
        let title = entries.last().map(|e| fold_case(e.title.trim()));
        let namesake = words
            .iter()
            .any(|w| title.as_ref().is_some_and(|t| *t == fold_case(w.trim())));
        let plain = if namesake {
            Vec::new()
        } else {
            unlinked_in(&body, &words)
        };
        if let Some(first) = plain.first() {
            unlinked.push((
                modified,
                Mention {
                    name: stem_of(&path),
                    path: path.clone(),
                    excerpt: first.excerpt.clone(),
                    link: first.link,
                    count: plain.len(),
                    linked: false,
                },
            ));
        }
        if !hits.is_empty() {
            found.push((path, modified, hits));
        }
    });
    if walked.is_none() {
        return Vec::new();
    }
    // the note on screen may live outside every root — opened from the command
    // line, or through the recents list — and the resolver cannot judge a link
    // against a note it has never heard of
    if !entries.iter().any(|e| e.path == target.path) {
        entries.push(target.clone());
    }

    let mut out: Vec<(SystemTime, Mention)> = Vec::new();
    // one verdict per distinct target, not per hit. Resolving is a pass over
    // every note in the vault, and a hub note that five hundred others link to
    // is five hundred hits spelling a handful of names between them; without
    // this the footer for it costs hits × notes and takes long enough to feel
    // like the scan never finished.
    let mut verdict: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    for (path, modified, hits) in found {
        // by name a `[[spec]]` could be any note called spec; the resolver is
        // the only thing that can say which, and it is the same call the click
        // on that link would make, so the footer cannot claim a mention the
        // link itself would not honour
        let mut kept = hits.iter().filter(|h| {
            let key = crate::md::link_key(&h.target);
            *verdict.entry(key).or_insert_with(|| {
                index::resolve(&entries, &h.target).is_some_and(|e| e.path == target.path)
            })
        });
        let Some(first) = kept.next() else {
            continue;
        };
        let count = 1 + kept.count();
        out.push((
            modified,
            Mention {
                name: stem_of(&path),
                path,
                excerpt: first.excerpt.clone(),
                link: first.link,
                count,
                linked: true,
            },
        ));
    }
    // most recently touched first, because that is the note the reader most
    // likely wrote the link from. The path breaks ties, so two notes saved in
    // the same second do not swap places between one scan and the next.
    let order = |(ma, a): &(SystemTime, Mention), (mb, b): &(SystemTime, Mention)| {
        mb.cmp(ma).then_with(|| a.path.cmp(&b.path))
    };
    out.sort_by(order);
    out.truncate(MAX_MENTIONS);
    // the linked rows first, then the unlinked, each group capped on its own;
    // the renderer tells them apart by `linked`
    unlinked.sort_by(order);
    unlinked.truncate(MAX_MENTIONS);
    out.extend(unlinked);
    out.into_iter().map(|(_, m)| m).collect()
}

/// The stem, not the title: every other list in the app names a note by its
/// file, and the footer should read the same way.
fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A scan running on a thread: the end of the channel it will answer on, and
/// the flag that asks it to give up.
pub struct Pending {
    rx: Receiver<Vec<Mention>>,
    cancel: Arc<AtomicBool>,
}

impl Drop for Pending {
    /// Letting go of a scan is how it is stopped.
    ///
    /// Dropping the receiver on its own tells the worker nothing: the send is
    /// the last thing it does, so it reads the whole vault first and only then
    /// discovers nobody is listening. Ten notes opened in a row — or ten
    /// checkboxes ticked, each one a save and so a new generation — used to
    /// leave ten whole-vault walks running at once, each with its own list of
    /// every note in it. The flag is checked per file, so the ones nobody
    /// wants stop at the next one.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Start a scan on a thread and hand back the handle to it.
///
/// The thread is never joined. If the reader moves on, or quits mid-scan, the
/// [`Pending`] is dropped, the scan gives up, and the thread finishes on its
/// own with nobody waiting for it — which is what keeps quitting instant even
/// while a big vault is being read.
pub fn spawn(target: Entry, roots: Vec<PathBuf>) -> Pending {
    let (tx, rx) = std::sync::mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    std::thread::spawn(move || {
        let _ = tx.send(scan(&target, &roots, &flag));
    });
    Pending { rx, cancel }
}

/// The cached answer for the note on screen, and the scan in flight for it.
///
/// Keyed by the note and a generation counter rather than by the note alone:
/// the vault changes under a session, and the counter is how a save says "look
/// again" without this having to know what a save is.
#[derive(Default)]
pub struct Backlinks {
    key: Option<Key>,
    rows: Vec<Mention>,
    pending: Option<(Key, Pending)>,
    generation: u64,
}

/// What an answer is an answer *to*: the note, and how many times the vault has
/// been declared out of date since the app started.
type Key = (PathBuf, u64);

impl Backlinks {
    /// Say that everything known is now suspect. Cheap: it bumps a number, and
    /// the next draw of the reading view is what actually pays for the rescan.
    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// The rows for the note at `path`, and never a wait for them. An answer
    /// already in hand comes straight back; otherwise a scan is started (or
    /// polled) and the caller gets nothing this frame, which is exactly what
    /// keeps the first frame of a page free of the footer.
    ///
    /// `start` builds what a scan needs — the note described as an index entry
    /// and the roots to walk — and is only called when one is actually about
    /// to be started, so the ordinary cached frame costs a path compare.
    pub fn rows_for(
        &mut self,
        path: &Path,
        start: impl FnOnce() -> (Entry, Vec<PathBuf>),
    ) -> &[Mention] {
        let key = (path.to_path_buf(), self.generation);
        if self.key.as_ref() == Some(&key) {
            return &self.rows;
        }
        if let Some((pending, scan)) = self.pending.take() {
            if pending == key {
                match scan.rx.try_recv() {
                    Ok(rows) => {
                        self.rows = rows;
                        self.key = Some(key);
                    }
                    Err(TryRecvError::Empty) => {
                        self.pending = Some((pending, scan));
                        return &[];
                    }
                    // the worker went away without answering, which only a
                    // panic can do. Remember the empty answer rather than
                    // starting the same doomed scan on every frame after it.
                    Err(TryRecvError::Disconnected) => {
                        self.rows = Vec::new();
                        self.key = Some(key);
                    }
                }
                return &self.rows;
            }
            // a scan for another note, or for an older generation: dropping it
            // here is how its thread is told to stop where it is
        }
        let (target, roots) = start();
        self.pending = Some((key, spawn(target, roots)));
        &[]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::write;

    /// The walk, run to the end: no test ever gives up on one.
    fn scan(target: &Entry, roots: &[PathBuf]) -> Vec<Mention> {
        super::scan(target, roots, &AtomicBool::new(false))
    }

    fn tmpdir(name: &str) -> PathBuf {
        crate::testutil::tmpdir("mentions", name)
    }

    /// The note on screen, as the app would describe it to a scan.
    fn target(dir: &Path, rel: &str, title: &str) -> Entry {
        target_entry(
            &dir.join(rel),
            title,
            std::slice::from_ref(&dir.to_path_buf()),
        )
    }

    fn names(of: &[&str]) -> Vec<String> {
        of.iter().map(|n| crate::md::link_key(n)).collect()
    }

    #[test]
    fn a_wikilink_that_resolves_to_the_note_is_found_and_one_that_does_not_is_ignored() {
        let hits = mentions_in(
            "see [[story-matrix]] and [[something-else]]\n",
            &names(&["story-matrix"]),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].target, "story-matrix");
        assert_eq!(hits[0].line, 0);
    }

    #[test]
    fn a_markdown_link_to_the_note_file_counts_as_a_link() {
        let hits = mentions_in(
            "see [the matrix](stories/story%20matrix.md#Rows) and [site](https://story-matrix.md)\n",
            &names(&["stories/story matrix"]),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].target, "stories/story matrix.md");
        assert_eq!(hits[0].line, 0);
    }

    #[test]
    fn an_aliased_link_counts_against_the_note_it_points_at_not_the_alias() {
        let body = "pulled from [[stories/story-matrix|the matrix]]\n";
        assert_eq!(
            mentions_in(body, &names(&["stories/story-matrix"])).len(),
            1
        );
        // the label is what is drawn, never what is named
        assert!(mentions_in(body, &names(&["the matrix"])).is_empty());
    }

    #[test]
    fn a_wikilink_inside_a_fenced_code_block_is_source_not_a_link() {
        let body = "before [[spec]]\n```\nnot a link: [[spec]]\n```\nafter [[spec]]\n";
        let hits = mentions_in(body, &names(&["spec"]));
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[1].line, 4);
        // a tilde fence is a fence too
        let body = "before [[spec]]\n~~~\nnot a link: [[spec]]\n~~~\nafter [[spec]]\n";
        let hits = mentions_in(body, &names(&["spec"]));
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[1].line, 4);
    }

    #[test]
    fn front_matter_is_stepped_over_rather_than_scanned_for_mentions() {
        let body = "---\nsee: \"[[spec]]\"\n---\nbody mentions [[spec]]\n";
        let hits = mentions_in(body, &names(&["spec"]));
        assert_eq!(hits.len(), 1);
        // and the line is counted from the top of the body, not the file
        assert_eq!(hits[0].line, 0);
    }

    fn excerpt_of(line: &str) -> (String, (usize, usize)) {
        let w = &crate::md::wikilinks(line)[0];
        excerpt(line, w.start, w.end)
    }

    /// The chars of `e` that the span names.
    fn spanned(e: &str, (a, b): (usize, usize)) -> String {
        e.chars().skip(a).take(b - a).collect()
    }

    #[test]
    fn the_excerpt_is_centred_on_the_link_and_says_where_it_was_cut() {
        let far = "x".repeat(150);
        let line = format!("{far} before [[spec]] after {far}");
        let (e, span) = excerpt_of(&line);
        assert!(e.starts_with('…') && e.ends_with('…'));
        assert_eq!(spanned(&e, span), "[[spec]]");
        assert_eq!(e.chars().count(), 1 + BEFORE_LINK + 8 + AFTER_LINK + 1);
        // a short line is not cut at all
        let (e, span) = excerpt_of("see [[spec]] for the rest");
        assert_eq!(e, "see [[spec]] for the rest");
        assert_eq!(spanned(&e, span), "[[spec]]");
    }

    #[test]
    fn the_markers_a_line_is_drawn_with_are_not_part_of_the_excerpt() {
        let (e, span) = excerpt_of("- see [[spec]] for the rest");
        assert_eq!(e, "see [[spec]] for the rest");
        assert_eq!(spanned(&e, span), "[[spec]]");
        let (e, _) = excerpt_of("> [!summary] TL;DR of [[spec]]");
        assert_eq!(e, "TL;DR of [[spec]]");
        let (e, _) = excerpt_of("  ## about [[spec]]");
        assert_eq!(e, "about [[spec]]");
        // markdown inside the line is kept for the renderer to style
        let (e, span) = excerpt_of("**Projects:** [[spec]]; more");
        assert_eq!(e, "**Projects:** [[spec]]; more");
        assert_eq!(spanned(&e, span), "[[spec]]");
    }

    #[test]
    fn a_table_row_is_reduced_to_the_cell_holding_the_link() {
        let (e, span) = excerpt_of("| Projects | see [[spec]] here | tight deadline |");
        assert_eq!(e, "see [[spec]] here");
        assert_eq!(spanned(&e, span), "[[spec]]");
    }

    #[test]
    fn runs_of_whitespace_collapse_without_losing_the_link() {
        let (e, span) = excerpt_of("a   lot\tof   space [[spec|the spec]]   here");
        assert_eq!(e, "a lot of space [[spec|the spec]] here");
        assert_eq!(spanned(&e, span), "[[spec|the spec]]");
    }

    #[test]
    fn several_mentions_in_one_note_collapse_to_one_row_with_the_first_excerpt_and_a_count() {
        let dir = tmpdir("collapse");
        write(&dir, "spec.md", "# Spec\n");
        write(
            &dir,
            "meta.md",
            "# Meta\nsee [[spec]] for the shape.\nand again [[spec]] later.\n",
        );
        let rows = scan(&target(&dir, "spec.md", "Spec"), std::slice::from_ref(&dir));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "meta");
        assert_eq!(rows[0].count, 2);
        assert_eq!(rows[0].excerpt, "see [[spec]] for the shape.");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_note_that_links_to_itself_is_not_a_mention_of_itself() {
        let dir = tmpdir("self");
        write(&dir, "spec.md", "# Spec\nsee [[spec]], which is here.\n");
        let rows = scan(&target(&dir, "spec.md", "Spec"), std::slice::from_ref(&dir));
        assert!(rows.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_note_nothing_links_to_produces_no_mentions_at_all() {
        let dir = tmpdir("none");
        write(&dir, "spec.md", "# Spec\n");
        write(&dir, "other.md", "# Other\nnothing to say\n");
        assert!(scan(&target(&dir, "spec.md", "Spec"), std::slice::from_ref(&dir)).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_scan_reaches_subfolders_and_skips_the_dirs_quick_open_skips() {
        let dir = tmpdir("walk");
        write(&dir, "spec.md", "# Spec\n");
        write(&dir, "deep/inner/note.md", "# Inner\nabout [[spec]].\n");
        write(&dir, ".obsidian/cache.md", "# Cache\n[[spec]]\n");
        write(&dir, "node_modules/readme.md", "# Dep\n[[spec]]\n");
        let rows = scan(&target(&dir, "spec.md", "Spec"), std::slice::from_ref(&dir));
        let names: Vec<&str> = rows.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["note"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_body_over_the_size_cap_is_skipped_rather_than_read() {
        let dir = tmpdir("huge");
        write(&dir, "spec.md", "# Spec\n");
        let filler = "x".repeat(MAX_BODY_BYTES as usize + 1);
        write(&dir, "dump.md", &format!("# Dump\n[[spec]]\n{filler}"));
        assert!(scan(&target(&dir, "spec.md", "Spec"), std::slice::from_ref(&dir)).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_note_too_big_to_read_is_still_a_note_the_resolver_has_to_rank() {
        let dir = tmpdir("bigrank");
        // the root `spec.md` is a dump this walk will not read, but it is
        // still the note a bare `[[spec]]` opens. Left out of the ranking, the
        // deep one would claim every mention in the vault and clicking any of
        // those links would open the root one — the footer promising something
        // the link does not do
        let filler = "x".repeat(MAX_BODY_BYTES as usize + 1);
        write(&dir, "spec.md", &format!("# Spec\n{filler}"));
        write(&dir, "deep/spec.md", "# Spec\n");
        write(&dir, "other.md", "# Other\nsee [[spec]].\n");
        assert!(scan(
            &target(&dir, "deep/spec.md", "Spec"),
            std::slice::from_ref(&dir)
        )
        .is_empty());
        // and the note the link does open still gets its row
        let rows = scan(&target(&dir, "spec.md", "Spec"), std::slice::from_ref(&dir));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "other");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_scan_nobody_is_waiting_for_stops_rather_than_reading_the_rest_of_the_vault() {
        let dir = tmpdir("cancel");
        write(&dir, "spec.md", "# Spec\n");
        write(&dir, "meta.md", "# Meta\nsee [[spec]].\n");
        let stop = AtomicBool::new(true);
        let rows = super::scan(
            &target(&dir, "spec.md", "Spec"),
            std::slice::from_ref(&dir),
            &stop,
        );
        assert!(rows.is_empty());
        // and the same walk, with nobody asking it to stop, answers
        assert_eq!(
            scan(&target(&dir, "spec.md", "Spec"), std::slice::from_ref(&dir)).len(),
            1
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_notes_of_the_same_name_are_told_apart_by_the_resolver_not_the_name() {
        let dir = tmpdir("ambiguous");
        write(&dir, "spec.md", "# Spec\n");
        write(&dir, "deep/spec.md", "# Spec\n");
        write(&dir, "other.md", "# Other\nsee [[spec]].\n");
        // both notes answer to the name; only one is the note the link opens
        let near = scan(&target(&dir, "spec.md", "Spec"), std::slice::from_ref(&dir));
        assert_eq!(near.len(), 1);
        assert_eq!(near[0].name, "other");
        let far = scan(
            &target(&dir, "deep/spec.md", "Spec"),
            std::slice::from_ref(&dir),
        );
        assert!(far.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    fn words(of: &[&str]) -> Vec<String> {
        of.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn an_unlinked_mention_is_the_title_as_a_whole_word_in_any_case() {
        let hits = unlinked_in(
            "The SPEC says so.\nA specific respect for spec_v2.\nsee spec\n",
            &words(&["spec"]),
        );
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].line, 0);
        assert_eq!(hits[0].target, "SPEC");
        assert_eq!(hits[0].excerpt, "The SPEC says so.");
        assert_eq!(spanned(&hits[0].excerpt, hits[0].link), "SPEC");
        assert_eq!(hits[1].line, 2);
        // a multi-word title matches across its spaces
        let hits = unlinked_in("about the story matrix here\n", &words(&["Story Matrix"]));
        assert_eq!(hits.len(), 1);
        assert_eq!(spanned(&hits[0].excerpt, hits[0].link), "story matrix");
    }

    #[test]
    fn a_word_inside_a_wikilink_is_a_link_not_an_unlinked_mention() {
        let body = "see [[spec]] and [[spec|the spec]] and [[other#spec]]\n";
        assert!(unlinked_in(body, &words(&["spec"])).is_empty());
        // but one beside the link still is
        let hits = unlinked_in("see [[spec]], the spec\n", &words(&["spec"]));
        assert_eq!(hits.len(), 1);
        // fenced code and front matter are source, not prose
        let body = "---\ntitle: spec\n---\n```\nspec\n```\nspec\n";
        assert_eq!(unlinked_in(body, &words(&["spec"])).len(), 1);
    }

    #[test]
    fn aliases_are_read_from_the_front_matter_inline_or_as_a_list() {
        assert_eq!(
            aliases_of("---\naliases: [The Spec, \"spec sheet\"]\n---\n"),
            vec!["The Spec", "spec sheet"]
        );
        assert_eq!(
            aliases_of("---\naliases:\n  - one\n  - 'two'\ntags: x\n---\n"),
            vec!["one", "two"]
        );
        assert!(aliases_of("# no front matter\naliases: no\n").is_empty());
    }

    #[test]
    fn a_note_that_says_the_title_or_an_alias_without_linking_is_mentioned_in() {
        let dir = tmpdir("unlinked");
        write(
            &dir,
            "spec.md",
            "---\naliases: [the plan]\n---\n# Spec\nthe spec itself\n",
        );
        write(
            &dir,
            "meta.md",
            "# Meta\nsee [[spec]] and the spec twice: spec\n",
        );
        write(&dir, "plan.md", "# Plan\nfollowing The Plan here\n");
        write(&dir, "other.md", "# Other\nspecific\n");
        let rows = scan(&target(&dir, "spec.md", "Spec"), std::slice::from_ref(&dir));
        let linked: Vec<&str> = rows
            .iter()
            .filter(|m| m.linked)
            .map(|m| m.name.as_str())
            .collect();
        assert_eq!(linked, vec!["meta"]);
        let mut unlinked: Vec<&Mention> = rows.iter().filter(|m| !m.linked).collect();
        unlinked.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<&str> = unlinked.iter().map(|m| m.name.as_str()).collect();
        // the note itself does not mention itself; `specific` is another word
        assert_eq!(names, vec!["meta", "plan"]);
        assert_eq!(unlinked[0].count, 2);
        assert_eq!(unlinked[0].excerpt, "see [[spec]] and the spec twice: spec");
        assert_eq!(spanned(&unlinked[0].excerpt, unlinked[0].link), "spec");
        assert_eq!(spanned(&unlinked[1].excerpt, unlinked[1].link), "The Plan");
        // the linked rows come first
        assert!(rows[0].linked);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_namesake_note_is_not_an_unlinked_mention_of_this_one() {
        let dir = tmpdir("namesake");
        write(&dir, "spec.md", "# Spec\n");
        write(&dir, "deep/spec.md", "# Spec\nthe other spec\n");
        assert!(scan(&target(&dir, "spec.md", "Spec"), std::slice::from_ref(&dir)).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_scan_of_a_vault_that_is_not_there_answers_with_nothing() {
        // the roots move under a session — a vault on a volume that is not
        // mounted, a folder someone deleted — and the footer is simply absent
        let dir = std::env::temp_dir().join("catcher-mentions-missing");
        let _ = fs::remove_dir_all(&dir);
        assert!(scan(&target(&dir, "spec.md", "Spec"), std::slice::from_ref(&dir)).is_empty());
    }
}

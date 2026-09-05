//! Search in all files: the third tab of ^O. The ranked list finds a note by
//! what it is called; this finds one by what it says, one row per matching
//! line, grouped under the note the line came from.
//!
//! Everything here is pure over strings, so the matcher, the snippet trim and
//! the grouping all test without a file on disk. Reading the bodies is the
//! caller's job, and it does that once per visit to the tab rather than once
//! per keystroke.

use std::fs;
use std::path::Path;

/// Lines past this are not offered: the list stops being something to read.
pub const MAX_HITS: usize = 200;
/// A file bigger than this is not a note, whatever its extension says.
const MAX_BODY_BYTES: u64 = 256 * 1024;

/// One piece of a query, after `parse`.
#[derive(Clone, Debug)]
enum Term {
    /// A bare word, or a `"quoted phrase"`: somewhere on the line, any case.
    Text(String),
    /// `/regex/`: matched against the lowered line, so literals go lowercase.
    Regex(regex::Regex),
    /// `path:x`: the note's path (relative to its root) contains `x`.
    Path(String),
    /// `file:x`: the note's filename contains `x`.
    File(String),
    /// `tag:x`: the note carries `x`, or a tag nested under it.
    Tag(String),
}

/// A parsed query: what `search` walks the bodies with. Empty for a blank
/// query, which matches nothing: every line of every note is not an answer
/// to anything.
#[derive(Clone, Debug, Default)]
pub struct Query {
    /// Each term beside whether it is negated (`-word`: must not be there).
    terms: Vec<(Term, bool)>,
}

impl Query {
    fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Does the note itself pass the `path:`, `file:` and `tag:` terms?
    fn note_ok(&self, body: &Body) -> bool {
        self.terms.iter().all(|(t, neg)| {
            let hit = match t {
                Term::Path(p) => body.rel.contains(p.as_str()),
                Term::File(f) => body.name.contains(f.as_str()),
                Term::Tag(want) => body.tags.iter().any(|t| crate::index::tag_under(t, want)),
                _ => return true,
            };
            hit != *neg
        })
    }

    /// Where on a lowered line the line terms hit: the char offset of the
    /// earliest, or `None`. Every positive term has to be somewhere on the
    /// line, in any order — `milk buy` finds "buy milk" — and no negated one
    /// may be. A query of note terms alone hits every line, at 0.
    fn hit(&self, lower: &str) -> Option<usize> {
        let mut first = usize::MAX;
        for (t, neg) in &self.terms {
            let at = match t {
                Term::Text(w) => lower.find(w.as_str()),
                Term::Regex(re) => re.find(lower).map(|m| m.start()),
                _ => continue,
            };
            match (at, neg) {
                (Some(_), true) | (None, false) => return None,
                (Some(at), false) => first = first.min(lower[..at].chars().count()),
                (None, true) => {}
            }
        }
        Some(if first == usize::MAX { 0 } else { first })
    }
}

/// `query` read into its terms. Whitespace separates them, except inside
/// `"quotes"`, `/slashes/` and `line:( )`; a leading `-` negates the term.
/// A `/regex/` that does not compile is searched for as text instead.
pub fn parse(query: &str) -> Query {
    let mut terms = Vec::new();
    for raw in tokens(query) {
        let (neg, raw) = match raw.strip_prefix('-') {
            Some(rest) if !rest.is_empty() => (true, rest),
            _ => (false, raw.as_str()),
        };
        let lower = raw.to_lowercase();
        let term = if let Some(p) = lower.strip_prefix("path:") {
            Term::Path(p.to_string())
        } else if let Some(f) = lower.strip_prefix("file:") {
            Term::File(f.to_string())
        } else if let Some(t) = lower.strip_prefix("tag:") {
            Term::Tag(crate::md::tag_key(t))
        } else if let Some(inner) = lower.strip_prefix("line:") {
            let inner = inner
                .strip_prefix('(')
                .and_then(|i| i.strip_suffix(')'))
                .unwrap_or(inner);
            for w in inner.split_whitespace() {
                terms.push((Term::Text(w.to_string()), neg));
            }
            continue;
        } else if let Some(re) = strip_around(&lower, '/', '/') {
            match regex::Regex::new(re) {
                Ok(re) => Term::Regex(re),
                Err(_) => Term::Text(re.to_string()),
            }
        } else {
            Term::Text(strip_around(&lower, '"', '"').unwrap_or(&lower).to_string())
        };
        let blank = matches!(&term, Term::Text(t) | Term::Path(t) | Term::File(t) | Term::Tag(t) if t.is_empty());
        if !blank {
            terms.push((term, neg));
        }
    }
    Query { terms }
}

/// `s` without its `open` and `close`, when it is wrapped in both and long
/// enough for them to be two characters.
fn strip_around(s: &str, open: char, close: char) -> Option<&str> {
    if s.chars().count() < 2 {
        return None;
    }
    s.strip_prefix(open)?.strip_suffix(close)
}

/// The tokens of `query`: whitespace-separated, but a `"..."`, `/.../` or
/// `(...)` group keeps its spaces. Quotes stay on so `parse` can tell a
/// phrase from a word, and an unclosed group runs to the end.
fn tokens(query: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut close: Option<char> = None;
    for c in query.chars() {
        match close {
            Some(cl) => {
                cur.push(c);
                if c == cl {
                    close = None;
                }
            }
            None if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            None => {
                let opener = match c {
                    '"' => Some('"'),
                    '(' => Some(')'),
                    // a slash starts a regex only at the start of a token or
                    // after a prefix, so `a/b` stays one word
                    '/' if cur.is_empty() || cur.ends_with(':') || cur == "-" => Some('/'),
                    _ => None,
                };
                cur.push(c);
                close = opener;
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The text terms of a query, lowered: what the snippet lights up. Negated
/// terms, regexes and note filters are not on a line to mark.
pub fn words(query: &str) -> Vec<String> {
    parse(query)
        .terms
        .into_iter()
        .filter_map(|(t, neg)| match t {
            Term::Text(w) if !neg => Some(w),
            _ => None,
        })
        .collect()
}

/// Where in `line` a `words` query hits: the char offset of the earliest
/// word, or `None`. Every word has to be somewhere on the line, in any
/// order, and case never matters.
pub fn find(words: &[String], line: &str) -> Option<usize> {
    if words.is_empty() {
        return None;
    }
    let lower = line.to_lowercase();
    let mut first = usize::MAX;
    for w in words {
        let at = lower.find(w.as_str())?;
        first = first.min(lower[..at].chars().count());
    }
    Some(first)
}

/// A note as `search` walks it: its lines as read, each beside its lowered
/// form, and what the note filters ask about — built once per visit to the
/// tab by `body`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Body {
    pub lines: Vec<(String, String)>,
    /// The note's tags in [`crate::md::tag_key`] form.
    pub tags: Vec<String>,
    /// Path relative to its root, lowered, for `path:`.
    pub rel: String,
    /// The filename without `.md`, lowered, for `file:`.
    pub name: String,
}

impl Body {
    /// The same body, filed at `rel` as `name`.
    pub fn at(mut self, rel: &str, name: &str) -> Body {
        self.rel = rel.to_lowercase();
        self.name = name.to_lowercase();
        self
    }
}

/// `text` split into lines and lowered once, ready for `search`.
pub fn body(text: &str) -> Body {
    Body {
        lines: text
            .lines()
            .map(|l| (l.to_string(), l.to_lowercase()))
            .collect(),
        tags: crate::index::tags_of(text),
        ..Body::default()
    }
}

/// The body to search, or nothing for a file that is too big or not text.
pub fn body_of(path: &Path) -> Option<String> {
    let too_big = fs::metadata(path).is_ok_and(|m| m.len() > MAX_BODY_BYTES);
    if too_big {
        return None;
    }
    fs::read_to_string(path).ok()
}

/// One matching line of one note.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    /// Index into the quick-open index the bodies were read for.
    pub entry: usize,
    pub line: usize,
    pub text: String,
}

/// Every hit across `bodies` (one per index entry, `None` where it could not
/// be read), in index order, cut at `cap`. The second number is how many
/// more there were — what "…and N more" says.
pub fn search(bodies: &[Option<Body>], query: &str, cap: usize) -> (Vec<Hit>, usize) {
    let query = parse(query);
    let mut hits = Vec::new();
    let mut over = 0;
    if query.is_empty() {
        return (hits, over);
    }
    for (entry, body) in bodies.iter().enumerate() {
        let Some(body) = body else { continue };
        if !query.note_ok(body) {
            continue;
        }
        for (line, (text, lower)) in body.lines.iter().enumerate() {
            if query.hit(lower).is_none() {
                continue;
            }
            if hits.len() >= cap {
                over += 1;
                continue;
            }
            hits.push(Hit {
                entry,
                line,
                text: text.trim().to_string(),
            });
        }
    }
    (hits, over)
}

/// What one drawn row of the contents tab stands for.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    /// The note the hits under it came from.
    Note(usize),
    Hit(Hit),
    /// "…and N more": the cap was reached.
    More(usize),
}

/// Hits grouped under their note: a header row each time the note changes,
/// which is once per note because `search` walks the index in order.
pub fn rows(hits: &[Hit], more: usize) -> Vec<Row> {
    let mut out = Vec::new();
    let mut last = None;
    for h in hits {
        if last != Some(h.entry) {
            out.push(Row::Note(h.entry));
            last = Some(h.entry);
        }
        out.push(Row::Hit(h.clone()));
    }
    if more > 0 {
        out.push(Row::More(more));
    }
    out
}

/// `text` cut to `width` columns around the first match, as (piece, is a
/// match) pairs: the pieces marked true are what the row draws in the accent.
/// A line that starts well before its match is trimmed from the left, with an
/// ellipsis, so the match is on screen rather than off the end of the row.
pub fn snippet(text: &str, words: &[String], width: usize) -> Vec<(String, bool)> {
    let chars: Vec<char> = text.chars().collect();
    let first = find(words, text).unwrap_or(0);
    // keep a few characters of context before the match
    let lead = 12.min(width / 3);
    let start = if chars.len() <= width {
        0
    } else {
        first.saturating_sub(lead)
    };
    let mut window: String = chars[start..].iter().collect();
    if start > 0 {
        // the ellipsis takes the place of the first kept char, not a column
        // of its own, so the trim never pushes the match off the right edge
        window = format!("…{}", window.chars().skip(1).collect::<String>());
    }
    let window = crate::md::truncate(&window, width);
    mark(&window, words)
}

/// Split `text` into pieces, marking every occurrence of every word.
fn mark(text: &str, words: &[String]) -> Vec<(String, bool)> {
    let lower: Vec<char> = text.to_lowercase().chars().collect();
    let chars: Vec<char> = text.chars().collect();
    // lowering can change a string's length in chars for a few scripts; the
    // marking is by char position and gives up rather than misalign
    if lower.len() != chars.len() {
        return vec![(text.to_string(), false)];
    }
    let mut hit = vec![false; chars.len()];
    for w in words {
        let wc: Vec<char> = w.chars().collect();
        if wc.is_empty() || wc.len() > chars.len() {
            continue;
        }
        for i in 0..=chars.len() - wc.len() {
            if lower[i..i + wc.len()] == wc[..] {
                hit[i..i + wc.len()].iter_mut().for_each(|h| *h = true);
            }
        }
    }
    let mut out: Vec<(String, bool)> = Vec::new();
    for (c, h) in chars.iter().zip(hit) {
        match out.last_mut() {
            Some((s, was)) if *was == h => s.push(*c),
            _ => out.push((c.to_string(), h)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_word_must_be_on_the_line_in_any_order_and_any_case() {
        let w = words("Milk buy");
        assert_eq!(find(&w, "Buy MILK today"), Some(0));
        assert_eq!(find(&w, "buy bread"), None);
        assert_eq!(find(&w, "some milk"), None);
        // the offset is the earliest word, in chars not bytes
        assert_eq!(find(&words("milk"), "ééé milk"), Some(4));
        assert_eq!(find(&words("   "), "anything"), None);
    }

    fn lines(bodies: &[Option<Body>], q: &str) -> Vec<(usize, usize)> {
        search(bodies, q, 100)
            .0
            .iter()
            .map(|h| (h.entry, h.line))
            .collect()
    }

    #[test]
    fn a_quoted_phrase_is_matched_whole_and_a_minus_word_must_be_absent() {
        let b = vec![Some(body("buy milk\nmilk to buy\nbuy bread\n"))];
        assert_eq!(lines(&b, "\"buy milk\""), vec![(0, 0)]);
        assert_eq!(lines(&b, "\"BUY Milk\""), vec![(0, 0)]);
        assert_eq!(lines(&b, "buy -milk"), vec![(0, 2)]);
        assert_eq!(lines(&b, "-milk"), vec![(0, 2)]);
        assert_eq!(lines(&b, "-\"to buy\" milk"), vec![(0, 0)]);
        // a lone dash is a word, not a negation of nothing
        assert!(lines(&b, "-").is_empty());
        assert!(lines(&b, "\"\"").is_empty());
    }

    #[test]
    fn path_file_and_tag_terms_filter_the_note_not_the_line() {
        let b = vec![
            Some(body("plan\n").at("Work/Projects/Plan.md", "Plan")),
            Some(body("---\ntags: [work/ops]\n---\nplan\n").at("ops/notes.md", "notes")),
            Some(body("plan #workshop\n").at("misc.md", "misc")),
        ];
        assert_eq!(lines(&b, "path:projects plan"), vec![(0, 0)]);
        assert_eq!(lines(&b, "file:NOTES plan"), vec![(1, 3)]);
        assert_eq!(lines(&b, "tag:work plan"), vec![(1, 3)]);
        assert_eq!(lines(&b, "tag:#work/ops plan"), vec![(1, 3)]);
        assert_eq!(lines(&b, "tag:workshop plan"), vec![(2, 0)]);
        assert_eq!(lines(&b, "-tag:work plan"), vec![(0, 0), (2, 0)]);
        // a note filter alone lists every line of the notes it keeps
        assert_eq!(lines(&b, "path:misc"), vec![(2, 0)]);
        assert!(lines(&b, "tag:").is_empty());
    }

    #[test]
    fn line_groups_and_regexes_match_within_one_line() {
        let b = vec![Some(body("buy milk\nmilk 12 to buy\nbuy bread\n"))];
        assert_eq!(lines(&b, "line:(milk buy)"), vec![(0, 0), (0, 1)]);
        assert_eq!(lines(&b, "line:bread"), vec![(0, 2)]);
        assert_eq!(lines(&b, "/\\d+/"), vec![(0, 1)]);
        assert_eq!(lines(&b, "/^buy\\s+(milk|bread)$/"), vec![(0, 0), (0, 2)]);
        assert_eq!(lines(&b, "-/\\d/ buy"), vec![(0, 0), (0, 2)]);
        // a regex that does not compile is looked for as text
        assert!(lines(&b, "/(/").is_empty());
        // a slash inside a word is just a slash
        assert!(lines(&b, "a/b").is_empty());
        assert_eq!(
            tokens("a \"b c\" line:(d e) /f g/ -h"),
            vec!["a", "\"b c\"", "line:(d e)", "/f g/", "-h"]
        );
        // what lights up: the words and phrases, not the filters
        assert_eq!(words("path:x -no \"a b\" c"), vec!["a b", "c"]);
    }

    #[test]
    fn hits_are_capped_and_the_overflow_counted() {
        let bodies = vec![
            Some(body("a\nb\na\n")),
            None,
            Some(body("A here\nnothing\n")),
        ];
        let (hits, more) = search(&bodies, "a", 10);
        assert_eq!(hits.len(), 3);
        assert_eq!(more, 0);
        assert_eq!(
            hits[2],
            Hit {
                entry: 2,
                line: 0,
                text: "A here".into()
            }
        );
        let (hits, more) = search(&bodies, "a", 2);
        assert_eq!(hits.len(), 2);
        assert_eq!(more, 1);
        assert!(search(&bodies, "", 10).0.is_empty());
    }

    #[test]
    fn rows_group_hits_under_their_note_with_a_more_row_when_cut() {
        let hit = |entry, line| Hit {
            entry,
            line,
            text: String::new(),
        };
        let hits = vec![hit(0, 1), hit(0, 5), hit(3, 0)];
        let grouped = rows(&hits, 2);
        assert_eq!(grouped.len(), 6);
        assert_eq!(grouped[0], Row::Note(0));
        assert_eq!(grouped[1], Row::Hit(hit(0, 1)));
        assert_eq!(grouped[2], Row::Hit(hit(0, 5)));
        assert_eq!(grouped[3], Row::Note(3));
        assert_eq!(grouped[5], Row::More(2));
        assert!(rows(&[], 0).is_empty());
    }

    #[test]
    fn a_snippet_marks_the_match_and_keeps_a_short_line_whole() {
        let s = snippet("Buy milk and Milk again", &words("milk"), 80);
        assert_eq!(
            s,
            vec![
                ("Buy ".to_string(), false),
                ("milk".to_string(), true),
                (" and ".to_string(), false),
                ("Milk".to_string(), true),
                (" again".to_string(), false),
            ]
        );
    }

    #[test]
    fn a_long_line_is_trimmed_around_the_first_match() {
        let text = format!("{}needle and then more words after it", "x".repeat(60));
        let s = snippet(&text, &words("needle"), 30);
        let joined: String = s.iter().map(|(p, _)| p.as_str()).collect();
        assert!(joined.starts_with('…'), "{joined}");
        assert!(joined.ends_with('…'), "{joined}");
        assert!(s.iter().any(|(p, m)| *m && p == "needle"), "{joined}");
        assert!(crate::md::str_width(&joined) <= 30);
        // a match near the start keeps the start
        let s = snippet(&text[60..], &words("needle"), 30);
        assert!(!s
            .iter()
            .map(|(p, _)| p.as_str())
            .collect::<String>()
            .starts_with('…'));
    }
}

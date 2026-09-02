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

/// The words of a query, lowered once so every line is not lowering them
/// again. Empty for a blank query, which matches nothing: every line of
/// every note is not an answer to anything.
pub fn words(query: &str) -> Vec<String> {
    query.split_whitespace().map(|w| w.to_lowercase()).collect()
}

/// Where in `line` a `words` query hits: the char offset of the earliest
/// word, or `None`. Every word has to be somewhere on the line, in any
/// order — `milk buy` finds "buy milk" — and case never matters.
pub fn find(words: &[String], line: &str) -> Option<usize> {
    find_lowered(words, &line.to_lowercase())
}

/// `find` over a line already lowered, so a body searched on every
/// keystroke is lowered once when it is read rather than once per query.
fn find_lowered(words: &[String], lower: &str) -> Option<usize> {
    if words.is_empty() {
        return None;
    }
    let mut first = usize::MAX;
    for w in words {
        let at = lower.find(w.as_str())?;
        first = first.min(lower[..at].chars().count());
    }
    Some(first)
}

/// A note's lines as read, each beside its lowered form: what `search`
/// walks, built once per visit to the tab by `body`.
pub type Body = Vec<(String, String)>;

/// `text` split into lines and lowered once, ready for `search`.
pub fn body(text: &str) -> Body {
    text.lines()
        .map(|l| (l.to_string(), l.to_lowercase()))
        .collect()
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
    let words = words(query);
    let mut hits = Vec::new();
    let mut over = 0;
    if words.is_empty() {
        return (hits, over);
    }
    for (entry, body) in bodies.iter().enumerate() {
        let Some(body) = body else { continue };
        for (line, (text, lower)) in body.iter().enumerate() {
            if find_lowered(&words, lower).is_none() {
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

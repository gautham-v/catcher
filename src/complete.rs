//! Inline completion while typing: note names after an unclosed `[[`, the
//! headings and block ids of that note after `[[note#`, and the vault's tags
//! inside a `#tag` word. Pure functions over one line and the cursor, so the
//! token and the text an accepted row inserts are testable without an app;
//! the app supplies the candidates and keeps the popup's state.

use std::collections::HashMap;
use crate::index::Entry;
use crate::search;

/// What the cursor is inside, and so what the popup offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `[[query` — a note name.
    Link,
    /// `[[note#query` — a heading or `^block` of `note` (this note when the
    /// name is empty).
    Anchor { note: String },
    /// `#query` — a tag.
    Tag,
}

/// The token under the cursor: its kind, the column its query starts at
/// (chars, not bytes) and the query typed so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: Kind,
    pub start: usize,
    pub query: String,
}

/// One row of the popup: what it shows and what it inserts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub label: String,
    /// For a block with no `^id` yet: the row of the target note the fresh id
    /// in `insert` is written onto when the row is taken.
    pub stamp: Option<usize>,
    /// A dim note beside the label: the folder, only when two candidates
    /// share a name and it is what tells them apart. Empty otherwise.
    pub detail: String,
    pub insert: String,
}

/// How many rows the popup shows at most.
pub const MAX_ROWS: usize = 8;

/// The token the cursor at `col` (chars) is in on `line`, or `None` when
/// the cursor is not after an unclosed `[[` and not in a `#tag` word.
pub fn token_at(line: &str, col: usize) -> Option<Token> {
    let chars: Vec<char> = line.chars().collect();
    let col = col.min(chars.len());
    if let Some(t) = link_token(&chars, col) {
        return Some(t);
    }
    tag_token(&chars, col)
}

fn link_token(chars: &[char], col: usize) -> Option<Token> {
    // the last `[[` before the cursor with no `]]` between it and the cursor
    let mut open = None;
    let mut i = 0;
    while i + 1 < col {
        if chars[i] == '[' && chars[i + 1] == '[' {
            open = Some(i + 2);
            i += 2;
            continue;
        }
        if chars[i] == ']' && chars[i + 1] == ']' {
            open = None;
            i += 2;
            continue;
        }
        i += 1;
    }
    let open = open?;
    let inside: String = chars[open..col].iter().collect();
    // `[[note|alias` is past the part a name completes
    if inside.contains('|') || inside.contains(']') {
        return None;
    }
    if let Some(hash) = inside.find('#') {
        let note = inside[..hash].trim().to_string();
        return Some(Token {
            kind: Kind::Anchor { note },
            start: open + inside[..=hash].chars().count(),
            query: inside[hash + 1..].to_string(),
        });
    }
    // `[[^` is Obsidian's way into this note's blocks, `#` left unsaid
    if inside.starts_with('^') {
        return Some(Token {
            kind: Kind::Anchor {
                note: String::new(),
            },
            start: open,
            query: inside,
        });
    }
    Some(Token {
        kind: Kind::Link,
        start: open,
        query: inside,
    })
}

fn tag_token(chars: &[char], col: usize) -> Option<Token> {
    let mut k = col;
    while k > 0 && crate::md::is_tag_char(chars[k - 1]) {
        k -= 1;
    }
    if k == 0 || chars[k - 1] != '#' {
        return None;
    }
    let hash = k - 1;
    let prev = hash.checked_sub(1).map(|p| chars[p]);
    if !crate::md::tag_boundary(prev) {
        return None;
    }
    // `[[#` is a link to a heading, and `## x` is a heading, not a tag
    if hash >= 2 && chars[hash - 1] == '[' && chars[hash - 2] == '[' {
        return None;
    }
    let query: String = chars[k..col].iter().collect();
    if query.is_empty() {
        // a bare `#` at the head of a line is a heading being typed
        if chars[..hash].iter().all(|c| c.is_whitespace()) {
            return None;
        }
    } else if !chars[k].is_alphabetic() {
        return None;
    }
    // the word continues past the cursor: not a tag being typed
    if chars.get(col).is_some_and(|c| crate::md::is_tag_char(*c)) {
        return None;
    }
    Some(Token {
        kind: Kind::Tag,
        start: k,
        query,
    })
}

/// The notes that answer `query`, best first: a name, or an alias when the
/// alias is what matched. At most [`MAX_ROWS`].
pub fn link_candidates(query: &str, entries: &[Entry]) -> Vec<Candidate> {
    let mut scored: Vec<(i64, Candidate)> = Vec::new();
    for e in entries {
        let name = e.name();
        if let Some(s) = search::score_entry(query, &name, &e.title, &e.rel) {
            scored.push((
                s,
                Candidate {
                    label: name.clone(),
                    stamp: None,
                    detail: e.folder.clone(),
                    insert: name.clone(),
                },
            ));
        }
        for a in &e.aliases {
            if let Some(s) = search::fuzzy(query, a) {
                scored.push((
                    s * 10 + 90,
                    Candidate {
                        label: a.clone(),
                        stamp: None,
                        detail: format!("→ {name}"),
                        insert: a.clone(),
                    },
                ));
            }
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.insert.cmp(&b.1.insert)));
    let mut out: Vec<Candidate> = scored.into_iter().map(|(_, c)| c).take(MAX_ROWS).collect();
    // the folder earns its place only when it tells two same-named notes apart
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for c in &out {
        *seen.entry(c.label.as_str()).or_default() += 1;
    }
    let dupes: Vec<bool> = out.iter().map(|c| seen[c.label.as_str()] > 1).collect();
    for (c, dupe) in out.iter_mut().zip(dupes) {
        if !dupe && !c.detail.starts_with('→') {
            c.detail.clear();
        }
    }
    out
}

/// The headings of a note that answer `query`, in document order, the way
/// the outline lists them — or, when the query opens with `^`, its blocks.
pub fn anchor_candidates(query: &str, lines: &[String]) -> Vec<Candidate> {
    if let Some(q) = query.strip_prefix('^') {
        return block_candidates(q, lines);
    }
    let blocks = crate::md::blocks(lines);
    let mut out: Vec<Candidate> = crate::outline::headings(lines, &blocks)
        .into_iter()
        .filter(|h| search::fuzzy(query, &h.text).is_some())
        .map(|h| Candidate {
            label: format!("{}{}", "  ".repeat(h.level.saturating_sub(1)), h.text),
            stamp: None,
            detail: String::new(),
            insert: h.text,
        })
        .collect();
    out.truncate(MAX_ROWS);
    out
}

/// One linkable block of a note: a paragraph or a list item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockRef {
    /// The first line, trimmed, without its `^id`.
    pub text: String,
    /// The line a `^id` sits on, or goes on: the block's last line.
    pub last: usize,
    /// The id the block already carries, if any.
    pub id: Option<String>,
}

/// The paragraphs and list items of `lines`, in document order. Headings,
/// fences, tables, front matter, comments, maths and indented code are not
/// blocks one links to. A list item is a block of its own; a run of plain
/// lines is one paragraph, and its id belongs on the run's last line.
pub fn block_refs(lines: &[String]) -> Vec<BlockRef> {
    let special = crate::md::blocks(lines);
    let skip = |row: usize| special.iter().any(|b| b.contains(row));
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = &lines[i];
        if skip(i) || line.trim().is_empty() || crate::fold::heading_level(line).is_some() {
            i += 1;
            continue;
        }
        let chars: Vec<char> = line.trim_start().chars().collect();
        let mut last = i;
        if !crate::md::is_list_item(&chars) {
            while last + 1 < lines.len() {
                let next = &lines[last + 1];
                let next_chars: Vec<char> = next.trim_start().chars().collect();
                if skip(last + 1)
                    || next.trim().is_empty()
                    || crate::fold::heading_level(next).is_some()
                    || crate::md::is_list_item(&next_chars)
                {
                    break;
                }
                last += 1;
            }
        }
        let id = crate::md::block_id_at(&lines[last]).map(|(_, id)| id);
        let text = match crate::md::block_id_at(line) {
            Some((cut, _)) => line.chars().take(cut).collect::<String>(),
            None => line.clone(),
        }
        .trim()
        .to_string();
        out.push(BlockRef { text, last, id });
        i = last + 1;
    }
    out
}

/// A six-character id for the block on `row`, unlike any in `taken`. Made
/// from the text so the same block gets the same id on a retry, and never
/// the same one twice within a note.
pub fn fresh_block_id(text: &str, row: usize, taken: &[String]) -> String {
    use std::hash::{Hash, Hasher};
    let mut salt = 0u64;
    loop {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (text, row, salt).hash(&mut h);
        let mut n = h.finish();
        let mut id = String::with_capacity(6);
        for _ in 0..6 {
            let d = (n % 36) as u8;
            id.push(if d < 10 { (b'0' + d) as char } else { (b'a' + d - 10) as char });
            n /= 36;
        }
        if !taken.contains(&id) {
            return id;
        }
        salt += 1;
    }
}

/// The blocks of a note that answer `query`, in document order: the row
/// shows the block's first line and, dimmed, the id it already has. A block
/// without one is offered a fresh id, written onto it when the row is taken.
pub fn block_candidates(query: &str, lines: &[String]) -> Vec<Candidate> {
    let refs = block_refs(lines);
    let taken: Vec<String> = refs.iter().filter_map(|b| b.id.clone()).collect();
    refs.into_iter()
        .filter(|b| {
            query.is_empty()
                || search::fuzzy(query, &b.text).is_some()
                || b.id.as_deref().is_some_and(|id| search::fuzzy(query, id).is_some())
        })
        .map(|b| match b.id {
            Some(id) => Candidate {
                label: b.text,
                stamp: None,
                detail: format!("^{id}"),
                insert: format!("^{id}"),
            },
            None => {
                let id = fresh_block_id(&b.text, b.last, &taken);
                Candidate {
                    label: b.text,
                    stamp: Some(b.last),
                    detail: String::new(),
                    insert: format!("^{id}"),
                }
            }
        })
        .take(MAX_ROWS)
        .collect()
}

/// `line` with ` ^id` on its end, the way Obsidian stamps a block.
pub fn stamp_line(line: &str, insert: &str) -> String {
    format!("{} {}", line.trim_end(), insert)
}

/// The tags that answer `query`, best first.
pub fn tag_candidates(query: &str, tags: &[String]) -> Vec<Candidate> {
    let mut scored: Vec<(i64, &String)> = tags
        .iter()
        .filter_map(|t| search::fuzzy(query, t).map(|s| (s, t)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    scored
        .into_iter()
        .map(|(_, t)| Candidate {
            label: format!("#{t}"),
            stamp: None,
            detail: String::new(),
            insert: t.clone(),
        })
        .take(MAX_ROWS)
        .collect()
}

/// `line` with `insert` in place of the token's query, and the column the
/// cursor lands on. A link is closed with `]]` when nothing after the
/// cursor closes it already; when it is, the cursor steps over the `]]`.
pub fn accept(line: &str, col: usize, token: &Token, insert: &str) -> (String, usize) {
    let chars: Vec<char> = line.chars().collect();
    let col = col.min(chars.len());
    let head: String = chars[..token.start.min(col)].iter().collect();
    let rest: String = chars[col..].iter().collect();
    let mut out = head;
    // a block picked from `[[^` lands as `[[#^id]]`, the link that means
    // "this note, that block"
    if insert.starts_with('^') && token.query.starts_with('^') && out.ends_with("[[") {
        out.push('#');
    }
    out.push_str(insert);
    let mut cursor = out.chars().count();
    if token.kind != Kind::Tag {
        let before_next_open = rest.find("[[").map_or(rest.as_str(), |i| &rest[..i]);
        if rest.starts_with("]]") {
            cursor += 2;
        } else if !before_next_open.contains("]]") {
            out.push_str("]]");
            cursor += 2;
        }
    }
    out.push_str(&rest);
    (out, cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok(line: &str, col: usize) -> Option<(Kind, usize, String)> {
        token_at(line, col).map(|t| (t.kind, t.start, t.query))
    }

    #[test]
    fn an_unclosed_wikilink_is_a_link_token_from_after_the_brackets() {
        assert_eq!(tok("see [[gro", 9), Some((Kind::Link, 6, "gro".into())));
        assert_eq!(tok("see [[", 6), Some((Kind::Link, 6, String::new())));
        // closed already, or the cursor before the brackets: nothing
        assert_eq!(tok("see [[groceries]] and", 21), None);
        assert_eq!(
            tok("see [[groceries]] and [[me", 26),
            Some((Kind::Link, 24, "me".into()))
        );
        assert_eq!(tok("see [[gro", 3), None);
        // past the alias bar there is nothing to complete
        assert_eq!(tok("[[note|ali", 10), None);
    }

    #[test]
    fn a_hash_inside_a_wikilink_asks_for_that_notes_headings() {
        assert_eq!(
            tok("[[note#Set", 10),
            Some((
                Kind::Anchor {
                    note: "note".into()
                },
                7,
                "Set".into()
            ))
        );
        assert_eq!(
            tok("[[#Set", 6),
            Some((
                Kind::Anchor {
                    note: String::new()
                },
                3,
                "Set".into()
            ))
        );
        assert_eq!(
            tok("[[note#^ab", 10),
            Some((
                Kind::Anchor {
                    note: "note".into()
                },
                7,
                "^ab".into()
            ))
        );
    }

    #[test]
    fn a_tag_word_is_a_tag_token_but_a_heading_is_not() {
        assert_eq!(tok("about #wo", 9), Some((Kind::Tag, 7, "wo".into())));
        assert_eq!(tok("#wo", 3), Some((Kind::Tag, 1, "wo".into())));
        assert_eq!(tok("(#wo", 4), Some((Kind::Tag, 2, "wo".into())));
        assert_eq!(tok("a #", 3), Some((Kind::Tag, 3, String::new())));
        // a heading being typed, a bare hash at the head of the line
        assert_eq!(tok("#", 1), None);
        assert_eq!(tok("## Title", 8), None);
        assert_eq!(tok("# Title", 7), None);
        // no boundary before the hash, or the word runs on past the cursor
        assert_eq!(tok("a#wo", 4), None);
        assert_eq!(tok("#work", 3), None);
        assert_eq!(tok("#1st", 4), None);
        // the cursor left the word
        assert_eq!(tok("#work done", 8), None);
    }

    #[test]
    fn accepting_a_link_replaces_the_query_and_closes_the_brackets() {
        let t = token_at("see [[gro", 9).unwrap();
        assert_eq!(
            accept("see [[gro", 9, &t, "groceries"),
            ("see [[groceries]]".into(), 17)
        );
        // closed already: step over the brackets rather than doubling them
        let t = token_at("see [[gro]] x", 9).unwrap();
        assert_eq!(
            accept("see [[gro]] x", 9, &t, "groceries"),
            ("see [[groceries]] x".into(), 17)
        );
        // a later link's `]]` is not this one's
        let t = token_at("[[gro and [[b]]", 5).unwrap();
        assert_eq!(
            accept("[[gro and [[b]]", 5, &t, "groceries"),
            ("[[groceries]] and [[b]]".into(), 13)
        );
    }

    #[test]
    fn a_bare_caret_after_the_brackets_lists_this_notes_blocks() {
        assert_eq!(
            tok("see [[^la", 9),
            Some((Kind::Anchor { note: String::new() }, 6, "^la".into()))
        );
        let t = token_at("see [[^la", 9).unwrap();
        assert_eq!(
            accept("see [[^la", 9, &t, "^ab12cd"),
            ("see [[#^ab12cd]]".into(), 16)
        );
        // through `#^` the hash is already there
        let t = token_at("[[note#^la", 10).unwrap();
        assert_eq!(accept("[[note#^la", 10, &t, "^ab12cd"), ("[[note#^ab12cd]]".into(), 16));
    }

    #[test]
    fn accepting_a_heading_or_a_tag_keeps_what_came_before() {
        let t = token_at("[[note#Se", 9).unwrap();
        assert_eq!(
            accept("[[note#Se", 9, &t, "Setup"),
            ("[[note#Setup]]".into(), 14)
        );
        let t = token_at("about #wo now", 9).unwrap();
        assert_eq!(
            accept("about #wo now", 9, &t, "work"),
            ("about #work now".into(), 11)
        );
    }

    #[test]
    fn blocks_are_paragraphs_and_list_items_with_ids_kept_or_minted() {
        let lines: Vec<String> = [
            "# Title",
            "",
            "First paragraph line one",
            "line two ^keep1",
            "",
            "- item a",
            "- item b ^itemb",
            "",
            "```",
            "code",
            "```",
            "",
            "Last one",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let refs = block_refs(&lines);
        assert_eq!(refs.len(), 4);
        assert_eq!(refs[0].text, "First paragraph line one");
        assert_eq!(refs[0].last, 3);
        assert_eq!(refs[0].id.as_deref(), Some("keep1"));
        assert_eq!(refs[1], BlockRef { text: "- item a".into(), last: 5, id: None });
        assert_eq!(refs[2].text, "- item b");
        assert_eq!(refs[2].id.as_deref(), Some("itemb"));
        assert_eq!(refs[3].last, 12);

        let all = block_candidates("", &lines);
        assert_eq!(all[0].insert, "^keep1");
        assert_eq!(all[0].stamp, None);
        assert_eq!(all[1].stamp, Some(5));
        assert_eq!(all[1].insert.len(), 7);
        assert_ne!(all[1].insert, all[3].insert);
        // a fresh id never repeats one the note has
        assert_eq!(fresh_block_id("x", 0, &["abcdef".into()]).len(), 6);
        let id = fresh_block_id("item a", 5, &[]);
        assert_ne!(fresh_block_id("item a", 5, std::slice::from_ref(&id)), id);

        let by_text = block_candidates("last", &lines);
        assert_eq!(by_text.len(), 1);
        assert_eq!(by_text[0].label, "Last one");
        assert_eq!(anchor_candidates("^la", &lines)[0].label, "Last one");
        assert_eq!(stamp_line("Last one  ", "^ab12cd"), "Last one ^ab12cd");
    }

    #[test]
    fn the_folder_shows_only_when_two_notes_share_a_name() {
        let entry = |rel: &str, folder: &str| Entry {
            path: std::path::PathBuf::from(format!("/v/{rel}.md")),
            title: "Plan".into(),
            rel: rel.into(),
            folder: folder.into(),
            modified: std::time::SystemTime::UNIX_EPOCH,
            aliases: Vec::new(),
            name: "plan".into(),
        };
        let one = [entry("work/plan", "work")];
        assert_eq!(link_candidates("pl", &one)[0].detail, "");
        let two = [entry("work/plan", "work"), entry("home/plan", "home")];
        let got = link_candidates("pl", &two);
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|c| !c.detail.is_empty()));
    }

    #[test]
    fn candidates_come_from_names_aliases_headings_blocks_and_tags() {
        let mut e = Entry {
            path: std::path::PathBuf::from("/v/groceries.md"),
            title: "Groceries".into(),
            rel: "groceries".into(),
            folder: String::new(),
            modified: std::time::SystemTime::UNIX_EPOCH,
            aliases: vec!["shopping".into()],
            name: "groceries".into(),
        };
        let links = link_candidates("shop", std::slice::from_ref(&e));
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].insert, "shopping");
        e.aliases.clear();
        assert!(link_candidates("xyz", std::slice::from_ref(&e)).is_empty());
        assert_eq!(
            link_candidates("gr", std::slice::from_ref(&e))[0].insert,
            "groceries"
        );

        let lines: Vec<String> = ["# Title", "## Setup", "text ^abc", "## Use"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let got: Vec<String> = anchor_candidates("", &lines)
            .into_iter()
            .map(|c| c.insert)
            .collect();
        assert_eq!(got, vec!["Title", "Setup", "Use"]);
        // blocks live behind `^`, with the id the line already carries
        assert_eq!(anchor_candidates("^", &lines)[0].insert, "^abc");
        assert_eq!(anchor_candidates("^", &lines)[0].label, "text");

        let tags = vec!["work".to_string(), "home".to_string(), "wip".to_string()];
        let got: Vec<String> = tag_candidates("w", &tags)
            .into_iter()
            .map(|c| c.insert)
            .collect();
        assert_eq!(got, vec!["wip", "work"]);
    }
}

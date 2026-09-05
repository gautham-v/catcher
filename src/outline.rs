//! The outline picker: every heading of the open note in one list, to jump
//! to or fold from. Pure functions over the buffer, so the list and its
//! filter can be tested without an app; the app supplies the lines and
//! their blocks and keeps the picker's state.

use crate::fold;
use crate::md::Block;
use crate::search;

/// One heading of the note: where it is, how deep, and what it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    /// The buffer line, zero-based.
    pub line: usize,
    /// `#` is 1, `######` is 6.
    pub level: usize,
    /// The heading's text with the hashes gone, trimmed.
    pub text: String,
}

/// Every heading in `lines`, in order. The same test the folder applies, so
/// a `#` inside a fence or a table is not a heading here either.
pub fn headings(lines: &[String], blocks: &[Block]) -> Vec<Heading> {
    (0..lines.len())
        .filter_map(|line| {
            let level = fold::heading_at(lines, blocks, line)?;
            Some(Heading {
                line,
                level,
                text: heading_text(&lines[line]),
            })
        })
        .collect()
}

/// The text of a heading line: the opening hashes gone, and a closing run of
/// hashes too when it stands on its own — `# Title #` is `Title`, `# C#` is
/// still `C#`.
fn heading_text(line: &str) -> String {
    let body = line.trim_start().trim_start_matches('#').trim();
    let closing = body.trim_end_matches('#');
    if closing.len() < body.len() && closing.ends_with(char::is_whitespace) {
        closing.trim_end().to_string()
    } else {
        body.to_string()
    }
}

/// The headings that answer `query`, in document order: an outline that
/// reshuffled itself by score would lose the shape its indent is showing.
/// Every heading, for an empty query.
pub fn filter(headings: &[Heading], query: &str) -> Vec<Heading> {
    headings
        .iter()
        .filter(|h| search::fuzzy(query, &h.text).is_some())
        .cloned()
        .collect()
}

/// The index in `headings` of the heading whose section `line` is in: the
/// nearest heading at or above it. `None` above the first heading.
pub fn containing(headings: &[Heading], line: usize) -> Option<usize> {
    headings.iter().rposition(|h| h.line <= line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::md;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(String::from).collect()
    }

    fn outline(text: &str) -> Vec<Heading> {
        let l = lines(text);
        let b = md::blocks(&l);
        headings(&l, &b)
    }

    #[test]
    fn every_heading_is_listed_with_its_line_level_and_text() {
        let hs = outline("# Title\nintro\n## One\na\n### Deep\n## Two\n");
        let got: Vec<(usize, usize, &str)> = hs
            .iter()
            .map(|h| (h.line, h.level, h.text.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                (0, 1, "Title"),
                (2, 2, "One"),
                (4, 3, "Deep"),
                (5, 2, "Two")
            ]
        );
    }

    #[test]
    fn a_hash_inside_a_fence_or_a_table_is_not_a_heading() {
        let hs = outline(
            "# Title\n```\n# not a heading\n```\n| a | b |\n|---|---|\n| # | x |\n## Two\n",
        );
        let got: Vec<&str> = hs.iter().map(|h| h.text.as_str()).collect();
        assert_eq!(got, vec!["Title", "Two"]);
    }

    #[test]
    fn a_tag_and_a_bare_hash_are_not_headings() {
        let hs = outline("#tag\n#\n####### seven\n   ## indented\n");
        let got: Vec<(usize, &str)> = hs.iter().map(|h| (h.level, h.text.as_str())).collect();
        assert_eq!(got, vec![(2, "indented")]);
    }

    #[test]
    fn a_closing_hash_run_is_dropped_but_a_hash_in_a_word_is_kept() {
        assert_eq!(heading_text("# Title #"), "Title");
        assert_eq!(heading_text("## Title ###"), "Title");
        assert_eq!(heading_text("# C#"), "C#");
        assert_eq!(heading_text("#   spaced   "), "spaced");
    }

    #[test]
    fn the_filter_is_fuzzy_and_keeps_document_order() {
        let hs = outline("# Meeting notes\n## Agenda\n## Action items\n## Notes\n");
        let texts =
            |q: &str| -> Vec<String> { filter(&hs, q).into_iter().map(|h| h.text).collect() };
        assert_eq!(texts("not"), vec!["Meeting notes", "Notes"]);
        assert_eq!(texts("ai"), vec!["Action items"]);
        assert!(filter(&hs, "xyz").is_empty());
        assert_eq!(filter(&hs, "").len(), 4);
        assert_eq!(filter(&hs, "  ").len(), 4);
    }

    #[test]
    fn the_heading_containing_a_line_is_the_nearest_one_above_it() {
        let hs = outline("intro\n# Title\ntext\n## One\na\n## Two\n");
        assert_eq!(containing(&hs, 0), None);
        assert_eq!(containing(&hs, 1), Some(0));
        assert_eq!(containing(&hs, 2), Some(0));
        assert_eq!(containing(&hs, 3), Some(1));
        assert_eq!(containing(&hs, 4), Some(1));
        assert_eq!(containing(&hs, 99), Some(2));
        assert_eq!(containing(&[], 5), None);
    }
}

//! The line transformations behind the palette's editing commands: pure
//! functions on one line, so each can be tested without an editor and the
//! editor can apply them to a whole selection the same way it applies them
//! to the cursor line.

/// Where a line's list marker ends, if it has one: after `- `, `* `, `+ `
/// or `1. ` / `1) `, indentation included.
fn marker_end(line: &str) -> Option<usize> {
    let indent = line.len() - line.trim_start().len();
    let rest = &line[indent..];
    let first = rest.chars().next()?;
    if matches!(first, '-' | '*' | '+') && rest[1..].starts_with(' ') {
        return Some(indent + 2);
    }
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let after = &rest[digits..];
    if after.starts_with(". ") || after.starts_with(") ") {
        return Some(indent + digits + 2);
    }
    None
}

/// Walk a line through the three states a task can be in: plain item,
/// unchecked, checked, and round to plain again. A line that is not a list
/// item at all becomes an unchecked one, so the command is never a no-op.
pub fn toggle_checkbox(line: &str) -> String {
    let Some(end) = marker_end(line) else {
        let indent = line.len() - line.trim_start().len();
        return format!("{}- [ ] {}", &line[..indent], &line[indent..]);
    };
    let (head, body) = line.split_at(end);
    if let Some(rest) = body.strip_prefix("[ ] ") {
        format!("{head}[x] {rest}")
    } else if let Some(rest) = body
        .strip_prefix("[x] ")
        .or_else(|| body.strip_prefix("[X] "))
    {
        format!("{head}{rest}")
    } else {
        format!("{head}[ ] {body}")
    }
}

/// none → `#` → `##` → `###` → none, keeping the text. A deeper heading
/// than three folds back to none too, rather than climbing forever.
pub fn cycle_heading(line: &str) -> String {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    let text = if hashes > 0 && line[hashes..].starts_with(' ') {
        line[hashes + 1..].to_string()
    } else if hashes > 0 && line.len() == hashes {
        String::new()
    } else {
        // not a heading: leading `#` runs without a space are text
        return format!("# {line}");
    };
    if hashes >= 3 {
        text
    } else {
        format!("{} {text}", "#".repeat(hashes + 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_item_walks_through_plain_unchecked_checked_and_back() {
        assert_eq!(toggle_checkbox("- item"), "- [ ] item");
        assert_eq!(toggle_checkbox("- [ ] item"), "- [x] item");
        assert_eq!(toggle_checkbox("- [x] item"), "- item");
        assert_eq!(toggle_checkbox("- [X] item"), "- item");
    }

    #[test]
    fn numbered_items_and_indentation_are_kept() {
        assert_eq!(toggle_checkbox("1. item"), "1. [ ] item");
        assert_eq!(toggle_checkbox("12) [ ] item"), "12) [x] item");
        assert_eq!(toggle_checkbox("  * [x] item"), "  * item");
        assert_eq!(toggle_checkbox("\t+ item"), "\t+ [ ] item");
    }

    #[test]
    fn a_plain_line_becomes_an_unchecked_item() {
        assert_eq!(toggle_checkbox("call mum"), "- [ ] call mum");
        assert_eq!(toggle_checkbox("  call mum"), "  - [ ] call mum");
        assert_eq!(toggle_checkbox(""), "- [ ] ");
        // a dash with nothing after it is not a list marker
        assert_eq!(toggle_checkbox("-"), "- [ ] -");
        assert_eq!(toggle_checkbox("2024"), "- [ ] 2024");
    }

    #[test]
    fn headings_cycle_through_three_levels_and_off() {
        assert_eq!(cycle_heading("title"), "# title");
        assert_eq!(cycle_heading("# title"), "## title");
        assert_eq!(cycle_heading("## title"), "### title");
        assert_eq!(cycle_heading("### title"), "title");
        assert_eq!(cycle_heading("#### deep"), "deep");
    }

    #[test]
    fn a_hash_run_with_no_space_is_text_not_a_heading() {
        assert_eq!(cycle_heading("#hashtag"), "# #hashtag");
        assert_eq!(cycle_heading(""), "# ");
        assert_eq!(cycle_heading("#"), "## ");
    }
}

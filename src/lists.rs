//! Ordered lists that have had lines dropped into them.
//!
//! A pasted list brings its own numbers along: five items copied off the top
//! of one note land in the middle of another still counting from 1. Markdown
//! renderers paper over that — every renderer numbers an ordered list itself,
//! from the first item's number — but catcher shows the file, so what the
//! file says is what the page says, and the file says `8. 9. 10. 1. 2.`.
//!
//! So after a paste the lists it landed in are put back in order: each run of
//! items counts up from its own first number, nested lists count separately,
//! and a list written in the lazy `1. 1. 1.` style — where every marker is
//! deliberately the same and the renderer does the counting — is left exactly
//! as it was.

use std::ops::Range;

/// The marker an ordered item starts with: `12. ` or `3) `.
struct Marker {
    /// Leading whitespace, in chars (all of it ASCII, so bytes agree).
    indent: usize,
    /// Byte range of the digits, for rewriting them in place.
    digits: Range<usize>,
    number: u64,
    punct: char,
    /// Column the item's text starts at. A following line indented this far
    /// belongs to the item; anything shallower has left the list.
    content: usize,
}

/// One list still being counted. There is one of these per open indent level,
/// so a nested list numbers itself without disturbing its parent.
struct Level {
    indent: usize,
    content: usize,
    punct: char,
    /// The number the next item at this level should carry.
    next: u64,
    run: usize,
}

/// An item found, with the number it should end up with.
struct Item {
    row: usize,
    run: usize,
    digits: Range<usize>,
    old: u64,
    new: u64,
}

/// `line` as an ordered-list item, if it is one. `1.text` is not: a marker
/// needs a space after it, the same rule the renderer parses by.
fn marker(line: &str) -> Option<Marker> {
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let rest = &line[indent..];
    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    // 10 digits is past anything a list means, and past what u64 parses blind
    if digits == 0 || digits > 9 {
        return None;
    }
    let number: u64 = rest[..digits].parse().ok()?;
    let punct = rest[digits..]
        .chars()
        .next()
        .filter(|c| matches!(c, '.' | ')'))?;
    let after = &rest[digits + 1..];
    let spaces = after.len() - after.trim_start_matches(' ').len();
    if spaces == 0 && !after.is_empty() {
        return None;
    }
    Some(Marker {
        indent,
        digits: indent..indent + digits,
        number,
        punct,
        content: indent + digits + 1 + spaces,
    })
}

/// Whether `line` already carries an ordered marker. The caller uses it to
/// tell the lines an edit brought in from the one it started on.
pub fn is_item(line: &str) -> bool {
    marker(line).is_some()
}

/// The fence a code block opens with, if `line` opens one. The closing fence
/// has to be at least as long, which is why the run is kept and not just a flag.
fn opening_fence(line: &str) -> Option<String> {
    let rest = line.trim_start();
    let ch = rest.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let run = rest.chars().take_while(|c| *c == ch).count();
    (run >= 3).then(|| ch.to_string().repeat(run))
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

/// Every ordered item in `lines`, each tagged with the run it belongs to and
/// the number counting says it should have.
fn plan(lines: &[String]) -> Vec<Item> {
    let mut items = Vec::new();
    let mut stack: Vec<Level> = Vec::new();
    let mut runs = 0usize;
    let mut fence: Option<String> = None;
    for (row, line) in lines.iter().enumerate() {
        // inside a code block a `1.` is code, not an item
        if let Some(open) = &fence {
            if line.trim_start().starts_with(open.as_str()) {
                fence = None;
            }
            continue;
        }
        if let Some(open) = opening_fence(line) {
            fence = Some(open);
            continue;
        }
        // a blank line does not end a list; a loose list is full of them
        if line.trim().is_empty() {
            continue;
        }
        let Some(m) = marker(line) else {
            // a paragraph or a bullet at the list's own indent ends it; text
            // indented as far as an item's own text is that item's, and
            // leaves the count where it was
            let ind = indent_of(line);
            while stack.last().is_some_and(|l| ind < l.content) {
                stack.pop();
            }
            continue;
        };
        // a shallower item closes the lists nested inside it, and a different
        // delimiter at the same indent is a different list altogether
        while stack
            .last()
            .is_some_and(|l| l.indent > m.indent || (l.indent == m.indent && l.punct != m.punct))
        {
            stack.pop();
        }
        let level = match stack.last_mut() {
            // an item at a level already open carries on its count
            Some(l) if l.indent == m.indent => l,
            // otherwise a list starts here, from whatever number it says
            _ => {
                runs += 1;
                stack.push(Level {
                    indent: m.indent,
                    content: m.content,
                    punct: m.punct,
                    next: m.number,
                    run: runs - 1,
                });
                stack.last_mut().expect("just pushed")
            }
        };
        level.content = m.content;
        items.push(Item {
            row,
            run: level.run,
            digits: m.digits,
            old: m.number,
            new: level.next,
        });
        level.next = level.next.saturating_add(1);
    }
    items
}

/// Put the ordered lists that rows `from..=to` fall in back in order, leaving
/// every other list in the note alone. Returns each rewritten row and how many
/// chars it grew by, so a caller holding a cursor on one can follow it.
pub fn renumber(lines: &mut [String], from: usize, to: usize) -> Vec<(usize, isize)> {
    let items = plan(lines);
    let touched: Vec<usize> = items
        .iter()
        .filter(|i| i.row >= from && i.row <= to)
        .map(|i| i.run)
        .collect();
    let mut shifts = Vec::new();
    let mut done: Vec<usize> = Vec::new();
    for run in touched {
        if done.contains(&run) {
            continue;
        }
        done.push(run);
        let members: Vec<&Item> = items.iter().filter(|i| i.run == run).collect();
        // `1. 1. 1.` is a style, not a mistake: the renderer counts, and
        // renumbering it would overwrite what the writer meant. The list as it
        // stood before the edit is what says so, so the new lines are left out
        // of the question — they arrived with numbering of their own.
        let standing: Vec<&&Item> = members
            .iter()
            .filter(|i| i.row < from || i.row > to)
            .collect();
        if standing.len() > 1 && standing.iter().all(|i| i.old == standing[0].old) {
            continue;
        }
        for item in members {
            if item.old == item.new {
                continue;
            }
            let before = item.digits.len();
            let after = item.new.to_string();
            let delta = after.len() as isize - before as isize;
            lines[item.row].replace_range(item.digits.clone(), &after);
            shifts.push((item.row, delta));
        }
    }
    shifts
}

#[cfg(test)]
mod tests {
    use super::renumber;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(String::from).collect()
    }

    fn run(text: &str, from: usize, to: usize) -> String {
        let mut ls = lines(text);
        renumber(&mut ls, from, to);
        ls.join("\n")
    }

    #[test]
    fn a_pasted_list_picks_up_the_count_it_landed_in() {
        let before = "1. one\n2. two\n1. three\n2. four";
        assert_eq!(run(before, 2, 3), "1. one\n2. two\n3. three\n4. four");
    }

    #[test]
    fn the_list_keeps_the_number_it_starts_at() {
        assert_eq!(run("5. a\n1. b\n1. c", 1, 2), "5. a\n6. b\n7. c");
    }

    #[test]
    fn a_lazy_list_is_left_alone() {
        // every marker the same is the writer letting the renderer count
        assert_eq!(run("1. a\n1. b\n1. c", 1, 1), "1. a\n1. b\n1. c");
    }

    #[test]
    fn only_the_list_that_was_touched_is_renumbered() {
        let before = "1. a\n1. b\n\ntext\n\n1. c\n1. d";
        assert_eq!(run(before, 5, 6), "1. a\n1. b\n\ntext\n\n1. c\n2. d");
    }

    #[test]
    fn nested_lists_count_separately() {
        let before = "1. a\n   1. x\n   1. y\n1. b";
        assert_eq!(run(before, 0, 3), "1. a\n   1. x\n   2. y\n2. b");
    }

    #[test]
    fn blank_lines_and_wrapped_lines_do_not_end_a_list() {
        let before = "1. a\n   still a\n\n1. b\n1. c";
        assert_eq!(run(before, 0, 4), "1. a\n   still a\n\n2. b\n3. c");
    }

    #[test]
    fn a_paragraph_ends_the_list() {
        let before = "1. a\n1. b\nnot a list\n1. c\n1. d";
        assert_eq!(run(before, 0, 1), "1. a\n2. b\nnot a list\n1. c\n1. d");
    }

    #[test]
    fn a_bullet_list_between_two_ordered_ones_keeps_them_apart() {
        let before = "1. a\n1. b\n- x\n1. c\n1. d";
        assert_eq!(run(before, 3, 4), "1. a\n1. b\n- x\n1. c\n2. d");
    }

    #[test]
    fn widening_a_marker_reports_the_shift() {
        let mut ls = lines("9. a\n1. b");
        let shifts = renumber(&mut ls, 0, 1);
        assert_eq!(ls, ["9. a", "10. b"]);
        assert_eq!(shifts, [(1, 1)]);
    }

    #[test]
    fn a_marker_needs_its_space() {
        assert_eq!(run("1. a\n2.b\n1. c", 0, 2), "1. a\n2.b\n1. c");
    }

    #[test]
    fn paren_markers_count_too_and_stay_apart_from_dots() {
        assert_eq!(run("1) a\n1) b", 0, 1), "1) a\n2) b");
        assert_eq!(run("1. a\n1) b\n1) c", 1, 2), "1. a\n1) b\n2) c");
    }

    #[test]
    fn a_list_inside_a_code_block_is_code() {
        let before = "1. a\n1. b\n\n```\n1. not an item\n1. nor this\n```";
        assert_eq!(
            run(before, 0, 1),
            "1. a\n2. b\n\n```\n1. not an item\n1. nor this\n```"
        );
    }

    #[test]
    fn nothing_touched_changes_nothing() {
        let before = "1. a\n1. b\n1. c\n\npara";
        assert_eq!(run(before, 4, 4), before);
    }

    #[test]
    fn task_items_keep_their_boxes() {
        let before = "1. [ ] a\n1. [x] b\n1. [ ] c";
        assert_eq!(run(before, 1, 2), "1. [ ] a\n2. [x] b\n3. [ ] c");
    }

    #[test]
    fn a_lazy_list_stays_lazy_when_a_lazy_paste_lands_in_it() {
        // the list as it stood is what says the style, not the arriving lines
        assert_eq!(
            run("1. a\n1. b\n1. x\n1. c", 2, 2),
            "1. a\n1. b\n1. x\n1. c"
        );
    }
}

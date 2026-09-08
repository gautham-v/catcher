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
//!
//! Tab and shift-Tab in the editor move an item between levels, which asks
//! the same question of the same lists, so [`shift`] lives here too.

use std::ops::{Range, RangeInclusive};
use std::sync::RwLock;

/// Whether the rules down a nested list are drawn. The nesting itself is not
/// optional — a nested item has to start clear of its parent's wrapped text
/// either way — so this is the `│`s and nothing else.
static GUIDES: RwLock<bool> = RwLock::new(true);

pub fn set_guides(on: bool) {
    if let Ok(mut w) = GUIDES.write() {
        *w = on;
    }
}

pub fn guides() -> bool {
    GUIDES.read().map(|b| *b).unwrap_or(true)
}

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
    fix(lines, from..=to, from..=to)
}

/// Renumber every run with an item in `touched`. `fresh` is the rows the edit
/// itself wrote: they are left out of the question of whether a list is lazy,
/// because the list as it stood before the edit is what says its style.
fn fix(
    lines: &mut [String],
    touched: RangeInclusive<usize>,
    fresh: RangeInclusive<usize>,
) -> Vec<(usize, isize)> {
    let items = plan(lines);
    let runs: Vec<usize> = items
        .iter()
        .filter(|i| touched.contains(&i.row))
        .map(|i| i.run)
        .collect();
    let mut shifts = Vec::new();
    let mut done: Vec<usize> = Vec::new();
    for run in runs {
        if done.contains(&run) {
            continue;
        }
        done.push(run);
        let members: Vec<&Item> = items.iter().filter(|i| i.run == run).collect();
        // `1. 1. 1.` is a style, not a mistake: the renderer counts, and
        // renumbering it would overwrite what the writer meant. The list as it
        // stood before the edit is what says so, so the new lines are left out
        // of the question — they arrived with numbering of their own.
        let standing: Vec<&&Item> = members.iter().filter(|i| !fresh.contains(&i.row)).collect();
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

/// Which way [`shift`] moves an item.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Shift {
    In,
    Out,
}

/// Where a list item's marker sits and where its text starts: `- `, `* `,
/// `+ `, `- [ ] ` and `12. ` all count. A task's box is part of its text, the
/// way the parser reads it, so a task nests its children two columns in like
/// any other bullet.
fn item(line: &str) -> Option<(usize, usize)> {
    if let Some(m) = marker(line) {
        return Some((m.indent, m.content));
    }
    let indent = indent_of(line);
    let rest = &line[indent..];
    // `- ` with nothing after it is still an item: it is where one is starting
    let bullet = matches!(rest, "-" | "*" | "+")
        || rest.starts_with("- ")
        || rest.starts_with("* ")
        || rest.starts_with("+ ");
    bullet.then_some((indent, indent + 2))
}

/// How far in one nesting level draws, past the text column of the item it
/// sits under. Two columns reads as nested at a glance without pushing a deep
/// list off the right of the page.
pub const STEP: usize = 2;

/// Where a line of a list is drawn, which is not where the file puts it: a
/// nested item sits at its parent's text column plus [`STEP`], so it starts
/// to the right of the parent's own wrapped lines instead of on top of them,
/// and a rule stands in each ancestor's text column so the block it belongs
/// to is countable at a glance rather than measurable with a finger.
pub struct Nest {
    /// Display columns the rules stand in, outermost first.
    pub guides: Vec<usize>,
    /// Display column the line's own marker — or its text, for the wrapped
    /// lines of an item — starts at.
    pub indent: usize,
    /// How deeply nested the item is, 1 being top level. Counted from the
    /// items above it rather than guessed from its indent, and unaffected by
    /// whether the rules are drawn.
    pub depth: usize,
}

/// [`Nest`] for the line on `row`, or `None` when it draws exactly as the
/// file reads it: anything at the margin, and anything outside a list.
pub fn nest(lines: &[String], row: usize) -> Option<Nest> {
    nest_at(lines, row, guides())
}

/// [`nest`], with the rules asked for rather than looked up.
fn nest_at(lines: &[String], row: usize, ruled: bool) -> Option<Nest> {
    let line = lines.get(row)?;
    let ind = indent_of(line);
    if ind == 0 {
        return None;
    }
    // the items this line sits inside, innermost first: each one is the
    // nearest line above that starts further out than the last
    let mut chain: Vec<(usize, usize)> = Vec::new();
    let mut want = ind;
    for above in lines[..row].iter().rev() {
        if above.trim().is_empty() {
            continue;
        }
        let i = indent_of(above);
        // as far in as us or further is somebody else's item, or our own
        // wrapped text; either way it says nothing about where we belong
        if i >= want {
            continue;
        }
        let Some((ai, ac)) = item(above) else {
            // a paragraph further out than us is where the list ended
            break;
        };
        chain.push((ai, ac));
        want = ai;
        if ai == 0 {
            break;
        }
    }
    // each level draws from the one outside it, so the columns are counted
    // from the outermost item in rather than from the file's own indents:
    // whatever a nested list was written with, it is drawn one step past the
    // text of the item it hangs from
    let mut guides = Vec::new();
    let mut depth = chain.len() + 1;
    let mut indent = chain.last().map_or(0, |(ai, _)| *ai);
    for (ai, ac) in chain.iter().rev() {
        let content = indent + (ac - ai);
        guides.push(content);
        indent = content + STEP;
    }
    if !is_list_item(line) {
        // the wrapped lines of an item are its own text and not a level of
        // their own: they sit at its text column, inside the rules it stands
        // in rather than gaining one
        let (_, own) = *chain.first()?;
        indent = guides.pop()? + ind.saturating_sub(own);
        depth -= 1;
    }
    // never draw an item further in than the file puts it: a list written
    // with more indent than it needs keeps what it was written with
    indent = indent.max(ind);
    if !ruled {
        guides.clear();
    }
    (indent != ind || !guides.is_empty()).then_some(Nest {
        guides,
        indent,
        depth,
    })
}

/// Whether `line` is a list item of any kind: the lines ⇥ moves between
/// levels rather than pushing along by a tab's worth of spaces.
pub fn is_list_item(line: &str) -> bool {
    item(line).is_some()
}

/// The rows the item on `row` takes with it when it moves: its own, and the
/// lines nested under it — deeper items and wrapped text alike.
fn block(lines: &[String], row: usize, indent: usize) -> usize {
    let mut end = row;
    for (r, line) in lines.iter().enumerate().skip(row + 1) {
        if line.trim().is_empty() {
            continue;
        }
        if indent_of(line) <= indent {
            break;
        }
        end = r;
    }
    end
}

/// The indent the item on `row` should land at, or `None` when it has nowhere
/// to go: the first item of a list has nothing to nest under, and an item at
/// the margin is already as far out as it goes.
fn target(lines: &[String], row: usize, indent: usize, dir: Shift) -> Option<usize> {
    for line in lines[..row].iter().rev() {
        if line.trim().is_empty() {
            continue;
        }
        let ind = indent_of(line);
        // deeper lines are somebody else's children, and the item's own
        // wrapped text; neither says anything about where this item belongs
        if ind > indent {
            continue;
        }
        let Some((_, content)) = item(line) else {
            break;
        };
        match dir {
            // in: under the item above, its text column becoming our indent
            Shift::In => return (ind == indent).then_some(content),
            // out: past the siblings above, to the level of the parent
            Shift::Out if ind < indent => return Some(ind),
            Shift::Out => continue,
        }
    }
    (dir == Shift::Out && indent > 0).then_some(0)
}

/// The last row of the list the move happens in, so the numbers can be put
/// right through to the end of it. `floor` is the shallowest indent still
/// inside: a paragraph at or outside it has left the list.
fn list_end(lines: &[String], from: usize, floor: usize) -> usize {
    let mut end = from;
    for (r, line) in lines.iter().enumerate().skip(from + 1) {
        if line.trim().is_empty() {
            continue;
        }
        let ind = indent_of(line);
        if ind < floor || (ind <= floor && item(line).is_none()) {
            break;
        }
        end = r;
    }
    end
}

/// The row each ordered run starts on.
fn run_starts(lines: &[String]) -> Vec<usize> {
    let mut runs: Vec<usize> = Vec::new();
    let mut rows = Vec::new();
    for it in plan(lines) {
        if !runs.contains(&it.run) {
            runs.push(it.run);
            rows.push(it.row);
        }
    }
    rows
}

/// Move the list item on `row` one level in or out, taking what is nested
/// under it along, and leave the ordered lists it left and joined counting
/// straight. A sublist that only starts because of the move starts at 1 —
/// the number the item carried counted for the list it came from, and means
/// nothing where it landed. Returns the last row it moved, so a caller
/// walking a run of items can skip the ones that came along; `None` when
/// there was nowhere to move.
pub fn shift(lines: &mut [String], row: usize, dir: Shift) -> Option<usize> {
    let (indent, _) = lines.get(row).and_then(|l| item(l))?;
    let to = target(lines, row, indent, dir)?;
    if to == indent {
        return None;
    }
    let end = block(lines, row, indent);
    let bound = list_end(lines, end, indent.min(to));
    let started = run_starts(lines);
    for line in lines[row..=end].iter_mut() {
        if line.trim().is_empty() {
            continue;
        }
        if to > indent {
            line.insert_str(0, &" ".repeat(to - indent));
        } else {
            let cut = (indent - to).min(indent_of(line));
            line.replace_range(..cut, "");
        }
    }
    for start in run_starts(lines) {
        if start >= row && start <= bound && !started.contains(&start) {
            if let Some(m) = marker(&lines[start]) {
                lines[start].replace_range(m.digits, "1");
            }
        }
    }
    fix(lines, row..=bound, row..=end);
    Some(end)
}

#[cfg(test)]
mod tests {
    use super::{renumber, shift, Shift};

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

    fn nesting(text: &str, row: usize) -> Option<(Vec<usize>, usize)> {
        super::nest_at(&lines(text), row, true).map(|n| (n.guides, n.indent))
    }

    #[test]
    fn with_the_rules_off_the_nesting_still_stands_clear() {
        // the rules are furniture; the step past the parent's text is not
        let ls = lines("1. a\n   1. x");
        let off = super::nest_at(&ls, 1, false).unwrap();
        assert_eq!(off.guides, Vec::new());
        assert_eq!(off.indent, 5);
        // and a wrapped line, which only ever had rules, draws as it reads
        let ls = lines("1. a\n   more");
        assert!(super::nest_at(&ls, 1, false).is_none());
    }

    #[test]
    fn a_nested_item_draws_past_its_parent_s_text() {
        // `1. a` puts its text at column 3, so its children start at 5 and a
        // rule stands in column 3 — clear of the parent's own wrapped lines
        let text = "1. a\n   1. x\n      wrapped\n      1. deep\n2. b";
        assert_eq!(nesting(text, 0), None);
        assert_eq!(nesting(text, 1), Some((vec![3], 5)));
        // wrapped text is the item's own, so it keeps its rules and its column
        assert_eq!(nesting(text, 2), Some((vec![3], 8)));
        // a third level counts from the second: 5 + 3 = 8, so 8 and 10
        assert_eq!(nesting(text, 3), Some((vec![3, 8], 10)));
        assert_eq!(nesting(text, 4), None);
    }

    #[test]
    fn bullets_and_tasks_count_from_their_own_text_columns() {
        assert_eq!(nesting("- a\n  - b\n    - c", 1), Some((vec![2], 4)));
        assert_eq!(nesting("- a\n  - b\n    - c", 2), Some((vec![2, 6], 8)));
        // a task's box is text, so its children hang from the bullet
        assert_eq!(nesting("- [ ] a\n  - b", 1), Some((vec![2], 4)));
        // and a list under a bullet counts from the bullet's text column
        assert_eq!(nesting("- a\n  1. x", 1), Some((vec![2], 4)));
    }

    #[test]
    fn a_paragraph_outside_the_list_is_not_a_parent() {
        assert_eq!(nesting("para\n   indented", 1), None);
        // and a blank line does not break the chain
        assert_eq!(nesting("1. a\n\n   1. x", 2), Some((vec![3], 5)));
    }

    #[test]
    fn a_list_written_with_more_indent_than_it_needs_keeps_it() {
        assert_eq!(nesting("1. a\n      1. x", 1), Some((vec![3], 6)));
    }

    fn moved(text: &str, row: usize, dir: Shift) -> String {
        let mut ls = lines(text);
        shift(&mut ls, row, dir);
        ls.join("\n")
    }

    #[test]
    fn a_nested_ordered_list_starts_at_one() {
        // the number it carried counted for the list it came from
        assert_eq!(
            moved("1. a\n2. b\n3. c", 1, Shift::In),
            "1. a\n   1. b\n2. c"
        );
    }

    #[test]
    fn nesting_an_item_joins_the_sublist_already_there() {
        assert_eq!(
            moved("1. a\n   1. x\n2. b", 2, Shift::In),
            "1. a\n   1. x\n   2. b"
        );
    }

    #[test]
    fn an_item_takes_what_is_nested_under_it() {
        let before = "- a\n- b\n  - x\n    wrapped\n- c";
        assert_eq!(
            moved(before, 1, Shift::In),
            "- a\n  - b\n    - x\n      wrapped\n- c"
        );
    }

    #[test]
    fn the_first_item_of_a_list_has_nothing_to_nest_under() {
        let before = "para\n\n1. a\n2. b";
        assert_eq!(moved(before, 2, Shift::In), before);
        assert_eq!(moved("- a\n- b", 0, Shift::In), "- a\n- b");
    }

    #[test]
    fn coming_back_out_rejoins_the_count_and_restarts_what_is_left() {
        let before = "1. a\n   1. x\n   2. y\n   3. z";
        assert_eq!(moved(before, 2, Shift::Out), "1. a\n   1. x\n2. y\n   1. z");
    }

    #[test]
    fn an_item_at_the_margin_has_nowhere_further_out() {
        assert_eq!(moved("- a\n- b", 1, Shift::Out), "- a\n- b");
    }

    #[test]
    fn a_task_nests_under_the_bullet_not_the_box() {
        assert_eq!(
            moved("- [ ] a\n- [x] b", 1, Shift::In),
            "- [ ] a\n  - [x] b"
        );
    }

    #[test]
    fn a_lazy_list_stays_lazy_when_an_item_is_nested_out_of_it() {
        assert_eq!(
            moved("1. a\n1. b\n1. c", 1, Shift::In),
            "1. a\n   1. b\n1. c"
        );
    }

    #[test]
    fn a_paragraph_below_is_not_part_of_the_list() {
        let before = "1. a\n2. b\n\npara\n\n5. c";
        assert_eq!(moved(before, 1, Shift::In), "1. a\n   1. b\n\npara\n\n5. c");
    }

    #[test]
    fn a_bullet_between_ordered_items_keeps_its_own_indent() {
        assert_eq!(moved("- a\n- b\n- c", 2, Shift::In), "- a\n- b\n  - c");
    }
}

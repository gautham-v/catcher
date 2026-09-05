//! Find and replace inside the open note: the overlay behind the palette's
//! "Find in note". Pure over lines, so stepping and replacing test without
//! an editor behind them; the overlay in `app` owns the cursor and the undo.

/// One match: its row and the char range it covers on that row.
pub type Match = (usize, usize, usize);

/// Every place `needle` occurs in `lines`, in reading order, case never
/// mattering. Nothing for an empty needle: a blank query matches nowhere
/// rather than everywhere.
pub fn matches(lines: &[String], needle: &str) -> Vec<Match> {
    let needle = needle.to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (row, line) in lines.iter().enumerate() {
        for (s, e) in spans_in(line, &needle) {
            out.push((row, s, e));
        }
    }
    out
}

/// The char ranges of `needle` (already lowered) on `line`, left to right and
/// non-overlapping.
fn spans_in(line: &str, needle: &str) -> Vec<(usize, usize)> {
    let lower: Vec<char> = line.to_lowercase().chars().collect();
    let chars = line.chars().count();
    // lowering can change a string's length in chars for a few scripts; the
    // ranges are by char position and give up rather than misalign
    if lower.len() != chars {
        return Vec::new();
    }
    let n: Vec<char> = needle.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + n.len() <= lower.len() {
        if lower[i..i + n.len()] == n[..] {
            out.push((i, i + n.len()));
            i += n.len();
        } else {
            i += 1;
        }
    }
    out
}

/// The index of the first match at or after `(row, col)`, wrapping to the
/// first when none is; `None` when there are no matches.
pub fn next_from(found: &[Match], (row, col): (usize, usize)) -> Option<usize> {
    if found.is_empty() {
        return None;
    }
    Some(
        found
            .iter()
            .position(|&(r, s, _)| (r, s) >= (row, col))
            .unwrap_or(0),
    )
}

/// `line` with chars `start..end` swapped for `with`.
pub fn replace_span(line: &str, start: usize, end: usize, with: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out: String = chars[..start.min(chars.len())].iter().collect();
    out.push_str(with);
    out.extend(chars[end.min(chars.len())..].iter());
    out
}

/// Every occurrence of `needle` in `lines` swapped for `with`: the new lines
/// and how many were replaced. What replace-all commits as one undo step.
pub fn replace_all(lines: &[String], needle: &str, with: &str) -> (Vec<String>, usize) {
    let needle = needle.to_lowercase();
    let mut count = 0;
    let out = lines
        .iter()
        .map(|line| {
            if needle.is_empty() {
                return line.clone();
            }
            let spans = spans_in(line, &needle);
            count += spans.len();
            // right to left, so the earlier offsets stay true
            spans
                .iter()
                .rev()
                .fold(line.clone(), |l, &(s, e)| replace_span(&l, s, e, with))
        })
        .collect();
    (out, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(String::from).collect()
    }

    #[test]
    fn matches_are_found_in_any_case_and_do_not_overlap() {
        let l = lines("Milk and milk\naaa\nnone");
        assert_eq!(matches(&l, "MILK"), vec![(0, 0, 4), (0, 9, 13)]);
        assert_eq!(matches(&l, "aa"), vec![(1, 0, 2)]);
        assert!(matches(&l, "").is_empty());
        // offsets are chars, not bytes
        assert_eq!(matches(&lines("ééx"), "x"), vec![(0, 2, 3)]);
    }

    #[test]
    fn the_next_match_is_at_or_after_the_cursor_and_wraps() {
        let found = vec![(0, 2, 4), (2, 0, 1), (2, 5, 6)];
        assert_eq!(next_from(&found, (0, 0)), Some(0));
        assert_eq!(next_from(&found, (0, 2)), Some(0));
        assert_eq!(next_from(&found, (0, 3)), Some(1));
        assert_eq!(next_from(&found, (2, 1)), Some(2));
        assert_eq!(next_from(&found, (9, 0)), Some(0));
        assert_eq!(next_from(&[], (0, 0)), None);
    }

    #[test]
    fn replace_all_swaps_every_occurrence_and_counts_them() {
        let l = lines("Milk and MILK\nno\nmilkmilk");
        let (out, n) = replace_all(&l, "milk", "tea");
        assert_eq!(out, lines("tea and tea\nno\nteatea"));
        assert_eq!(n, 4);
        // a replacement that contains the needle is not chased
        let (out, n) = replace_all(&lines("a"), "a", "aa");
        assert_eq!(out, lines("aa"));
        assert_eq!(n, 1);
        let (out, n) = replace_all(&l, "", "x");
        assert_eq!(out, l);
        assert_eq!(n, 0);
        assert_eq!(replace_span("héllo", 1, 2, "E"), "hEllo");
    }
}

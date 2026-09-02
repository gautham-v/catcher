/// Case-insensitive subsequence fuzzy match. Higher is better; None = no match.
/// Rewards contiguous runs, word-boundary hits, and early matches. A query
/// with spaces is several words, each matched on its own, so `story mat`
/// finds `story-matrix`: the space is where the reader's word ends, not a
/// character the filename has to contain.
pub fn fuzzy(query: &str, text: &str) -> Option<i64> {
    let mut words = query.split_whitespace();
    let Some(first) = words.next() else {
        return Some(0);
    };
    let lowered = |w: &str| {
        w.chars()
            .flat_map(char::to_lowercase)
            .collect::<Vec<char>>()
    };
    let mut score = fuzzy_word(&lowered(first), text)?;
    for w in words {
        score += fuzzy_word(&lowered(w), text)?;
    }
    Some(score)
}

/// `query` is already lowered. The haystack is lowered as it is walked,
/// with one char of lookback for the word-boundary test, so scoring an
/// index of thousands of entries allocates nothing per entry.
fn fuzzy_word(q: &[char], text: &str) -> Option<i64> {
    let mut score: i64 = 0;
    let mut qi = 0;
    let mut last_hit: Option<usize> = None;
    let mut prev: Option<char> = None;
    for (ti, c) in text.chars().flat_map(char::to_lowercase).enumerate() {
        if qi < q.len() && c == q[qi] {
            score += 1;
            if last_hit == Some(ti.wrapping_sub(1)) {
                score += 4; // contiguous
            }
            if prev.is_none_or(|p| !p.is_alphanumeric()) {
                score += 3; // word boundary
            }
            last_hit = Some(ti);
            qi += 1;
            if qi == q.len() {
                break;
            }
        }
        prev = Some(c);
    }
    if qi < q.len() {
        return None;
    }
    // earlier first-hit is better; shorter haystack is a mild tiebreak
    score -= (last_hit.unwrap_or(0) as i64) / 8;
    Some(score)
}

/// How well an index entry answers `query`: the filename first, the title
/// second, and the path relative to its root a weaker third, so
/// `applications/log` finds it too.
pub fn score_entry(query: &str, name: &str, title: &str, rel: &str) -> Option<i64> {
    let by_name = fuzzy(query, name).map(|s| s * 10 + 100);
    let by_title = fuzzy(query, title).map(|s| s * 10 + 50);
    let by_path = fuzzy(query, rel);
    by_name.into_iter().chain(by_title).chain(by_path).max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_ranks_name_then_title_then_path() {
        assert!(score_entry(
            "story mat",
            "story-matrix",
            "Story matrix",
            "interviews/stories/story-matrix"
        )
        .is_some());
        assert!(score_entry(
            "xyz",
            "story-matrix",
            "Story matrix",
            "interviews/stories/story-matrix"
        )
        .is_none());
        // the path alone is enough, but weaker than a filename hit
        let by_path = score_entry(
            "interviews",
            "story-matrix",
            "Story matrix",
            "interviews/stories/story-matrix",
        )
        .unwrap();
        let by_name = score_entry(
            "story",
            "story-matrix",
            "Story matrix",
            "interviews/stories/story-matrix",
        )
        .unwrap();
        assert!(by_name > by_path);
    }

    #[test]
    fn matches() {
        assert!(fuzzy("groc", "Groceries").is_some());
        assert!(fuzzy("xyz", "Groceries").is_none());
        assert!(fuzzy("", "anything").is_some());
        let exact = fuzzy("meet", "Meeting notes").unwrap();
        let scattered = fuzzy("meet", "my extra effort today").unwrap();
        assert!(exact > scattered);
    }

    #[test]
    fn a_space_in_the_query_separates_words_rather_than_being_one() {
        assert!(fuzzy("story mat", "story-matrix").is_some());
        assert!(fuzzy("story mat", "airstream-global-chip-shortage").is_none());
        assert!(fuzzy("mat story", "story-matrix").is_some());
        assert_eq!(fuzzy("   ", "anything"), Some(0));
    }
}

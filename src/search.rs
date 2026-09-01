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
    let mut score = fuzzy_word(first, text)?;
    for w in words {
        score += fuzzy_word(w, text)?;
    }
    Some(score)
}

fn fuzzy_word(query: &str, text: &str) -> Option<i64> {
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut score: i64 = 0;
    let mut qi = 0;
    let mut last_hit: Option<usize> = None;
    for (ti, &c) in t.iter().enumerate() {
        if qi < q.len() && c == q[qi] {
            score += 1;
            if last_hit == Some(ti.wrapping_sub(1)) {
                score += 4; // contiguous
            }
            if ti == 0 || !t[ti - 1].is_alphanumeric() {
                score += 3; // word boundary
            }
            last_hit = Some(ti);
            qi += 1;
            if qi == q.len() {
                break;
            }
        }
    }
    if qi < q.len() {
        return None;
    }
    // earlier first-hit is better; shorter haystack is a mild tiebreak
    score -= (last_hit.unwrap_or(0) as i64) / 8;
    Some(score)
}

/// Score a note by title and body; title hits dominate.
/// How well a note answers `query`: the filename is what the palette shows,
/// so it ranks first; the title is a second chance, and the body a weaker
/// third.
pub fn score_note(query: &str, name: &str, title: &str, body: &str) -> Option<i64> {
    let by_name = fuzzy(query, name).map(|s| s * 10 + 100);
    let by_title = fuzzy(query, title).map(|s| s * 10 + 50);
    let by_body = fuzzy(query, body);
    by_name.into_iter().chain(by_title).chain(by_body).max()
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn notes_rank_name_then_title_then_body() {
        let by_name = score_note("log", "log", "Daily journal", "nothing here").unwrap();
        let by_title = score_note("log", "2026-09-01", "Log", "nothing here").unwrap();
        let by_body = score_note("log", "2026-09-01", "Journal", "see the log").unwrap();
        assert!(by_name > by_title);
        assert!(by_title > by_body);
        assert!(score_note("xyz", "a", "b", "c").is_none());
    }
}

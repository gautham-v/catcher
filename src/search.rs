/// Case-insensitive subsequence fuzzy match. Higher is better; None = no match.
/// Rewards contiguous runs, word-boundary hits, and early matches.
pub fn fuzzy(query: &str, text: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
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
pub fn score_note(query: &str, title: &str, body: &str) -> Option<i64> {
    let by_title = fuzzy(query, title).map(|s| s * 10 + 100);
    let by_body = fuzzy(query, body);
    match (by_title, by_body) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
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
}

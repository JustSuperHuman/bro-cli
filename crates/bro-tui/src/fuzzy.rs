//! Tiny fuzzy matcher for the palette and launcher: every query character must appear in order; contiguous
//! runs, word starts and early matches score higher.

/// Score `hay` against `query` (case-insensitive, spaces in the query ignored). None = no match.
pub fn score(query: &str, hay: &str) -> Option<i32> {
    let q: Vec<char> = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if q.is_empty() {
        return Some(0);
    }
    let h: Vec<char> = hay.to_lowercase().chars().collect();
    let mut qi = 0;
    let mut score = 0i32;
    let mut prev: Option<usize> = None;
    for (i, &c) in h.iter().enumerate() {
        if qi < q.len() && c == q[qi] {
            let start = i == 0 || !h[i - 1].is_alphanumeric();
            score += 10;
            if start {
                score += 8;
            }
            if prev == Some(i.wrapping_sub(1)) {
                score += 6;
            }
            if i < 12 {
                score += (12 - i as i32) / 3;
            }
            prev = Some(i);
            qi += 1;
        }
    }
    (qi == q.len()).then_some(score - h.len() as i32 / 8)
}

/// Indices of `items` matching `query`, best first (stable for ties).
pub fn filter<T>(query: &str, items: &[T], text: impl Fn(&T) -> String) -> Vec<usize> {
    let mut scored: Vec<(i32, usize)> = items.iter().enumerate().filter_map(|(i, it)| score(query, &text(it)).map(|s| (s, i))).collect();
    if query.trim().is_empty() {
        return scored.into_iter().map(|x| x.1).collect();
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|x| x.1).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_sensibly() {
        assert!(score("usg", "usage view").is_some());
        assert!(score("xyz", "usage").is_none());
        assert!(score("", "anything").is_some());
        let items = ["split right", "usage: meters", "open profiles", "quit bro"];
        assert_eq!(filter("pro", &items, |s| s.to_string())[0], 2);
        assert_eq!(filter("q", &items, |s| s.to_string())[0], 3);
        assert_eq!(filter("", &items, |s| s.to_string()), vec![0, 1, 2, 3]);
        assert!(score("sr", "split right").unwrap() > score("sr", "session resume later").unwrap_or(0) - 100);
    }
}

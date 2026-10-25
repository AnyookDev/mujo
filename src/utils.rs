pub fn fuzzy_score(needle: &str, haystack: &str) -> i32 {
    if needle.is_empty() {
        return 0;
    }
    let needle_lower = needle.to_lowercase();
    let haystack_lower = haystack.to_lowercase();
    if haystack_lower == needle_lower {
        return 10000;
    }
    if let Some(pos) = haystack_lower.find(&needle_lower) {
        let bonus = if pos == 0 { 100 } else { 50 };
        return 1000 + bonus - (haystack.len() as i32);
    }
    let mut needle_chars = needle_lower.chars();
    let mut current = needle_chars.next();
    let mut score = 0;
    let mut last_pos = 0;
    let mut consecutive = 0;
    for (i, c) in haystack_lower.chars().enumerate() {
        if Some(c) == current {
            score += 10;
            if i == last_pos {
                consecutive += 1;
                score += consecutive * 5;
            } else {
                consecutive = 0;
            }
            last_pos = i + 1;
            current = needle_chars.next();
        }
    }
    if current.is_none() {
        score
    } else {
        0
    }
}

pub fn hslice(s: &str, off: usize) -> String {
    s.chars().skip(off).collect()
}

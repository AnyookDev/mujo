use crate::utils::fuzzy_score;

/// Search entries against a query, returning indices into `entries` sorted
/// by relevance (best first). Each entry is a (full_path, display_name)
/// pair. Matching is case-insensitive and fuzzy; a query containing spaces
/// is also tried with the spaces replaced by '/' so e.g. "work git" can
/// match "work/github". Entries scoring 0 are dropped, and ties keep their
/// original order (the sort is stable).
pub fn search(entries: &[(String, String)], query: &str) -> Vec<usize> {
    if query.trim().is_empty() {
        return (0..entries.len()).collect();
    }

    let query = query.to_lowercase();
    let query_alt = query.replace(' ', "/");

    let mut scored: Vec<(usize, i32)> = entries
        .iter()
        .enumerate()
        .map(|(idx, (path, name))| {
            let score = fuzzy_score(&query, path)
                .max(fuzzy_score(&query_alt, path))
                .max(fuzzy_score(&query, name))
                .max(fuzzy_score(&query_alt, name));
            (idx, score)
        })
        .filter(|(_, score)| *score > 0)
        .collect();

    scored.sort_by_key(|a| std::cmp::Reverse(a.1));
    scored.into_iter().map(|(idx, _)| idx).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mock entry list from the test plan, as (full_path, display_name)
    /// pairs - the same shape App::rebuild_filtered feeds to search().
    fn mock_entries() -> Vec<(String, String)> {
        [
            "work/github",
            "work/gitlab",
            "personal/bank",
            "shopping/amazon",
            "test-entry",
        ]
        .iter()
        .map(|p| {
            let name = p.rsplit('/').next().unwrap_or(p).to_string();
            (p.to_string(), name)
        })
        .collect()
    }

    #[test]
    fn test_exact_match() {
        let entries = mock_entries();
        // "github" exactly matches the display name of work/github, and no
        // other entry comes close enough to score above zero.
        assert_eq!(search(&entries, "github"), vec![0]);
    }

    #[test]
    fn test_prefix_match() {
        let entries = mock_entries();
        // "git" matches both work/* entries as a substring of their names.
        // They score identically, so the stable sort keeps original order.
        assert_eq!(search(&entries, "git"), vec![0, 1]);
    }

    #[test]
    fn test_fuzzy_match() {
        let entries = mock_entries();
        // "gh" is not a substring of "github" but fuzzy-matches it
        // (g...h subsequence). No other entry contains both letters in
        // order after a 'g'.
        assert_eq!(search(&entries, "gh"), vec![0]);
    }

    #[test]
    fn test_case_insensitive() {
        let entries = mock_entries();
        assert_eq!(search(&entries, "GITHUB"), vec![0]);
        assert_eq!(search(&entries, "BANK"), vec![2]);
        assert_eq!(search(&entries, "AmAzOn"), vec![3]);
    }

    #[test]
    fn test_no_match() {
        let entries = mock_entries();
        assert!(search(&entries, "xyz").is_empty());
        // A query whose letters exist but never in the right order.
        assert!(search(&entries, "zzz").is_empty());
    }

    #[test]
    fn test_multiple_matches() {
        let entries = mock_entries();
        // "work" matches both work/* entries.
        let results = search(&entries, "work");
        assert_eq!(results.len(), 2);
        assert!(results.contains(&0));
        assert!(results.contains(&1));

        // Relevance ordering, verified with a query that scores the
        // matches differently: "a" substring-matches the amazon filename
        // at position 0 (highest bonus), then bank, then gitlab.
        assert_eq!(search(&entries, "a"), vec![3, 2, 1]);
    }

    #[test]
    fn test_path_matching() {
        let entries = mock_entries();
        // "work/git" matches the nested paths of both work/* entries.
        assert_eq!(search(&entries, "work/git"), vec![0, 1]);
        // A full nested path matches exactly.
        assert_eq!(search(&entries, "shopping/amazon"), vec![3]);
        // A folder name alone finds the entry beneath it.
        assert_eq!(search(&entries, "personal"), vec![2]);
    }

    #[test]
    fn test_special_chars() {
        let entries = mock_entries();
        // Extend the mock list with dots, dashes and underscores.
        let mut entries = entries;
        for p in ["notes.old", "my_page", "archive/old-stuff_v2"] {
            let name = p.rsplit('/').next().unwrap_or(p).to_string();
            entries.push((p.to_string(), name));
        }
        // Indices: 0=work/github 1=work/gitlab 2=personal/bank
        //          3=shopping/amazon 4=test-entry 5=notes.old
        //          6=my_page 7=archive/old-stuff_v2

        // Dashes, dots and underscores are matched literally.
        assert_eq!(search(&entries, "test-entry"), vec![4]);
        assert_eq!(search(&entries, "notes.old"), vec![5]);
        assert_eq!(search(&entries, "my_page"), vec![6]);

        // "old" matches both the dotted and the dashed/underscored names;
        // the archive entry ranks first via its filename prefix bonus.
        assert_eq!(search(&entries, "old"), vec![7, 5]);
    }

    #[test]
    fn test_empty_query_returns_all() {
        let entries = mock_entries();
        assert_eq!(search(&entries, ""), vec![0, 1, 2, 3, 4]);
        assert_eq!(search(&entries, "   "), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn test_space_in_query_matches_path_separator() {
        let entries = mock_entries();
        // "work git" behaves like "work/git" thanks to the '/' replacement.
        assert_eq!(search(&entries, "work git"), vec![0, 1]);
    }
}

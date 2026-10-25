//! Scenario 4: TUI search navigation - create real entries on disk, then
//! run the same search App::rebuild_filtered feeds into the list.

use mujo::search::search;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_search_navigation() {
    // Create real entries in a temp store, as they look on disk before
    // App::load_entries walks the tree.
    let dir = TempDir::new().unwrap();
    for name in ["alpha", "beta", "gamma"] {
        fs::write(dir.path().join(format!("{}.gpg", name)), "placeholder").unwrap();
    }

    // Build the (full_path, display_name) pairs exactly the way
    // App::rebuild_filtered does: store-relative path without ".gpg".
    let mut entries: Vec<(String, String)> = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let file = e.file_name().into_string().ok()?;
            let stem = file.strip_suffix(".gpg")?.to_string();
            Some((stem.clone(), stem))
        })
        .collect();
    entries.sort();

    let idx_of = |name: &str| {
        entries
            .iter()
            .position(|(p, _)| p == name)
            .unwrap_or_else(|| panic!("entry {} missing from the list", name))
    };

    // Search "a": every entry contains an 'a', but "alpha" prefix-matches
    // and must rank first.
    let results = search(&entries, "a");
    assert!(!results.is_empty(), "\"a\" should match entries");
    assert!(
        results.contains(&idx_of("alpha")),
        "\"a\" should find alpha"
    );
    assert_eq!(
        entries[results[0]].0, "alpha",
        "\"a\" should rank alpha as the best match"
    );

    // Search "g": only "gamma" contains a 'g'.
    let results = search(&entries, "g");
    assert_eq!(results.len(), 1, "only gamma should match \"g\"");
    assert_eq!(entries[results[0]].0, "gamma");

    // An exact query finds exactly gamma.
    assert_eq!(search(&entries, "gamma"), vec![idx_of("gamma")]);

    // A query matching nothing yields no results.
    assert!(search(&entries, "zzz").is_empty());
}

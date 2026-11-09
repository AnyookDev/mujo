use super::App;
use crate::models::PassEntry;
use crate::search;
use std::time::Instant;

impl App {
    pub fn move_selection(&mut self, delta: i32) {
        let len = self.filtered_entries.len() as i32;
        if len > 0 {
            let new = (self.selected as i32 + delta).clamp(0, len - 1) as usize;
            if new != self.selected {
                self.selected = new;
                self.clear_preview();
            }
        }
    }

    pub fn jump_selection(&mut self, delta: i32) {
        if self.filtered_entries.is_empty() {
            return;
        }

        let current_idx = self.filtered_entries[self.selected];
        let current = &self.entries[current_idx];
        let depth = current.depth;
        let parent = current.path.parent().map(|p| p.to_path_buf());

        let siblings: Vec<usize> = self
            .filtered_entries
            .iter()
            .enumerate()
            .filter(|(_, &ei)| {
                let e = &self.entries[ei];
                e.depth == depth && e.path.parent().map(|p| p.to_path_buf()) == parent
            })
            .map(|(i, _)| i)
            .collect();

        if siblings.is_empty() {
            return;
        }

        let pos = siblings
            .iter()
            .position(|&i| i == self.selected)
            .unwrap_or(0);
        let new_pos = (pos as i32 + delta).clamp(0, siblings.len() as i32 - 1) as usize;
        let new_selected = siblings[new_pos];
        if new_selected != self.selected {
            self.selected = new_selected;
            self.clear_preview();
        }
    }

    /// Scan same-folder siblings forward from (but not including) the current
    /// selection, wrapping, for the first entry whose display_name starts with
    /// `prefix` (ASCII-case-insensitive). Returns true when a match was found
    /// and the selection moved.
    pub fn jump_to_prefix(&mut self, prefix: &str) -> bool {
        if self.filtered_entries.is_empty() || prefix.is_empty() {
            return false;
        }

        let prefix_lower = prefix.to_lowercase();

        let current_idx = self.filtered_entries[self.selected];
        let current = &self.entries[current_idx];
        let depth = current.depth;
        let parent = current.path.parent().map(|p| p.to_path_buf());

        // Collect sibling positions (indices into filtered_entries) in order.
        let siblings: Vec<usize> = self
            .filtered_entries
            .iter()
            .enumerate()
            .filter(|(_, &ei)| {
                let e = &self.entries[ei];
                e.depth == depth && e.path.parent().map(|p| p.to_path_buf()) == parent
            })
            .map(|(i, _)| i)
            .collect();

        if siblings.is_empty() {
            return false;
        }

        // Find origin position in the sibling list.
        let origin_pos = siblings
            .iter()
            .position(|&i| i == self.selected)
            .unwrap_or(0);

        // Scan forward from origin+1, wrapping, stopping before origin.
        let n = siblings.len();
        for step in 1..=n {
            let pos = (origin_pos + step) % n;
            let fi = siblings[pos];
            let name = &self.entries[self.filtered_entries[fi]].display_name;
            if name.to_lowercase().starts_with(&prefix_lower) {
                if fi != self.selected {
                    self.selected = fi;
                    self.clear_preview();
                }
                return true;
            }
        }

        false
    }

    pub fn toggle_folder(&mut self) {
        if let Some(idx) = self.filtered_entries.get(self.selected) {
            let entry = &mut self.entries[*idx];
            if entry.is_dir {
                entry.expanded = !entry.expanded;
                self.rebuild_filtered();
            }
        }
    }

    pub fn rebuild_filtered(&mut self) {
        self.filtered_entries.clear();

        if !self.search_query.is_empty() {
            let entries_for_search: Vec<(String, String)> = self
                .entries
                .iter()
                .map(|entry| {
                    let full_path = entry
                        .path
                        .strip_prefix(&self.store_path)
                        .unwrap_or(&entry.path)
                        .to_string_lossy()
                        .to_string();
                    (full_path, entry.display_name.clone())
                })
                .collect();

            self.filtered_entries = search::search(&entries_for_search, &self.search_query);
        } else {
            let mut visible_entries: Vec<(usize, &PassEntry)> = Vec::new();
            let mut skip_depth = None::<usize>;

            for (i, entry) in self.entries.iter().enumerate() {
                if let Some(skip) = skip_depth {
                    if entry.depth > skip {
                        continue;
                    } else {
                        skip_depth = None;
                    }
                }

                if entry.is_dir && !entry.expanded {
                    skip_depth = Some(entry.depth);
                }

                visible_entries.push((i, entry));
            }

            self.filtered_entries = visible_entries.into_iter().map(|(idx, _)| idx).collect();
        }

        self.selected = self
            .selected
            .min(self.filtered_entries.len().saturating_sub(1));
    }

    /// Enter/decrypt the currently selected entry, mirroring the Normal-mode
    /// Enter behavior: toggle a collapsed folder, move into an expanded one,
    /// otherwise decrypt the entry. Shared by Normal mode and Search mode
    /// so Enter behaves identically in both.
    pub fn activate_selected(&mut self) {
        if let Some(idx) = self.filtered_entries.get(self.selected) {
            let entry = &self.entries[*idx];
            if entry.is_dir && !entry.expanded {
                self.toggle_folder();
            } else if entry.is_dir && entry.expanded {
                self.move_selection(1);
            } else if self.current_content.is_none()
                || self.current_entry_path != self.get_selected_rel_path()
            {
                if let Err(e) = self.decrypt_current_entry(true) {
                    self.message = Some((format!("Decrypt error: {}", e), Instant::now()));
                }
            }
        }
    }

    /// Clear an active search filter and rebuild the full entry list. The
    /// currently selected entry stays selected when it still exists in the
    /// full list (matched by path), so clearing the filter is not
    /// disorienting. No-op when no filter is active.
    pub fn clear_search_filter(&mut self) {
        if self.search_query.is_empty() {
            return;
        }
        // Capture BEFORE clearing so the selection can be restored after
        // the full list is rebuilt. Both Esc paths (Search mode and Normal
        // mode) funnel through this helper, so this covers both.
        let keep = self
            .filtered_entries
            .get(self.selected)
            .and_then(|&i| self.entries.get(i))
            .map(|e| e.path.clone());

        // rebuild_filtered() omits children of collapsed folders, so a
        // searched entry nested in a collapsed folder would be missing from
        // the rebuilt list and the selection would fall back to 0. Expand
        // every ancestor directory of the captured entry (from its parent
        // up to the store root) so it becomes visible after the rebuild.
        if let Some(ref path) = keep {
            let mut ancestor = path.parent();
            while let Some(dir) = ancestor {
                if !dir.starts_with(&self.store_path) || dir == self.store_path {
                    break;
                }
                if let Some(entry) = self.entries.iter_mut().find(|e| e.is_dir && e.path == dir) {
                    entry.expanded = true;
                }
                ancestor = dir.parent();
            }
        }

        self.search_query.clear();
        self.rebuild_filtered();
        match keep {
            Some(path) => self.select_entry_by_path(&path),
            None => self.selected = 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::config::Config;
    use crate::models::{AppMode, GitPaneState, NewStoreStage, PassEntry};
    use std::path::PathBuf;
    use std::time::Instant;

    /// Build a minimal App whose entry list is a flat list of files at depth 0
    /// under a fake store root. All share the same parent (the root), so they
    /// are all siblings for jump_to_prefix.
    fn app_with_entries(names: &[&str]) -> App {
        let root = PathBuf::from("/store");
        let entries: Vec<PassEntry> = names
            .iter()
            .map(|&name| PassEntry {
                path: root.join(format!("{}.gpg", name)),
                name: name.to_string(),
                display_name: name.to_string(),
                is_dir: false,
                depth: 0,
                expanded: false,
            })
            .collect();
        let n = entries.len();
        let now = Instant::now();
        App {
            store_path: root,
            entries,
            filtered_entries: (0..n).collect(),
            selected: 0,
            scroll_offset: 0,
            mode: AppMode::Normal,
            command_buffer: String::new(),
            command_cursor: 0,
            search_query: String::new(),
            show_password: false,
            show_qr: false,
            current_content: None,
            current_entry_path: None,
            message: None,
            should_quit: false,
            editor: None,
            git_output: String::new(),
            git_pane: GitPaneState::Uninitialized,
            git_branch: String::new(),
            git_ahead: 0,
            git_behind: 0,
            git_uncommitted: 0,
            git_has_upstream: false,
            git_has_remote: false,
            git_remote_name: "origin".to_string(),
            git_status_short: String::new(),
            needs_clear: false,
            tui_active: false,
            git_log: String::new(),
            git_scroll: 0,
            git_h_scroll: 0,
            help_scroll_offset: 0,
            help_return_mode: AppMode::Normal,
            entry_to_delete: None,
            pending_pull: false,
            pull_return_mode: AppMode::Git,
            last_reload: now,
            security_clear_at: None,
            clipboard_tool: None,
            last_click: None,
            context_menu: None,
            move_source: None,
            config: Config::default(),
            preview_scroll_offset: 0,
            preview_h_scroll: 0,
            preview_selected_line: None,
            git_identity_name: String::new(),
            git_pending_count: 0,
            last_sync: None,
            git_last_remote_update: None,
            new_store_stage: NewStoreStage::Confirm,
            jump_buf: String::new(),
            jump_at: None,
        }
    }

    fn name(app: &App) -> &str {
        &app.entries[app.filtered_entries[app.selected]].display_name
    }

    #[test]
    fn single_char_lands_on_first_match() {
        // entries: alice, bob, carol
        // Start at alice (0); pressing 'B' should land on bob (1).
        let mut app = app_with_entries(&["alice", "bob", "carol"]);
        let matched = app.jump_to_prefix("b");
        assert!(matched);
        assert_eq!(name(&app), "bob");
    }

    #[test]
    fn two_chars_narrow_to_precise_match() {
        // entries: alice, anna, bob — "al" matches alice but not anna.
        let mut app = app_with_entries(&["anna", "alice", "bob"]);
        // Start at anna (0); "al" should jump to alice (1).
        let matched = app.jump_to_prefix("al");
        assert!(matched);
        assert_eq!(name(&app), "alice");
    }

    #[test]
    fn dead_prefix_returns_false() {
        let mut app = app_with_entries(&["alice", "bob", "carol"]);
        let matched = app.jump_to_prefix("xyz");
        assert!(!matched);
        assert_eq!(app.selected, 0, "selection must not move on no match");
    }

    #[test]
    fn matching_is_case_insensitive() {
        let mut app = app_with_entries(&["Alice", "Bob", "Carol"]);
        // Lowercase prefix against capitalised display_name.
        let matched = app.jump_to_prefix("b");
        assert!(matched);
        assert_eq!(name(&app), "Bob");
    }

    #[test]
    fn wraps_from_last_entry() {
        // entries: apple, banana, cherry — start at cherry (2); 'a' wraps to apple (0).
        let mut app = app_with_entries(&["apple", "banana", "cherry"]);
        app.selected = 2;
        let matched = app.jump_to_prefix("a");
        assert!(matched);
        assert_eq!(name(&app), "apple");
    }

    #[test]
    fn jj_prefix_matches_jj_entry_rather_than_cycling() {
        // "JJ" prefix: an entry named "JJsomething" must be preferred over
        // cycling back to "jack" on a second 'J'.
        let mut app = app_with_entries(&["jack", "JJsomething", "jill"]);
        // Start at jack (0); "jj" should jump to JJsomething (1).
        let matched = app.jump_to_prefix("jj");
        assert!(matched);
        assert_eq!(name(&app).to_lowercase(), "jjsomething");
    }

    #[test]
    fn no_match_on_empty_list() {
        let mut app = app_with_entries(&[]);
        assert!(!app.jump_to_prefix("a"));
    }
}

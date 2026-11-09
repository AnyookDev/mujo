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

    pub fn jump_to_letter(&mut self, letter: char) {
        if self.filtered_entries.is_empty() {
            return;
        }

        let target = letter.to_lowercase().next().unwrap_or(letter);

        let current_idx = self.filtered_entries[self.selected];
        let current = &self.entries[current_idx];
        let depth = current.depth;
        let parent = current.path.parent().map(|p| p.to_path_buf());

        // Only siblings at the same depth under the same parent, matching the
        // first letter case-insensitively. Keeps the jump bounded to the
        // current level, like jump_selection.
        let matches: Vec<usize> = self
            .filtered_entries
            .iter()
            .enumerate()
            .filter(|(_, &ei)| {
                let e = &self.entries[ei];
                e.depth == depth
                    && e.path.parent().map(|p| p.to_path_buf()) == parent
                    && e.display_name.to_lowercase().starts_with(target)
            })
            .map(|(i, _)| i)
            .collect();

        if matches.is_empty() {
            return;
        }

        // First match after the current position, wrapping to the beginning.
        let new_selected = match matches.iter().position(|&i| i == self.selected) {
            Some(p) => {
                let after = &matches[p + 1..];
                let before = &matches[..p];
                after
                    .first()
                    .or(before.first())
                    .copied()
                    .unwrap_or(self.selected)
            }
            None => matches[0],
        };

        if new_selected != self.selected {
            self.selected = new_selected;
            self.clear_preview();
        }
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

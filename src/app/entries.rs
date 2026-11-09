use super::App;
use crate::models::{DecryptedContent, PassEntry};
use crate::pass::PassCommand;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::Instant;
use walkdir::WalkDir;

impl App {
    pub fn create_folder(&mut self, name: &str) -> Result<()> {
        let normalized = name
            .trim_start_matches('/')
            .trim_end_matches('/')
            .replace("//", "/");

        if normalized.is_empty() {
            return Err(anyhow::anyhow!("Folder name cannot be empty"));
        }

        if normalized.starts_with("..")
            || normalized.contains("/../")
            || normalized.ends_with("/..")
        {
            return Err(anyhow::anyhow!("Invalid path"));
        }

        let folder_path = self.store_path.join(&normalized);

        if !folder_path.starts_with(&self.store_path) {
            return Err(anyhow::anyhow!("Path escapes password store"));
        }

        std::fs::create_dir_all(&folder_path)?;

        // Git cannot track an empty directory: seed the folder with a
        // .gitkeep (invisible to the UI, which skips dotfiles) so it
        // survives a clone.
        let _ = std::fs::write(folder_path.join(".gitkeep"), "");

        if self.store_path.join(".git").exists() {
            // Stage through the shared git_cmd path (git -C <store>), then
            // commit: a push sends only commits and the ahead count is
            // derived from commits, so staging alone would leave the pane
            // reporting "0 ahead" while the folder sits uncommitted.
            let _ = self.git_cmd(&["add", &normalized]);
            let marker = format!("{}/.gitkeep", normalized);
            let pending = self
                .git_cmd(&["status", "--porcelain", "--", &marker])
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if pending {
                // On failure keep the staged state (nothing is reset here)
                // and surface the error like other git failures.
                if let Err(e) = self.commit_staged(&format!("Add folder {} to store.", normalized))
                {
                    self.message = Some((format!("Git commit failed: {}", e), Instant::now()));
                }
            }
        }

        self.message = Some((format!("{} created", normalized), Instant::now()));

        Ok(())
    }

    /// Remove a folder from the store directly instead of via `pass rm`.
    /// pass's rm for a directory commits "Remove $path/ from store." with
    /// a trailing slash - the slash is added by pass itself, so normalising
    /// the argument cannot remove it - which reads inconsistently next to
    /// the folder-add message ("Add folder <name> to store."). Removing
    /// the directory here and committing through the shared git_cmd path
    /// yields "Remove <path> from store.", with nested paths keeping
    /// their internal separators.
    ///
    /// The alt-screen repair wrapper is applied here rather than at the
    /// call site: with_alt_repair's closure cannot capture &mut self, so
    /// the handler cannot wrap this &mut self method in it. No gpg child
    /// runs on this path (unlike pass rm), so the repair is for symmetry
    /// with the other removal flows only.
    pub fn remove_folder(&mut self, rel_path: &str) -> Result<()> {
        let dir = self.store_path.join(rel_path);
        self.with_alt_repair(true, || {
            if dir.is_dir() {
                std::fs::remove_dir_all(&dir)?;
            }
            Ok(())
        })?;

        // Stage the deletion and commit, only in a git-tracked store. The
        // directory is already gone from the working tree, so `git rm -r`
        // stages the tracked deletions beneath it; -f --ignore-unmatch
        // keeps a folder holding modified or never-committed files (e.g.
        // a fresh .gitkeep) from failing the whole removal.
        if self.git_is_initialized() {
            let _ = self.git_cmd(&["rm", "-r", "-f", "--ignore-unmatch", rel_path]);
            // Commit only when the removal actually staged something for
            // this path, like create_folder commits only when its
            // .gitkeep is pending: an empty commit would fail with
            // "nothing to commit".
            let pending = self
                .git_cmd(&["status", "--porcelain", "--", rel_path])
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if pending {
                if let Err(e) = self.commit_staged(&format!("Remove {} from store.", rel_path)) {
                    self.message = Some((format!("Git commit failed: {}", e), Instant::now()));
                }
            }
        }

        Ok(())
    }

    pub fn load_entries(&mut self) -> Result<()> {
        self.entries.clear();

        for entry in WalkDir::new(&self.store_path)
            .sort_by(|a, b| {
                let a_parent = a.path().parent();
                let b_parent = b.path().parent();

                if a_parent == b_parent {
                    let a_is_dir = a.file_type().is_dir();
                    let b_is_dir = b.file_type().is_dir();

                    match (a_is_dir, b_is_dir) {
                        (true, false) => std::cmp::Ordering::Less,
                        (false, true) => std::cmp::Ordering::Greater,
                        _ => a.file_name().cmp(b.file_name()),
                    }
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            let depth = path.strip_prefix(&self.store_path)?.components().count();

            if path == self.store_path {
                continue;
            }

            // Hidden entries are skipped by store-relative COMPONENT, not
            // just by the yielded file name: WalkDir still descends into a
            // hidden directory that was skipped, and its children (whose
            // own names are not dot-prefixed) would otherwise leak into
            // the listing. Checking the stripped path also keeps the store
            // root's own prefix from matching. This covers .git as well as
            // any other dot directory (e.g. a config dir created under a
            // HOME that doubles as the store in tests).
            if path.strip_prefix(&self.store_path)?.components().any(|c| {
                c.as_os_str()
                    .to_str()
                    .map(|s| s.starts_with('.'))
                    .unwrap_or(false)
            }) {
                continue;
            }

            let is_dir = path.is_dir();
            let is_gpg = path.extension().map(|e| e == "gpg").unwrap_or(false);

            if is_dir || is_gpg {
                let entry = PassEntry::new(
                    path.to_path_buf(),
                    &self.store_path,
                    depth.saturating_sub(1),
                );
                self.entries.push(entry);
            }
        }

        Ok(())
    }

    pub fn get_selected_rel_path(&self) -> Option<String> {
        self.filtered_entries.get(self.selected).map(|idx| {
            let entry = &self.entries[*idx];
            let path = entry
                .path
                .strip_prefix(&self.store_path)
                .unwrap_or(&entry.path)
                .to_string_lossy()
                .to_string();
            path.strip_suffix(".gpg").unwrap_or(&path).to_string()
        })
    }

    /// Decrypt the selected entry. `interactive` is true when the user
    /// asked for this decrypt (Enter, reveal, edit load): pass show
    /// invokes gpg, whose pinentry-curses leaves the terminal on the
    /// normal screen, so the TUI repairs the alt screen afterwards (see
    /// with_alt_repair). The preview refresh calls this on every cursor
    /// move with false - a background show must never touch the terminal.
    pub fn decrypt_current_entry(&mut self, interactive: bool) -> Result<()> {
        if let Some(rel_path) = self.get_selected_rel_path() {
            let entry = &self.entries[*self.filtered_entries.get(self.selected).unwrap()];

            if entry.is_dir {
                return Ok(());
            }

            let content = if self.store_path.exists() {
                self.with_alt_repair(interactive, || PassCommand::show(&rel_path))?
            } else {
                "SuperSecretP@ssw0rd123!\nusername: johndoe\nurl: https://example.com/\nemail: john@example.com".to_string()
            };

            self.current_content = Some(DecryptedContent::parse(&content));
            self.current_entry_path = Some(rel_path);
            self.show_password = false;
            self.show_qr = false;
            self.preview_scroll_offset = 0;
            self.preview_h_scroll = 0;
            self.preview_selected_line = None;
        }

        Ok(())
    }

    pub fn clear_preview(&mut self) {
        self.current_content = None;
        self.current_entry_path = None;
        self.show_password = false;
        self.show_qr = false;
        self.preview_scroll_offset = 0;
        self.preview_h_scroll = 0;
        self.preview_selected_line = None;
    }

    fn normalize_gpg(path: &Path) -> String {
        let s = path.to_string_lossy();
        s.strip_suffix(".gpg").unwrap_or(&s).to_string()
    }

    /// Select the entry whose path matches `path` in the current
    /// filtered_entries list, falling back to the first entry when it is
    /// not present (e.g. it was deleted while filtered). Adjusts
    /// scroll_offset so the selection is not above the viewport; the
    /// bottom edge is clamped by the render-time scrolling.
    pub fn select_entry_by_path(&mut self, path: &Path) {
        // Match on entry.path (the full store path), never display_name.
        // 1. Exact equality first.
        let mut matched = self
            .filtered_entries
            .iter()
            .position(|&idx| self.entries[idx].path == path);

        // 2. Normalized comparison: ignores a trailing ".gpg" suffix, which
        //    differs between captured paths and entry paths.
        if matched.is_none() {
            let norm = Self::normalize_gpg(path);
            matched = self
                .filtered_entries
                .iter()
                .position(|&idx| Self::normalize_gpg(&self.entries[idx].path) == norm);
        }

        // 3. Canonicalized comparison: handles symlinked store paths or
        //    differing prefixes (e.g. /home/me vs /home/me/).
        if matched.is_none() {
            if let Ok(canon) = std::fs::canonicalize(path) {
                matched = self.filtered_entries.iter().position(|&idx| {
                    std::fs::canonicalize(&self.entries[idx].path)
                        .map(|c| c == canon)
                        .unwrap_or(false)
                });
            }
        }

        // 4. Last resort: match on the file name alone.
        if matched.is_none() {
            if let Some(name) = path.file_name() {
                matched = self
                    .filtered_entries
                    .iter()
                    .position(|&idx| self.entries[idx].path.file_name() == Some(name));
            }
        }

        self.selected = matched.unwrap_or(0);
        // Keep the restored selection on-screen: never above the viewport
        // top (the bottom edge is clamped by the render-time scrolling).
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        }
    }

    pub fn reload_entries(&mut self) -> Result<()> {
        let expanded_states: std::collections::HashMap<PathBuf, bool> = self
            .entries
            .iter()
            .filter(|e| e.is_dir)
            .map(|e| (e.path.clone(), e.expanded))
            .collect();

        self.load_entries()?;

        for entry in &mut self.entries {
            if entry.is_dir {
                if let Some(&expanded) = expanded_states.get(&entry.path) {
                    entry.expanded = expanded;
                }
            }
        }

        self.rebuild_filtered();
        Ok(())
    }
}

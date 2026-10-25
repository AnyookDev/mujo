use crate::config::Config;
use crate::git;
use crate::models::{
    AppMode, ClipboardTool, ContextMenu, DecryptedContent, EntryEditor, GitPaneState, MenuAction,
    MenuItem, NewStoreStage, PassEntry, SyncStatus,
};
use crate::pass::{store_path, PassCommand};
use crate::password::generate_password;
use crate::search;
use anyhow::Result;
use crossterm::{
    execute,
    style::Print,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io::{stdout, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};
use walkdir::WalkDir;

pub struct App {
    pub store_path: PathBuf,
    pub entries: Vec<PassEntry>,
    pub filtered_entries: Vec<usize>,
    pub selected: usize,
    pub scroll_offset: usize,
    pub mode: AppMode,
    pub command_buffer: String,
    /// Cursor position (in characters) within command_buffer for prompts
    /// that support in-place text editing. Currently used only by the
    /// New Store GPG ID field; other prompt modes still edit at the end.
    pub command_cursor: usize,
    pub search_query: String,
    pub show_password: bool,
    pub show_qr: bool,
    pub current_content: Option<DecryptedContent>,
    pub current_entry_path: Option<String>,
    pub message: Option<(String, Instant)>,
    pub should_quit: bool,
    pub editor: Option<EntryEditor>,
    pub git_output: String,
    pub git_pane: GitPaneState,
    pub git_branch: String,
    pub git_ahead: u32,
    pub git_behind: u32,
    /// Staged, modified and untracked entries together, from the parsed
    /// status. Ahead/behind are commit counts and can both read zero while
    /// the tree is dirty, so the Git pane renders this alongside them.
    pub git_uncommitted: usize,
    pub git_has_upstream: bool,
    /// True when the store has at least one git remote, set by the same
    /// refresh_git_info pass that fills ahead/behind and has_upstream.
    /// Gates the Git pane's Status row (there is no remote name to print
    /// without one) and tracking_label's "no remote" wording.
    pub git_has_remote: bool,
    /// The remote push/pull/status resolve to: the tracked upstream's
    /// remote, else the sole configured remote, else "origin". Refreshed
    /// by refresh_git_info and consumed by the Git pane labels.
    pub git_remote_name: String,
    pub git_status_short: String,
    /// Set when the terminal was handed to a child that may prompt on
    /// /dev/tty (git push/pull/fetch, and the pass commands that invoke
    /// gpg's pinentry-curses): the main event loop must call
    /// terminal.clear() before the next draw, because EnterAlternateScreen
    /// clears the screen on entry and ratatui's diff would otherwise
    /// repaint nothing.
    pub needs_clear: bool,
    /// True while the TUI owns the terminal (alternate screen entered, raw
    /// mode on). Gates the tty handover (git_cmd via
    /// with_terminal_handover) and the alt-screen repair for the
    /// gpg-invoking pass commands (via with_alt_repair): under `cargo
    /// test` no TUI is running, so the escape sequences would bypass
    /// libtest's capture and garble the terminal.
    pub tui_active: bool,
    pub git_log: String,
    pub git_scroll: usize,
    pub git_h_scroll: usize,
    /// Scroll offset of the full-screen help view (AppMode::Help). Kept
    /// separate from preview_scroll_offset so closing help leaves the
    /// preview's scroll position intact.
    pub help_scroll_offset: usize,
    /// Mode to restore when the help view closes. Recorded by
    /// handlers::open_help, so help can be opened from (and return to)
    /// any mode - from the Editor that lands back in the editor with the
    /// cursor, per-field scroll and unsaved buffer intact.
    pub help_return_mode: AppMode,
    pub entry_to_delete: Option<usize>,
    /// True while a pull that would merge is awaiting the y/n confirmation
    /// (AppMode::ConfirmDelete, the same prompt the delete flow uses). Set
    /// by pull_with_confirm (the git pane's and Normal mode's pull keys),
    /// consumed by handle_confirm_delete.
    pub pending_pull: bool,
    /// Mode to restore when a pull confirmation is answered: where the
    /// pull was asked from (the git pane or Normal mode), recorded by
    /// pull_with_confirm and consumed by handle_confirm_delete so the
    /// user lands back where they pressed p.
    pub pull_return_mode: AppMode,
    pub last_reload: Instant,
    pub security_clear_at: Option<Instant>,
    pub clipboard_tool: Option<ClipboardTool>,
    pub last_click: Option<(Instant, (u16, u16), u8)>,
    pub context_menu: Option<ContextMenu>,
    pub move_source: Option<String>,
    pub config: Config,
    pub preview_scroll_offset: usize,
    pub preview_h_scroll: usize,
    pub preview_selected_line: Option<usize>,
    pub git_identity_name: String,
    pub git_pending_count: usize,
    pub last_sync: Option<SyncStatus>,
    /// Fallback Sync-line label for when there is no in-session sync
    /// record: derived from the remote-tracking ref's reflog during
    /// refresh_git_info ("last remote update <relative>"), so a level
    /// branch after a restart does not read "(no sync yet)". None when
    /// there is no upstream (the no-upstream logic applies) or no
    /// commits; an in-session last_sync record always wins.
    pub git_last_remote_update: Option<String>,
    /// Stage of the first-run "New Password Store" setup pane, active when
    /// no store directory exists at startup.
    pub new_store_stage: NewStoreStage,
}

/// The sentence the ConfirmDelete bar shows while a pull that would merge
/// is awaiting confirmation. Single source of truth for app.rs's
/// confirm_prompt() and ui.rs's ConfirmDelete arm, so the two cannot
/// drift apart (a pending pull reuses AppMode::ConfirmDelete).
pub const PULL_CONFIRM_SENTENCE: &str = "Pull would merge - continue? ";

impl App {
    pub fn new() -> Result<Self> {
        let store_path = store_path();

        let config = Config::load().unwrap_or_default();

        let mut app = Self {
            store_path: store_path.clone(),
            entries: Vec::new(),
            filtered_entries: Vec::new(),
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
            last_reload: Instant::now(),
            security_clear_at: None,
            clipboard_tool: None,
            last_click: None,
            context_menu: None,
            move_source: None,
            config,
            preview_scroll_offset: 0,
            preview_h_scroll: 0,
            preview_selected_line: None,
            git_identity_name: String::new(),
            git_pending_count: 0,
            last_sync: None,
            git_last_remote_update: None,
            new_store_stage: NewStoreStage::Confirm,
        };

        if store_path.exists() {
            app.load_entries()?;
        } else {
            // No password store yet: start in the first-run setup pane
            // instead of showing placeholder entries.
            app.mode = AppMode::NewStore;
            app.new_store_stage = NewStoreStage::Confirm;
        }

        app.rebuild_filtered();
        Ok(app)
    }

    /// Re-read config.toml and apply it in place. The UI reads theme colors
    /// from self.config on every frame, so a successful reload takes effect
    /// on the next redraw without any further work.
    pub fn reload_config(&mut self) {
        match Config::load() {
            Ok(config) => {
                self.config = config;
                self.message = Some(("Config reloaded".to_string(), Instant::now()));
            }
            Err(e) => {
                self.message = Some((format!("Config error: {}", e), Instant::now()));
            }
        }
    }

    /// Generate a password with the editor's current Length/Charset settings
    /// and put it into the Password field, replacing any previous value.
    /// The editor stays open, so Ctrl+g can be pressed repeatedly to
    /// re-roll until the result is satisfactory.
    pub fn generate_password_in_editor(&mut self) {
        if let Some(ref mut editor) = self.editor {
            let password = generate_password(
                editor.gen_length,
                &editor.gen_charset,
                &editor.gen_custom_charset,
            );
            editor.set_password(&password);
        }
    }

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

    pub fn copy_ctrl_c(&mut self) {
        let to_copy: Option<(String, bool)> = if let Some(content) = &self.current_content {
            if let Some(i) = self.preview_selected_line {
                content.lines.get(i).cloned().map(|l| (l, i == 0))
            } else {
                content.lines.first().cloned().map(|l| (l, true))
            }
        } else {
            None
        };

        if let Some((text, is_password)) = to_copy {
            self.copy_to_clipboard(&text);
            // Keep the "(Ns)" hint in sync with the configured timeout; when
            // auto-clear is disabled no countdown is shown.
            let msg = match (is_password, self.clipboard_timeout()) {
                (true, Some(d)) => format!("Password copied to clipboard ({}s)", d.as_secs()),
                (false, Some(d)) => format!("Line copied to clipboard ({}s)", d.as_secs()),
                (true, None) => "Password copied to clipboard".to_string(),
                (false, None) => "Line copied to clipboard".to_string(),
            };
            self.message = Some((msg, Instant::now()));
        }
    }

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

    /// Strip a trailing ".gpg" suffix from a path for comparison purposes:
    /// captured selection paths may carry the on-disk ".gpg" extension
    /// while entry paths do not (or vice versa).
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

    pub fn copy_to_clipboard(&mut self, text: &str) {
        // Backend selection is by OS, not by spawn failure: on macOS
        // pbcopy is the native backend; on Linux Wayland's wl-copy is
        // preferred, with xclip as the X11 fallback.
        if cfg!(target_os = "macos") {
            if Command::new("pbcopy")
                .stdin(Stdio::piped())
                .spawn()
                .map(|mut child| {
                    if let Some(stdin) = child.stdin.as_mut() {
                        use std::io::Write;
                        let _ = stdin.write_all(text.as_bytes());
                    }
                    child.wait()
                })
                .is_ok()
            {
                self.clipboard_tool = Some(ClipboardTool::Pbcopy);
                self.message = Some(("Copied to clipboard!".to_string(), Instant::now()));
                self.security_clear_at = self.clipboard_timeout().map(|d| Instant::now() + d);
                return;
            }
        } else {
            if Command::new("wl-copy").arg(text).spawn().is_ok() {
                self.clipboard_tool = Some(ClipboardTool::WlCopy);
                self.message = Some(("Copied to clipboard!".to_string(), Instant::now()));
                self.security_clear_at = self.clipboard_timeout().map(|d| Instant::now() + d);
                return;
            }

            if Command::new("xclip")
                .args(["-selection", "clipboard"])
                .stdin(Stdio::piped())
                .spawn()
                .map(|mut child| {
                    if let Some(stdin) = child.stdin.as_mut() {
                        use std::io::Write;
                        let _ = stdin.write_all(text.as_bytes());
                    }
                    child.wait()
                })
                .is_ok()
            {
                self.clipboard_tool = Some(ClipboardTool::Xclip);
                self.message = Some(("Copied to clipboard!".to_string(), Instant::now()));
                self.security_clear_at = self.clipboard_timeout().map(|d| Instant::now() + d);
                return;
            }
        }

        self.message = Some(("Failed to copy to clipboard".to_string(), Instant::now()));
    }

    pub fn paste_from_clipboard() -> Option<String> {
        // Mirror copy_to_clipboard's OS-based backend choice: pbpaste on
        // macOS, wl-paste with an xclip fallback on Linux.
        if cfg!(target_os = "macos") {
            if let Ok(output) = std::process::Command::new("pbpaste").output() {
                if output.status.success() {
                    return Some(String::from_utf8_lossy(&output.stdout).to_string());
                }
            }
            return None;
        }

        if let Ok(output) = std::process::Command::new("wl-paste")
            .arg("--no-newline")
            .output()
        {
            if output.status.success() {
                return Some(String::from_utf8_lossy(&output.stdout).to_string());
            }
        }

        if let Ok(output) = std::process::Command::new("xclip")
            .args(["-selection", "clipboard", "-o"])
            .output()
        {
            if output.status.success() {
                return Some(String::from_utf8_lossy(&output.stdout).to_string());
            }
        }

        None
    }

    /// The configured clipboard auto-clear delay. `None` when auto-clear
    /// is disabled (timeout_seconds = 0).
    pub fn clipboard_timeout(&self) -> Option<Duration> {
        if self.config.clipboard.timeout_seconds == 0 {
            None
        } else {
            Some(Duration::from_secs(self.config.clipboard.timeout_seconds))
        }
    }

    pub fn clear_clipboard(&mut self) {
        match self.clipboard_tool {
            Some(ClipboardTool::WlCopy) => {
                let _ = Command::new("wl-copy").arg("--clear").spawn();
            }
            Some(ClipboardTool::Pbcopy) => {
                let _ = Command::new("sh").arg("-c").arg("echo -n | pbcopy").spawn();
            }
            Some(ClipboardTool::Xclip) => {
                let _ = Command::new("sh")
                    .arg("-c")
                    .arg("echo -n | xclip -selection clipboard")
                    .spawn();
                let _ = Command::new("sh")
                    .arg("-c")
                    .arg("echo -n | xclip -selection primary")
                    .spawn();
            }
            None => {}
        }

        self.clipboard_tool = None;
        self.security_clear_at = None;
    }

    pub fn wipe_all_sensitive_data(&mut self) {
        self.clear_clipboard();
        self.clear_preview();
    }

    pub fn update(&mut self) {
        let now = Instant::now();

        if now.duration_since(self.last_reload) > Duration::from_secs(5) {
            let _ = self.reload_entries();
            self.last_reload = now;
        }

        if let Some((_, time)) = &self.message {
            if now.duration_since(*time) > Duration::from_secs(3) {
                self.message = None;
            }
        }

        if let Some(clear_at) = self.security_clear_at {
            if now >= clear_at {
                self.clear_clipboard();
                self.message = Some(("Clipboard cleared".to_string(), Instant::now()));
            }
        }
    }

    pub fn open_editor(&mut self, edit_existing: bool) {
        let base_path = self.get_selected_rel_path().unwrap_or_default();

        if edit_existing {
            if self.current_content.is_none() {
                let _ = self.decrypt_current_entry(true);
            }

            if let Some(content) = &self.current_content {
                let content_str = content.lines.join("\n");
                let mut editor = EntryEditor::from_decrypted(&base_path, &content_str);
                editor.gen_length = self.config.password_generation.default_length;
                editor.gen_charset = self.config.password_generation.charset.clone();
                editor.gen_custom_charset = self.config.password_generation.custom_charset.clone();
                self.editor = Some(editor);
                self.mode = AppMode::Editor;
            }
        } else {
            // A new entry has nothing to preview: clear the right pane so
            // it does not keep showing the previously selected (unrelated)
            // entry while the user composes the new one. Editing an
            // existing entry keeps its preview.
            self.clear_preview();

            let parent = if let Some(rel_path) = self.get_selected_rel_path() {
                let entry = &self.entries[*self.filtered_entries.get(self.selected).unwrap()];
                if entry.is_dir {
                    rel_path + "/"
                } else {
                    std::path::Path::new(&rel_path)
                        .parent()
                        .map(|p| p.to_string_lossy().to_string() + "/")
                        .unwrap_or_default()
                }
            } else {
                String::new()
            };

            let mut editor = EntryEditor::new(&parent);
            editor.gen_length = self.config.password_generation.default_length;
            editor.gen_charset = self.config.password_generation.charset.clone();
            editor.gen_custom_charset = self.config.password_generation.custom_charset.clone();
            self.editor = Some(editor);
            self.mode = AppMode::Editor;
        }
    }

    pub fn has_unsaved_changes(&self) -> bool {
        if let Some(ref editor) = self.editor {
            // Compare against the editor's INITIAL state, snapshotted at
            // creation: EntryEditor::new() sets original_path to the
            // prefilled parent path and original_content to the initial
            // "\n", while from_decrypted() snapshots the on-disk path and
            // content. An untouched editor is therefore never dirty, for
            // both new and existing entries; typing a name or any content
            // makes it dirty.
            let path_changed = editor.path != editor.original_path;
            let content_changed = editor.to_content() != editor.original_content;
            path_changed || content_changed
        } else {
            false
        }
    }

    /// Close the editor, discarding any unsaved changes. The preview pane
    /// keeps showing the original on-disk content when it was decrypted for
    /// the entry that was being edited - editor edits never touch
    /// `current_content`, so it already holds the original - and is cleared
    /// otherwise (e.g. a discarded new entry).
    pub fn discard_editor_changes(&mut self) {
        let edited_existing = self.editor.as_ref().map(|e| !e.is_new).unwrap_or(false);
        let edited_path = self.editor.as_ref().map(|e| e.original_path.clone());
        self.editor = None;

        if edited_existing && edited_path == self.current_entry_path {
            return;
        }
        self.clear_preview();
    }

    pub fn save_entry(&mut self, force: bool) -> Result<bool> {
        // Clone the editor data needed below into owned values up front:
        // holding `ref editor` (a borrow of self.editor) across the &mut
        // self calls (reload_entries, rebuild_filtered, ...) would be a
        // borrow-check error (E0502).
        let Some(editor) = self.editor.as_ref() else {
            return Ok(false);
        };
        let path = editor.path.clone();
        let is_new = editor.is_new;
        let content = editor.to_content();
        // `editor` is a &EntryEditor borrow; it ends here on its own, so
        // the &mut self calls below are fine. (An explicit drop() on a
        // reference is a no-op and triggers the dropping_references lint.)

        if path.is_empty() || path.ends_with('/') {
            self.message = Some((
                "Error: Enter a name for the new entry".to_string(),
                Instant::now(),
            ));
            return Ok(false);
        }

        let full_path = self.store_path.join(format!("{}.gpg", path));
        if full_path.exists() && is_new {
            self.message = Some((format!("Error: {} already exists", path), Instant::now()));
            return Ok(false);
        }

        if let Some(parent) = std::path::Path::new(&path).parent() {
            let parent_path = self.store_path.join(parent);
            if !parent_path.exists() {
                self.message = Some((
                    format!("Error: Folder '{}' does not exist", parent.display()),
                    Instant::now(),
                ));
                return Ok(false);
            }
        }

        if !force && self.has_unsaved_changes() {
            self.mode = AppMode::ConfirmSave;
            return Ok(false);
        }

        // pass insert invokes gpg (pinentry-curses): repair the alt
        // screen afterwards, like the other gpg paths.
        if let Err(e) = self.with_alt_repair(true, || PassCommand::insert(&path, &content)) {
            self.message = Some((format!("Error saving: {}", e), Instant::now()));
            return Ok(false);
        }

        self.current_content = Some(DecryptedContent::parse(&content));
        self.current_entry_path = Some(path.clone());
        self.message = Some((format!("Saved {}", path), Instant::now()));
        self.reload_entries()?;

        // Move the tree selection to the saved entry so the tree and
        // the preview agree. rebuild_filtered() omits children of
        // collapsed folders, so expand every ancestor directory of the
        // new entry (from its parent up to the store root) first -
        // same pattern as clear_search_filter.
        let saved_path = self.store_path.join(format!("{}.gpg", path));
        let mut ancestor = saved_path.parent();
        while let Some(dir) = ancestor {
            if !dir.starts_with(&self.store_path) || dir == self.store_path {
                break;
            }
            if let Some(entry) = self.entries.iter_mut().find(|e| e.is_dir && e.path == dir) {
                entry.expanded = true;
            }
            ancestor = dir.parent();
        }
        self.rebuild_filtered();
        self.select_entry_by_path(&saved_path);

        Ok(true)
    }

    /// Generate a password entry at "<selected>/generated" with `pass
    /// generate` and copy it to the clipboard. Currently uncalled: the
    /// unadvertised Shift+C binding that invoked it was removed in favour
    /// of the editor's built-in generator (Ctrl+g), but the method is
    /// retained public for future use.
    pub fn generate_password_entry(&mut self, length: usize) -> Result<()> {
        let base_path = self
            .get_selected_rel_path()
            .ok_or_else(|| anyhow::anyhow!("No selection"))?;

        let new_path = format!("{}/generated", base_path);
        let password =
            self.with_alt_repair(true, || PassCommand::generate(&new_path, length, false))?;

        self.copy_to_clipboard(&password);
        self.message = Some((format!("Generated: {}", new_path), Instant::now()));
        self.reload_entries()?;

        Ok(())
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

    /// Subcommands that may prompt on /dev/tty (ssh asking for a password):
    /// the terminal is handed to the child for these only. The prompt is
    /// written to the tty - the same tty mujo draws on - so while mujo sits
    /// in raw mode on the alternate screen the prompt would land on top of
    /// the UI and never be cleaned up.
    fn prompts_on_tty(args: &[&str]) -> bool {
        matches!(args.first(), Some(&"push") | Some(&"pull") | Some(&"fetch"))
    }

    /// Hand the terminal over to a child that may prompt on /dev/tty (git
    /// push/pull/fetch asking ssh for a password, or the pass commands
    /// that invoke gpg and therefore pinentry-curses, which switches the
    /// terminal to the normal screen on exit while mujo keeps drawing on
    /// the alternate screen) and run `f`, restoring the TUI afterwards.
    /// While handed over: raw mode off, alternate screen left, so the
    /// child's prompt is readable on the normal screen; afterwards the
    /// alternate screen is re-entered and needs_clear tells the event
    /// loop to clear before the next draw, since EnterAlternateScreen
    /// clears on entry and ratatui's diff would otherwise consider every
    /// cell unchanged. The handover only runs when a TUI actually owns
    /// the screen (tui_active): under `cargo test` no TUI is running, and
    /// the escape sequences would go straight to fd 1, bypassing
    /// libtest's capture and garbling the terminal. The unit-test build
    /// still records the handover decision (needs_clear) so the gating
    /// stays observable.
    pub(crate) fn with_terminal_handover<T>(
        &mut self,
        interactive: bool,
        context: Option<String>,
        f: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let on_tty = interactive && self.tui_active && !cfg!(test);
        if on_tty {
            let _ = disable_raw_mode();
            let _ = execute!(stdout(), LeaveAlternateScreen);
            let _ = stdout().flush();
            // One context line so the silent case (e.g. a passwordless
            // YubiKey key that only needs a touch) still shows that mujo
            // handed the terminal to the child and is not frozen. Written
            // to stderr, where the child's teed output follows.
            if let Some(line) = context {
                eprintln!("{}", line);
            }
        }
        let result = f();
        if interactive && self.tui_active {
            if on_tty {
                let _ = execute!(stdout(), EnterAlternateScreen);
                let _ = enable_raw_mode();
                let _ = stdout().flush();
            }
            self.needs_clear = true;
        }
        result
    }

    /// Run a gpg-invoking pass command with the alt screen already up, then
    /// repair afterwards. pinentry-curses emits ?1049l on exit, which drops
    /// the terminal to the normal screen, so re-assert the alt screen with
    /// ?1049h. DECSET 47 (?47h) and 1047 are ignored by some terminals,
    /// making the repair a silent no-op there; 1049 is honoured (it is the
    /// same sequence crossterm's EnterAlternateScreen emits at startup).
    /// 1049h CLEARS the alt screen on entry, which needs_clear already
    /// absorbs via the full repaint before the next draw.
    pub(crate) fn with_alt_repair<T>(
        &mut self,
        interactive: bool,
        f: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let result = f();
        if interactive && self.tui_active {
            if !cfg!(test) {
                let _ = execute!(stdout(), Print("\x1b[?1049h"));
                let _ = stdout().flush();
            }
            self.needs_clear = true;
        }
        result
    }

    /// Run a git subcommand in the store, capturing stdout/stderr. Git is
    /// invoked directly (`git -C <store>`) so its real exit status
    /// propagates - `pass git` exits 0 regardless of what git did, which
    /// made every failure invisible. EXCEPTION: `init` still goes through
    /// `pass git init`, whose cmd_git adds .gitattributes and makes the
    /// initial commit, with a reliable status (`git "$@" || exit 1`).
    /// For push/pull/fetch the terminal is handed over first so an ssh
    /// prompt is readable (see with_terminal_handover).
    fn git_cmd(&mut self, args: &[&str]) -> Result<String> {
        let interactive = Self::prompts_on_tty(args) && self.tui_active;
        let on_tty = interactive && !cfg!(test);
        // One context line so the silent case (e.g. a passwordless
        // YubiKey key that only needs a touch) still shows that mujo
        // handed the terminal to git and is not frozen. The handover
        // helper emits it after leaving the alternate screen, so it lands
        // on the visible terminal where the child's teed output follows.
        let context = if on_tty {
            Some(format!("mujo: git {}", args.first().copied().unwrap_or("")))
        } else {
            None
        };
        // Interactive runs tee the child's stderr to the (now visible)
        // terminal while still capturing it, so prompts like OpenSSH's
        // "Confirm user presence for key ..." are seen live AND end up in
        // git_output. Non-interactive runs keep the plain captured path.
        let store = self.store_path.clone();
        self.with_terminal_handover(interactive, context, || {
            if args.first() == Some(&"init") {
                PassCommand::git(args)
            } else if on_tty {
                PassCommand::git_in_tee(&store, args)
            } else {
                PassCommand::git_in(&store, args)
            }
        })
    }

    /// Like git_cmd but for argument lists that own their strings (built by
    /// the git module's command builders).
    fn git_cmd_owned(&mut self, args: &[String]) -> Result<String> {
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        self.git_cmd(&refs)
    }

    /// The remote URL of the password store for `remote`, resolved from git
    /// itself with config.toml as fallback. Used to label sync messages in
    /// the Git pane.
    fn remote_url(&mut self, remote: &str) -> String {
        self.git_cmd(&["remote", "get-url", remote])
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.config.git.remote_url.clone())
    }

    /// The store's configured git remotes, in `git remote` order, run
    /// through the same non-interactive `git -C <store>` path as the other
    /// plain queries. Empty when the store directory is absent or git
    /// lists nothing: callers treat an empty list as "no remote
    /// configured" rather than falling back to a hardcoded "origin" that
    /// git would then fail to resolve.
    fn remotes(&self) -> Vec<String> {
        if !self.store_path.exists() {
            return Vec::new();
        }
        PassCommand::git_in(&self.store_path, &["remote"])
            .map(|out| {
                out.lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Resolve the remote to push/pull, in order of preference:
    /// (a) the remote of the branch's tracked upstream, parsed from
    ///     `git rev-parse --abbrev-ref --symbolic-full-name @{u}`
    ///     (e.g. "fudge/master" -> "fudge");
    /// (b) the sole configured remote when `git remote` lists exactly one;
    /// (c) a remote literally named "origin" when one exists;
    /// (d) otherwise Err describing the configuration problem: no remote
    ///     at all, or several remotes with no upstream to disambiguate.
    /// Callers surface the message on the Sync line and skip git
    /// entirely - the old fallback to a literal "origin" made git fail
    /// with "fatal: 'origin' does not appear to be a git repository",
    /// which reads like a broken URL rather than "you have no remote".
    fn resolve_remote_name(&mut self) -> std::result::Result<String, String> {
        if let Ok(upstream) =
            self.git_cmd(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        {
            if let Some(remote) = git::remote_from_upstream(upstream.trim()) {
                return Ok(remote);
            }
        }
        let remotes = self.remotes();
        if remotes.len() == 1 {
            return Ok(remotes[0].clone());
        }
        if remotes.iter().any(|r| r == "origin") {
            return Ok("origin".to_string());
        }
        if remotes.is_empty() {
            return Err(
                "no remote configured - git remote add <name> <url> (or set git.remote_url in config.toml)"
                    .to_string(),
            );
        }
        Err(
            "multiple remotes and no upstream - set one with git branch -u <remote>/<branch>"
                .to_string(),
        )
    }

    /// True when the branch's tracked upstream resolves to a remote. The
    /// push path derives --set-upstream from this: resolve_remote_name's
    /// Ok cases are exactly "upstream tracked" (flag not needed) and "a
    /// remote chosen without an upstream" (flag needed, so the first push
    /// establishes tracking).
    fn has_tracked_upstream(&mut self) -> bool {
        self.git_cmd(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
            .ok()
            .and_then(|upstream| git::remote_from_upstream(upstream.trim()))
            .is_some()
    }

    /// Record a push/pull that never reached git: resolve_remote_name
    /// could not pick a remote. No git command runs and no terminal
    /// handover happens - the failure is a configuration problem, and
    /// running git would only report a misleading "'origin' does not
    /// appear to be a git repository". The Sync line renders the fix-it
    /// message ("Push failed: no remote configured - ...") through the
    /// usual failed-sync path.
    fn sync_blocked(&mut self, action: &str, msg: String) {
        self.last_sync = Some(SyncStatus {
            action: action.to_string(),
            // Blocked before any remote could be resolved: none to name.
            remote: String::new(),
            url: String::new(),
            at: Instant::now(),
            success: false,
            detail: Self::short_sync_reason(&msg),
        });
        self.message = Some((
            format!("Git {} failed: {}", action.to_lowercase(), msg),
            Instant::now(),
        ));
    }

    /// The short Sync-line reason for a blocked sync. The full message
    /// ("no remote configured - git remote add ...") is too wide for the
    /// half-pane Sync line and truncates, so the line carries just the
    /// problem's name; the full text stays on the message bar and the
    /// Status hint line.
    fn short_sync_reason(msg: &str) -> String {
        if msg.contains("multiple remotes") {
            "multiple remotes".to_string()
        } else if msg.contains("no remote") {
            "no remote".to_string()
        } else {
            msg.to_string()
        }
    }

    /// The fallback Sync line for a successful sync whose git output was
    /// empty: "<verb> <remote> (<url>)". The resolved remote name is
    /// included so a local-path URL, or a store with several remotes,
    /// still says which remote was used.
    fn sync_fallback_line(verb: &str, remote: &str, url: &str) -> String {
        format!("{} {} ({})", verb, remote, url)
    }

    /// The Sync line's fallback label from a remote-tracking ref's reflog
    /// timestamp (%ct, unix seconds): "last remote update <relative>",
    /// through the same relative-time formatter as the in-session sync
    /// age. "(unknown)" when the reflog gave no parsable timestamp (e.g.
    /// the ref does not exist yet). The wording is deliberately neutral:
    /// a fetch updates the ref too, so the label must never imply push.
    fn format_remote_update_label(ts: Option<u64>, now: SystemTime) -> String {
        let age = ts
            .and_then(|t| std::time::UNIX_EPOCH.checked_add(Duration::from_secs(t)))
            .and_then(|t| now.duration_since(t).ok());
        match age {
            Some(age) => format!("last remote update {}", crate::ui::relative_time(age)),
            None => "(unknown)".to_string(),
        }
    }

    /// Read the remote-tracking ref's last update time via its reflog and
    /// format the Sync line's fallback label. Called from
    /// refresh_git_info, so the git round trip happens per refresh, never
    /// per frame.
    fn last_remote_update_label(&mut self, remote: &str, branch: &str) -> String {
        let tracking = format!("{}/{}", remote, branch);
        let ts = self
            .git_cmd(&["reflog", "show", "-1", "--format=%ct", &tracking])
            .ok()
            .and_then(|out| out.trim().parse::<u64>().ok());
        Self::format_remote_update_label(ts, SystemTime::now())
    }

    /// The current branch name, for explicit `push <remote> <branch>` /
    /// `pull <remote> <branch>`. None on a detached or unborn HEAD, where
    /// no branch ref resolves.
    fn current_branch(&mut self) -> Option<String> {
        self.git_cmd(&["rev-parse", "--abbrev-ref", "HEAD"])
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s != "HEAD")
    }

    pub fn git_is_initialized(&self) -> bool {
        self.store_path.join(".git").exists()
    }

    pub fn open_git_pane(&mut self) {
        self.git_scroll = 0;
        self.git_h_scroll = 0;
        self.command_buffer.clear();
        // last_sync is intentionally NOT reset here: the last sync outcome
        // should stay visible until the next push/pull (or app restart),
        // so the user sees "Pushed to <url> 5 min ago" when they return.
        if self.git_is_initialized() {
            self.git_pane = GitPaneState::Ready;
            self.refresh_git_info();
        } else {
            self.git_pane = GitPaneState::Uninitialized;
        }
        self.mode = AppMode::Git;
    }

    fn apply_git_config_remote(&mut self) {
        // Operate on the resolved remote (upstream's remote, sole remote,
        // or "origin") rather than a hardcoded "origin". With no usable
        // remote resolve_remote_name errs; the (Some(cfg), None) arm below
        // then auto-configures "origin" from config.toml via
        // git_add_remote, so probe that name - but only when no remotes
        // exist at all (e.g. .git was recreated). With remotes present but
        // none named "origin", the resolver's ambiguity error stands
        // instead of being silently resolved by conjuring a third remote.
        let remote = match self.resolve_remote_name() {
            Ok(remote) => remote,
            Err(_) => "origin".to_string(),
        };
        let git_url = self
            .git_cmd(&["remote", "get-url", &remote])
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let config_url = self.config.git.remote_opt().map(|s| s.to_string());

        match (config_url, git_url) {
            // Both set: if config.toml was edited externally with a different
            // URL, treat config.toml as the source of truth and update git.
            (Some(cfg), Some(git)) => {
                if cfg != git && self.git_cmd(&["remote", "set-url", &remote, &cfg]).is_ok() {
                    self.git_output = format!("Remote {} updated to {}", remote, cfg);
                }
            }
            // Only in config.toml (e.g. .git was recreated): auto-configure.
            // git_add_remote also persists the URL back to config.toml.
            // Only conjure a remote when none exists: with remotes present
            // but none named "origin", the resolver's ambiguity error should
            // stand rather than be silently resolved by adding a third
            // remote.
            (Some(cfg), None) => {
                if self.remotes().is_empty() {
                    let _ = self.git_add_remote(&cfg);
                }
            }
            // Only in git: persist it to config.toml so it survives a
            // recreated .git directory.
            (None, Some(git)) => {
                self.config.git.remote_url = git;
                let _ = self.config.save();
            }
            (None, None) => {}
        }
    }

    pub fn refresh_git_info(&mut self) {
        if !self.git_is_initialized() {
            self.git_pane = GitPaneState::Uninitialized;
            return;
        }

        self.apply_git_config_remote();

        // An empty repo (no commits yet) has no HEAD: commands that resolve
        // it fail with raw git stderr like "ambiguous argument 'HEAD'".
        // Detect that case up front and handle it gracefully instead.
        let has_commits = self
            .git_cmd(&["rev-parse", "--verify", "--quiet", "HEAD"])
            .is_ok();

        self.git_branch = if has_commits {
            self.git_cmd(&["rev-parse", "--abbrev-ref", "HEAD"])
                .unwrap_or_else(|_| "(unknown)".to_string())
                .trim()
                .to_string()
        } else {
            // symbolic-ref works on an unborn branch (e.g. "main").
            self.git_cmd(&["symbolic-ref", "--short", "HEAD"])
                .map(|s| format!("{} (no commits yet)", s.trim()))
                .unwrap_or_else(|_| "(no commits yet)".to_string())
        };

        // Structured status: branch line gives upstream/ahead/behind, the
        // remaining lines list dirty files. The first line doubles as the
        // short status display (same format as `git status -sb`).
        let status_out = self
            .git_cmd(&["--no-pager", "status", "--porcelain", "-b"])
            .unwrap_or_default();
        self.git_status_short = status_out.lines().next().unwrap_or("").to_string();
        let status = git::parse_status(&status_out);
        self.git_has_upstream = status.upstream.is_some();
        self.git_ahead = status.ahead;
        self.git_behind = status.behind;
        self.git_uncommitted = status.uncommitted;
        // Whether any remote exists at all, from the same refresh that
        // sets ahead/behind and has_upstream: the Status row omits the
        // remote name and tracking reads "no remote" when this is false.
        self.git_has_remote = !self.remotes().is_empty();
        // Keep the pane's remote label in sync with what push/pull will
        // actually use - never a hardcoded "origin". Empty when no remote
        // resolves (the row omits the prefix in that case).
        self.git_remote_name = self.resolve_remote_name().unwrap_or_default();

        // Derive the Sync line's fallback label from the remote-tracking
        // ref's reflog, piggybacking on this refresh (never per frame).
        // Only with an upstream and commits: otherwise the no-upstream
        // logic applies and there is no ref to read a reflog from.
        self.git_last_remote_update = if has_commits && self.git_has_upstream {
            // Clone the field strings before the &mut self call: passing
            // &self.git_remote_name into last_remote_update_label would
            // hold an immutable borrow of self across its mutable one
            // (E0502).
            let remote = self.git_remote_name.clone();
            let branch = self.git_branch.clone();
            Some(self.last_remote_update_label(&remote, &branch))
        } else {
            None
        };

        self.git_log = if has_commits {
            self.git_cmd(&[
                "--no-pager",
                "log",
                "-n",
                "50",
                "--pretty=format:%h %ad %s",
                "--date=format:%Y-%m-%d %H:%M",
            ])
            .unwrap_or_else(|_| "(no commits yet)".to_string())
        } else {
            "No commits yet".to_string()
        };
    }

    /// True only if BOTH user.name and user.email resolve to non-empty
    /// values (git accepts empty-string config values, so a successful
    /// exit is not enough - the value must be non-empty too).
    fn git_identity_configured(&mut self) -> bool {
        let name = self
            .git_cmd(&["config", "user.name"])
            .ok()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let email = self
            .git_cmd(&["config", "user.email"])
            .ok()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        !name.is_empty() && !email.is_empty()
    }

    /// Run the commit and VERIFY it landed. Returns the actual git error
    /// on failure so the UI never reports success for a commit that did
    /// not happen.
    fn commit_staged(&mut self, message: &str) -> Result<()> {
        self.git_cmd_owned(&git::commit_args(message))?;
        // If HEAD still does not resolve, the commit did not land - do not
        // report success.
        self.git_cmd(&["rev-parse", "--verify", "HEAD"])?;
        Ok(())
    }

    /// Seed every empty directory under the store with an empty .gitkeep:
    /// git cannot track empty directories, so without this a folder created
    /// in the UI would be lost by a clone. Both pass and mujo skip dotfiles
    /// when listing, so the marker stays invisible. .git itself is excluded.
    fn add_gitkeeps_to_empty_dirs(&self) {
        for entry in WalkDir::new(&self.store_path)
            .into_iter()
            .filter_entry(|e| e.file_name() != ".git")
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_dir()
                && std::fs::read_dir(entry.path())
                    .map(|mut rd| rd.next().is_none())
                    .unwrap_or(false)
            {
                let _ = std::fs::write(entry.path().join(".gitkeep"), "");
            }
        }
    }

    pub fn git_init(&mut self) -> Result<()> {
        let out = self.git_cmd(&["init"])?;
        self.git_output = out;
        self.command_buffer.clear();

        // Fix existing empty folders before counting: each seeded .gitkeep
        // shows up as a pending file and is captured by the initial commit.
        self.add_gitkeeps_to_empty_dirs();

        // Count pending password files. `status --porcelain` covers both
        // untracked files and anything already staged by an earlier failed
        // init attempt.
        let pending = self
            .git_cmd(&["status", "--porcelain"])?
            .lines()
            .filter(|l| l.contains(".gpg"))
            .count();
        self.git_pending_count = pending;

        if pending > 0 {
            self.git_cmd(&["add", "-A"])?;

            // git refuses to commit without user.name/user.email. Instead of
            // failing (or silently inventing an identity), ask the user.
            if !self.git_identity_configured() {
                self.git_identity_name.clear();
                self.git_pane = GitPaneState::AskIdentityName;
                self.message = Some((
                    "Git requires user.name and user.email to commit".to_string(),
                    Instant::now(),
                ));
                return Ok(());
            }

            self.finish_initial_commit();
            return Ok(());
        }

        self.message = Some(("Git initialized".to_string(), Instant::now()));
        self.git_pane = GitPaneState::AskRemote;
        Ok(())
    }

    /// Commit the staged files and report the real outcome. Called both
    /// from git_init (identity already configured) and after the user
    /// enters a name/email in the identity prompts.
    fn finish_initial_commit(&mut self) {
        let pending = self.git_pending_count;
        match self.commit_staged("Initial commit: add existing passwords") {
            Ok(()) => {
                // Only claim success now that HEAD verifiably exists.
                self.git_output = format!("Initial commit: {} passwords", pending);
                self.message = Some((
                    format!(
                        "Git initialized and committed {} existing passwords",
                        pending
                    ),
                    Instant::now(),
                ));
            }
            Err(e) => {
                // Surface the actual git error instead of a generic success.
                self.message = Some((
                    format!("Git initialized, but the initial commit failed: {}", e),
                    Instant::now(),
                ));
            }
        }
        self.git_pending_count = 0;
        self.git_pane = GitPaneState::AskRemote;
        self.command_buffer.clear();
    }

    /// Called after the user enters their email in the identity prompt.
    /// Sets the identity store-locally and retries the initial commit.
    pub fn finish_git_identity(&mut self, email: String) {
        let name = self.git_identity_name.trim().to_string();
        let _ = self.git_cmd(&["config", "user.name", &name]);
        let _ = self.git_cmd(&["config", "user.email", email.trim()]);

        if self.git_pending_count > 0 {
            self.finish_initial_commit();
        } else {
            self.message = Some(("Git identity configured".to_string(), Instant::now()));
            self.git_pane = GitPaneState::AskRemote;
            self.command_buffer.clear();
        }
    }

    /// User declined to configure an identity: do NOT fake success. Tell
    /// them exactly what is staged and how to finish manually.
    pub fn skip_git_identity(&mut self) {
        self.message = Some((
            if self.git_pending_count > 0 {
                format!(
                    "{} file(s) staged but NOT committed. Configure user.name/user.email, then run 'pass git commit'",
                    self.git_pending_count
                )
            } else {
                "Git identity setup skipped".to_string()
            },
            Instant::now(),
        ));
        self.git_pending_count = 0;
        self.git_pane = GitPaneState::AskRemote;
        self.command_buffer.clear();
    }

    pub fn git_add_remote(&mut self, url: &str) -> Result<()> {
        let out = self.git_cmd(&["remote", "add", "origin", url])?;
        self.git_output = out;

        // Persist the remote URL to mujo's config.toml as well, so it
        // stays in sync with git's config and survives a recreated .git.
        self.config.git.remote_url = url.trim().to_string();
        if let Err(e) = self.config.save() {
            self.message = Some((
                format!("Warning: could not save config: {}", e),
                Instant::now(),
            ));
        }
        Ok(())
    }

    /// True when git's own output says the sync was a no-op: push prints
    /// "Everything up-to-date" and pull prints "Already up to date." when
    /// the remote already matches. Matched on the up-to-date wording rather
    /// than a whole line, so either phrasing (and any surrounding output)
    /// classifies correctly.
    fn output_is_up_to_date(out: &str) -> bool {
        let lower = out.to_lowercase();
        lower.contains("up-to-date") || lower.contains("up to date")
    }

    /// The SyncStatus action for a successful sync, read from the outcome
    /// instead of hardcoded: "Up to date" when git reported nothing to
    /// transfer, otherwise the caller's past-tense verb ("Pushed to" /
    /// "Pulled from").
    fn sync_success_action(verb: &str, out: &str) -> String {
        if Self::output_is_up_to_date(out) {
            "Up to date".to_string()
        } else {
            verb.to_string()
        }
    }

    /// The SyncStatus action for a successful pull, derived from repository
    /// state rather than git's text: a merge over a local-path remote
    /// prints "Already up to date." and never "Merge made by", so text
    /// matching mislabels it. HEAD unchanged by the pull -> "Already up to
    /// date"; the new HEAD has more than one parent -> "Merged"; otherwise
    /// (single-parent new HEAD) -> "Fast-forwarded".
    fn classify_pull_outcome(
        before: Option<&str>,
        after: Option<&str>,
        parent_count: usize,
    ) -> &'static str {
        if before.is_some() && before == after {
            "Already up to date"
        } else if parent_count > 1 {
            "Merged"
        } else {
            "Fast-forwarded"
        }
    }

    /// The prompt line for AppMode::ConfirmDelete, which the delete flow and
    /// the pull-confirmation flow share (pending_pull says which). Only the
    /// question differs; the y/n handling in handle_confirm_delete is common
    /// to both. The pull sentence comes from PULL_CONFIRM_SENTENCE, the
    /// same const the status bar renders.
    pub fn confirm_prompt(&self) -> String {
        if self.pending_pull {
            format!("{}y:confirm n:cancel", PULL_CONFIRM_SENTENCE)
        } else {
            "Delete this entry? y:confirm n:cancel".to_string()
        }
    }

    /// True when a pull needs explicit confirmation: behind > 0 with either
    /// ahead > 0 (diverged - the pull will merge) or uncommitted changes
    /// (the merge may abort or conflict over a dirty tree). A clean
    /// fast-forward (behind > 0, ahead == 0, clean tree) and an up-to-date
    /// tree are not confirmed - neither has a side effect worth a prompt.
    pub(crate) fn pull_needs_confirm(ahead: u32, behind: u32, uncommitted: usize) -> bool {
        behind > 0 && (ahead > 0 || uncommitted > 0)
    }

    /// git's output when it is non-empty, else the formatted fallback line.
    /// A no-op push connects, exits 0 and may print nothing on stdout, so an
    /// empty output must not blank the pane.
    fn sync_output(out: &str, fallback: String) -> String {
        if out.trim().is_empty() {
            fallback
        } else {
            out.to_string()
        }
    }

    pub fn git_push(&mut self) -> Result<()> {
        let needs_upstream = !self.has_tracked_upstream();
        let remote = match self.resolve_remote_name() {
            Ok(remote) => remote,
            Err(msg) => {
                // No remote to push to: record the configuration problem
                // on the Sync line and return without running git (and
                // without the terminal handover) - git would only fail
                // with a misleading "'origin' does not appear to be a git
                // repository".
                self.sync_blocked("Push", msg);
                return Ok(());
            }
        };
        let branch = self.current_branch();
        let url = self.remote_url(&remote);
        match self.git_cmd_owned(&git::push_args(&remote, branch.as_deref(), needs_upstream)) {
            Ok(out) => {
                let action = Self::sync_success_action("Pushed to", &out);
                self.git_output =
                    Self::sync_output(&out, Self::sync_fallback_line("Pushed to", &remote, &url));
                self.last_sync = Some(SyncStatus {
                    action,
                    remote,
                    url,
                    at: Instant::now(),
                    success: true,
                    detail: String::new(),
                });
                self.message = Some(("Git: pushed".to_string(), Instant::now()));
            }
            Err(e) => {
                self.git_output = e.to_string();
                self.last_sync = Some(SyncStatus {
                    action: "Push".to_string(),
                    remote,
                    url,
                    at: Instant::now(),
                    success: false,
                    detail: e.to_string(),
                });
                self.message = Some(("Git push failed".to_string(), Instant::now()));
                self.refresh_git_info();
                return Err(e);
            }
        }
        self.refresh_git_info();
        Ok(())
    }

    /// The current HEAD commit id, None on an unborn or unresolvable HEAD.
    fn head_rev(&mut self) -> Option<String> {
        self.git_cmd(&["rev-parse", "HEAD"])
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// The number of parents of the current HEAD commit (0 when HEAD is
    /// unborn or unresolvable; a root commit has no parents).
    fn head_parent_count(&mut self) -> usize {
        self.git_cmd(&["show", "-s", "--format=%P", "HEAD"])
            .map(|s| s.split_whitespace().count())
            .unwrap_or(0)
    }

    /// The arguments for a pull: the git module's pull arguments plus
    /// "--no-rebase". git 2.27+ refuses a divergent pull with "fatal: Need
    /// to specify how to reconcile divergent branches" unless the user has
    /// pull.rebase or pull.ff configured; passing the strategy explicitly
    /// keeps mujo's merge-on-divergence behaviour independent of global
    /// config (a fast-forwardable pull still fast-forwards).
    fn pull_args_with_strategy(remote: &str, branch: Option<&str>) -> Vec<String> {
        let mut args = git::pull_args(remote, branch);
        args.push("--no-rebase".to_string());
        args
    }

    /// The arguments for the local merge of a fetched upstream ref.
    /// --no-edit matters: the child owns the terminal during the handover,
    /// so a merge-message editor must not open.
    fn merge_upstream_args(upstream: &str) -> Vec<String> {
        vec![
            "merge".to_string(),
            "--no-edit".to_string(),
            upstream.to_string(),
        ]
    }

    /// Fetch from the resolved remote without merging. Run before the
    /// pull-confirmation check: pull_needs_confirm reads the last-known
    /// ahead/behind counts, which are stale until a fetch, so an unfetched
    /// divergence would otherwise merge without prompting. Goes through the
    /// same git_cmd handover path as push/pull (fetch may prompt for ssh
    /// credentials). A missing remote is reported through the usual
    /// blocked-sync path and returns Ok, like git_push/git_pull.
    pub fn git_fetch(&mut self) -> Result<()> {
        let remote = match self.resolve_remote_name() {
            Ok(remote) => remote,
            Err(msg) => {
                self.sync_blocked("Pull", msg);
                return Ok(());
            }
        };
        match self.git_cmd(&["fetch", &remote]) {
            Ok(out) => {
                let url = self.remote_url(&remote);
                self.git_output = Self::sync_output(
                    &out,
                    Self::sync_fallback_line("Fetched from", &remote, &url),
                );
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// The shared pull flow, used by both the git pane's `p` and Normal
    /// mode's `p`. A pull that would merge (or may abort over a dirty
    /// tree) is the one accidental keypress with a side effect, so it is
    /// confirmed first with the same y/n mechanism the delete flow uses; a
    /// clean fast-forward merges immediately.
    ///
    /// The confirm predicate reads the last-known status, which is stale
    /// until a fetch: fetch first, refresh the git info, THEN evaluate
    /// pull_needs_confirm on the fresh ahead/behind/uncommitted counts. An
    /// unfetched divergence would otherwise merge without prompting. The
    /// fetch IS the network half of the pull, so what follows is the
    /// local-only merge (git_merge_upstream) - calling git_pull here would
    /// fetch a second time, costing a second ssh round trip, password
    /// prompt and terminal handover. The confirmation bar itself is the
    /// prompt, so no extra message is set here.
    ///
    /// Records self.mode in pull_return_mode before raising the
    /// confirmation, so handle_confirm_delete returns the user to where
    /// the pull was asked from (the git pane or Normal mode).
    ///
    /// A blocked fetch (no remote, or no upstream to merge from) returns
    /// Ok and has already reported why on the Sync line, so the merge step
    /// is skipped when either is missing: running `git merge @{u}` anyway
    /// would only overwrite the fix-it message with a raw git error like
    /// "fatal: No upstream configured for branch 'master'".
    pub fn pull_with_confirm(&mut self) {
        self.pull_return_mode = self.mode;
        if let Err(e) = self.git_fetch() {
            self.message = Some((format!("Git fetch failed: {}", e), Instant::now()));
        } else {
            self.refresh_git_info();
            // refresh_git_info just re-derived both flags from the store,
            // so they reflect the state the fetch ran against.
            if !self.git_has_remote || !self.git_has_upstream {
                return;
            }
            if App::pull_needs_confirm(self.git_ahead, self.git_behind, self.git_uncommitted) {
                self.pending_pull = true;
                self.mode = AppMode::ConfirmDelete;
            } else if let Err(e) = self.git_merge_upstream() {
                self.message = Some((format!("Error: {}", e), Instant::now()));
            }
        }
    }

    pub fn git_pull(&mut self) -> Result<()> {
        let remote = match self.resolve_remote_name() {
            Ok(remote) => remote,
            Err(msg) => {
                // No remote to pull from: see git_push's blocked path.
                self.sync_blocked("Pull", msg);
                return Ok(());
            }
        };
        let branch = self.current_branch();
        let url = self.remote_url(&remote);
        // Capture HEAD before the pull: the outcome is classified from
        // repository state below, not from git's text (a merge over a
        // local-path remote prints "Already up to date." and never "Merge
        // made by", so text matching mislabels it).
        let before_head = self.head_rev();
        match self.git_cmd_owned(&Self::pull_args_with_strategy(&remote, branch.as_deref())) {
            Ok(out) => {
                let after_head = self.head_rev();
                let parent_count = self.head_parent_count();
                let action = Self::classify_pull_outcome(
                    before_head.as_deref(),
                    after_head.as_deref(),
                    parent_count,
                );
                self.git_output =
                    Self::sync_output(&out, Self::sync_fallback_line("Pulled from", &remote, &url));
                self.last_sync = Some(SyncStatus {
                    action: action.to_string(),
                    remote,
                    url,
                    at: Instant::now(),
                    success: true,
                    detail: String::new(),
                });
                self.message = Some(("Git: pulled".to_string(), Instant::now()));
                self.reload_entries()?;
            }
            Err(e) => {
                self.git_output = e.to_string();
                self.last_sync = Some(SyncStatus {
                    action: "Pull".to_string(),
                    remote,
                    url,
                    at: Instant::now(),
                    success: false,
                    detail: e.to_string(),
                });
                self.message = Some(("Git pull failed".to_string(), Instant::now()));
                self.refresh_git_info();
                return Err(e);
            }
        }
        self.refresh_git_info();
        Ok(())
    }

    /// Merge the already-fetched upstream ref into the current branch: the
    /// local half of a pull, run after git_fetch has refreshed the remote
    /// tracking ref. Splitting the two keeps the pull handler from
    /// fetching twice (two ssh round trips, password prompts and terminal
    /// handovers per pull). Records the outcome on the Sync line and
    /// reloads the entries, like git_pull.
    pub(crate) fn git_merge_upstream(&mut self) -> Result<()> {
        let remote = self.resolve_remote_name().unwrap_or_default();
        let url = if remote.is_empty() {
            String::new()
        } else {
            self.remote_url(&remote)
        };
        // Capture HEAD before the merge: the outcome is classified from
        // repository state, not from git's text (see classify_pull_outcome).
        let before_head = self.head_rev();
        match self.git_cmd_owned(&Self::merge_upstream_args("@{u}")) {
            Ok(out) => {
                let after_head = self.head_rev();
                let parent_count = self.head_parent_count();
                let action = Self::classify_pull_outcome(
                    before_head.as_deref(),
                    after_head.as_deref(),
                    parent_count,
                );
                self.git_output =
                    Self::sync_output(&out, Self::sync_fallback_line("Pulled from", &remote, &url));
                self.last_sync = Some(SyncStatus {
                    action: action.to_string(),
                    remote,
                    url,
                    at: Instant::now(),
                    success: true,
                    detail: String::new(),
                });
                self.message = Some(("Git: pulled".to_string(), Instant::now()));
                self.reload_entries()?;
            }
            Err(e) => {
                self.git_output = e.to_string();
                self.last_sync = Some(SyncStatus {
                    action: "Pull".to_string(),
                    remote,
                    url,
                    at: Instant::now(),
                    success: false,
                    detail: e.to_string(),
                });
                self.message = Some(("Git pull failed".to_string(), Instant::now()));
                self.refresh_git_info();
                return Err(e);
            }
        }
        self.refresh_git_info();
        Ok(())
    }

    pub fn show_context_menu(&mut self, x: u16, y: u16) {
        // "Decrypted" must mean the entry UNDER THE CURSOR is the one whose
        // content is in memory - not merely that some earlier entry was
        // decrypted. Right-clicking a different entry moves the selection
        // without clearing the preview, so comparing current_entry_path
        // against the selected entry's path is what keeps the menu (and its
        // actions) tied to the newly right-clicked file rather than the
        // previously decrypted one.
        let selected_rel = self.get_selected_rel_path();
        let is_decrypted = self.current_content.is_some()
            && self.current_entry_path.is_some()
            && self.current_entry_path == selected_rel;

        let items = if let Some(&idx) = self.filtered_entries.get(self.selected) {
            let entry = &self.entries[idx];
            if entry.is_dir {
                vec![
                    MenuItem {
                        label: "Expand".to_string(),
                        action: MenuAction::Expand,
                        enabled: !entry.expanded,
                    },
                    MenuItem {
                        label: "Collapse".to_string(),
                        action: MenuAction::Collapse,
                        enabled: entry.expanded,
                    },
                    MenuItem {
                        label: "New entry here".to_string(),
                        action: MenuAction::New,
                        enabled: true,
                    },
                    MenuItem {
                        label: "Add folder".to_string(),
                        action: MenuAction::AddFolder,
                        enabled: true,
                    },
                ]
            } else if is_decrypted {
                // Decrypted file: re-decrypting achieves nothing, so no
                // Open/Decrypt. Copy password and Show QR work because the
                // content is already in memory.
                vec![
                    MenuItem {
                        label: "Edit".to_string(),
                        action: MenuAction::Edit,
                        enabled: true,
                    },
                    MenuItem {
                        label: "Reveal/Hide".to_string(),
                        action: MenuAction::Reveal,
                        enabled: true,
                    },
                    MenuItem {
                        label: "Copy password".to_string(),
                        action: MenuAction::CopyPassword,
                        enabled: true,
                    },
                    MenuItem {
                        label: "Show QR".to_string(),
                        action: MenuAction::ShowQr,
                        enabled: true,
                    },
                    MenuItem {
                        label: "Move".to_string(),
                        action: MenuAction::Move,
                        enabled: true,
                    },
                    MenuItem {
                        label: "Delete".to_string(),
                        action: MenuAction::Delete,
                        enabled: true,
                    },
                ]
            } else {
                // Not yet decrypted: Copy password cannot work (nothing is
                // in memory to copy), so it is not offered.
                vec![
                    MenuItem {
                        label: "Open/Decrypt".to_string(),
                        action: MenuAction::Open,
                        enabled: true,
                    },
                    MenuItem {
                        label: "Edit".to_string(),
                        action: MenuAction::Edit,
                        enabled: true,
                    },
                    MenuItem {
                        label: "Move".to_string(),
                        action: MenuAction::Move,
                        enabled: true,
                    },
                    MenuItem {
                        label: "Delete".to_string(),
                        action: MenuAction::Delete,
                        enabled: true,
                    },
                ]
            }
        } else {
            vec![
                MenuItem {
                    label: "New entry".to_string(),
                    action: MenuAction::New,
                    enabled: true,
                },
                MenuItem {
                    label: "Add folder".to_string(),
                    action: MenuAction::AddFolder,
                    enabled: true,
                },
            ]
        };

        self.context_menu = Some(ContextMenu {
            x,
            y,
            items,
            selected: 0,
        });
        self.mode = AppMode::ContextMenu;
    }

    pub fn execute_menu_action(&mut self, action: MenuAction) {
        self.context_menu = None;
        self.mode = AppMode::Normal;

        match action {
            MenuAction::Open => {
                if let Err(e) = self.decrypt_current_entry(true) {
                    self.message = Some((format!("Error: {}", e), Instant::now()));
                }
            }
            MenuAction::Edit => self.open_editor(true),
            MenuAction::New => self.open_editor(false),
            MenuAction::Delete => {
                if self.filtered_entries.get(self.selected).is_some() {
                    self.entry_to_delete = Some(self.selected);
                    self.mode = AppMode::ConfirmDelete;
                }
            }
            MenuAction::CopyPassword => {
                if let Some(password) = self
                    .current_content
                    .as_ref()
                    .and_then(|c| c.lines.first().cloned())
                {
                    self.copy_to_clipboard(&password);
                }
            }
            MenuAction::Reveal => self.show_password = !self.show_password,
            MenuAction::ShowQr => self.show_qr = true,
            MenuAction::Expand => self.toggle_folder(),
            MenuAction::Collapse => self.toggle_folder(),
            MenuAction::AddFolder => {
                self.mode = AppMode::AddFolder;
                self.command_buffer.clear();
            }
            MenuAction::Move => {
                if let Some(source) = self.get_selected_rel_path() {
                    self.move_source = Some(source);
                    self.mode = AppMode::Move;
                    self.command_buffer.clear();
                }
            }
            MenuAction::GitPush => {
                if let Err(e) = self.git_push() {
                    self.message = Some((format!("Error: {}", e), Instant::now()));
                }
            }
            MenuAction::GitPull => {
                if let Err(e) = self.git_pull() {
                    self.message = Some((format!("Error: {}", e), Instant::now()));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_push_reports_up_to_date() {
        let action = App::sync_success_action("Pushed to", "Everything up-to-date");
        assert_eq!(action, "Up to date");
        assert_ne!(action, "Pushed to");
    }

    #[test]
    fn real_push_keeps_pushed_wording() {
        let out = "To /tmp/store\n   abc123..def456  main -> main\n";
        assert_eq!(App::sync_success_action("Pushed to", out), "Pushed to");
    }

    #[test]
    fn noop_pull_reports_up_to_date() {
        let action = App::sync_success_action("Pulled from", "Already up to date.");
        assert_eq!(action, "Up to date");
        assert_ne!(action, "Pulled from");
    }

    #[test]
    fn real_pull_keeps_pulled_wording() {
        let out = "Updating abc123..def456\nFast-forward\n  file | 1 +\n";
        assert_eq!(App::sync_success_action("Pulled from", out), "Pulled from");
    }

    #[test]
    fn pull_needs_confirm_classifies_outcomes() {
        // Diverged: the pull will merge.
        assert!(App::pull_needs_confirm(1, 2, 0));
        // Dirty tree and behind: the merge may abort or conflict.
        assert!(App::pull_needs_confirm(0, 1, 3));
        // Clean fast-forward: no confirmation.
        assert!(!App::pull_needs_confirm(0, 2, 0));
        // Up to date and clean.
        assert!(!App::pull_needs_confirm(0, 0, 0));
        // Dirty but not behind: the pull is a no-op.
        assert!(!App::pull_needs_confirm(0, 0, 5));
    }

    #[test]
    fn empty_output_falls_back_to_formatted_line() {
        assert_eq!(
            App::sync_output("", "Pushed to origin".to_string()),
            "Pushed to origin"
        );
        assert_eq!(
            App::sync_output("   \n\t", "Pushed to origin".to_string()),
            "Pushed to origin"
        );
        assert_eq!(
            App::sync_output("To origin\n", "fallback".to_string()),
            "To origin\n"
        );
    }

    #[test]
    fn pull_args_include_no_rebase() {
        let args = App::pull_args_with_strategy("origin", Some("main"));
        assert!(
            args.iter().any(|a| a == "--no-rebase"),
            "pull args must contain --no-rebase: {:?}",
            args
        );
        assert!(args.contains(&"origin".to_string()));
        assert!(args.contains(&"main".to_string()));
    }

    #[test]
    fn merge_upstream_args_include_no_edit() {
        let args = App::merge_upstream_args("@{u}");
        assert!(
            args.iter().any(|a| a == "--no-edit"),
            "merge args must contain --no-edit: {:?}",
            args
        );
        assert!(args.contains(&"merge".to_string()));
        assert!(args.contains(&"@{u}".to_string()));
    }

    #[test]
    fn pull_outcome_classified_from_head_state() {
        // HEAD unchanged by the pull: nothing was transferred.
        assert_eq!(
            App::classify_pull_outcome(Some("abc123"), Some("abc123"), 1),
            "Already up to date"
        );
        // New HEAD with more than one parent: the pull merged.
        assert_eq!(
            App::classify_pull_outcome(Some("abc123"), Some("def456"), 2),
            "Merged"
        );
        // New HEAD with a single parent: fast-forward.
        assert_eq!(
            App::classify_pull_outcome(Some("abc123"), Some("def456"), 1),
            "Fast-forwarded"
        );
    }

    /// A remote-less store (git init only, no remote): git_fetch blocks
    /// with the "no remote configured" Sync line and pull_with_confirm
    /// must stop there - no merge may run whose raw git error ("No
    /// upstream configured for branch ...") overwrites that message.
    #[test]
    fn pull_with_confirm_without_remote_keeps_blocked_message() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let home = dir.path().to_str().unwrap().to_string();
        let _env = EnvGuard::set(&[
            ("PASSWORD_STORE_DIR", Some(home.as_str())),
            ("HOME", Some(home.as_str())),
            ("XDG_CONFIG_HOME", None),
        ]);
        let mut app = App::new().expect("app");

        std::fs::create_dir_all(&app.store_path).expect("store dir");
        let store_dir = app.store_path.clone();
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&store_dir)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };

        run(&["init"]);
        run(&["config", "user.name", "Test"]);
        run(&["config", "user.email", "test@example.com"]);

        app.pull_with_confirm();

        // The Sync line still reflects the blocked fetch, not a merge
        // error, and names the actual problem.
        let sync = app.last_sync.expect("blocked fetch recorded on Sync line");
        assert!(!sync.success);
        assert!(
            sync.detail.contains("no remote"),
            "detail must name the missing remote: {}",
            sync.detail
        );
        assert!(
            app.message
                .as_ref()
                .map(|(m, _)| m.contains("no remote configured"))
                .unwrap_or(false),
            "message must keep the blocked-fetch wording"
        );
        // No merge ran: the confirmation prompt was never raised.
        assert!(!app.pending_pull);
        assert_eq!(app.mode, AppMode::Normal);
    }

    #[test]
    fn pull_confirm_prompt_differs_from_delete_prompt() {
        let mut app = temp_app(false);
        assert_eq!(
            app.confirm_prompt(),
            "Delete this entry? y:confirm n:cancel"
        );
        app.pending_pull = true;
        assert_eq!(
            app.confirm_prompt(),
            "Pull would merge - continue? y:confirm n:cancel"
        );
    }

    /// Set environment variables for the duration of a test, restoring the
    /// previous values on drop. Holds the shared environment lock; the lock
    /// is non-reentrant, so never hold two guards at once.
    struct EnvGuard {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn set(vars: &[(&'static str, Option<&str>)]) -> Self {
            let lock = crate::testenv::lock();
            let saved = vars
                .iter()
                .map(|(k, _)| (*k, std::env::var_os(k)))
                .collect();
            for (k, v) in vars {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
            EnvGuard { saved, _lock: lock }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in self.saved.drain(..) {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    /// An App whose store (and HOME, so config loading never touches the
    /// user's real config) lives in a fresh temp directory.
    fn temp_app(tui_active: bool) -> App {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let home = dir.path().to_str().unwrap().to_string();
        let _env = EnvGuard::set(&[
            ("PASSWORD_STORE_DIR", Some(home.as_str())),
            ("HOME", Some(home.as_str())),
            ("XDG_CONFIG_HOME", None),
        ]);
        let mut app = App::new().expect("app");
        app.tui_active = tui_active;
        app
    }

    #[test]
    fn create_folder_seeds_gitkeep() {
        let mut app = temp_app(false);
        app.create_folder("work").expect("create folder");
        assert!(app.store_path.join("work").join(".gitkeep").is_file());
    }

    /// Creating a folder in a git-tracked store must COMMIT, not just
    /// stage: a push sends only commits and the ahead count is derived
    /// from commits. Uses a real repo with a bare remote as upstream so
    /// the ahead count is observable.
    #[test]
    fn create_folder_commits_and_increments_ahead() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let home = dir.path().to_str().unwrap().to_string();
        let _env = EnvGuard::set(&[
            ("PASSWORD_STORE_DIR", Some(home.as_str())),
            ("HOME", Some(home.as_str())),
            ("XDG_CONFIG_HOME", None),
        ]);
        let mut app = App::new().expect("app");
        app.tui_active = false;

        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&app.store_path)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };

        // Repo with an identity and a bare remote as tracked upstream.
        run(&["init"]);
        run(&["config", "user.name", "Test"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["commit", "--allow-empty", "-m", "Initial commit"]);
        let bare = tempfile::TempDir::new().expect("bare tempdir");
        let bare_path = bare.path().to_str().unwrap().to_string();
        run(&["init", "--bare", &bare_path]);
        run(&["remote", "add", "origin", &bare_path]);
        run(&["push", "-u", "origin", "HEAD"]);

        app.refresh_git_info();
        assert_eq!(app.git_ahead, 0, "upstream must start in sync");

        app.create_folder("work").expect("create folder");

        let subject = Command::new("git")
            .args(["log", "-1", "--format=%s"])
            .current_dir(&app.store_path)
            .output()
            .expect("git log");
        assert_eq!(
            String::from_utf8_lossy(&subject.stdout).trim(),
            "Add folder work to store."
        );

        app.refresh_git_info();
        assert_eq!(app.git_ahead, 1, "folder commit must count as ahead");
    }

    /// A store with no remotes: remotes() lists nothing (whether or not
    /// git is even initialized), a sole remote resolves to itself, and
    /// with none at all resolve_remote_name errs with the fix-it hint
    /// instead of returning a literal "origin" that git would fail to
    /// resolve.
    #[test]
    fn remotes_and_resolve_remote_name_without_remotes() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let home = dir.path().to_str().unwrap().to_string();
        let _env = EnvGuard::set(&[
            ("PASSWORD_STORE_DIR", Some(home.as_str())),
            ("HOME", Some(home.as_str())),
            ("XDG_CONFIG_HOME", None),
        ]);
        let mut app = App::new().expect("app");

        // Store directory absent: empty, and no error.
        assert!(app.remotes().is_empty());

        std::fs::create_dir_all(&app.store_path).expect("store dir");
        // Capture the path in a local before the closure: borrowing
        // app.store_path in the closure would hold an immutable borrow of
        // app across the later resolve_remote_name() call (E0502).
        let store_dir = app.store_path.clone();
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&store_dir)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };

        // A git-initialized store with no remote still lists none, and
        // resolve_remote_name reports the configuration problem.
        run(&["init"]);
        run(&["config", "user.name", "Test"]);
        run(&["config", "user.email", "test@example.com"]);
        assert!(app.remotes().is_empty());
        let err = app.resolve_remote_name().expect_err("no remotes must err");
        assert!(
            err.contains("no remote configured"),
            "error must name the missing remote: {err}"
        );

        // Exactly one remote resolves to it without an upstream.
        run(&["remote", "add", "fudge", "/tmp/mujo-test-remote"]);
        assert_eq!(app.remotes(), vec!["fudge".to_string()]);
        assert_eq!(app.resolve_remote_name().ok(), Some("fudge".to_string()));
    }

    /// With remotes present but none named "origin", apply_git_config_remote
    /// must NOT conjure a third remote from config.toml's git.remote_url:
    /// the resolver's "multiple remotes" ambiguity error should stand as a
    /// prompt to act, not be silently resolved away.
    #[test]
    fn apply_git_config_remote_does_not_add_origin_when_remotes_exist() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let home = dir.path().to_str().unwrap().to_string();
        let _env = EnvGuard::set(&[
            ("PASSWORD_STORE_DIR", Some(home.as_str())),
            ("HOME", Some(home.as_str())),
            ("XDG_CONFIG_HOME", None),
        ]);
        let mut app = App::new().expect("app");

        std::fs::create_dir_all(&app.store_path).expect("store dir");
        // Capture the path in a local before the closure: borrowing
        // app.store_path in the closure would hold an immutable borrow of
        // app across the later resolve_remote_name() call (E0502).
        let store_dir = app.store_path.clone();
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&store_dir)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };

        run(&["init"]);
        run(&["config", "user.name", "Test"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["remote", "add", "fudge2", "/tmp/mujo-test-remote-a"]);
        run(&["remote", "add", "second", "/tmp/mujo-test-remote-b"]);

        app.config.git.remote_url = "/tmp/mujo-test-remote-config".to_string();
        app.apply_git_config_remote();

        let mut remotes = app.remotes();
        remotes.sort();
        assert_eq!(
            remotes,
            vec!["fudge2".to_string(), "second".to_string()],
            "no third remote may be conjured from config.toml"
        );
        let err = app
            .resolve_remote_name()
            .expect_err("ambiguity must still err");
        assert!(
            err.contains("multiple remotes"),
            "error must report the ambiguity: {err}"
        );
    }

    /// With no remotes at all (e.g. .git was recreated),
    /// apply_git_config_remote auto-configures "origin" from config.toml's
    /// git.remote_url, after which resolve_remote_name resolves to it.
    #[test]
    fn apply_git_config_remote_adds_origin_when_no_remotes() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let home = dir.path().to_str().unwrap().to_string();
        let _env = EnvGuard::set(&[
            ("PASSWORD_STORE_DIR", Some(home.as_str())),
            ("HOME", Some(home.as_str())),
            ("XDG_CONFIG_HOME", None),
        ]);
        let mut app = App::new().expect("app");

        std::fs::create_dir_all(&app.store_path).expect("store dir");
        let store_dir = app.store_path.clone();
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&store_dir)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };

        run(&["init"]);
        run(&["config", "user.name", "Test"]);
        run(&["config", "user.email", "test@example.com"]);

        let url = "/tmp/mujo-test-remote-config".to_string();
        app.config.git.remote_url = url.clone();
        app.apply_git_config_remote();

        assert_eq!(app.remotes(), vec!["origin".to_string()]);
        assert_eq!(app.resolve_remote_name().ok(), Some("origin".to_string()));
        // The remote git ended up with points at config.toml's URL.
        assert_eq!(
            app.git_cmd(&["remote", "get-url", "origin"]).ok(),
            Some(url)
        );
    }

    #[test]
    fn git_cmd_without_tui_leaves_needs_clear_false() {
        let mut app = temp_app(false);
        assert!(!app.tui_active, "tui_active must default to false");

        // "push" is an interactive subcommand, but with tui_active false
        // git_cmd must take the plain captured path and never hand over
        // the terminal.
        let _ = app.git_cmd(&["push"]);
        assert!(!app.needs_clear);
    }

    #[test]
    fn git_cmd_with_tui_sets_needs_clear_for_interactive() {
        let mut app = temp_app(true);
        let _ = app.git_cmd(&["push"]);
        assert!(app.needs_clear);
    }

    #[test]
    fn git_cmd_with_tui_non_interactive_leaves_needs_clear_false() {
        let mut app = temp_app(true);
        let _ = app.git_cmd(&["status"]);
        assert!(!app.needs_clear);
    }

    #[test]
    fn handover_flag_set_for_gpg_prompting_pass_commands() {
        // insert/generate/rm/init and a user-initiated show all funnel
        // through with_terminal_handover with interactive=true: the flag
        // must be recorded even under cfg!(test), where the raw terminal
        // I/O is skipped.
        let mut app = temp_app(true);
        let _ = app.with_terminal_handover(true, None, || Ok::<(), anyhow::Error>(()));
        assert!(app.needs_clear);
    }

    #[test]
    fn handover_flag_not_set_for_preview_show() {
        let mut app = temp_app(true);
        let _ = app.with_terminal_handover(false, None, || Ok::<(), anyhow::Error>(()));
        assert!(!app.needs_clear);
    }

    #[test]
    fn handover_requires_tui_active() {
        let mut app = temp_app(false);
        let _ = app.with_terminal_handover(true, None, || Ok::<(), anyhow::Error>(()));
        assert!(!app.needs_clear);
    }

    #[test]
    fn alt_repair_sets_needs_clear_for_interactive_with_tui() {
        let mut app = temp_app(true);
        let _ = app.with_alt_repair(true, || Ok::<(), anyhow::Error>(()));
        assert!(app.needs_clear);
    }

    #[test]
    fn alt_repair_requires_tui_active() {
        let mut app = temp_app(false);
        let _ = app.with_alt_repair(true, || Ok::<(), anyhow::Error>(()));
        assert!(!app.needs_clear);
    }

    #[test]
    fn alt_repair_requires_interactive() {
        let mut app = temp_app(true);
        let _ = app.with_alt_repair(false, || Ok::<(), anyhow::Error>(()));
        assert!(!app.needs_clear);
    }

    /// A user-initiated decrypt (Enter/reveal/edit load) hands the
    /// terminal over for pass show's gpg/pinentry child; the preview
    /// refresh (interactive=false) must not. The store points at a temp
    /// directory, so the spawned `pass show` fails fast on the bogus
    /// file without ever reaching a real entry.
    #[test]
    fn interactive_show_sets_handover_flag_but_preview_does_not() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let home = dir.path().to_str().unwrap().to_string();
        let _env = EnvGuard::set(&[
            ("PASSWORD_STORE_DIR", Some(home.as_str())),
            ("HOME", Some(home.as_str())),
            ("XDG_CONFIG_HOME", None),
        ]);
        let mut app = App::new().expect("app");
        app.tui_active = true;
        std::fs::write(app.store_path.join("a.gpg"), "not a real gpg file").expect("write entry");
        app.load_entries().expect("load entries");
        app.rebuild_filtered();

        let _ = app.decrypt_current_entry(true);
        assert!(app.needs_clear);

        app.needs_clear = false;
        let _ = app.decrypt_current_entry(false);
        assert!(!app.needs_clear);
    }

    /// The fallback Sync line names the resolved remote, so a local-path
    /// URL (or several remotes) still says which remote was used.
    #[test]
    fn sync_fallback_line_names_remote() {
        let line = App::sync_fallback_line("Pushed to", "origin", "/home/me/pass-store.git");
        assert_eq!(line, "Pushed to origin (/home/me/pass-store.git)");
        assert!(line.contains("origin"));
    }

    /// Blocked-sync Sync lines carry the short reason (the half-pane line
    /// would otherwise truncate the full fix-it text).
    #[test]
    fn blocked_sync_reason_is_shortened() {
        assert_eq!(
            App::short_sync_reason(
                "no remote configured - git remote add <name> <url> (or set git.remote_url in config.toml)"
            ),
            "no remote"
        );
        assert_eq!(
            App::short_sync_reason(
                "multiple remotes and no upstream - set one with git branch -u <remote>/<branch>"
            ),
            "multiple remotes"
        );
    }

    /// The reflog-derived fallback label: "last remote update <relative>"
    /// through the shared relative-time formatter, "(unknown)" when the
    /// reflog gave no parsable timestamp.
    #[test]
    fn remote_update_label_formats_reflog_timestamp() {
        let now = SystemTime::now();
        let ts = now
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_secs()
            - 90;
        let label = App::format_remote_update_label(Some(ts), now);
        assert!(
            label.starts_with("last remote update "),
            "label must name the reflog source: {label}"
        );
        assert!(label.ends_with("1 min ago"), "{label}");
        assert_eq!(App::format_remote_update_label(None, now), "(unknown)");
    }
}

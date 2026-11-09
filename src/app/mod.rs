use crate::config::Config;
use crate::models::{
    AppMode, ClipboardTool, ContextMenu, DecryptedContent, EntryEditor, GitPaneState, NewStoreStage, PassEntry, SyncStatus,
};
use crate::pass::store_path;
use crate::password::generate_password;
use anyhow::Result;
use std::path::PathBuf;
use std::time::{Duration, Instant};

mod clipboard;
mod editor_ops;
mod entries;
mod git_ops;
mod menu;
mod nav;

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
            app.mode = AppMode::NewStore;
            app.new_store_stage = NewStoreStage::Confirm;
        }

        app.rebuild_filtered();
        Ok(app)
    }

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
}

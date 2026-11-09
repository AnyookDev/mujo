use crate::app::{App, JUMP_TIMEOUT};
use crate::keys::{accepts, Action};
use crate::models::AppMode;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Instant;

use super::{open_help, PREVIEW_H_STEP, PREVIEW_V_STEP, TREE_JUMP};

pub fn handle_normal_mode(app: &mut App, key: KeyEvent) -> Result<()> {
    // While the QR is shown the key set collapses: w and Esc close it,
    // r/c/e close it first and then fall through to their normal preview
    // action (so reveal, copy and edit happen in the normal preview
    // layout, where they are visible), and every other key is ignored.
    if app.show_qr {
        match (key.code, key.modifiers) {
            (KeyCode::Char('w'), m)
                if !m.intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL) =>
            {
                app.show_qr = false;
                return Ok(());
            }
            (KeyCode::Esc, _) => {
                app.show_qr = false;
                return Ok(());
            }
            // r deliberately differs from the preview's r while the QR is
            // shown: it closes the QR and REVEALS the password instead of
            // toggling. Falling through to the toggle arm would flip the
            // current state - hiding the password when the user pressed r
            // to see it - so do not simplify this back to a fall-through.
            (KeyCode::Char('r'), m)
                if !m.intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL) =>
            {
                app.show_qr = false;
                app.show_password = true;
                return Ok(());
            }
            (KeyCode::Char('c') | KeyCode::Char('e'), m)
                if !m.intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL) =>
            {
                // Close the QR, then let the normal arm below act.
                app.show_qr = false;
            }
            _ => return Ok(()),
        }
    }
    // Clear the jump buffer on any key that isn't a Shift+alpha jump.
    let is_jump_key = key.modifiers.contains(KeyModifiers::SHIFT)
        && matches!(key.code, KeyCode::Char(c) if c.is_alphabetic());
    if !is_jump_key {
        app.jump_buf.clear();
        app.jump_at = None;
    }

    match key.code {
        // Ctrl-modified Char arms must precede every plain Char arm:
        // crossterm reports Ctrl+letter as Char(letter) with CONTROL, and
        // the plain arms below carry no modifier guard, so a Ctrl arm
        // placed after them is unreachable (first matching arm wins - the
        // same ordering rule the Ctrl+v paste arms in this file follow).
        KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Err(e) = app.git_push() {
                app.message = Some((format!("Error: {}", e), Instant::now()));
            }
        }
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.reload_config();
        }
        // Quit stays on plain 'q' only: 'Q' now falls through to the
        // Shift+Letter jump arm below.
        KeyCode::Char('q') => app.should_quit = true,
        // Tree jumps: Ctrl+Up/Down everywhere, plus Ctrl+U/D (advertised on
        // Mac, where Mission Control captures Ctrl+arrow). accepts ignores
        // the platform, so both chords work everywhere. Wrong modifiers
        // (plain, Shift) do not match, so the plain navigation and
        // Shift+letter jump arms below stay reachable.
        KeyCode::Down | KeyCode::Char('d')
            if accepts(Action::JumpDown, key.code, key.modifiers) =>
        {
            app.jump_selection(TREE_JUMP);
        }
        KeyCode::Up | KeyCode::Char('u') if accepts(Action::JumpUp, key.code, key.modifiers) => {
            app.jump_selection(-TREE_JUMP);
        }
        // Shift+Up/Down must be matched BEFORE the plain Down/Up arms: in
        // Rust the first matching arm wins and the plain arms carry no
        // guard, so any Shift-variant listed after them is unreachable.
        // They scroll the preview content vertically while an entry is
        // decrypted, matching the mouse wheel and the Git pane's Shift+
        // arrow scrolling. Without a preview they navigate the tree.
        KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if app.current_content.is_some() {
                app.preview_scroll_offset += PREVIEW_V_STEP;
            } else {
                app.move_selection(1);
            }
        }
        KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if app.current_content.is_some() {
                if app.preview_scroll_offset >= PREVIEW_V_STEP {
                    app.preview_scroll_offset -= PREVIEW_V_STEP;
                } else {
                    app.preview_scroll_offset = 0;
                }
            } else {
                app.move_selection(-1);
            }
        }
        // Plain navigation arms explicitly exclude SHIFT (and CONTROL, for
        // symmetry) so the shifted/controlled variants above stay reachable
        // regardless of arm ordering.
        KeyCode::Char('j') | KeyCode::Down
            if !key
                .modifiers
                .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL) =>
        {
            app.move_selection(1);
        }
        KeyCode::Char('k') | KeyCode::Up
            if !key
                .modifiers
                .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL) =>
        {
            app.move_selection(-1);
        }
        KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if app.current_content.is_some() {
                app.preview_h_scroll = app.preview_h_scroll.saturating_sub(PREVIEW_H_STEP);
            }
        }
        KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if app.current_content.is_some() {
                app.preview_h_scroll += PREVIEW_H_STEP;
            }
        }
        KeyCode::Char('h') | KeyCode::Left
            if !key
                .modifiers
                .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL) =>
        {
            if let Some(idx) = app.filtered_entries.get(app.selected) {
                let entry = &app.entries[*idx];
                if entry.is_dir && entry.expanded {
                    app.toggle_folder();
                    app.clear_preview();
                } else if entry.depth > 0 {
                    let parent_depth = entry.depth.saturating_sub(1);
                    for (i, e) in app.entries.iter().enumerate() {
                        if e.is_dir && e.depth == parent_depth && entry.path.starts_with(&e.path) {
                            if e.expanded {
                                let parent_idx = i;
                                app.entries[parent_idx].expanded = false;
                                app.rebuild_filtered();
                                if let Some(new_selected) =
                                    app.filtered_entries.iter().position(|&fi| fi == parent_idx)
                                {
                                    app.selected = new_selected;
                                }
                                app.clear_preview();
                            }
                            break;
                        }
                    }
                }
            }
        }
        KeyCode::Char('l') | KeyCode::Right
            if !key
                .modifiers
                .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL) =>
        {
            if let Some(idx) = app.filtered_entries.get(app.selected) {
                let entry = &app.entries[*idx];
                if entry.is_dir && !entry.expanded {
                    app.toggle_folder();
                } else if entry.is_dir && entry.expanded {
                    app.move_selection(1);
                }
            }
        }
        KeyCode::Enter => {
            app.activate_selected();
        }
        KeyCode::Char('r') => {
            if app.current_content.is_some() {
                app.show_password = !app.show_password;
            }
        }
        KeyCode::Char('w') => {
            if app.current_content.is_some() {
                app.show_qr = !app.show_qr;
            }
        }
        KeyCode::Esc => {
            // The QR-shown case is handled by the guard at the top of this
            // handler (Esc closes the QR there), so this arm only runs
            // with the QR closed: a leftover search filter must never be
            // unrecoverable, so clear it first (restoring the full entry
            // list) before falling back to exiting the decrypted view.
            if !app.search_query.is_empty() {
                // A leftover search filter must never be unrecoverable:
                // clear it first (restoring the full entry list) before
                // falling back to exiting the decrypted view.
                app.clear_search_filter();
            } else {
                app.clear_preview();
            }
        }
        KeyCode::Char('e') => {
            if let Some(&idx) = app.filtered_entries.get(app.selected) {
                if !app.entries[idx].is_dir {
                    app.open_editor(true);
                }
            }
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.copy_ctrl_c();
        }
        KeyCode::Char('c')
            if !key
                .modifiers
                .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL) =>
        {
            let password_to_copy = app
                .current_content
                .as_ref()
                .and_then(|content| content.lines.first().cloned());

            if let Some(password) = password_to_copy {
                app.copy_to_clipboard(&password);
                // Keep the "(Ns)" hint in sync with the configured timeout;
                // when auto-clear is disabled no countdown is shown.
                let msg = match app.clipboard_timeout() {
                    Some(d) => format!("Password copied to clipboard ({}s)", d.as_secs()),
                    None => "Password copied to clipboard".to_string(),
                };
                app.message = Some((msg, Instant::now()));
            }
        }
        KeyCode::Char('n') => app.open_editor(false),
        KeyCode::Char('f') => {
            app.mode = AppMode::AddFolder;
            app.command_buffer.clear();
        }
        KeyCode::Char('m') => {
            if let Some(source) = app.get_selected_rel_path() {
                app.move_source = Some(source);
                app.mode = AppMode::Move;
                app.command_buffer.clear();
            }
        }
        KeyCode::Char('d') => {
            if let Some(_idx) = app.filtered_entries.get(app.selected) {
                app.entry_to_delete = Some(app.selected);
                app.mode = AppMode::ConfirmDelete;
            }
        }
        KeyCode::Char('p') => {
            // Same flow as the git pane's p: fetch, then confirm or merge,
            // so a diverged pull never merges silently from Normal mode.
            app.pull_with_confirm();
        }
        KeyCode::Char('g') => {
            app.open_git_pane();
        }
        // Shift+Letter: type-ahead prefix jump. Accumulates characters into
        // jump_buf; resets the buffer when JUMP_TIMEOUT has elapsed since the
        // last keypress. Tries the full prefix, then falls back to the last
        // character alone when the prefix matched nothing and is multi-char.
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::SHIFT) && c.is_alphabetic() => {
            let now = Instant::now();
            let timed_out = app
                .jump_at
                .map(|t| now.duration_since(t) >= JUMP_TIMEOUT)
                .unwrap_or(true);
            if timed_out {
                app.jump_buf.clear();
            }
            app.jump_buf.push(c.to_lowercase().next().unwrap_or(c));
            app.jump_at = Some(now);

            if !app.jump_to_prefix(&app.jump_buf.clone()) && app.jump_buf.len() > 1 {
                // Full prefix matched nothing; retry with just the last char.
                let last = app.jump_buf.chars().next_back().unwrap().to_string();
                app.jump_buf = last.clone();
                app.jump_to_prefix(&last);
            }
        }
        KeyCode::Char('/') => {
            app.mode = AppMode::Search;
            app.command_buffer.clear();
            app.search_query.clear();
        }
        // '?' opens the full-screen help view. Previously unbound: the
        // Shift+letter jump arm above only matches alphabetic characters,
        // so '?' fell through to the catch-all. In the live dispatch
        // maybe_open_help intercepts '?' before this handler runs; this
        // arm keeps direct calls (and tests) opening help the same way.
        KeyCode::Char('?') => {
            open_help(app);
        }
        // Preview paging: PgUp/PgDn everywhere, plus b (up) and Space
        // (down). The printable chords are guarded to plain presses with a
        // preview open, so they never steal a keystroke from the
        // Shift+letter jump and never fire with nothing to scroll; the
        // text-entry modes (Search, Editor, prompts) have their own
        // handlers and never see these arms.
        KeyCode::PageDown
            if accepts(Action::PageDown, key.code, key.modifiers)
                && app.current_content.is_some() =>
        {
            app.preview_scroll_offset += 3;
        }
        KeyCode::Char(' ')
            if accepts(Action::PageDown, key.code, key.modifiers)
                && !key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL)
                && app.current_content.is_some() =>
        {
            app.preview_scroll_offset += 3;
        }
        KeyCode::PageUp if accepts(Action::PageUp, key.code, key.modifiers) => {
            if app.current_content.is_some() && app.preview_scroll_offset >= 3 {
                app.preview_scroll_offset -= 3;
            } else {
                app.preview_scroll_offset = 0;
            }
        }
        KeyCode::Char('b')
            if accepts(Action::PageUp, key.code, key.modifiers)
                && !key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL)
                && app.current_content.is_some() =>
        {
            if app.preview_scroll_offset >= 3 {
                app.preview_scroll_offset -= 3;
            } else {
                app.preview_scroll_offset = 0;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, JUMP_TIMEOUT};
    use crate::config::Config;
    use crate::models::{AppMode, GitPaneState, NewStoreStage, PassEntry};
    use crossterm::event::{KeyEvent, KeyEventKind, KeyModifiers};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn fake_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    fn shift(c: char) -> KeyEvent {
        fake_key(KeyCode::Char(c), KeyModifiers::SHIFT)
    }

    fn plain(c: char) -> KeyEvent {
        fake_key(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn esc() -> KeyEvent {
        fake_key(KeyCode::Esc, KeyModifiers::NONE)
    }

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

    fn selected_name(app: &App) -> &str {
        &app.entries[app.filtered_entries[app.selected]].display_name
    }

    #[test]
    fn single_shift_char_jumps_to_first_match() {
        let mut app = app_with_entries(&["alice", "bob", "carol"]);
        handle_normal_mode(&mut app, shift('B')).unwrap();
        assert_eq!(selected_name(&app), "bob");
        assert_eq!(app.jump_buf, "b");
    }

    #[test]
    fn two_shift_chars_narrow_the_jump() {
        // entries: bob, alice, anna. Start at bob (0).
        // Pressing A: prefix "a", scan from bob+1 → first 'a' match is alice (1). Land there.
        // Pressing L next: prefix "al", scan from alice+1 → wraps, only alice matches "al". Stay.
        let mut app = app_with_entries(&["bob", "alice", "anna"]);
        handle_normal_mode(&mut app, shift('A')).unwrap();
        assert_eq!(selected_name(&app), "alice");
        handle_normal_mode(&mut app, shift('L')).unwrap();
        assert_eq!(selected_name(&app), "alice");
        assert_eq!(app.jump_buf, "al");
    }

    #[test]
    fn dead_prefix_falls_back_to_last_char() {
        // "bz" matches nothing; fallback to "z" also matches nothing —
        // selection stays put, buf is "z".
        let mut app = app_with_entries(&["alice", "bob", "carol"]);
        handle_normal_mode(&mut app, shift('B')).unwrap(); // buf = "b", jumps to bob
        assert_eq!(selected_name(&app), "bob");
        handle_normal_mode(&mut app, shift('Z')).unwrap(); // "bz" dead, fallback "z" dead
        assert_eq!(selected_name(&app), "bob", "selection must not move");
        assert_eq!(app.jump_buf, "z", "buf must be the last char alone");
    }

    #[test]
    fn dead_prefix_fallback_matches_last_char() {
        // "bq" dead, but "q" matches quincy — should land on quincy.
        let mut app = app_with_entries(&["alice", "bob", "quincy"]);
        handle_normal_mode(&mut app, shift('B')).unwrap(); // buf = "b"
        handle_normal_mode(&mut app, shift('Q')).unwrap(); // "bq" dead, fallback "q"→quincy
        assert_eq!(selected_name(&app), "quincy");
        assert_eq!(app.jump_buf, "q");
    }

    #[test]
    fn timeout_clears_buffer() {
        let mut app = app_with_entries(&["alice", "bob", "carol"]);
        handle_normal_mode(&mut app, shift('B')).unwrap();
        assert_eq!(app.jump_buf, "b");
        // Backdate jump_at past the timeout threshold.
        app.jump_at = Some(Instant::now() - JUMP_TIMEOUT - Duration::from_millis(1));
        handle_normal_mode(&mut app, shift('C')).unwrap();
        // Buffer was cleared then 'c' pushed → "c", not "bc".
        assert_eq!(app.jump_buf, "c");
        assert_eq!(selected_name(&app), "carol");
    }

    #[test]
    fn esc_clears_buffer() {
        let mut app = app_with_entries(&["alice", "bob"]);
        handle_normal_mode(&mut app, shift('B')).unwrap();
        assert_eq!(app.jump_buf, "b");
        handle_normal_mode(&mut app, esc()).unwrap();
        assert!(app.jump_buf.is_empty());
        assert!(app.jump_at.is_none());
    }

    #[test]
    fn non_shift_alpha_clears_buffer() {
        let mut app = app_with_entries(&["alice", "bob"]);
        handle_normal_mode(&mut app, shift('B')).unwrap();
        assert_eq!(app.jump_buf, "b");
        // Plain 'j' (move down) should clear the jump buffer.
        handle_normal_mode(&mut app, plain('j')).unwrap();
        assert!(app.jump_buf.is_empty());
        assert!(app.jump_at.is_none());
    }

    #[test]
    fn buffer_cleared_on_mode_change_to_search() {
        let mut app = app_with_entries(&["alice", "bob"]);
        handle_normal_mode(&mut app, shift('B')).unwrap();
        assert_eq!(app.jump_buf, "b");
        handle_normal_mode(&mut app, plain('/')).unwrap();
        assert!(app.jump_buf.is_empty());
        assert_eq!(app.mode, AppMode::Search);
    }

    #[test]
    fn buffer_cleared_on_mode_change_to_add_folder() {
        let mut app = app_with_entries(&["alice", "bob"]);
        handle_normal_mode(&mut app, shift('B')).unwrap();
        handle_normal_mode(&mut app, plain('f')).unwrap();
        assert!(app.jump_buf.is_empty());
        assert_eq!(app.mode, AppMode::AddFolder);
    }

    /// Shift+uppercase letters must feed the jump buffer and must NOT
    /// trigger the plain-char actions (e.g. 'q' quits, 'g' opens git pane).
    #[test]
    fn test_shift_uppercase_letters_jump_and_do_not_trigger_actions() {
        // 'Q' with SHIFT must not set should_quit.
        let mut app = app_with_entries(&["quincy", "zara"]);
        handle_normal_mode(&mut app, shift('Q')).unwrap();
        assert!(!app.should_quit, "Shift+Q must not quit");
        assert_eq!(app.jump_buf, "q");

        // 'G' with SHIFT must not open the git pane.
        let mut app2 = app_with_entries(&["quincy", "zara"]);
        handle_normal_mode(&mut app2, shift('G')).unwrap();
        assert_eq!(app2.mode, AppMode::Normal, "Shift+G must not open git pane");
        assert_eq!(app2.jump_buf, "g");
    }

    #[test]
    fn jj_matches_jj_prefixed_entry() {
        let mut app = app_with_entries(&["jack", "JJsomething", "jill"]);
        // Start at jack (0); two 'J' presses: "jj" matches JJsomething.
        handle_normal_mode(&mut app, shift('J')).unwrap();
        handle_normal_mode(&mut app, shift('J')).unwrap();
        assert_eq!(app.jump_buf, "jj");
        assert_eq!(selected_name(&app).to_lowercase(), "jjsomething");
    }
}

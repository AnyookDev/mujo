use crate::app::App;
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
        // Shift+Letter: jump to the first sibling entry starting with that
        // letter (case-insensitive). Every uppercase letter reaches this
        // arm now that git push, config reload and quit have moved to
        // Ctrl+P / Ctrl+R / Ctrl+Q and the unadvertised Shift+C generate
        // binding is gone.
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::SHIFT) && c.is_alphabetic() => {
            app.jump_to_letter(c);
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

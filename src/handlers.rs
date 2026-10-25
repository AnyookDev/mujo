use crate::app::App;
use crate::keys::{accepts, Action};
use crate::models::{AppMode, GitPaneState, NewStoreStage};
use crate::pass::PassCommand;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Instant;

const TREE_JUMP: i32 = 5;
const PREVIEW_H_STEP: usize = 4;
const PREVIEW_V_STEP: usize = 3;
const GIT_H_STEP: usize = 4;
const GIT_V_STEP: usize = 3;
const HELP_V_STEP: usize = 3;

/// Byte index of the `char_idx`-th character of `s` (multi-byte safe).
/// Clamps to the string length, so a cursor at or past the end maps to
/// `s.len()`.
fn char_to_byte_idx(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

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

pub fn handle_search_mode(app: &mut App, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Esc => {
            // Esc leaves search mode with the filter cleared, so the full
            // entry list is restored instead of a stuck filtered tree.
            app.clear_search_filter();
            app.mode = AppMode::Normal;
        }
        KeyCode::Enter => {
            // Open/decrypt the selected entry (same as Enter in Normal
            // mode), then return to Normal mode. Any remaining filter can
            // still be cleared with Esc afterwards.
            app.activate_selected();
            app.mode = AppMode::Normal;
        }
        // Arrow keys move the tree selection while the search box stays
        // focused: they don't conflict with typing (letters still go to
        // the query) and the mode is unchanged.
        KeyCode::Down => {
            app.move_selection(1);
        }
        KeyCode::Up => {
            app.move_selection(-1);
        }
        // PageUp/PageDown jump the selection by the tree pane's scroll
        // step, matching Ctrl+Up/Down in Normal mode.
        KeyCode::PageDown => {
            app.jump_selection(TREE_JUMP);
        }
        KeyCode::PageUp => {
            app.jump_selection(-TREE_JUMP);
        }
        KeyCode::Char(c) => {
            app.search_query.push(c);
            app.rebuild_filtered();
        }
        KeyCode::Backspace => {
            app.search_query.pop();
            app.rebuild_filtered();
        }
        _ => {}
    }
    Ok(())
}

/// Open the full-screen help view from any mode, recording the current
/// mode so closing help can return to it (the Editor included: the editor
/// stays open in app.editor the whole time, so cursor, per-field scroll
/// and unsaved buffer are all intact on return).
pub fn open_help(app: &mut App) {
    app.help_return_mode = app.mode;
    app.mode = AppMode::Help;
    app.help_scroll_offset = 0;
}

/// The mode-independent help keys, run at the top of main's key dispatch
/// before the per-mode handlers. Returns true when the key was handled:
///
/// - Ctrl+/ (Action::Help) opens help from ANY mode except Help itself,
///   where the same chord closes the view (see handle_help_mode).
/// - "?" opens help only in the modes where it is not a printable
///   character - Normal, Git and the three Confirm prompts. In Editor,
///   Search, AddFolder and Move, "?" is text and falls through to the
///   mode's own handler.
pub fn maybe_open_help(app: &mut App, key: &KeyEvent) -> bool {
    if app.mode == AppMode::Help {
        return false;
    }
    if accepts(Action::Help, key.code, key.modifiers) {
        open_help(app);
        return true;
    }
    if key.code == KeyCode::Char('?')
        && matches!(
            app.mode,
            AppMode::Normal
                | AppMode::Git
                | AppMode::ConfirmDelete
                | AppMode::ConfirmSave
                | AppMode::ConfirmDiscard
        )
    {
        open_help(app);
        return true;
    }
    false
}

/// Full-screen help view: lists every binding grouped by pane. Esc, q, ?
/// and Ctrl+/ all close it and return to the mode help was opened from
/// (help_return_mode), NOT a hardcoded Normal; Up/Down and k/j scroll the
/// body by one line, Shift+arrows and PgUp/PgDn by page, reusing the Git
/// pane's scroll keys.
pub fn handle_help_mode(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
            app.mode = app.help_return_mode;
        }
        // Ctrl+/ closes as well: maybe_open_help skips Help mode, so the
        // universal open chord doubles as a close chord here. 0x1F is
        // reported as Char('7') with CONTROL (and as Char('_') on some
        // terminals), so match both spellings.
        KeyCode::Char('7') | KeyCode::Char('_')
            if key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            app.mode = app.help_return_mode;
        }
        KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
            app.help_scroll_offset = app.help_scroll_offset.saturating_sub(HELP_V_STEP);
        }
        KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
            app.help_scroll_offset += HELP_V_STEP;
        }
        // Line-by-line scrolling: plain Up/Down and vim j/k. Guarded
        // against SHIFT/CONTROL so the Shift arms above (and unbound Ctrl
        // chords) keep their meaning; draw_help clamps the offset against
        // the body length.
        KeyCode::Up | KeyCode::Char('k')
            if !key
                .modifiers
                .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL) =>
        {
            app.help_scroll_offset = app.help_scroll_offset.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j')
            if !key
                .modifiers
                .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL) =>
        {
            app.help_scroll_offset += 1;
        }
        // Paging: PgUp/PgDn everywhere, plus b (up) and Space (down); top
        // and bottom via Home/End everywhere, g/G (advertised on Mac). The
        // help view has no text entry, so the printable chords are free.
        KeyCode::PageUp | KeyCode::Char('b')
            if accepts(Action::PageUp, key.code, key.modifiers) =>
        {
            app.help_scroll_offset = app.help_scroll_offset.saturating_sub(HELP_V_STEP);
        }
        KeyCode::PageDown | KeyCode::Char(' ')
            if accepts(Action::PageDown, key.code, key.modifiers) =>
        {
            app.help_scroll_offset += HELP_V_STEP;
        }
        KeyCode::Home | KeyCode::Char('g') if accepts(Action::Top, key.code, key.modifiers) => {
            app.help_scroll_offset = 0;
        }
        KeyCode::End | KeyCode::Char('G') if accepts(Action::Bottom, key.code, key.modifiers) => {
            // draw_help clamps against the body length.
            app.help_scroll_offset = usize::MAX / 2;
        }
        _ => {}
    }
    Ok(())
}

/// First-run setup: shown when no ~/.password-store exists. Stage 1 asks
/// whether to set up a pass store; stage 2 asks for the GPG ID, validates
/// that a matching GPG key exists (`gpg --list-keys`), and only then runs
/// `pass init <gpg-id>` on Enter. An invalid key keeps the prompt open
/// (with the typed input preserved for correction); Esc cancels back to
/// stage 1. The GPG ID field is a proper single-line editor: Left/Right/
/// Home/End move the cursor within the text (command_cursor), typing and
/// Ctrl+v insert at the cursor, and Backspace deletes the character before
/// it - so a typo in the middle no longer requires deleting everything
/// after it.
pub fn handle_new_store_mode(app: &mut App, key: KeyEvent) -> Result<()> {
    match app.new_store_stage {
        NewStoreStage::Confirm => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                app.new_store_stage = NewStoreStage::AskGpgId;
                app.command_buffer.clear();
                app.command_cursor = 0;
            }
            // Without a store there is nothing to browse or edit, so
            // declining the setup is the same as quitting.
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('n') | KeyCode::Char('N') => {
                app.should_quit = true;
            }
            _ => {}
        },
        NewStoreStage::AskGpgId => match key.code {
            KeyCode::Esc => {
                app.command_buffer.clear();
                app.command_cursor = 0;
                app.new_store_stage = NewStoreStage::Confirm;
            }
            // Cursor movement within the input line.
            KeyCode::Left => {
                app.command_cursor = app.command_cursor.saturating_sub(1);
            }
            KeyCode::Right => {
                let len = app.command_buffer.chars().count();
                app.command_cursor = (app.command_cursor + 1).min(len);
            }
            KeyCode::Home => {
                app.command_cursor = 0;
            }
            KeyCode::End => {
                app.command_cursor = app.command_buffer.chars().count();
            }
            // Ctrl+v must be matched BEFORE the plain Char(c) arm: crossterm
            // reports it as Char('v') with CONTROL, and without this arm the
            // catch-all would insert a literal 'v' instead of pasting.
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(text) = App::paste_from_clipboard() {
                    // The GPG ID field is single-line: strip newlines and
                    // other control characters from the paste, and trim
                    // surrounding whitespace (a trailing newline is common
                    // when copying from a terminal).
                    let cleaned: String = text.chars().filter(|c| !c.is_control()).collect();
                    let cleaned = cleaned.trim();
                    if !cleaned.is_empty() {
                        // Insert at the cursor, like a normal text field.
                        let byte_idx = char_to_byte_idx(&app.command_buffer, app.command_cursor);
                        app.command_buffer.insert_str(byte_idx, cleaned);
                        app.command_cursor += cleaned.chars().count();
                    }
                } else {
                    app.message = Some(("Clipboard empty or unavailable".to_string(), Instant::now()));
                }
            }
            KeyCode::Enter => {
                let gpg_id = app.command_buffer.trim().to_string();
                if gpg_id.is_empty() {
                    app.message = Some(("GPG ID cannot be empty".to_string(), Instant::now()));
                } else {
                    match PassCommand::validate_gpg_key(&gpg_id) {
                        Ok(true) => {
                            // pass init is covered by the alt-screen repair
                            // for symmetry with the other gpg-invoking pass
                            // commands (see with_alt_repair).
                            match app.with_alt_repair(true, || PassCommand::init(&gpg_id)) {
                                Ok(_) => {
                                    app.command_buffer.clear();
                                    app.command_cursor = 0;
                                    app.new_store_stage = NewStoreStage::Confirm;
                                    app.mode = AppMode::Normal;
                                    let _ = app.load_entries();
                                    app.rebuild_filtered();
                                    app.message = Some(("Password store created".to_string(), Instant::now()));
                                }
                                Err(e) => {
                                    app.message = Some((format!("Store creation failed: {}", e), Instant::now()));
                                }
                            }
                        }
                        Ok(false) => {
                            // Keep the prompt (and the typed input) so the
                            // user can correct a typo and retry.
                            app.message = Some((
                                format!("GPG key not found: {}. Check with `gpg --list-keys`", gpg_id),
                                Instant::now(),
                            ));
                        }
                        Err(e) => {
                            // validate_gpg_key's errors are already
                            // user-facing and actionable ("gpg not found on
                            // PATH — install gnupg and pass", or
                            // "gpg failed: <stderr>"); show them as-is so a
                            // missing gpg binary is never masked by a
                            // generic prefix (or a raw "os error" string).
                            app.message = Some((e.to_string(), Instant::now()));
                        }
                    }
                }
            }
            KeyCode::Char(c) => {
                // Insert at the cursor position, not just at the end.
                let byte_idx = char_to_byte_idx(&app.command_buffer, app.command_cursor);
                app.command_buffer.insert(byte_idx, c);
                app.command_cursor += 1;
            }
            KeyCode::Backspace
                // Delete the character BEFORE the cursor (if any).
                if app.command_cursor > 0 => {
                    let start = char_to_byte_idx(&app.command_buffer, app.command_cursor - 1);
                    let end = char_to_byte_idx(&app.command_buffer, app.command_cursor);
                    app.command_buffer.replace_range(start..end, "");
                    app.command_cursor -= 1;
                }
            _ => {}
        },
    }
    Ok(())
}

pub fn handle_editor_mode(app: &mut App, key: crossterm::event::KeyEvent) -> Result<()> {
    use crossterm::event::KeyModifiers;

    // Ctrl+g generates a password with the editor's current Length/Charset
    // settings and replaces the Password field in place. No mode change:
    // the editor stays open and Ctrl+g can be pressed again to re-roll.
    if key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.generate_password_in_editor();
        return Ok(());
    }

    let mut text_to_copy: Option<String> = None;

    if let Some(ref mut editor) = app.editor {
        match key.code {
            KeyCode::Esc => {
                if app.has_unsaved_changes() {
                    // Unsaved work must not be silently discarded: ask
                    // whether to save, discard, or keep editing.
                    app.mode = AppMode::ConfirmDiscard;
                } else {
                    app.editor = None;
                    app.mode = AppMode::Normal;
                }
            }
            KeyCode::Enter => {
                editor.insert_char('\n');
            }
            KeyCode::Backspace => {
                editor.backspace();
            }
            KeyCode::Delete => {
                editor.delete();
            }
            KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
                if !editor.has_selection {
                    editor.start_selection();
                }
                editor.move_cursor_left();
                editor.update_selection();
            }
            KeyCode::Left => {
                editor.clear_selection();
                editor.move_cursor_left();
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
                if !editor.has_selection {
                    editor.start_selection();
                }
                editor.move_cursor_right();
                editor.update_selection();
            }
            KeyCode::Right => {
                editor.clear_selection();
                editor.move_cursor_right();
            }
            // Password generation controls, active only while the Password
            // field is focused (cursor_line == 1) so plain arrows keep
            // working as field navigation in the path and content fields.
            // Ctrl+Up increases and Ctrl+Down decreases the generated
            // password length; these arms must come before the plain
            // Up/Down arms, which carry no modifier guard.
            // Generated password length: Ctrl+Up/Down everywhere, plus
            // Ctrl+U/D (advertised on Mac). Same action enum as the tree
            // jump, different effect per pane. Active only while the
            // Password field is focused, like before.
            KeyCode::Up | KeyCode::Char('u')
                if editor.cursor_line == 1 && accepts(Action::JumpUp, key.code, key.modifiers) =>
            {
                editor.gen_length += 1;
            }
            KeyCode::Down | KeyCode::Char('d')
                if editor.cursor_line == 1
                    && accepts(Action::JumpDown, key.code, key.modifiers) =>
            {
                editor.gen_length = editor.gen_length.saturating_sub(1).max(8);
            }
            KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
                if !editor.has_selection {
                    editor.start_selection();
                }
                editor.move_cursor_up();
                editor.update_selection();
            }
            // Explicit field navigation (Path -> Password -> Additional
            // Content). Backspace never changes fields; only Up/Down do.
            KeyCode::Up => {
                editor.clear_selection();
                match editor.cursor_line {
                    // Path is the topmost field: stay.
                    0 => {}
                    // Password -> Path, but only for new entries: for an
                    // existing entry the Path field is locked (read-only),
                    // so it cannot be focused and Up stays on Password.
                    1 if editor.is_new => {
                        editor.cursor_line = 0;
                        editor.cursor_col = editor.path.len();
                    }
                    // Password of an existing entry: Path is locked, stay.
                    1 => {}
                    // Top line of Additional Content -> Password.
                    2 => {
                        editor.cursor_line = 1;
                        let pw_len = editor.content.lines().next().map(|l| l.len()).unwrap_or(0);
                        editor.cursor_col = editor.cursor_col.min(pw_len);
                    }
                    // Within Additional Content: previous line, column clamped.
                    _ => {
                        editor.cursor_line -= 1;
                        let line_len = editor
                            .content
                            .lines()
                            .nth(editor.cursor_line - 1)
                            .map(|l| l.len())
                            .unwrap_or(0);
                        editor.cursor_col = editor.cursor_col.min(line_len);
                    }
                }
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                if !editor.has_selection {
                    editor.start_selection();
                }
                editor.move_cursor_down();
                editor.update_selection();
            }
            KeyCode::Down => {
                editor.clear_selection();
                if editor.cursor_line == 0 {
                    // Path -> Password.
                    editor.cursor_line = 1;
                    let pw_len = editor.content.lines().next().map(|l| l.len()).unwrap_or(0);
                    editor.cursor_col = editor.cursor_col.min(pw_len);
                } else if editor.cursor_line == 1 {
                    // Password -> first line of Additional Content. This
                    // must be unconditional: a new entry's content is
                    // "\n", whose lines().count() is 1, so a generic
                    // `cursor_line < total` check would wrongly make Down
                    // a no-op here. The box always has a first line, and
                    // insert_char pads content lines as needed.
                    editor.cursor_line = 2;
                    let first_len = editor.content.lines().nth(1).map(|l| l.len()).unwrap_or(0);
                    editor.cursor_col = editor.cursor_col.min(first_len);
                } else {
                    // Within Additional Content: the bottom line is
                    // `total`. Down from the bottom line is a no-op since
                    // nothing is below Additional Content.
                    let total = editor.content.lines().count();
                    if editor.cursor_line < total {
                        editor.cursor_line += 1;
                        let line_len = editor
                            .content
                            .lines()
                            .nth(editor.cursor_line - 1)
                            .map(|l| l.len())
                            .unwrap_or(0);
                        editor.cursor_col = editor.cursor_col.min(line_len);
                    }
                }
            }
            // Line start/end: Home/End everywhere, plus Ctrl+A/E (advertised
            // on Mac). Works on every field line, like Home/End before it.
            KeyCode::Home | KeyCode::Char('a')
                if accepts(Action::LineStart, key.code, key.modifiers) =>
            {
                editor.cursor_col = 0;
            }
            KeyCode::End | KeyCode::Char('e')
                if accepts(Action::LineEnd, key.code, key.modifiers) =>
            {
                let line_len = editor.current_line_len();
                editor.cursor_col = line_len;
            }
            // Charset cycling is active only while the Password field is
            // focused (cursor_line == 1). In Additional Content (lines 2+)
            // Tab inserts 4 spaces of indentation and Shift+Tab dedents
            // back to the previous tab stop; in the Path field Tab inserts
            // a literal tab (unchanged). Ctrl+g generates with these
            // settings from anywhere in the editor.
            KeyCode::Tab if editor.cursor_line == 1 => {
                editor.gen_charset = match editor.gen_charset.as_str() {
                    "all" => "alphanumeric",
                    "alphanumeric" => "letters",
                    "letters" => "custom",
                    // "custom" (and anything unknown) rejoins the cycle
                    // at "all".
                    _ => "all",
                }
                .to_string();
            }
            KeyCode::Tab if editor.cursor_line >= 2 => {
                editor.insert_indent();
            }
            // Some terminals report Shift+Tab as BackTab; dedent only in
            // Additional Content, matching the Tab indent arm above.
            KeyCode::BackTab if editor.cursor_line >= 2 => {
                editor.clear_selection();
                editor.dedent();
            }
            KeyCode::Tab => {
                editor.insert_char('\t');
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if editor.has_selection {
                    text_to_copy = editor.get_selected_text();
                    editor.clear_selection();
                }
            }
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                editor.clear_selection();
                if let Some(text) = App::paste_from_clipboard() {
                    for c in text.chars() {
                        if c == '\n' {
                            editor.insert_char('\n');
                        } else {
                            editor.insert_char(c);
                        }
                    }
                    app.message = Some(("Pasted from clipboard".to_string(), Instant::now()));
                } else {
                    app.message =
                        Some(("Clipboard empty or unavailable".to_string(), Instant::now()));
                }
            }
            // The Ctrl+U/D chords are matched by the guarded length arms
            // above only inside the Password field; swallow them elsewhere
            // so the text catch-all below never inserts their letter.
            KeyCode::Char('u') | KeyCode::Char('d')
                if key.modifiers.contains(KeyModifiers::CONTROL) => {}
            KeyCode::Char(c) => {
                editor.clear_selection();
                editor.insert_char(c);
            }
            _ => {}
        }
    }

    if let Some(text) = text_to_copy {
        app.copy_to_clipboard(&text);
        // Keep the "(Ns)" hint in sync with the configured timeout; when
        // auto-clear is disabled no countdown is shown.
        let msg = match app.clipboard_timeout() {
            Some(d) => format!("Selection copied to clipboard ({}s)", d.as_secs()),
            None => "Selection copied to clipboard".to_string(),
        };
        app.message = Some((msg, Instant::now()));
    }

    Ok(())
}

pub fn handle_git_mode(app: &mut App, key: KeyEvent) -> Result<()> {
    match app.git_pane {
        GitPaneState::Uninitialized => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                if let Err(e) = app.git_init() {
                    app.message = Some((format!("Git init failed: {}", e), Instant::now()));
                }
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                app.mode = AppMode::Normal;
            }
            _ => {}
        },
        GitPaneState::AskRemote => match key.code {
            KeyCode::Esc => {
                app.command_buffer.clear();
                app.git_pane = GitPaneState::Ready;
                app.refresh_git_info();
            }
            // Ctrl+v must be matched BEFORE the plain Char(c) arm: crossterm
            // reports it as Char('v') with CONTROL, and without this arm the
            // catch-all would insert a literal 'v' instead of pasting.
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(text) = App::paste_from_clipboard() {
                    // The remote URL field is single-line: strip newlines
                    // and other control characters from the paste, and trim
                    // surrounding whitespace (a trailing newline is common
                    // when copying from a terminal).
                    let cleaned: String = text.chars().filter(|c| !c.is_control()).collect();
                    let cleaned = cleaned.trim();
                    if !cleaned.is_empty() {
                        app.command_buffer.push_str(cleaned);
                    }
                } else {
                    app.message =
                        Some(("Clipboard empty or unavailable".to_string(), Instant::now()));
                }
            }
            KeyCode::Enter => {
                let url = app.command_buffer.trim().to_string();
                if !url.is_empty() {
                    if let Err(e) = app.git_add_remote(&url) {
                        app.message = Some((format!("Remote failed: {}", e), Instant::now()));
                    }
                }
                app.command_buffer.clear();
                app.git_pane = GitPaneState::Ready;
                app.refresh_git_info();
            }
            KeyCode::Char(c) => app.command_buffer.push(c),
            KeyCode::Backspace => {
                app.command_buffer.pop();
            }
            _ => {}
        },
        GitPaneState::AskIdentityName => match key.code {
            KeyCode::Enter => {
                let name = app.command_buffer.trim().to_string();
                app.command_buffer.clear();
                if name.is_empty() {
                    app.message = Some(("Name cannot be empty".to_string(), Instant::now()));
                } else {
                    app.git_identity_name = name;
                    app.git_pane = GitPaneState::AskIdentityEmail;
                }
            }
            KeyCode::Esc => app.skip_git_identity(),
            // Ctrl+v before the plain Char(c) arm, as in the other prompts:
            // the catch-all would otherwise insert a literal 'v'.
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(text) = App::paste_from_clipboard() {
                    // Single-line field: strip control characters and trim
                    // surrounding whitespace from the paste.
                    let cleaned: String = text.chars().filter(|c| !c.is_control()).collect();
                    let cleaned = cleaned.trim();
                    if !cleaned.is_empty() {
                        app.command_buffer.push_str(cleaned);
                    }
                } else {
                    app.message =
                        Some(("Clipboard empty or unavailable".to_string(), Instant::now()));
                }
            }
            KeyCode::Char(c) => app.command_buffer.push(c),
            KeyCode::Backspace => {
                app.command_buffer.pop();
            }
            _ => {}
        },
        GitPaneState::AskIdentityEmail => match key.code {
            KeyCode::Enter => {
                let email = app.command_buffer.trim().to_string();
                if email.is_empty() {
                    app.message = Some(("Email cannot be empty".to_string(), Instant::now()));
                } else {
                    app.finish_git_identity(email);
                }
            }
            KeyCode::Esc => app.skip_git_identity(),
            // Ctrl+v before the plain Char(c) arm, as in the other prompts:
            // the catch-all would otherwise insert a literal 'v'.
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(text) = App::paste_from_clipboard() {
                    // Single-line field: strip control characters and trim
                    // surrounding whitespace from the paste.
                    let cleaned: String = text.chars().filter(|c| !c.is_control()).collect();
                    let cleaned = cleaned.trim();
                    if !cleaned.is_empty() {
                        app.command_buffer.push_str(cleaned);
                    }
                } else {
                    app.message =
                        Some(("Clipboard empty or unavailable".to_string(), Instant::now()));
                }
            }
            KeyCode::Char(c) => app.command_buffer.push(c),
            KeyCode::Backspace => {
                app.command_buffer.pop();
            }
            _ => {}
        },
        GitPaneState::Ready => match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                app.mode = AppMode::Normal;
            }
            // Ctrl+p is the git pane's only push binding, matching the chord
            // Normal mode and the footer advertise. Must precede the plain
            // Char('p') pull arm (first matching arm wins, the file's
            // Ctrl-arm ordering rule).
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Err(e) = app.git_push() {
                    app.message = Some((format!("Error: {}", e), Instant::now()));
                }
            }
            KeyCode::Char('p') => {
                // Fetch, then confirm-or-merge: the shared flow (see
                // App::pull_with_confirm), also used by Normal mode's p.
                app.pull_with_confirm();
            }
            KeyCode::Char('r') => {
                app.refresh_git_info();
            }
            // Paging and top/bottom: PgUp/PgDn and Home/End everywhere,
            // plus b, Space, g and G (advertised on Mac). Git mode has no
            // text entry, so the printable chords cannot steal keystrokes.
            KeyCode::PageUp | KeyCode::Char('b')
                if accepts(Action::PageUp, key.code, key.modifiers) =>
            {
                app.git_scroll = app.git_scroll.saturating_sub(GIT_V_STEP);
            }
            KeyCode::PageDown | KeyCode::Char(' ')
                if accepts(Action::PageDown, key.code, key.modifiers) =>
            {
                app.git_scroll += GIT_V_STEP;
            }
            KeyCode::Home | KeyCode::Char('g') if accepts(Action::Top, key.code, key.modifiers) => {
                app.git_scroll = 0;
            }
            KeyCode::End | KeyCode::Char('G')
                if accepts(Action::Bottom, key.code, key.modifiers) =>
            {
                // draw_git clamps git_scroll against the log length; a
                // large sentinel jumps to the bottom without knowing the
                // pane height here (and leaves room for Shift+Down).
                app.git_scroll = usize::MAX / 2;
            }
            // Shift+Up/Down scrolls the LOG vertically only - the Status
            // and Sync sections are short and pinned - consistent with the
            // Shift+arrow navigation used elsewhere in the app.
            KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
                app.git_scroll = app.git_scroll.saturating_sub(GIT_V_STEP);
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                app.git_scroll += GIT_V_STEP;
            }
            // Shift+Left/Right scrolls the WHOLE PANE (Status + Sync + Log)
            // sideways, consistent with the Shift+arrow navigation used
            // elsewhere.
            KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
                app.git_h_scroll = app.git_h_scroll.saturating_sub(GIT_H_STEP);
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
                app.git_h_scroll += GIT_H_STEP;
            }
            _ => {}
        },
    }
    Ok(())
}

pub fn handle_confirm_delete(app: &mut App, code: KeyCode) -> Result<()> {
    // A pending pull reuses this y/n prompt (the same mechanism the delete
    // flow uses): y merges the already-fetched upstream (git_merge_upstream;
    // the fetch already ran when the prompt was raised), n/Esc leaves
    // everything untouched. Both return to pull_return_mode, the mode the
    // pull was asked from (the git pane or Normal mode).
    if app.pending_pull {
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                app.pending_pull = false;
                app.mode = app.pull_return_mode;
                if let Err(e) = app.git_merge_upstream() {
                    app.message = Some((format!("Error: {}", e), Instant::now()));
                }
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('q') => {
                app.pending_pull = false;
                app.mode = app.pull_return_mode;
                app.message = Some(("Pull cancelled".to_string(), Instant::now()));
            }
            _ => {}
        }
        return Ok(());
    }
    match code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            if let Some(idx) = app.entry_to_delete {
                let entry = &app.entries[app.filtered_entries[idx]];
                let rel_path = entry
                    .path
                    .strip_prefix(&app.store_path)
                    .unwrap_or(&entry.path)
                    .to_string_lossy()
                    .to_string();
                let rel_path = rel_path
                    .strip_suffix(".gpg")
                    .unwrap_or(&rel_path)
                    .to_string();
                // Only the removal argument is normalised: the tree node was
                // already identified above, and pass interpolates the path
                // it receives into its own commit message.
                let rel_path = normalize_remove_path(&rel_path).to_string();
                let is_dir = entry.is_dir;

                app.entry_to_delete = None;

                // Single files keep the pass rm path (its commit message
                // has no trailing slash); folders are removed directly
                // (see App::remove_folder), because pass's own rm commit
                // for a directory reads "Remove <path>/ from store." with
                // a trailing slash. remove_folder applies the alt-screen
                // repair itself: with_alt_repair's closure cannot capture
                // &mut app here.
                let result = if is_dir {
                    app.remove_folder(&rel_path)
                } else {
                    // pass rm invokes gpg, whose pinentry-curses leaves the
                    // terminal on the normal screen: repair the alt screen
                    // afterwards, like the other gpg paths.
                    app.with_alt_repair(true, || PassCommand::remove(&rel_path, is_dir))
                };
                if let Err(e) = result {
                    app.mode = AppMode::Normal;
                    app.message = Some((format!("{}: error - {}", rel_path, e), Instant::now()));
                } else {
                    app.mode = AppMode::Normal;
                    app.message = Some((format!("{} deleted", rel_path), Instant::now()));
                    let _ = app.reload_entries();
                }
            } else {
                app.entry_to_delete = None;
                app.mode = AppMode::Normal;
            }
        }
        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('q') => {
            // Report the store-relative path (like the other operation
            // messages: "Saved <path>", "Moved <a> → <b>"), not just the
            // display name, so deletion feedback stays consistent.
            let not_deleted_msg = if let Some(idx) = app.entry_to_delete {
                app.filtered_entries
                    .get(idx)
                    .and_then(|&filtered_idx| app.entries.get(filtered_idx))
                    .map(|e| {
                        let rel_path = e
                            .path
                            .strip_prefix(&app.store_path)
                            .unwrap_or(&e.path)
                            .to_string_lossy()
                            .to_string();
                        let rel_path = rel_path
                            .strip_suffix(".gpg")
                            .unwrap_or(&rel_path)
                            .to_string();
                        format!("{} not deleted", rel_path)
                    })
            } else {
                None
            };

            app.entry_to_delete = None;
            app.mode = AppMode::Normal;

            if let Some(msg) = not_deleted_msg {
                app.message = Some((msg, Instant::now()));
            } else {
                app.message = Some(("Delete cancelled".to_string(), Instant::now()));
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_add_folder_mode(app: &mut App, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Esc => {
            app.mode = AppMode::Normal;
            app.command_buffer.clear();
        }
        KeyCode::Enter => {
            if !app.command_buffer.is_empty() {
                let folder_name = app.command_buffer.clone();
                if let Err(e) = app.create_folder(&folder_name) {
                    app.message = Some((format!("Error creating folder: {}", e), Instant::now()));
                } else {
                    let _ = app.reload_entries();
                }
            }
            app.mode = AppMode::Normal;
            app.command_buffer.clear();
        }
        KeyCode::Char(c) => app.command_buffer.push(c),
        KeyCode::Backspace => {
            app.command_buffer.pop();
        }
        _ => {}
    }
    Ok(())
}

/// Strip a single trailing separator from a store-relative path for the
/// removal commit message: a folder's trailing "/" would read "Remove X/
/// from store.". Internal separators are kept, so nested paths stay
/// "Misc/Ebay". (Folders no longer go through pass rm - see
/// App::remove_folder - but the argument is normalised the same way for
/// both paths.)
fn normalize_remove_path(path: &str) -> &str {
    path.strip_suffix('/').unwrap_or(path)
}

/// Normalize a move destination to a store-relative path. Relative paths
/// pass through unchanged; an absolute path under `store_path` is trimmed
/// to its store-relative form. Returns `None` for an absolute path outside
/// the store, which the caller reports as an error.
fn normalize_dest(dest: &str, store_path: &std::path::Path) -> Option<String> {
    if dest.starts_with('/') {
        dest.strip_prefix(store_path.to_string_lossy().as_ref())
            .map(|rest| rest.trim_start_matches('/').to_string())
    } else {
        Some(dest.to_string())
    }
}

pub fn handle_move_mode(app: &mut App, code: KeyCode) -> Result<()> {
    let source = app.move_source.clone().unwrap_or_default();

    match code {
        KeyCode::Esc => {
            app.mode = AppMode::Normal;
            app.command_buffer.clear();
            app.move_source = None;
        }
        KeyCode::Enter => {
            if !app.command_buffer.is_empty() {
                let dest_raw = app.command_buffer.trim().trim_end_matches('/');

                if dest_raw.is_empty() {
                    app.message = Some(("Destination cannot be empty".to_string(), Instant::now()));
                    return Ok(());
                }

                if source == dest_raw {
                    app.message = Some((
                        "Source and destination are the same".to_string(),
                        Instant::now(),
                    ));
                    app.mode = AppMode::Normal;
                    app.command_buffer.clear();
                    app.move_source = None;
                    return Ok(());
                }

                // Normalize the destination before handing it to `pass mv`.
                // pass builds its destination as "$PREFIX/<dest>", so an
                // absolute dest would be recreated *inside* the store:
                // moving an entry to "/home/me/nested1" produced
                // ~/.password-store/home/me/nested1. Strip a leading store
                // prefix (e.g. a pasted absolute path into the store) and
                // reject any other absolute path.
                let dest: String = match normalize_dest(dest_raw, &app.store_path) {
                    Some(d) => d,
                    None => {
                        app.message = Some((
                            format!(
                                "Destination must be inside the password store ({})",
                                app.store_path.to_string_lossy()
                            ),
                            Instant::now(),
                        ));
                        app.mode = AppMode::Normal;
                        app.command_buffer.clear();
                        app.move_source = None;
                        return Ok(());
                    }
                };

                if dest.is_empty() {
                    app.message = Some((
                        "Destination cannot be the store root".to_string(),
                        Instant::now(),
                    ));
                    app.mode = AppMode::Normal;
                    app.command_buffer.clear();
                    app.move_source = None;
                    return Ok(());
                }

                let store_path = &app.store_path;
                let dest_with_gpg = if dest.ends_with(".gpg") {
                    store_path.join(&dest)
                } else {
                    store_path.join(format!("{}.gpg", dest))
                };

                if dest_with_gpg.exists() {
                    app.message = Some((format!("{} already exists", dest), Instant::now()));
                    app.mode = AppMode::Normal;
                    app.command_buffer.clear();
                    app.move_source = None;
                    return Ok(());
                }

                let parent = std::path::Path::new(&dest).parent();
                if let Some(p) = parent {
                    let parent_path = store_path.join(p);
                    if !parent_path.exists() {
                        app.message = Some((
                            format!("Creating new folder: {}", p.display()),
                            Instant::now(),
                        ));
                    }
                }

                let dest_path = store_path.join(&dest);
                let is_dest_dir = dest_path.is_dir();

                if let Err(e) = PassCommand::mv(&source, &dest, is_dest_dir) {
                    app.message = Some((format!("Move failed: {}", e), Instant::now()));
                } else {
                    let final_dest = if is_dest_dir {
                        format!(
                            "{}/{}",
                            dest,
                            std::path::Path::new(&source)
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or(&source)
                        )
                    } else {
                        dest.clone()
                    };
                    app.message =
                        Some((format!("Moved {} → {}", source, final_dest), Instant::now()));
                    let _ = app.reload_entries();
                }
            }
            app.mode = AppMode::Normal;
            app.command_buffer.clear();
            app.move_source = None;
        }
        KeyCode::Char(c) => app.command_buffer.push(c),
        KeyCode::Backspace => {
            app.command_buffer.pop();
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_confirm_save(app: &mut App, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Char('y') | KeyCode::Char('Y') => match app.save_entry(true) {
            Ok(true) => {
                app.editor = None;
                app.mode = AppMode::Normal;
            }
            Ok(false) => {
                app.mode = AppMode::Editor;
            }
            Err(e) => {
                app.message = Some((format!("Error: {}", e), Instant::now()));
                app.mode = AppMode::Editor;
            }
        },
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            // 'n' reverts the editor to its original state instead of
            // keeping the edits: restore the snapshotted path/content,
            // clear any selection, and put the cursor at the start of the
            // Additional Content area (cursor_line 2, col 0) so the user
            // keeps editing from the original. Esc keeps the edits and
            // just returns to the editor.
            if code == KeyCode::Esc {
                app.mode = AppMode::Editor;
            } else if let Some(editor) = app.editor.as_mut() {
                editor.path = editor.original_path.clone();
                editor.content = editor.original_content.clone();
                editor.clear_selection();
                editor.cursor_line = 2;
                editor.cursor_col = 0;
                app.mode = AppMode::Editor;
            } else {
                // No editor open (should not happen in this mode): fall
                // back to Normal.
                app.mode = AppMode::Normal;
            }
            app.message = Some(("Save cancelled".to_string(), Instant::now()));
        }
        _ => {}
    }
    Ok(())
}

/// Esc from the editor with unsaved changes: y saves and exits, n discards
/// and exits, Esc returns to the editor. The editor stays open (in
/// app.editor) for the whole prompt so both "save" and "keep editing" can
/// still act on it.
pub fn handle_confirm_discard(app: &mut App, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            match app.save_entry(true) {
                Ok(true) => {
                    app.editor = None;
                    app.mode = AppMode::Normal;
                }
                // Save refused (e.g. empty entry name): the message from
                // save_entry explains why; return to the editor to fix it.
                Ok(false) => {
                    app.mode = AppMode::Editor;
                }
                Err(e) => {
                    app.message = Some((format!("Error: {}", e), Instant::now()));
                    app.mode = AppMode::Editor;
                }
            }
        }
        KeyCode::Char('n') | KeyCode::Char('N') => {
            app.discard_editor_changes();
            app.mode = AppMode::Normal;
        }
        KeyCode::Esc => {
            // Keep editing: the editor is untouched, just leave the prompt.
            app.mode = AppMode::Editor;
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{EntryEditor, PassEntry};
    use std::path::Path;

    #[test]
    fn test_normalize_absolute_dest() {
        let store = Path::new("/home/user/.password-store");
        assert_eq!(
            normalize_dest("/home/user/.password-store/work/github", store),
            Some("work/github".to_string())
        );
    }

    #[test]
    fn test_normalize_relative_dest() {
        let store = Path::new("/home/user/.password-store");
        assert_eq!(
            normalize_dest("work/github", store),
            Some("work/github".to_string())
        );
    }

    #[test]
    fn test_normalize_nested_dest() {
        let store = Path::new("/home/user/.password-store");
        assert_eq!(
            normalize_dest(
                "/home/user/.password-store/work/deep/nested/folder/entry",
                store
            ),
            Some("work/deep/nested/folder/entry".to_string())
        );
    }

    #[test]
    fn test_normalize_remove_path_strips_trailing_separator() {
        // A folder path carries a trailing separator; the removal commit
        // message must not read "Remove VerifyEmpty/ from store.".
        assert_eq!(normalize_remove_path("VerifyEmpty/"), "VerifyEmpty");
        // Internal separators stay intact for nested folders.
        assert_eq!(normalize_remove_path("Misc/Ebay/"), "Misc/Ebay");
        // A path without a trailing separator passes through unchanged.
        assert_eq!(normalize_remove_path("Misc/Ebay"), "Misc/Ebay");
    }

    /// Serialize env mutation across tests: env vars are process-global,
    /// so parallel test threads would otherwise see each other's values.
    /// Delegates to testenv::lock so EnvGuard and pass's StoreDirGuard
    /// acquire the SAME process-global lock and exclude each other (a
    /// mid-`pass rm` test can no longer see another test's temp store).
    /// The lock is non-reentrant: never hold two guards at once.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::testenv::lock()
    }

    /// Override environment variables for the duration of a test,
    /// restoring the previous values on drop. Holds the shared
    /// environment lock (pass::env_lock) for its whole lifetime; the lock
    /// is non-reentrant, so never hold two guards at once.
    struct EnvGuard {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn set(vars: &[(&'static str, &std::ffi::OsStr)]) -> Self {
            let lock = env_lock();
            let saved = vars
                .iter()
                .map(|(k, _)| (*k, std::env::var_os(k)))
                .collect();
            for (k, v) in vars {
                std::env::set_var(k, v);
            }
            EnvGuard { saved, _lock: lock }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in &self.saved {
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    /// A minimal App for key-handling tests: no disk access, a fixed entry
    /// list, and every other field at its neutral value.
    fn test_app(names: &[&str]) -> App {
        let store = Path::new("/mujo-test-store");
        let entries: Vec<PassEntry> = names
            .iter()
            .map(|name| PassEntry::new(store.join(format!("{}.gpg", name)), store, 1))
            .collect();
        let filtered: Vec<usize> = (0..entries.len()).collect();
        App {
            store_path: store.to_path_buf(),
            entries,
            filtered_entries: filtered,
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
            config: crate::config::Config::default(),
            preview_scroll_offset: 0,
            preview_h_scroll: 0,
            preview_selected_line: None,
            git_identity_name: String::new(),
            git_pending_count: 0,
            last_sync: None,
            git_last_remote_update: None,
            new_store_stage: NewStoreStage::Confirm,
        }
    }

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn test_shift_uppercase_letters_jump_and_do_not_trigger_actions() {
        // One entry per jumped letter; 'alpha' is selected initially.
        let cases = [
            ('C', "charlie"),
            ('P', "personal"),
            ('Q', "quartz"),
            ('R', "romeo"),
        ];
        for (letter, expected) in cases {
            let mut app = test_app(&["alpha", "charlie", "personal", "quartz", "romeo"]);
            handle_normal_mode(&mut app, key(KeyCode::Char(letter), KeyModifiers::SHIFT)).unwrap();
            assert_eq!(
                app.entries[app.selected].display_name, expected,
                "Shift+{} must jump to '{}'",
                letter, expected
            );
            // None of the removed uppercase actions may fire.
            assert_eq!(app.mode, AppMode::Normal);
            assert!(!app.should_quit);
            assert!(
                app.git_output.is_empty(),
                "Shift+{} must not run git",
                letter
            );
            assert!(
                app.message.is_none(),
                "Shift+{} must not set a message",
                letter
            );
        }
    }

    #[test]
    fn test_ctrl_p_triggers_git_push() {
        // git push shells out to `pass git`: point PASSWORD_STORE_DIR at a
        // throwaway directory so the test can never touch a real store.
        let tmp = tempfile::TempDir::new().unwrap();
        let _env = EnvGuard::set(&[("PASSWORD_STORE_DIR", tmp.path().as_os_str())]);

        let mut app = test_app(&["alpha"]);
        handle_normal_mode(&mut app, key(KeyCode::Char('p'), KeyModifiers::CONTROL)).unwrap();

        // The store has no remote, so the push is blocked before git runs
        // and the Sync line carries the fix-it message instead of git's
        // misleading "'origin' does not appear to be a git repository".
        // No jump happened and the mode is unchanged.
        let sync = app.last_sync.expect("blocked push must record a Sync");
        assert_eq!(sync.action, "Push");
        assert!(!sync.success);
        // The Sync line carries the shortened reason; the full fix-it
        // text stays on the message bar.
        assert!(
            sync.detail.contains("no remote"),
            "error must name the missing remote: {}",
            sync.detail
        );
        assert!(
            app.message
                .as_ref()
                .map(|(m, _)| m.contains("no remote configured"))
                .unwrap_or(false),
            "message must keep the full fix-it wording: {:?}",
            app.message
        );
        assert_eq!(app.selected, 0);
        assert_eq!(app.mode, AppMode::Normal);
    }

    #[test]
    fn test_git_mode_push_on_ctrl_p() {
        // git push shells out to `pass git`: point PASSWORD_STORE_DIR at a
        // throwaway directory so the test can never touch a real store.
        let tmp = tempfile::TempDir::new().unwrap();
        let _env = EnvGuard::set(&[("PASSWORD_STORE_DIR", tmp.path().as_os_str())]);

        // Ctrl+p is the git pane's only push binding: the unadvertised
        // Shift+P alias was removed (Normal mode never had it either).
        let mut app = test_app(&["alpha"]);
        app.mode = AppMode::Git;
        app.git_pane = GitPaneState::Ready;
        handle_git_mode(&mut app, key(KeyCode::Char('p'), KeyModifiers::CONTROL)).unwrap();
        // The store has no remote, so the push is blocked before git
        // runs and the Sync line records why - proving the chord
        // reached the push path.
        let sync = app.last_sync.expect("Ctrl+p must record a Sync");
        assert!(!sync.success, "Ctrl+p must block push");
        assert!(sync.detail.contains("no remote"), "{}", sync.detail);
        assert_eq!(app.mode, AppMode::Git);
    }

    #[test]
    fn test_git_mode_shift_p_is_unbound() {
        let mut app = test_app(&["alpha"]);
        app.mode = AppMode::Git;
        app.git_pane = GitPaneState::Ready;
        handle_git_mode(&mut app, key(KeyCode::Char('P'), KeyModifiers::SHIFT)).unwrap();
        assert!(app.last_sync.is_none(), "Shift+P must not push");
        assert_eq!(app.mode, AppMode::Git);
    }

    #[test]
    fn test_git_mode_pull_confirm_flow() {
        // The pull path fetches before evaluating the confirm predicate, so
        // this needs a real repo with an upstream to fetch from: a diverged
        // clone makes p ask first, and n cancels without running git (the
        // fetch records no Sync, and neither does the cancelled pull).
        use std::process::Command;

        let tmp = tempfile::TempDir::new().unwrap();
        let _env = EnvGuard::set(&[
            ("PASSWORD_STORE_DIR", tmp.path().as_os_str()),
            ("HOME", tmp.path().as_os_str()),
            ("XDG_CONFIG_HOME", tmp.path().as_os_str()),
        ]);

        let run = |dir: &Path, args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };

        // Upstream bare repo, a first clone pushed with tracking, and a
        // second clone that commits and pushes: the first clone is behind.
        let bare = tempfile::TempDir::new().unwrap();
        run(bare.path(), &["init", "--bare"]);

        let clone1 = tempfile::TempDir::new().unwrap();
        run(
            clone1.path(),
            &["clone", bare.path().to_str().unwrap(), "."],
        );
        run(clone1.path(), &["config", "user.name", "Test"]);
        run(clone1.path(), &["config", "user.email", "test@example.com"]);
        std::fs::write(clone1.path().join("base.gpg"), "x").unwrap();
        run(clone1.path(), &["add", "base.gpg"]);
        run(clone1.path(), &["commit", "-m", "base"]);
        run(clone1.path(), &["push", "-u", "origin", "HEAD"]);

        let clone2 = tempfile::TempDir::new().unwrap();
        run(
            clone2.path(),
            &["clone", bare.path().to_str().unwrap(), "."],
        );
        run(clone2.path(), &["config", "user.name", "Test"]);
        run(clone2.path(), &["config", "user.email", "test@example.com"]);
        std::fs::write(clone2.path().join("remote.gpg"), "x").unwrap();
        run(clone2.path(), &["add", "remote.gpg"]);
        run(clone2.path(), &["commit", "-m", "remote change"]);
        run(clone2.path(), &["push"]);

        // The app's store is the first clone; a local commit diverges it.
        let mut app = test_app(&["alpha"]);
        app.store_path = clone1.path().to_path_buf();
        app.mode = AppMode::Git;
        app.git_pane = GitPaneState::Ready;
        std::fs::write(clone1.path().join("local.gpg"), "x").unwrap();
        run(clone1.path(), &["add", "local.gpg"]);
        run(clone1.path(), &["commit", "-m", "local change"]);

        handle_git_mode(&mut app, key(KeyCode::Char('p'), KeyModifiers::NONE)).unwrap();
        assert!(app.pending_pull, "diverged pull must ask for confirmation");
        assert_eq!(app.mode, AppMode::ConfirmDelete);

        handle_confirm_delete(&mut app, KeyCode::Char('n')).unwrap();
        assert!(!app.pending_pull);
        assert_eq!(app.mode, AppMode::Git);
        assert!(app.last_sync.is_none(), "cancelled pull must not run git");

        // The store is still diverged (the cancelled pull changed nothing):
        // a second pull, confirmed with y, merges and returns to the git
        // pane - the mode the pull was asked from.
        handle_git_mode(&mut app, key(KeyCode::Char('p'), KeyModifiers::NONE)).unwrap();
        assert!(app.pending_pull, "still-diverged pull must confirm again");
        handle_confirm_delete(&mut app, KeyCode::Char('y')).unwrap();
        assert!(!app.pending_pull);
        assert_eq!(
            app.mode,
            AppMode::Git,
            "confirming from the git pane must return to the git pane"
        );
    }

    #[test]
    fn test_normal_mode_pull_confirm_flow_returns_to_normal() {
        // Normal mode's p runs the same fetch-then-confirm flow as the git
        // pane's p: a diverged store raises the confirmation instead of
        // merging silently, and confirming returns to Normal mode (not the
        // git pane).
        use std::process::Command;

        let tmp = tempfile::TempDir::new().unwrap();
        let _env = EnvGuard::set(&[
            ("PASSWORD_STORE_DIR", tmp.path().as_os_str()),
            ("HOME", tmp.path().as_os_str()),
            ("XDG_CONFIG_HOME", tmp.path().as_os_str()),
        ]);

        let run = |dir: &Path, args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };

        // Upstream bare repo, a first clone pushed with tracking, and a
        // second clone that commits and pushes: the first clone is behind.
        let bare = tempfile::TempDir::new().unwrap();
        run(bare.path(), &["init", "--bare"]);

        let clone1 = tempfile::TempDir::new().unwrap();
        run(
            clone1.path(),
            &["clone", bare.path().to_str().unwrap(), "."],
        );
        run(clone1.path(), &["config", "user.name", "Test"]);
        run(clone1.path(), &["config", "user.email", "test@example.com"]);
        std::fs::write(clone1.path().join("base.gpg"), "x").unwrap();
        run(clone1.path(), &["add", "base.gpg"]);
        run(clone1.path(), &["commit", "-m", "base"]);
        run(clone1.path(), &["push", "-u", "origin", "HEAD"]);

        let clone2 = tempfile::TempDir::new().unwrap();
        run(
            clone2.path(),
            &["clone", bare.path().to_str().unwrap(), "."],
        );
        run(clone2.path(), &["config", "user.name", "Test"]);
        run(clone2.path(), &["config", "user.email", "test@example.com"]);
        std::fs::write(clone2.path().join("remote.gpg"), "x").unwrap();
        run(clone2.path(), &["add", "remote.gpg"]);
        run(clone2.path(), &["commit", "-m", "remote change"]);
        run(clone2.path(), &["push"]);

        // The app's store is the first clone, diverged by a local commit.
        // The mode stays Normal: the pull is asked from the tree view.
        let mut app = test_app(&["alpha"]);
        app.store_path = clone1.path().to_path_buf();
        std::fs::write(clone1.path().join("local.gpg"), "x").unwrap();
        run(clone1.path(), &["add", "local.gpg"]);
        run(clone1.path(), &["commit", "-m", "local change"]);

        handle_normal_mode(&mut app, key(KeyCode::Char('p'), KeyModifiers::NONE)).unwrap();
        assert!(app.pending_pull, "diverged pull must ask for confirmation");
        assert_eq!(app.mode, AppMode::ConfirmDelete);

        handle_confirm_delete(&mut app, KeyCode::Char('y')).unwrap();
        assert!(!app.pending_pull);
        assert_eq!(
            app.mode,
            AppMode::Normal,
            "confirming from Normal must return to Normal"
        );
    }

    #[test]
    fn test_ctrl_r_triggers_config_reload() {
        // reload_config reads (and creates) the config file: point HOME and
        // XDG_CONFIG_HOME at a temp dir so the real config is never touched.
        let tmp = tempfile::TempDir::new().unwrap();
        let _env = EnvGuard::set(&[
            ("HOME", tmp.path().as_os_str()),
            ("XDG_CONFIG_HOME", tmp.path().as_os_str()),
        ]);

        let mut app = test_app(&["alpha"]);
        handle_normal_mode(&mut app, key(KeyCode::Char('r'), KeyModifiers::CONTROL)).unwrap();

        // reload_config always reports its outcome as a message.
        assert!(app.message.is_some());
        assert_eq!(app.mode, AppMode::Normal);
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn test_q_and_ctrl_q_quit() {
        let mut app = test_app(&["alpha"]);
        handle_normal_mode(&mut app, key(KeyCode::Char('q'), KeyModifiers::NONE)).unwrap();
        assert!(app.should_quit);

        let mut app = test_app(&["alpha"]);
        handle_normal_mode(&mut app, key(KeyCode::Char('q'), KeyModifiers::CONTROL)).unwrap();
        assert!(app.should_quit);
    }

    #[test]
    fn test_question_mark_opens_help() {
        // Both plain '?' and Shift-qualified '?' (how US keyboards send it)
        // must open the help view with a fresh scroll offset.
        for mods in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
            let mut app = test_app(&["alpha"]);
            handle_normal_mode(&mut app, key(KeyCode::Char('?'), mods)).unwrap();
            assert_eq!(app.mode, AppMode::Help);
            assert_eq!(app.help_scroll_offset, 0);
        }
    }

    #[test]
    fn test_help_mode_close_keys() {
        for code in [KeyCode::Esc, KeyCode::Char('q'), KeyCode::Char('?')] {
            let mut app = test_app(&["alpha"]);
            app.mode = AppMode::Help;
            app.help_scroll_offset = 7;
            handle_help_mode(&mut app, key(code, KeyModifiers::NONE)).unwrap();
            assert_eq!(app.mode, AppMode::Normal, "{code:?} must close help");
            // Reopening resets the scroll offset.
            handle_normal_mode(&mut app, key(KeyCode::Char('?'), KeyModifiers::NONE)).unwrap();
            assert_eq!(app.mode, AppMode::Help);
            assert_eq!(app.help_scroll_offset, 0);
        }
    }

    #[test]
    fn test_help_mode_scrolls() {
        let mut app = test_app(&["alpha"]);
        app.mode = AppMode::Help;
        handle_help_mode(&mut app, key(KeyCode::Down, KeyModifiers::SHIFT)).unwrap();
        assert_eq!(app.help_scroll_offset, HELP_V_STEP);
        handle_help_mode(&mut app, key(KeyCode::Up, KeyModifiers::SHIFT)).unwrap();
        assert_eq!(app.help_scroll_offset, 0);
        handle_help_mode(&mut app, key(KeyCode::PageDown, KeyModifiers::NONE)).unwrap();
        assert_eq!(app.help_scroll_offset, HELP_V_STEP);
        handle_help_mode(&mut app, key(KeyCode::PageUp, KeyModifiers::NONE)).unwrap();
        assert_eq!(app.help_scroll_offset, 0);
    }

    #[test]
    fn test_help_mode_scrolls_by_one_line() {
        let mut app = test_app(&["alpha"]);
        app.mode = AppMode::Help;
        handle_help_mode(&mut app, key(KeyCode::Down, KeyModifiers::NONE)).unwrap();
        assert_eq!(app.help_scroll_offset, 1);
        handle_help_mode(&mut app, key(KeyCode::Char('j'), KeyModifiers::NONE)).unwrap();
        assert_eq!(app.help_scroll_offset, 2);
        handle_help_mode(&mut app, key(KeyCode::Up, KeyModifiers::NONE)).unwrap();
        assert_eq!(app.help_scroll_offset, 1);
        handle_help_mode(&mut app, key(KeyCode::Char('k'), KeyModifiers::NONE)).unwrap();
        assert_eq!(app.help_scroll_offset, 0);
    }

    #[test]
    fn test_ctrl_slash_and_f1_open_help_from_normal() {
        // Ctrl+/ transmits 0x1F, which crossterm reports as Char('7') with
        // CONTROL; F1 is the live-but-unadvertised universal alternative.
        let mut app = test_app(&["alpha"]);
        assert!(maybe_open_help(
            &mut app,
            &key(KeyCode::Char('7'), KeyModifiers::CONTROL)
        ));
        assert_eq!(app.mode, AppMode::Help);
        assert_eq!(app.help_return_mode, AppMode::Normal);

        let mut app = test_app(&["alpha"]);
        assert!(maybe_open_help(
            &mut app,
            &key(KeyCode::F(1), KeyModifiers::NONE)
        ));
        assert_eq!(app.mode, AppMode::Help);
    }

    #[test]
    fn test_help_from_editor_and_question_mark_stays_printable() {
        // Ctrl+/ opens help even from the Editor, recording Editor as the
        // return mode.
        let mut app = test_app(&["alpha"]);
        app.mode = AppMode::Editor;
        app.editor = Some(EntryEditor::new("work/"));
        assert!(maybe_open_help(
            &mut app,
            &key(KeyCode::Char('_'), KeyModifiers::CONTROL)
        ));
        assert_eq!(app.mode, AppMode::Help);
        assert_eq!(app.help_return_mode, AppMode::Editor);

        // "?" in the Editor is a printable character: maybe_open_help
        // leaves it alone and the editor handler inserts it into the
        // Additional Content field.
        let mut app = test_app(&["alpha"]);
        app.mode = AppMode::Editor;
        app.editor = Some(EntryEditor::new("work/"));
        app.editor.as_mut().unwrap().cursor_line = 2;
        assert!(!maybe_open_help(
            &mut app,
            &key(KeyCode::Char('?'), KeyModifiers::NONE)
        ));
        assert_eq!(app.mode, AppMode::Editor);
        handle_editor_mode(&mut app, key(KeyCode::Char('?'), KeyModifiers::NONE)).unwrap();
        assert!(
            app.editor.as_ref().unwrap().content.contains('?'),
            "'?' must be inserted into the editor content"
        );
    }

    #[test]
    fn test_closing_help_returns_to_previous_mode() {
        // Every close chord lands back in the mode help was opened from -
        // the Editor here, not a hardcoded Normal.
        for (code, mods) in [
            (KeyCode::Esc, KeyModifiers::NONE),
            (KeyCode::Char('q'), KeyModifiers::NONE),
            (KeyCode::Char('?'), KeyModifiers::NONE),
            (KeyCode::Char('7'), KeyModifiers::CONTROL),
        ] {
            let mut app = test_app(&["alpha"]);
            app.mode = AppMode::Editor;
            app.editor = Some(EntryEditor::new("work/"));
            open_help(&mut app);
            assert_eq!(app.mode, AppMode::Help);
            handle_help_mode(&mut app, key(code, mods)).unwrap();
            assert_eq!(
                app.mode,
                AppMode::Editor,
                "{code:?} must restore the editor"
            );
        }

        // From Normal the return mode is Normal.
        let mut app = test_app(&["alpha"]);
        open_help(&mut app);
        handle_help_mode(&mut app, key(KeyCode::Esc, KeyModifiers::NONE)).unwrap();
        assert_eq!(app.mode, AppMode::Normal);
    }

    /// Removing a folder must commit "Remove <path> from store." with no
    /// trailing slash: pass's own rm for a directory commits "Remove
    /// <path>/ from store.", so folders bypass pass rm entirely (see
    /// App::remove_folder).
    #[test]
    fn test_folder_removal_commit_message_has_no_trailing_slash() {
        use std::process::Command;

        let tmp = tempfile::TempDir::new().unwrap();
        let _env = EnvGuard::set(&[
            ("PASSWORD_STORE_DIR", tmp.path().as_os_str()),
            ("HOME", tmp.path().as_os_str()),
            ("XDG_CONFIG_HOME", tmp.path().as_os_str()),
        ]);

        let mut app = App::new().unwrap();
        std::fs::create_dir_all(app.store_path.join("temptest")).unwrap();
        std::fs::write(app.store_path.join("temptest").join("entry.gpg"), "x").unwrap();

        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&app.store_path)
                .output()
                .unwrap();
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
        run(&["add", "-A"]);
        run(&["commit", "-m", "initial"]);

        app.load_entries().unwrap();
        app.rebuild_filtered();
        let idx = app
            .entries
            .iter()
            .position(|e| e.is_dir && e.display_name == "temptest")
            .expect("temptest folder in tree");
        app.filtered_entries = vec![idx];
        app.selected = 0;
        app.entry_to_delete = Some(0);
        app.mode = AppMode::ConfirmDelete;

        handle_confirm_delete(&mut app, KeyCode::Char('y')).unwrap();

        assert!(
            !app.store_path.join("temptest").exists(),
            "folder must be gone from disk"
        );
        assert_eq!(app.mode, AppMode::Normal);
        let out = Command::new("git")
            .args(["log", "-1", "--format=%s"])
            .current_dir(&app.store_path)
            .output()
            .unwrap();
        let subject = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert_eq!(subject, "Remove temptest from store.");
        assert!(!subject.ends_with('/'), "no trailing slash: {subject}");
    }
}

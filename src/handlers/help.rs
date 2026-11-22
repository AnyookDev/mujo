use crate::app::App;
use crate::keys::{accepts, Action};
use crate::models::AppMode;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::HELP_V_STEP;

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

/// How far one page step moves: the visible body height less one line, so
/// a line of context carries over between pages. At least one line, so
/// paging still moves before the first draw has recorded a height.
fn help_page(app: &App) -> usize {
    app.help_body_height.saturating_sub(1).max(1)
}

/// Full-screen help view: lists every binding grouped by pane. Esc, q, ?
/// and Ctrl+/ all close it and return to the mode help was opened from
/// (help_return_mode), NOT a hardcoded Normal. Up/Down and k/j scroll one
/// line. Shift+Up/Down scrolls HELP_V_STEP lines. PgUp/PgDn (and b/Space)
/// scroll a page, the visible body height.
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
            app.help_scroll_offset = app.help_scroll_offset.saturating_sub(help_page(app));
        }
        KeyCode::PageDown | KeyCode::Char(' ')
            if accepts(Action::PageDown, key.code, key.modifiers) =>
        {
            app.help_scroll_offset += help_page(app);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::normal::tests::app_with_entries;
    use crossterm::event::{KeyEventKind, KeyEventState};

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn help_app(body_height: usize) -> App {
        let mut app = app_with_entries(&["alice"]);
        app.mode = AppMode::Help;
        app.help_body_height = body_height;
        app
    }

    #[test]
    fn page_down_and_up_move_by_body_height_less_one() {
        let mut app = help_app(20);
        handle_help_mode(&mut app, key(KeyCode::PageDown, KeyModifiers::NONE)).unwrap();
        assert_eq!(app.help_scroll_offset, 19);
        handle_help_mode(&mut app, key(KeyCode::PageUp, KeyModifiers::NONE)).unwrap();
        assert_eq!(app.help_scroll_offset, 0);
    }

    #[test]
    fn space_and_b_page_like_pgdn_and_pgup() {
        let mut app = help_app(10);
        handle_help_mode(&mut app, key(KeyCode::Char(' '), KeyModifiers::NONE)).unwrap();
        assert_eq!(app.help_scroll_offset, 9);
        handle_help_mode(&mut app, key(KeyCode::Char('b'), KeyModifiers::NONE)).unwrap();
        assert_eq!(app.help_scroll_offset, 0);
    }

    #[test]
    fn shift_arrows_still_move_by_three_lines() {
        let mut app = help_app(20);
        handle_help_mode(&mut app, key(KeyCode::Down, KeyModifiers::SHIFT)).unwrap();
        assert_eq!(app.help_scroll_offset, HELP_V_STEP);
    }

    #[test]
    fn page_step_is_at_least_one_line_before_first_draw() {
        let mut app = help_app(0);
        handle_help_mode(&mut app, key(KeyCode::PageDown, KeyModifiers::NONE)).unwrap();
        assert_eq!(app.help_scroll_offset, 1);
    }
}

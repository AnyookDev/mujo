use crate::app::App;
use crate::models::AppMode;
use anyhow::Result;
use crossterm::event::KeyCode;

use super::TREE_JUMP;

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

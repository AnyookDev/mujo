use crate::app::App;
use crate::keys::{accepts, Action};
use crate::models::{AppMode, GitPaneState};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Instant;

use super::{GIT_H_STEP, GIT_V_STEP};

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

use crate::app::App;
use crate::models::{AppMode, NewStoreStage};
use crate::pass::PassCommand;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Instant;

use super::char_to_byte_idx;

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

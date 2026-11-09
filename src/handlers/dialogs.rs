use crate::app::App;
use crate::models::AppMode;
use crate::pass::PassCommand;
use anyhow::Result;
use crossterm::event::KeyCode;
use std::time::Instant;

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
        assert_eq!(normalize_remove_path("VerifyEmpty/"), "VerifyEmpty");
        assert_eq!(normalize_remove_path("Misc/Ebay/"), "Misc/Ebay");
        assert_eq!(normalize_remove_path("Misc/Ebay"), "Misc/Ebay");
    }
}

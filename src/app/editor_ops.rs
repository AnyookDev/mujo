use super::App;
use crate::models::{AppMode, DecryptedContent, EntryEditor};
use crate::pass::PassCommand;
use anyhow::Result;
use std::time::Instant;

impl App {
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
}

use super::App;
use crate::models::{AppMode, ContextMenu, MenuAction, MenuItem};
use std::time::Instant;

impl App {
    pub fn show_context_menu(&mut self, x: u16, y: u16) {
        // "Decrypted" must mean the entry UNDER THE CURSOR is the one whose
        // content is in memory - not merely that some earlier entry was
        // decrypted. Right-clicking a different entry moves the selection
        // without clearing the preview, so comparing current_entry_path
        // against the selected entry's path is what keeps the menu (and its
        // actions) tied to the newly right-clicked file rather than the
        // previously decrypted one.
        let selected_rel = self.get_selected_rel_path();
        let is_decrypted = self.current_content.is_some()
            && self.current_entry_path.is_some()
            && self.current_entry_path == selected_rel;

        let items = if let Some(&idx) = self.filtered_entries.get(self.selected) {
            let entry = &self.entries[idx];
            if entry.is_dir {
                vec![
                    MenuItem {
                        label: "Expand".to_string(),
                        action: MenuAction::Expand,
                        enabled: !entry.expanded,
                    },
                    MenuItem {
                        label: "Collapse".to_string(),
                        action: MenuAction::Collapse,
                        enabled: entry.expanded,
                    },
                    MenuItem {
                        label: "New entry here".to_string(),
                        action: MenuAction::New,
                        enabled: true,
                    },
                    MenuItem {
                        label: "Add folder".to_string(),
                        action: MenuAction::AddFolder,
                        enabled: true,
                    },
                ]
            } else if is_decrypted {
                // Decrypted file: re-decrypting achieves nothing, so no
                // Open/Decrypt. Copy password and Show QR work because the
                // content is already in memory.
                vec![
                    MenuItem {
                        label: "Edit".to_string(),
                        action: MenuAction::Edit,
                        enabled: true,
                    },
                    MenuItem {
                        label: "Reveal/Hide".to_string(),
                        action: MenuAction::Reveal,
                        enabled: true,
                    },
                    MenuItem {
                        label: "Copy password".to_string(),
                        action: MenuAction::CopyPassword,
                        enabled: true,
                    },
                    MenuItem {
                        label: "Show QR".to_string(),
                        action: MenuAction::ShowQr,
                        enabled: true,
                    },
                    MenuItem {
                        label: "Move".to_string(),
                        action: MenuAction::Move,
                        enabled: true,
                    },
                    MenuItem {
                        label: "Delete".to_string(),
                        action: MenuAction::Delete,
                        enabled: true,
                    },
                ]
            } else {
                // Not yet decrypted: Copy password cannot work (nothing is
                // in memory to copy), so it is not offered.
                vec![
                    MenuItem {
                        label: "Open/Decrypt".to_string(),
                        action: MenuAction::Open,
                        enabled: true,
                    },
                    MenuItem {
                        label: "Edit".to_string(),
                        action: MenuAction::Edit,
                        enabled: true,
                    },
                    MenuItem {
                        label: "Move".to_string(),
                        action: MenuAction::Move,
                        enabled: true,
                    },
                    MenuItem {
                        label: "Delete".to_string(),
                        action: MenuAction::Delete,
                        enabled: true,
                    },
                ]
            }
        } else {
            vec![
                MenuItem {
                    label: "New entry".to_string(),
                    action: MenuAction::New,
                    enabled: true,
                },
                MenuItem {
                    label: "Add folder".to_string(),
                    action: MenuAction::AddFolder,
                    enabled: true,
                },
            ]
        };

        self.context_menu = Some(ContextMenu {
            x,
            y,
            items,
            selected: 0,
        });
        self.mode = AppMode::ContextMenu;
    }

    pub fn execute_menu_action(&mut self, action: MenuAction) {
        self.context_menu = None;
        self.mode = AppMode::Normal;

        match action {
            MenuAction::Open => {
                if let Err(e) = self.decrypt_current_entry(true) {
                    self.message = Some((format!("Error: {}", e), Instant::now()));
                }
            }
            MenuAction::Edit => self.open_editor(true),
            MenuAction::New => self.open_editor(false),
            MenuAction::Delete => {
                if self.filtered_entries.get(self.selected).is_some() {
                    self.entry_to_delete = Some(self.selected);
                    self.mode = AppMode::ConfirmDelete;
                }
            }
            MenuAction::CopyPassword => {
                if let Some(password) = self
                    .current_content
                    .as_ref()
                    .and_then(|c| c.lines.first().cloned())
                {
                    self.copy_to_clipboard(&password);
                }
            }
            MenuAction::Reveal => self.show_password = !self.show_password,
            MenuAction::ShowQr => self.show_qr = true,
            MenuAction::Expand => self.toggle_folder(),
            MenuAction::Collapse => self.toggle_folder(),
            MenuAction::AddFolder => {
                self.mode = AppMode::AddFolder;
                self.command_buffer.clear();
            }
            MenuAction::Move => {
                if let Some(source) = self.get_selected_rel_path() {
                    self.move_source = Some(source);
                    self.mode = AppMode::Move;
                    self.command_buffer.clear();
                }
            }
            MenuAction::GitPush => {
                if let Err(e) = self.git_push() {
                    self.message = Some((format!("Error: {}", e), Instant::now()));
                }
            }
            MenuAction::GitPull => {
                if let Err(e) = self.git_pull() {
                    self.message = Some((format!("Error: {}", e), Instant::now()));
                }
            }
            _ => {}
        }
    }
}

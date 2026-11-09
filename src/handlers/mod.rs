mod dialogs;
mod editor;
mod git;
mod help;
mod new_store;
mod normal;
mod search;

pub use dialogs::{
    handle_add_folder_mode, handle_confirm_delete, handle_confirm_discard, handle_confirm_save,
    handle_move_mode,
};
pub use editor::{
    handle_editor_double_click, handle_editor_mode, handle_editor_mouse,
    handle_editor_triple_click,
};
pub use git::handle_git_mode;
pub use help::{handle_help_mode, maybe_open_help, open_help};
pub use new_store::handle_new_store_mode;
pub use normal::handle_normal_mode;
pub use search::handle_search_mode;

pub(crate) const TREE_JUMP: i32 = 5;
pub(crate) const PREVIEW_H_STEP: usize = 4;
pub(crate) const PREVIEW_V_STEP: usize = 3;
pub(crate) const GIT_H_STEP: usize = 4;
pub(crate) const GIT_V_STEP: usize = 3;
pub(crate) const HELP_V_STEP: usize = 3;

/// Byte index of the `char_idx`-th character of `s` (multi-byte safe).
/// Clamps to the string length, so a cursor at or past the end maps to
/// `s.len()`.
pub(crate) fn char_to_byte_idx(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

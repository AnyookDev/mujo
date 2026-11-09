mod editor;
mod entry;
mod menu;
mod sync;
mod totp;

pub use editor::EntryEditor;
pub use entry::{DecryptedContent, PassEntry};
pub use menu::{ContextMenu, MenuAction, MenuItem};
pub use sync::SyncStatus;
pub use totp::Totp;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum AppMode {
    Normal,
    Search,
    Editor,
    Git,
    Help,
    ConfirmDelete,
    ConfirmSave,
    ConfirmDiscard,
    AddFolder,
    Move,
    ContextMenu,
    NewStore,
}

/// Stages of the first-run "New Password Store" setup pane, shown when no
/// store directory exists.
#[derive(Clone, Copy, PartialEq)]
pub enum NewStoreStage {
    /// Ask whether to set up a pass password store.
    Confirm,
    /// Ask for the GPG ID to initialize the store with.
    AskGpgId,
}

#[derive(Clone, Copy, PartialEq)]
pub enum GitPaneState {
    Uninitialized,
    AskRemote,
    AskIdentityName,
    AskIdentityEmail,
    Ready,
}

#[derive(Clone, Copy)]
pub enum ClipboardTool {
    WlCopy,
    Pbcopy,
    Xclip,
}

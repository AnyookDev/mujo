#[derive(Clone)]
pub struct ContextMenu {
    pub x: u16,
    pub y: u16,
    pub items: Vec<MenuItem>,
    pub selected: usize,
}

#[derive(Clone)]
pub struct MenuItem {
    pub label: String,
    pub action: MenuAction,
    pub enabled: bool,
}

#[derive(Clone)]
pub enum MenuAction {
    Open,
    Edit,
    New,
    Delete,
    CopyPassword,
    Reveal,
    ShowQr,
    Expand,
    Collapse,
    AddFolder,
    Move,
    GitPush,
    GitPull,
    Cancel,
}

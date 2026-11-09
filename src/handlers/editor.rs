use crate::app::App;
use crate::keys::{accepts, Action};
use crate::models::AppMode;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use std::time::Instant;

pub fn handle_editor_mode(app: &mut App, key: KeyEvent) -> Result<()> {
    use crossterm::event::KeyModifiers;

    // Ctrl+g generates a password with the editor's current Length/Charset
    // settings and replaces the Password field in place. No mode change:
    // the editor stays open and Ctrl+g can be pressed again to re-roll.
    if key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.generate_password_in_editor();
        return Ok(());
    }

    let mut text_to_copy: Option<String> = None;

    if let Some(ref mut editor) = app.editor {
        match key.code {
            KeyCode::Esc => {
                if app.has_unsaved_changes() {
                    // Unsaved work must not be silently discarded: ask
                    // whether to save, discard, or keep editing.
                    app.mode = AppMode::ConfirmDiscard;
                } else {
                    app.editor = None;
                    app.mode = AppMode::Normal;
                }
            }
            KeyCode::Enter => {
                editor.insert_char('\n');
            }
            KeyCode::Backspace => {
                editor.backspace();
            }
            KeyCode::Delete => {
                editor.delete();
            }
            KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
                if !editor.has_selection {
                    editor.start_selection();
                }
                editor.move_cursor_left();
                editor.update_selection();
            }
            KeyCode::Left => {
                editor.clear_selection();
                editor.move_cursor_left();
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
                if !editor.has_selection {
                    editor.start_selection();
                }
                editor.move_cursor_right();
                editor.update_selection();
            }
            KeyCode::Right => {
                editor.clear_selection();
                editor.move_cursor_right();
            }
            // Password generation controls, active only while the Password
            // field is focused (cursor_line == 1) so plain arrows keep
            // working as field navigation in the path and content fields.
            // Ctrl+Up increases and Ctrl+Down decreases the generated
            // password length; these arms must come before the plain
            // Up/Down arms, which carry no modifier guard.
            // Generated password length: Ctrl+Up/Down everywhere, plus
            // Ctrl+U/D (advertised on Mac). Same action enum as the tree
            // jump, different effect per pane. Active only while the
            // Password field is focused, like before.
            KeyCode::Up | KeyCode::Char('u')
                if editor.cursor_line == 1 && accepts(Action::JumpUp, key.code, key.modifiers) =>
            {
                editor.gen_length += 1;
            }
            KeyCode::Down | KeyCode::Char('d')
                if editor.cursor_line == 1
                    && accepts(Action::JumpDown, key.code, key.modifiers) =>
            {
                editor.gen_length = editor.gen_length.saturating_sub(1).max(8);
            }
            KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
                if !editor.has_selection {
                    editor.start_selection();
                }
                editor.move_cursor_up();
                editor.update_selection();
            }
            // Explicit field navigation (Path -> Password -> Additional
            // Content). Backspace never changes fields; only Up/Down do.
            KeyCode::Up => {
                editor.clear_selection();
                match editor.cursor_line {
                    // Path is the topmost field: stay.
                    0 => {}
                    // Password -> Path, but only for new entries: for an
                    // existing entry the Path field is locked (read-only),
                    // so it cannot be focused and Up stays on Password.
                    1 if editor.is_new => {
                        editor.cursor_line = 0;
                        editor.cursor_col = editor.path.len();
                    }
                    // Password of an existing entry: Path is locked, stay.
                    1 => {}
                    // Top line of Additional Content -> Password.
                    2 => {
                        editor.cursor_line = 1;
                        let pw_len = editor.content.lines().next().map(|l| l.len()).unwrap_or(0);
                        editor.cursor_col = editor.cursor_col.min(pw_len);
                    }
                    // Within Additional Content: previous line, column clamped.
                    _ => {
                        editor.cursor_line -= 1;
                        let line_len = editor
                            .content
                            .lines()
                            .nth(editor.cursor_line - 1)
                            .map(|l| l.len())
                            .unwrap_or(0);
                        editor.cursor_col = editor.cursor_col.min(line_len);
                    }
                }
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                if !editor.has_selection {
                    editor.start_selection();
                }
                editor.move_cursor_down();
                editor.update_selection();
            }
            KeyCode::Down => {
                editor.clear_selection();
                if editor.cursor_line == 0 {
                    // Path -> Password.
                    editor.cursor_line = 1;
                    let pw_len = editor.content.lines().next().map(|l| l.len()).unwrap_or(0);
                    editor.cursor_col = editor.cursor_col.min(pw_len);
                } else if editor.cursor_line == 1 {
                    // Password -> first line of Additional Content. This
                    // must be unconditional: a new entry's content is
                    // "\n", whose lines().count() is 1, so a generic
                    // `cursor_line < total` check would wrongly make Down
                    // a no-op here. The box always has a first line, and
                    // insert_char pads content lines as needed.
                    editor.cursor_line = 2;
                    let first_len = editor.content.lines().nth(1).map(|l| l.len()).unwrap_or(0);
                    editor.cursor_col = editor.cursor_col.min(first_len);
                } else {
                    // Within Additional Content: the bottom line is
                    // `total`. Down from the bottom line is a no-op since
                    // nothing is below Additional Content.
                    let total = editor.content.lines().count();
                    if editor.cursor_line < total {
                        editor.cursor_line += 1;
                        let line_len = editor
                            .content
                            .lines()
                            .nth(editor.cursor_line - 1)
                            .map(|l| l.len())
                            .unwrap_or(0);
                        editor.cursor_col = editor.cursor_col.min(line_len);
                    }
                }
            }
            // Line start/end: Home/End everywhere, plus Ctrl+A/E (advertised
            // on Mac). Works on every field line, like Home/End before it.
            KeyCode::Home | KeyCode::Char('a')
                if accepts(Action::LineStart, key.code, key.modifiers) =>
            {
                editor.cursor_col = 0;
            }
            KeyCode::End | KeyCode::Char('e')
                if accepts(Action::LineEnd, key.code, key.modifiers) =>
            {
                let line_len = editor.current_line_len();
                editor.cursor_col = line_len;
            }
            // Charset cycling is active only while the Password field is
            // focused (cursor_line == 1). In Additional Content (lines 2+)
            // Tab inserts 4 spaces of indentation and Shift+Tab dedents
            // back to the previous tab stop; in the Path field Tab inserts
            // a literal tab (unchanged). Ctrl+g generates with these
            // settings from anywhere in the editor.
            KeyCode::Tab if editor.cursor_line == 1 => {
                editor.gen_charset = match editor.gen_charset.as_str() {
                    "all" => "alphanumeric",
                    "alphanumeric" => "letters",
                    "letters" => "custom",
                    // "custom" (and anything unknown) rejoins the cycle
                    // at "all".
                    _ => "all",
                }
                .to_string();
            }
            KeyCode::Tab if editor.cursor_line >= 2 => {
                editor.insert_indent();
            }
            // Some terminals report Shift+Tab as BackTab; dedent only in
            // Additional Content, matching the Tab indent arm above.
            KeyCode::BackTab if editor.cursor_line >= 2 => {
                editor.clear_selection();
                editor.dedent();
            }
            KeyCode::Tab => {
                editor.insert_char('\t');
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if editor.has_selection {
                    text_to_copy = editor.get_selected_text();
                    editor.clear_selection();
                }
            }
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                editor.clear_selection();
                if let Some(text) = App::paste_from_clipboard() {
                    for c in text.chars() {
                        if c == '\n' {
                            editor.insert_char('\n');
                        } else {
                            editor.insert_char(c);
                        }
                    }
                    app.message = Some(("Pasted from clipboard".to_string(), Instant::now()));
                } else {
                    app.message =
                        Some(("Clipboard empty or unavailable".to_string(), Instant::now()));
                }
            }
            // The Ctrl+U/D chords are matched by the guarded length arms
            // above only inside the Password field; swallow them elsewhere
            // so the text catch-all below never inserts their letter.
            KeyCode::Char('u') | KeyCode::Char('d')
                if key.modifiers.contains(KeyModifiers::CONTROL) => {}
            KeyCode::Char(c) => {
                editor.clear_selection();
                editor.insert_char(c);
            }
            _ => {}
        }
    }

    if let Some(text) = text_to_copy {
        app.copy_to_clipboard(&text);
        // Keep the "(Ns)" hint in sync with the configured timeout; when
        // auto-clear is disabled no countdown is shown.
        let msg = match app.clipboard_timeout() {
            Some(d) => format!("Selection copied to clipboard ({}s)", d.as_secs()),
            None => "Selection copied to clipboard".to_string(),
        };
        app.message = Some((msg, Instant::now()));
    }

    Ok(())
}

fn rect_contains(rect: ratatui::layout::Rect, x: u16, y: u16) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

pub fn handle_editor_double_click(
    app: &mut App,
    x: u16,
    y: u16,
    size: ratatui::layout::Rect,
) -> anyhow::Result<()> {
    use ratatui::layout::{Constraint, Direction, Layout, Rect};

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Max(2)])
        .split(size);

    let main_panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(main_chunks[0]);

    let editor_area = main_panes[1];

    if let Some(ref mut editor) = app.editor {
        let inner = Rect::new(
            editor_area.x + 1,
            editor_area.y + 1,
            editor_area.width.saturating_sub(2),
            editor_area.height.saturating_sub(2),
        );

        // Must match the layout in ui::draw_editor: a 1-line spacer at the
        // top, then path/password/generation/content (new) or
        // path/note/password/generation/content (existing).
        let chunks = if editor.is_new {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Length(3),
                    Constraint::Length(3),
                    Constraint::Length(4),
                    Constraint::Min(0),
                ])
                .split(inner)
        } else {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Length(3),
                    Constraint::Length(3),
                    Constraint::Length(3),
                    Constraint::Length(4),
                    Constraint::Min(0),
                ])
                .split(inner)
        };

        let content_idx = if editor.is_new { 4 } else { 5 };

        if rect_contains(chunks[content_idx], x, y) {
            let line_in_content = (y.saturating_sub(chunks[content_idx].y + 2)) as usize
                + editor.content_scroll_offset;
            let lines: Vec<&str> = editor.content.lines().skip(1).collect();

            if let Some(line) = lines.get(line_in_content) {
                let click_col =
                    x.saturating_sub(chunks[content_idx].x + 1) as usize + editor.content_h_scroll;
                let click_col = click_col.min(line.len());

                let word_separators: &[char] = &[' ', '\t'];
                let start = line[..click_col]
                    .rfind(word_separators)
                    .map(|i| i + 1)
                    .unwrap_or(0);

                let end = if click_col < line.len() {
                    line[click_col..]
                        .find(word_separators)
                        .map(|i| click_col + i)
                        .unwrap_or(line.len())
                } else {
                    line.len()
                };

                if start < end {
                    editor.cursor_line = 2 + line_in_content;
                    editor.cursor_col = start;
                    editor.sel_start_line = editor.cursor_line;
                    editor.sel_start_col = start;
                    editor.sel_end_line = editor.cursor_line;
                    editor.sel_end_col = end;
                    editor.has_selection = true;
                    editor.selecting = false;
                }
            }
        }
    }

    Ok(())
}

pub fn handle_editor_triple_click(
    app: &mut App,
    x: u16,
    y: u16,
    size: ratatui::layout::Rect,
) -> anyhow::Result<()> {
    use ratatui::layout::{Constraint, Direction, Layout, Rect};

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Max(2)])
        .split(size);

    let main_panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(main_chunks[0]);

    let editor_area = main_panes[1];

    if let Some(ref mut editor) = app.editor {
        let inner = Rect::new(
            editor_area.x + 1,
            editor_area.y + 1,
            editor_area.width.saturating_sub(2),
            editor_area.height.saturating_sub(2),
        );

        // Must match the layout in ui::draw_editor: a 1-line spacer at the
        // top, then path/password/generation/content (new) or
        // path/note/password/generation/content (existing).
        let chunks = if editor.is_new {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Length(3),
                    Constraint::Length(3),
                    Constraint::Length(4),
                    Constraint::Min(0),
                ])
                .split(inner)
        } else {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Length(3),
                    Constraint::Length(3),
                    Constraint::Length(3),
                    Constraint::Length(4),
                    Constraint::Min(0),
                ])
                .split(inner)
        };

        let content_idx = if editor.is_new { 4 } else { 5 };

        if rect_contains(chunks[content_idx], x, y) {
            let line_in_content = (y.saturating_sub(chunks[content_idx].y + 2)) as usize
                + editor.content_scroll_offset;
            let lines: Vec<&str> = editor.content.lines().skip(1).collect();

            if let Some(line) = lines.get(line_in_content) {
                editor.cursor_line = 2 + line_in_content;
                editor.cursor_col = 0;
                editor.sel_start_line = editor.cursor_line;
                editor.sel_start_col = 0;
                editor.sel_end_line = editor.cursor_line;
                editor.sel_end_col = line.len();
                editor.has_selection = true;
                editor.selecting = false;
            }
        }
    }

    Ok(())
}

pub fn handle_editor_mouse(
    app: &mut App,
    mouse: MouseEvent,
    size: ratatui::layout::Rect,
) -> anyhow::Result<()> {
    use ratatui::layout::{Constraint, Direction, Layout, Rect};

    let (x, y) = (mouse.column, mouse.row);

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Max(2)])
        .split(size);

    let main_panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(main_chunks[0]);

    let editor_area = main_panes[1];

    if let Some(editor) = &mut app.editor {
        let inner = Rect::new(
            editor_area.x + 1,
            editor_area.y + 1,
            editor_area.width.saturating_sub(2),
            editor_area.height.saturating_sub(2),
        );

        // Must match the layout in ui::draw_editor: a 1-line spacer at the
        // top, then path/password/generation/content (new) or
        // path/note/password/generation/content (existing).
        let chunks = if editor.is_new {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Length(3),
                    Constraint::Length(3),
                    Constraint::Length(4),
                    Constraint::Min(0),
                ])
                .split(inner)
        } else {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Length(3),
                    Constraint::Length(3),
                    Constraint::Length(3),
                    Constraint::Length(4),
                    Constraint::Min(0),
                ])
                .split(inner)
        };

        let path_idx = 1;
        let password_idx = if editor.is_new { 2 } else { 3 };
        let content_idx = if editor.is_new { 4 } else { 5 };

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if rect_contains(chunks[content_idx], x, y) {
                    let line_in_content = (y.saturating_sub(chunks[content_idx].y + 2)) as usize
                        + editor.content_scroll_offset;
                    let lines: Vec<&str> = editor.content.lines().skip(1).collect();

                    editor.cursor_line = 2 + line_in_content;
                    if let Some(line) = lines.get(line_in_content) {
                        let click_col = x.saturating_sub(chunks[content_idx].x + 1) as usize
                            + editor.content_h_scroll;
                        editor.cursor_col = click_col.min(line.len());
                    } else {
                        editor.cursor_col = 0;
                    }

                    editor.clear_selection();
                    editor.start_selection();
                } else if rect_contains(chunks[path_idx], x, y) && editor.is_new {
                    editor.clear_selection();
                    editor.cursor_line = 0;
                    let click_col =
                        x.saturating_sub(chunks[path_idx].x + 1) as usize + editor.path_h_scroll;
                    editor.cursor_col = click_col.min(editor.path.len());
                } else if rect_contains(chunks[password_idx], x, y) {
                    editor.clear_selection();
                    editor.cursor_line = 1;
                    let password_len = editor.get_password().unwrap_or("").len();
                    let click_col = x.saturating_sub(chunks[password_idx].x + 1) as usize
                        + editor.password_h_scroll;
                    editor.cursor_col = click_col.min(password_len);
                } else {
                    editor.clear_selection();
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if rect_contains(chunks[content_idx], x, y) && editor.selecting {
                    let line_in_content = (y.saturating_sub(chunks[content_idx].y + 2)) as usize
                        + editor.content_scroll_offset;
                    let lines: Vec<&str> = editor.content.lines().skip(1).collect();

                    editor.cursor_line = 2 + line_in_content;
                    if let Some(line) = lines.get(line_in_content) {
                        let click_col = x.saturating_sub(chunks[content_idx].x + 1) as usize
                            + editor.content_h_scroll;
                        editor.cursor_col = click_col.min(line.len());
                    } else {
                        editor.cursor_col = 0;
                    }
                    editor.update_selection();
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                editor.selecting = false;
            }
            _ => {}
        }
    }

    Ok(())
}

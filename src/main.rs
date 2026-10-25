use mujo::{app, diagnose, handlers, keys, models, ui};

use app::App;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseButton,
        MouseEvent, MouseEventKind,
    },
    execute,
    style::Print,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::time::{Duration, Instant};

fn rect_contains(rect: ratatui::layout::Rect, x: u16, y: u16) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

fn arg_value(flag: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Actions that must be resolved before any terminal setup.
#[derive(Debug, PartialEq, Eq)]
enum CliAction {
    Diagnose,
    Version,
    Help,
    Run,
}

/// Classify the process arguments into the action to take. Mirrors the
/// shape --diagnose used so --version/--help also resolve before any
/// terminal setup (no alternate screen, no raw mode). Anything
/// unrecognised falls through to starting the TUI.
fn classify_args<I, S>(args: I) -> CliAction
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut it = args.into_iter();
    let _program = it.next();
    for arg in it {
        match arg.as_ref() {
            "--diagnose" => return CliAction::Diagnose,
            "--version" | "-V" => return CliAction::Version,
            "--help" | "-h" => return CliAction::Help,
            _ => {}
        }
    }
    CliAction::Run
}

fn main() -> anyhow::Result<()> {
    // Flag handling happens before any terminal setup: --diagnose prints
    // an environment report and exits (it must not require a password
    // store to exist; the --glyphs/--truecolor overrides are applied
    // inside report() so the output reflects what a normal run would
    // actually use), --version/--help print and exit, and anything else
    // starts the TUI.
    match classify_args(std::env::args()) {
        CliAction::Diagnose => {
            println!("{}", diagnose::report());
            return Ok(());
        }
        CliAction::Version => {
            println!("mujo {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        CliAction::Help => {
            println!("mujo - a TUI front end for pass");
            println!();
            println!("Usage: mujo [--diagnose | --version | --help]");
            println!("  --diagnose  print an environment report and exit");
            println!("  --version   print the version and exit");
            println!("  --help      print this help and exit");
            println!();
            println!("Key bindings are documented inside the TUI via '?'.");
            return Ok(());
        }
        CliAction::Run => {}
    }
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    // Save the shell's cursor on the normal screen (DECSC) before entering
    // the alt screen: the alt-screen repairs emit ?1049h, which also SAVES
    // the cursor, overwriting the position recorded at startup. Without
    // this, LeaveAlternateScreen's implicit restore would put the shell
    // prompt at the last repaired alt-screen position instead of where the
    // user left it.
    execute!(stdout, Print("\x1b7"))?;
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let res = run_app();
    disable_raw_mode()?;
    execute!(stdout, LeaveAlternateScreen, DisableMouseCapture)?;
    // Restore the shell's saved cursor (DECRC) so the prompt lands where
    // the user left it, not at the last alt-screen repair position.
    execute!(stdout, Print("\x1b8"))?;
    res
}

fn run_app() -> anyhow::Result<()> {
    let mut app = App::new()?;
    // main() has already entered the alternate screen and enabled raw mode:
    // the TUI owns the terminal from here on, so git_cmd may hand it to
    // git for interactive push/pull/fetch.
    app.tui_active = true;
    // Combine CLI flag, environment overrides and config into the effective
    // mode before resolving glyphs/truecolor.
    app.config.display.glyphs =
        ui::effective_glyph_mode(arg_value("--glyphs").as_deref(), &app.config.display.glyphs);
    app.config.display.truecolor = ui::effective_truecolor_mode(
        arg_value("--truecolor").as_deref(),
        &app.config.display.truecolor,
    );
    app.config.display.keyboard = keys::effective_keyboard_mode(
        arg_value("--keyboard").as_deref(),
        &app.config.display.keyboard,
    );
    // The keyboard platform must be resolved BEFORE the first draw: the
    // help body and the status bar advertise chords through keys::advertised.
    keys::set_keyboard(keys::resolve_keyboard(&app.config.display.keyboard));
    ui::set_glyphs(ui::resolve_glyphs(&app.config.display.glyphs));
    ui::set_truecolor(&app.config.display.truecolor);
    let stdout = io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let tick_rate = Duration::from_millis(100);
    let mut last_tick = Instant::now();

    loop {
        let size = terminal.size()?;
        terminal.draw(|f| ui::draw(f, &mut app))?;

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if crossterm::event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Press {
                        handle_key_event(&mut app, key)?;
                    }
                }
                Event::Mouse(mouse) => {
                    handle_mouse_event(&mut app, mouse, size)?;
                }
                _ => {}
            }
        }

        // A tty-handoff subcommand (push/pull/fetch) left and re-entered
        // the alternate screen: EnterAlternateScreen clears on entry, so
        // ratatui's diff would consider every cell unchanged and repaint
        // nothing. Force a full clear before the next draw.
        if app.needs_clear {
            terminal.clear()?;
            app.needs_clear = false;
        }

        if last_tick.elapsed() >= tick_rate {
            app.update();
            last_tick = Instant::now();
        }

        if app.should_quit {
            app.wipe_all_sensitive_data();
            break;
        }
    }
    Ok(())
}

fn handle_key_event(app: &mut App, key: crossterm::event::KeyEvent) -> anyhow::Result<()> {
    use crossterm::event::KeyModifiers;
    use models::AppMode;

    // Help is reachable from every mode: Ctrl+/ and F1 anywhere, "?"
    // wherever it is not a printable character. Runs before the per-mode
    // handlers so the universal chord wins over mode-specific bindings.
    if handlers::maybe_open_help(app, &key) {
        return Ok(());
    }

    if app.mode == AppMode::Editor
        && key.code == crossterm::event::KeyCode::Char('s')
        && key.modifiers.contains(KeyModifiers::CONTROL)
    {
        match app.save_entry(false) {
            Ok(true) => {
                app.editor = None;
                app.mode = AppMode::Normal;
            }
            Ok(false) => {}
            Err(e) => {
                app.message = Some((format!("Error: {}", e), Instant::now()));
            }
        }
        return Ok(());
    }

    if app.mode == AppMode::ContextMenu {
        return handle_context_menu_keys(app, key.code);
    }

    match app.mode {
        AppMode::Normal => handlers::handle_normal_mode(app, key)?,
        AppMode::Search => handlers::handle_search_mode(app, key.code)?,
        AppMode::Editor => handlers::handle_editor_mode(app, key)?,
        AppMode::Git => handlers::handle_git_mode(app, key)?,
        AppMode::Help => handlers::handle_help_mode(app, key)?,
        AppMode::ConfirmDelete => handlers::handle_confirm_delete(app, key.code)?,
        AppMode::ConfirmSave => handlers::handle_confirm_save(app, key.code)?,
        AppMode::ConfirmDiscard => handlers::handle_confirm_discard(app, key.code)?,
        AppMode::AddFolder => handlers::handle_add_folder_mode(app, key.code)?,
        AppMode::Move => handlers::handle_move_mode(app, key.code)?,
        AppMode::NewStore => handlers::handle_new_store_mode(app, key)?,
        AppMode::ContextMenu => unreachable!(),
    }
    Ok(())
}

fn handle_mouse_event(
    app: &mut App,
    mouse: MouseEvent,
    size: ratatui::layout::Rect,
) -> anyhow::Result<()> {
    use ratatui::layout::{Constraint, Direction, Layout, Rect};

    let (x, y) = (mouse.column, mouse.row);

    let (is_double_click, is_triple_click) =
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            let now = Instant::now();
            let count = match app.last_click {
                Some((time, pos, count))
                    if time.elapsed() < Duration::from_millis(400) && pos == (x, y) =>
                {
                    count.saturating_add(1).min(3)
                }
                _ => 1,
            };
            app.last_click = Some((now, (x, y), count));
            (count == 2, count == 3)
        } else {
            (false, false)
        };

    match app.mode {
        models::AppMode::Editor => {
            if is_triple_click {
                return handle_editor_triple_click(app, x, y, size);
            }
            if is_double_click {
                return handle_editor_double_click(app, x, y, size);
            }
            return handle_editor_mouse(app, mouse, size);
        }
        models::AppMode::Git => {
            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    // The setup prompts (Uninitialized/AskRemote/AskProxy) are modal,
                    // like ConfirmSave: ignore outside clicks so a stray click cannot
                    // abandon a half-typed remote URL. The Ready pane is non-modal:
                    // clicking outside it exits Git mode and the click falls through
                    // to the shared navigation handling below (tree selection,
                    // search focus, preview).
                    if !matches!(app.git_pane, models::GitPaneState::Ready) {
                        return Ok(());
                    }

                    let main_chunks = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([Constraint::Min(5), Constraint::Max(2)])
                        .split(size);

                    let main_panes = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
                        .split(main_chunks[0]);

                    let git_area = main_panes[1];

                    if rect_contains(git_area, x, y) {
                        return Ok(());
                    }

                    app.mode = models::AppMode::Normal;
                    // Fall through to the shared click handling below.
                }
                MouseEventKind::ScrollDown => {
                    app.git_scroll += 3;
                    return Ok(());
                }
                MouseEventKind::ScrollUp => {
                    if app.git_scroll >= 3 {
                        app.git_scroll -= 3;
                    } else {
                        app.git_scroll = 0;
                    }
                    return Ok(());
                }
                _ => {
                    return Ok(());
                }
            }
        }
        models::AppMode::ConfirmSave | models::AppMode::ConfirmDiscard => {
            // Block navigation while a confirmation prompt is pending.
            // The user must answer with the keyboard (y/n/Esc) first.
            return Ok(());
        }
        models::AppMode::Help => {
            // The full-screen help view is modal like the setup prompts:
            // ignore mouse input so a click cannot mutate the tree
            // selection hidden underneath it.
            return Ok(());
        }
        models::AppMode::NewStore => {
            // First-run setup is modal: ignore mouse input entirely until
            // the store exists (or the user quits).
            return Ok(());
        }
        _ => {}
    }

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Max(2)])
        .split(size);

    let main_panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(main_chunks[0]);

    let left_column = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(main_panes[0]);

    let search_area = left_column[0];
    let tree_area = left_column[1];
    let preview_area = main_panes[1];

    match mouse.kind {
        MouseEventKind::Moved => {}

        MouseEventKind::Down(MouseButton::Left) => {
            if app.mode == models::AppMode::ContextMenu {
                if let Some(ref menu) = app.context_menu {
                    // Must match the width computed in ui::draw_context_menu:
                    // +6 = 2 padding + 2 border + 2 for the selected item's
                    // rounded caps.
                    let menu_height = menu.items.len() as u16 + 2;
                    let menu_width =
                        menu.items.iter().map(|i| i.label.len()).max().unwrap_or(10) as u16 + 6;
                    let menu_area = Rect::new(menu.x, menu.y, menu_width, menu_height);

                    if rect_contains(menu_area, x, y) {
                        let clicked_row = (y.saturating_sub(menu_area.y + 1)) as usize;
                        if clicked_row < menu.items.len() && menu.items[clicked_row].enabled {
                            let action = menu.items[clicked_row].action.clone();
                            app.execute_menu_action(action);
                        } else {
                            app.context_menu = None;
                            app.mode = models::AppMode::Normal;
                        }
                    } else {
                        app.context_menu = None;
                        app.mode = models::AppMode::Normal;
                    }
                }
                return Ok(());
            }

            if rect_contains(search_area, x, y) {
                app.mode = models::AppMode::Search;
                app.search_query.clear();
                app.rebuild_filtered();
            } else if rect_contains(tree_area, x, y) && !app.filtered_entries.is_empty() {
                let row = if y > tree_area.y && y < tree_area.y + tree_area.height - 1 {
                    (y - tree_area.y - 1) as usize
                } else {
                    return Ok(());
                };
                let entry_idx = app.scroll_offset + row;

                if entry_idx < app.filtered_entries.len() {
                    app.selected = entry_idx;
                    app.clear_preview();

                    if is_double_click {
                        if let Some(&idx) = app.filtered_entries.get(app.selected) {
                            let entry = &app.entries[idx];
                            if entry.is_dir {
                                app.toggle_folder();
                            } else if let Err(e) = app.decrypt_current_entry(true) {
                                app.message = Some((format!("Error: {}", e), Instant::now()));
                            }
                        }
                    }
                }
            } else if rect_contains(preview_area, x, y) && app.current_content.is_some() {
                let relative_y = y.saturating_sub(preview_area.y + 1) as usize;
                let display_line = relative_y + app.preview_scroll_offset;
                if display_line == 1 {
                    app.show_password = !app.show_password;
                    app.preview_selected_line = Some(0);
                } else if display_line >= 3 {
                    let content_idx = display_line - 2;
                    if let Some(ref c) = app.current_content {
                        if content_idx < c.lines.len() {
                            app.preview_selected_line = Some(content_idx);
                        }
                    }
                }
            }
        }

        MouseEventKind::Down(MouseButton::Right) => {
            if rect_contains(tree_area, x, y) || rect_contains(preview_area, x, y) {
                if rect_contains(tree_area, x, y) && !app.filtered_entries.is_empty() {
                    let row = if y > tree_area.y && y < tree_area.y + tree_area.height - 1 {
                        (y - tree_area.y - 1) as usize
                    } else {
                        return Ok(());
                    };
                    let entry_idx = app.scroll_offset + row;
                    if entry_idx < app.filtered_entries.len() {
                        app.selected = entry_idx;
                    }
                }
                app.show_context_menu(x, y);
            }
        }

        MouseEventKind::ScrollUp => {
            if rect_contains(tree_area, x, y) && !app.filtered_entries.is_empty() {
                app.move_selection(-1);
            } else if rect_contains(preview_area, x, y) && app.current_content.is_some() {
                if app.preview_scroll_offset >= 3 {
                    app.preview_scroll_offset -= 3;
                } else {
                    app.preview_scroll_offset = 0;
                }
            }
        }

        MouseEventKind::ScrollDown => {
            if rect_contains(tree_area, x, y) && !app.filtered_entries.is_empty() {
                app.move_selection(1);
            } else if rect_contains(preview_area, x, y) && app.current_content.is_some() {
                app.preview_scroll_offset += 3;
            }
        }
        _ => {}
    }

    Ok(())
}

fn handle_editor_double_click(
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

fn handle_editor_triple_click(
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

fn handle_editor_mouse(
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

fn handle_context_menu_keys(app: &mut App, code: crossterm::event::KeyCode) -> anyhow::Result<()> {
    let Some(ref mut menu) = app.context_menu else {
        app.mode = models::AppMode::Normal;
        return Ok(());
    };

    match code {
        crossterm::event::KeyCode::Esc => {
            app.context_menu = None;
            app.mode = models::AppMode::Normal;
        }
        crossterm::event::KeyCode::Up => {
            if menu.selected > 0 {
                menu.selected -= 1;
            }
        }
        crossterm::event::KeyCode::Down => {
            if menu.selected < menu.items.len().saturating_sub(1) {
                menu.selected += 1;
            }
        }
        crossterm::event::KeyCode::Enter if menu.items[menu.selected].enabled => {
            let action = menu.items[menu.selected].action.clone();
            app.execute_menu_action(action);
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_args_version_flags() {
        assert_eq!(classify_args(["mujo", "--version"]), CliAction::Version);
        assert_eq!(classify_args(["mujo", "-V"]), CliAction::Version);
    }

    #[test]
    fn classify_args_help_flags() {
        assert_eq!(classify_args(["mujo", "--help"]), CliAction::Help);
        assert_eq!(classify_args(["mujo", "-h"]), CliAction::Help);
    }

    #[test]
    fn classify_args_diagnose_flag() {
        assert_eq!(classify_args(["mujo", "--diagnose"]), CliAction::Diagnose);
    }

    #[test]
    fn classify_args_default_is_run() {
        assert_eq!(classify_args(["mujo"]), CliAction::Run);
        assert_eq!(classify_args(Vec::<String>::new()), CliAction::Run);
        assert_eq!(classify_args(["mujo", "--glyphs", "ascii"]), CliAction::Run);
    }
}

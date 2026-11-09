use super::hints::{
    editor_hints, git_ready_hints, help_hints, key_unit, normal_mode_hints, pack_units,
};
use super::{cap_style, get_colors, glyphs, message_style};
use crate::app::{App, PULL_CONFIRM_SENTENCE};
use crate::models::{AppMode, GitPaneState, NewStoreStage};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub(crate) fn clamp_menu_rect(
    click_x: u16,
    click_y: u16,
    menu_w: u16,
    menu_h: u16,
    frame: Rect,
) -> Rect {
    let menu_width = menu_w.min(frame.width);
    let menu_height = menu_h.min(frame.height);
    let menu_x = click_x.min(frame.width.saturating_sub(menu_width));
    let menu_y = click_y.min(frame.height.saturating_sub(menu_height));
    Rect::new(menu_x, menu_y, menu_width, menu_height)
}

pub(crate) fn draw_context_menu(
    f: &mut Frame,
    menu: &crate::models::ContextMenu,
    primary: Color,
    secondary: Color,
    text: Color,
    base_bg: Color,
) {
    let frame = f.size();
    let menu_height = menu.items.len() as u16 + 2;
    // +6 = 2 padding + 2 border + 2 for the selected item's rounded caps,
    // so the pill's right cap stays inside the menu even on long labels.
    let menu_width = menu.items.iter().map(|i| i.label.len()).max().unwrap_or(10) as u16 + 6;

    let area = clamp_menu_rect(menu.x, menu.y, menu_width, menu_height, frame);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(primary));

    let items: Vec<Line> = menu
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let style = if i == menu.selected {
                Style::default()
                    .bg(primary)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else if !item.enabled {
                Style::default().fg(secondary)
            } else {
                Style::default().fg(text)
            };

            if item.label == "---" {
                Line::from("─".repeat(menu_width.saturating_sub(2) as usize))
            } else if i == menu.selected {
                // Rounded-cap pill, matching the sidebar's selection tag: the
                // caps use the terminal background so they blend into the menu,
                // and the body carries the primary color.
                Line::from(vec![
                    Span::styled(glyphs().cap_left, cap_style(primary, base_bg)),
                    Span::styled(
                        format!(" {} ", item.label),
                        Style::default()
                            .fg(Color::Black)
                            .bg(primary)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(glyphs().cap_right, cap_style(primary, base_bg)),
                ])
            } else {
                Line::from(Span::styled(format!(" {} ", item.label), style))
            }
        })
        .collect();

    let paragraph = Paragraph::new(Text::from(items)).block(block);
    f.render_widget(paragraph, area);
}

/// Units for the ConfirmDelete status bar, shared by the delete flow and
/// the pull-confirmation flow (pending_pull says which): only the sentence
/// differs - "Delete <target>?" vs the pull confirmation from
/// PULL_CONFIRM_SENTENCE - while the y/n key units are common to both.
/// Extracted from draw_status_bar so tests can render the real units
/// rather than a copy of their literals.
pub(crate) fn confirm_delete_units(
    pending_pull: bool,
    target: Option<String>,
    text: Color,
    key_style: Style,
    dim: Style,
) -> Vec<Vec<Span<'static>>> {
    let sentence = if pending_pull {
        PULL_CONFIRM_SENTENCE.to_string()
    } else {
        format!(
            "Delete {}? ",
            target.unwrap_or_else(|| "this entry".to_string())
        )
    };
    vec![
        vec![Span::styled(sentence, Style::default().fg(text))],
        key_unit("y", key_style, ":confirm ", dim),
        key_unit("n", key_style, ":cancel", dim),
    ]
}

pub(crate) fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let (primary, secondary, text, bg) = get_colors(app);

    let mode_str = match app.mode {
        AppMode::Normal => "Normal",
        AppMode::Search => "Search",
        AppMode::Editor => "Editor",
        AppMode::Git => "Git",
        AppMode::Help => "Help",
        AppMode::ConfirmDelete => "Confirm",
        AppMode::ConfirmSave => "Confirm",
        AppMode::ConfirmDiscard => "Confirm",
        AppMode::AddFolder => "Add",
        AppMode::Move => "Move",
        AppMode::ContextMenu => "Menu",
        AppMode::NewStore => "New",
    };

    let cyan_bold = Style::default().fg(primary).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(secondary);

    // The delete confirmation names the target by its store-relative path
    // (e.g. "Delete work/github?"), matching how the other operation
    // prompts and messages reference entries ("Move 'work/github' to:",
    // "Saved work/github") instead of the bare display name.
    let delete_target = if let Some(idx) = app.entry_to_delete {
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
                rel_path
                    .strip_suffix(".gpg")
                    .unwrap_or(&rel_path)
                    .to_string()
            })
    } else {
        None
    };

    let has_message = app.message.is_some();

    // The Preview pane is a sub-view of Normal mode: an entry is decrypted
    // and shown while the tree remains navigable. Surface it in the status
    // bar as "Normal | Preview" so the user can tell the two states apart.
    let preview_active = app.mode == AppMode::Normal && app.current_content.is_some();

    let mode_style = Style::default()
        .bg(primary)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD);

    if app.mode == AppMode::AddFolder {
        let line1 = vec![
            Span::styled(glyphs().cap_left, cap_style(primary, bg)),
            Span::styled(format!(" {} ", mode_str), mode_style),
            Span::styled(glyphs().cap_right, cap_style(primary, secondary)),
            Span::styled(
                " Folder ",
                Style::default()
                    .fg(primary)
                    .bg(secondary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(glyphs().cap_right, cap_style(secondary, bg)),
            Span::styled(" ", Style::default()),
            Span::styled(
                "Type folder name (e.g., work/vpn): ",
                Style::default().fg(text),
            ),
            Span::styled(&app.command_buffer, Style::default().fg(primary)),
        ];

        let line2 = vec![
            Span::styled("   ", Style::default()),
            Span::styled("Enter", cyan_bold),
            Span::styled(":done  ", Style::default().fg(secondary)),
            Span::styled("Esc", cyan_bold),
            Span::styled(":clear", Style::default().fg(secondary)),
        ];

        let text = Text::from(vec![Line::from(line1), Line::from(line2)]);

        let paragraph = Paragraph::new(text);
        f.render_widget(paragraph, area);
        return;
    }

    if app.mode == AppMode::Move {
        let source = app.move_source.clone().unwrap_or_default();
        // The sub-pill names what is being moved: a directory or a password
        // file, based on the selected entry.
        let move_label = app
            .filtered_entries
            .get(app.selected)
            .and_then(|&idx| app.entries.get(idx))
            .map(|e| if e.is_dir { "Folder" } else { "Password" })
            .unwrap_or("Password");
        let line1 = vec![
            Span::styled(glyphs().cap_left, cap_style(primary, bg)),
            Span::styled(format!(" {} ", mode_str), mode_style),
            Span::styled(glyphs().cap_right, cap_style(primary, secondary)),
            Span::styled(
                format!(" {} ", move_label),
                Style::default()
                    .fg(primary)
                    .bg(secondary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(glyphs().cap_right, cap_style(secondary, bg)),
            Span::styled(" ", Style::default()),
            Span::styled(format!("Move '{}' to: ", source), Style::default().fg(text)),
            Span::styled(&app.command_buffer, Style::default().fg(primary)),
        ];

        let line2 = vec![
            Span::styled("   ", Style::default()),
            Span::styled("Enter", cyan_bold),
            Span::styled(":move  ", Style::default().fg(secondary)),
            Span::styled("Esc", cyan_bold),
            Span::styled(":cancel", Style::default().fg(secondary)),
        ];

        let text = Text::from(vec![Line::from(line1), Line::from(line2)]);

        let paragraph = Paragraph::new(text);
        f.render_widget(paragraph, area);
        return;
    }

    // The status bar is two rows tall and reads "[ModeBox] | [Context]
    // [helpbar units]". The context slot sits between the mode box and the
    // key hints: a pending operation message (success or error, e.g.
    // "Password store created" or "GPG key not found: ...") takes it over
    // in the theme foreground color (red only for errors); otherwise it
    // carries the
    // state-derived label ("Setup", "New", "Edit", "Preview") in the
    // foreground color. Messages are only surfaced in the modes that
    // produce them (Normal, Editor, NewStore) so a stale toast can never
    // hide a prompt like "Delete x?" in ConfirmDelete.
    let message_modes = matches!(
        app.mode,
        AppMode::Normal | AppMode::Editor | AppMode::NewStore
    );
    let message_text = app
        .message
        .as_ref()
        .map(|(m, _)| m.clone())
        .unwrap_or_default();

    let context: Option<(String, Style, bool)> = if has_message && message_modes {
        // Compute the style BEFORE moving message_text into the tuple:
        // both in one expression would move the String and then borrow it.
        // The bool flags whether the context renders as a sub-pill.
        // Messages always render as plain text (no pill caps); only the
        // state-derived contexts below render as sub-pills.
        let style = message_style(&message_text, text);
        Some((message_text, style, false))
    } else {
        let ctx: Option<String> = match app.mode {
            AppMode::NewStore => Some("Setup".to_string()),
            AppMode::Editor => {
                let is_new = app.editor.as_ref().map(|e| e.is_new).unwrap_or(false);
                Some(if is_new { "New" } else { "Edit" }.to_string())
            }
            AppMode::Git if !matches!(app.git_pane, GitPaneState::Ready) => {
                Some("Setup".to_string())
            }
            AppMode::Normal if !app.jump_buf.is_empty() => Some(format!("jump: {}", app.jump_buf)),
            AppMode::Normal if preview_active => Some("Preview".to_string()),
            AppMode::ContextMenu => Some("Context menu".to_string()),
            // The Confirm modes carry no context sub-pill: their sentence
            // ("Delete x?", "Entry modified!", "Unsaved changes — ...") is
            // rendered as a plain help span ahead of the key hints in the
            // units section below.
            _ => None,
        };
        ctx.map(|c| (c, Style::default().fg(text), true))
    };

    // The helpbar is area.height rows tall (normally two, but one on a
    // short terminal). Content is built as a list of unbreakable units (a
    // keybinding plus its action, or a standalone text chunk) and packed
    // manually: line 1 gets what fits after the mode prefix and context
    // slot, everything else moves to the following full-width rows as
    // complete units. This replaces ratatui's Wrap, which breaks at
    // arbitrary characters and could split a pair like "Ctrl+s:save" or
    // overflow line 1.
    let mut prefix: Vec<Span<'static>> = vec![
        Span::styled(glyphs().cap_left, cap_style(primary, bg)),
        Span::styled(format!(" {} ", mode_str), mode_style),
    ];
    if let Some((ctx, style, is_pill)) = context {
        if is_pill {
            prefix.push(Span::styled(
                glyphs().cap_right,
                cap_style(primary, secondary),
            ));
            prefix.push(Span::styled(
                format!(" {} ", ctx),
                Style::default()
                    .fg(primary)
                    .bg(secondary)
                    .add_modifier(Modifier::BOLD),
            ));
            prefix.push(Span::styled(glyphs().cap_right, cap_style(secondary, bg)));
        } else {
            // Plain message: close the mode pill with its right cap (same
            // as the no-context case), then a single leading space and the
            // message. No secondary background and no caps - prose, not a
            // pill. Errors keep their red foreground; other messages use
            // the theme's foreground color.
            prefix.push(Span::styled(glyphs().cap_right, cap_style(primary, bg)));
            prefix.push(Span::styled(" ".to_string(), dim));
            prefix.push(Span::styled(ctx, style));
        }
        prefix.push(Span::styled(" ".to_string(), dim));
    } else {
        prefix.push(Span::styled(glyphs().cap_right, cap_style(primary, bg)));
        prefix.push(Span::styled(" ", Style::default()));
    }

    let mut units: Vec<Vec<Span<'static>>> = Vec::new();
    let width = area.width as usize;
    let prefix_width: usize = prefix.iter().map(|s| s.content.chars().count()).sum();
    let first_capacity = width.saturating_sub(prefix_width);

    // While a message occupies the context slot, the key hints step aside
    // so the feedback is what the user reads.
    if !(has_message && message_modes) {
        match app.mode {
            AppMode::Normal => {
                units.extend(normal_mode_hints(
                    preview_active,
                    width,
                    area.height as usize,
                    prefix_width,
                    cyan_bold,
                    dim,
                ));
            }
            AppMode::NewStore => match app.new_store_stage {
                NewStoreStage::Confirm => {
                    units.push(key_unit("Y", cyan_bold, ":set up store ", dim));
                    units.push(key_unit("Esc", cyan_bold, ":quit", dim));
                }
                NewStoreStage::AskGpgId => {
                    units.push(key_unit("Enter", cyan_bold, ":create ", dim));
                    units.push(key_unit("Esc", cyan_bold, ":back", dim));
                }
            },
            AppMode::ConfirmDelete => {
                units.extend(confirm_delete_units(
                    app.pending_pull,
                    delete_target.clone(),
                    text,
                    cyan_bold,
                    dim,
                ));
            }
            AppMode::Search => {
                units.push(vec![Span::styled(
                    "Type to filter entries ".to_string(),
                    dim,
                )]);
                units.push(key_unit("Enter", cyan_bold, ":done ", dim));
                units.push(key_unit("Esc", cyan_bold, ":clear", dim));
            }
            AppMode::Editor => {
                units.extend(editor_hints(cyan_bold, dim));
            }
            AppMode::ContextMenu => {
                units.push(key_unit("↑↓", cyan_bold, ":nav ", dim));
                units.push(key_unit("Enter", cyan_bold, ":select ", dim));
                units.push(key_unit("Esc", cyan_bold, ":close", dim));
            }
            AppMode::Git => match app.git_pane {
                GitPaneState::Uninitialized => {
                    units.push(key_unit("Y", cyan_bold, ":set up ", dim));
                    units.push(key_unit("Esc", cyan_bold, ":cancel", dim));
                }
                GitPaneState::AskRemote => {
                    units.push(key_unit("Enter", cyan_bold, ":add remote ", dim));
                    units.push(key_unit("Esc", cyan_bold, ":skip", dim));
                }
                GitPaneState::AskIdentityName | GitPaneState::AskIdentityEmail => {
                    units.push(key_unit("Enter", cyan_bold, ":next ", dim));
                    units.push(key_unit("Esc", cyan_bold, ":skip commit", dim));
                }
                GitPaneState::Ready => {
                    units.extend(git_ready_hints(cyan_bold, dim));
                }
            },
            AppMode::Help => {
                units.extend(help_hints(cyan_bold, dim));
            }
            AppMode::ConfirmSave => {
                // The editor is still open while this prompt shows, so its
                // is_new flag distinguishes creating an entry from editing
                // an existing one; each gets wording that matches the act.
                let is_new = app.editor.as_ref().map(|e| e.is_new).unwrap_or(false);
                if is_new {
                    units.push(vec![Span::styled(
                        "Save new password? - ".to_string(),
                        Style::default().fg(text),
                    )]);
                    units.push(key_unit("y", cyan_bold, ":yes ", dim));
                } else {
                    units.push(vec![Span::styled(
                        "Entry modified - ".to_string(),
                        Style::default().fg(text),
                    )]);
                    units.push(key_unit("y", cyan_bold, ":save ", dim));
                }
                units.push(key_unit("n", cyan_bold, ":cancel", dim));
            }
            AppMode::ConfirmDiscard => {
                units.push(vec![Span::styled(
                    "Unsaved changes — ".to_string(),
                    Style::default().fg(text),
                )]);
                units.push(key_unit("y", cyan_bold, ":save ", dim));
                units.push(key_unit("n", cyan_bold, ":discard ", dim));
                units.push(key_unit("Esc", cyan_bold, ":keep editing", dim));
            }
            AppMode::AddFolder | AppMode::Move => unreachable!(),
        }
    }

    let packed = pack_units(units, width, first_capacity);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut line1 = prefix;
    if let Some(first_units) = packed.first() {
        line1.extend(first_units.iter().cloned());
    }
    lines.push(Line::from(line1));
    for line in packed
        .iter()
        .skip(1)
        .take(area.height.saturating_sub(1) as usize)
    {
        lines.push(Line::from(line.clone()));
    }

    let text = Text::from(lines);

    let paragraph = Paragraph::new(text);

    f.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_menu_fits_normally() {
        let frame = Rect::new(0, 0, 80, 24);
        let rect = clamp_menu_rect(10, 5, 20, 10, frame);
        assert_eq!(rect.x, 10);
        assert_eq!(rect.y, 5);
        assert_eq!(rect.width, 20);
        assert_eq!(rect.height, 10);
    }

    #[test]
    fn test_menu_clamped_bottom_right() {
        let frame = Rect::new(0, 0, 80, 24);
        let rect = clamp_menu_rect(79, 23, 20, 10, frame);
        assert!(rect.x + rect.width <= 80);
        assert!(rect.y + rect.height <= 24);
        assert_eq!(rect.x, 60);
        assert_eq!(rect.y, 14);
    }

    #[test]
    fn test_menu_clamped_top_left() {
        let frame = Rect::new(0, 0, 80, 24);
        let rect = clamp_menu_rect(0, 0, 20, 10, frame);
        assert_eq!(rect.x, 0);
        assert_eq!(rect.y, 0);
        assert!(rect.x + rect.width <= 80);
        assert!(rect.y + rect.height <= 24);
    }

    #[test]
    fn test_menu_larger_than_frame() {
        let frame = Rect::new(0, 0, 80, 24);
        let rect = clamp_menu_rect(40, 12, 100, 30, frame);
        assert!(rect.width <= 80);
        assert!(rect.height <= 24);
        assert_eq!(rect.width, 80);
        assert_eq!(rect.height, 24);
    }
}

use super::{cap_style, get_colors, glyphs};
use crate::app::App;
use crate::models::AppMode;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

pub(crate) fn draw_search_bar(f: &mut Frame, app: &App, area: Rect) {
    let (primary, secondary, text, _) = get_colors(app);

    let (search_text, style) = if app.mode == AppMode::Search {
        if app.search_query.is_empty() {
            ("Type to search...", Style::default().fg(Color::Gray))
        } else {
            (&*app.search_query, Style::default().fg(primary))
        }
    } else {
        ("", Style::default().fg(text))
    };

    let border_color = if app.mode == AppMode::Search {
        primary
    } else {
        secondary
    };

    let block = Block::default()
        .title(" Search ")
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(border_color));

    let paragraph = Paragraph::new(search_text).style(style).block(block);

    f.render_widget(paragraph, area);

    if app.mode == AppMode::Search {
        let cursor_x = area.x + 1 + app.search_query.len() as u16;
        let cursor_y = area.y + 1;
        f.set_cursor(cursor_x, cursor_y);
    }
}

pub(crate) fn draw_sidebar(f: &mut Frame, app: &mut App, area: Rect) {
    let (primary, secondary, text, bg) = get_colors(app);
    let pane_height = area.height.saturating_sub(2) as usize;

    if pane_height == 0 {
        return;
    }

    if app.filtered_entries.len() <= pane_height {
        app.scroll_offset = 0;
    } else if app.selected < app.scroll_offset {
        app.scroll_offset = app.selected;
    } else if app.selected >= app.scroll_offset + pane_height {
        app.scroll_offset = app.selected - pane_height + 1;
    }

    let max_scroll = app.filtered_entries.len().saturating_sub(pane_height);
    if app.scroll_offset > max_scroll {
        app.scroll_offset = max_scroll;
    }

    let visible_entries: Vec<_> = app
        .filtered_entries
        .iter()
        .skip(app.scroll_offset)
        .take(pane_height)
        .enumerate()
        .collect();

    let items: Vec<Line> = visible_entries
        .iter()
        .map(|(display_idx, &idx)| {
            let actual_idx = app.scroll_offset + display_idx;
            let entry = &app.entries[idx];
            let is_selected = actual_idx == app.selected;

            let prefix = if entry.is_dir {
                if entry.expanded {
                    glyphs().tri_closed
                } else {
                    glyphs().tri_open
                }
            } else {
                "  "
            };

            let name = entry.display_name.clone();

            let style = if is_selected {
                Style::default()
                    .bg(primary)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else if entry.is_dir {
                // Folders read by weight, not loud color: bold text keeps
                // them distinct while primary stays reserved for the
                // selection.
                Style::default().fg(text).add_modifier(Modifier::BOLD)
            } else {
                // Password entries take the muted secondary color so the
                // bold folder names stand out against them.
                Style::default().fg(secondary)
            };

            let indent = " ".repeat(entry.depth * 2);
            let content = format!("{}{}{}", indent, prefix, name);

            // Every row reserves a fixed 2-column left gutter, so the entry
            // name sits on the same column whether the row is selected or
            // not (no horizontal jump). Unselected rows fill the gutter
            // with two plain spaces; selected rows turn it into the pill's
            // left cap plus one padding space, and close the pill with a
            // padding space and the right cap after the name.
            if is_selected {
                Line::from(vec![
                    Span::styled(glyphs().cap_left, cap_style(primary, bg)),
                    Span::styled(" ", Style::default().bg(primary)),
                    Span::styled(content, style),
                    Span::styled(" ", Style::default().bg(primary)),
                    Span::styled(glyphs().cap_right, cap_style(primary, bg)),
                ])
            } else if entry.is_dir {
                // The expand/collapse marker is drawn in its own span so it
                // can take the muted secondary color. The depth indent must
                // be emitted explicitly here: this branch builds the row
                // from `name` rather than `content`, and omitting the
                // indent left folder names 2*depth columns left of file
                // names at the same depth (and made selecting a folder
                // shift it right, since the selected branch uses `content`
                // which includes the indent).
                Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(indent.clone(), Style::default()),
                    Span::styled(prefix, Style::default().fg(secondary)),
                    Span::styled(name, style),
                ])
            } else {
                Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(content, style),
                ])
            }
        })
        .collect();

    let block = Block::default()
        .title(" Password Store ")
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(secondary));

    let paragraph = Paragraph::new(Text::from(items)).block(block);

    f.render_widget(paragraph, area);
}

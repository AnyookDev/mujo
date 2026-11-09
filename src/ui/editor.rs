use super::get_colors;
use super::hints::chord_pair;
use crate::app::App;
use crate::keys::{self, Action};
use crate::utils::hslice;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub(crate) fn draw_editor(f: &mut Frame, app: &mut App, area: Rect) {
    let (primary, secondary, text, _) = get_colors(app);

    if let Some(editor) = &mut app.editor {
        let title = if editor.is_new {
            " New Entry "
        } else {
            " Edit Entry "
        };

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(secondary));

        let inner = block.inner(area);
        f.render_widget(block, area);

        // A 1-line spacer at the top matches the Preview pane's white space
        // below the pane title. The password generation controls sit between
        // the Password and Additional Content fields, with a blank line
        // above and below them. Chunk indices:
        //   new:      0=spacer 1=path 2=password 3=generation 4=content
        //   existing: 0=spacer 1=path 2=note    3=password 4=generation 5=content
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
        let gen_idx = if editor.is_new { 3 } else { 4 };
        let content_idx = if editor.is_new { 4 } else { 5 };

        let is_path_active = editor.cursor_line == 0;
        let is_path_editable = editor.is_new;

        let path_title = " Path ";

        let path_box = Block::default()
            .title(path_title)
            .borders(Borders::ALL)
            .border_style(if is_path_active && is_path_editable {
                Style::default().fg(primary)
            } else if is_path_editable {
                Style::default().fg(secondary)
            } else {
                Style::default().fg(Color::DarkGray)
            });

        let path_inner_w = chunks[path_idx].width.saturating_sub(2) as usize;
        if is_path_active && is_path_editable {
            editor.ensure_h_scroll(path_inner_w);
        }

        let path_text = if editor.path.ends_with('/') && is_path_editable && is_path_active {
            Text::from(Line::from(vec![
                Span::styled(&editor.path, Style::default().fg(text)),
                Span::styled("type a name here...", Style::default().fg(Color::DarkGray)),
            ]))
        } else {
            Text::from(hslice(&editor.path, editor.path_h_scroll))
        };

        let path_para = Paragraph::new(path_text).block(path_box);
        f.render_widget(path_para, chunks[path_idx]);

        if !editor.is_new {
            let note_text = Text::from(vec![
                Line::from(""),
                Line::from(vec![
                    Span::styled("Path locked: Use ", Style::default().fg(secondary)),
                    Span::styled("m", Style::default().fg(primary)),
                    Span::styled(
                        " in main view to move/rename",
                        Style::default().fg(secondary),
                    ),
                ]),
                Line::from(""),
            ]);
            let note_para = Paragraph::new(note_text).alignment(Alignment::Center);
            f.render_widget(note_para, chunks[2]);
        }

        let is_password_active = editor.cursor_line == 1;
        if is_password_active {
            editor.ensure_h_scroll(chunks[password_idx].width.saturating_sub(2) as usize);
        }
        let password_shown = hslice(
            editor.get_password().unwrap_or(""),
            editor.password_h_scroll,
        );

        let password_box = Block::default()
            .title(" Password ")
            .borders(Borders::ALL)
            .border_style(if is_password_active {
                Style::default().fg(primary)
            } else {
                Style::default().fg(secondary)
            });

        let password_para = Paragraph::new(password_shown).block(password_box);
        f.render_widget(password_para, chunks[password_idx]);

        // Password generation controls between the Password and Additional
        // Content fields. Not cursor-focusable: Ctrl+Up/Down adjust the
        // length and Tab cycles the charset while the Password field is
        // active, and Ctrl+g generates with these settings from anywhere in
        // the editor. Centered like the "Path locked" note, with no title
        // or bounding rules - a blank line above and below the two control
        // lines.
        let length_label = chord_pair(
            keys::advertised(Action::JumpUp).unwrap_or("Ctrl+\u{2191}"),
            keys::advertised(Action::JumpDown).unwrap_or("Ctrl+\u{2193}"),
        );
        let charset_display = match editor.gen_charset.as_str() {
            "alphanumeric" => "alphanumeric",
            "letters" => "letters",
            "custom" => "custom",
            // "all" and anything unrecognized.
            _ => "all",
        };
        let gen_lines = vec![
            Line::from(""),
            Line::from(vec![
                Span::styled("Length: ", Style::default().fg(primary)),
                Span::styled(
                    editor.gen_length.to_string(),
                    Style::default().fg(text).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("    {length_label}:+/-"),
                    Style::default().fg(secondary),
                ),
            ]),
            Line::from(vec![
                Span::styled("Charset: ", Style::default().fg(primary)),
                Span::styled(charset_display, Style::default().fg(text)),
                Span::styled("    Tab:cycle", Style::default().fg(secondary)),
            ]),
            Line::from(""),
        ];
        let gen_para = Paragraph::new(Text::from(gen_lines)).alignment(Alignment::Center);
        f.render_widget(gen_para, chunks[gen_idx]);

        let content_active = editor.cursor_line >= 2;
        let content_box = Block::default()
            .title(" Additional Content ")
            .borders(Borders::ALL)
            .border_style(if content_active {
                Style::default().fg(primary)
            } else {
                Style::default().fg(secondary)
            });

        let content_height = chunks[content_idx].height.saturating_sub(2) as usize;

        let cursor_content_line = editor.cursor_line.saturating_sub(2);
        if cursor_content_line >= editor.content_scroll_offset + content_height {
            editor.content_scroll_offset =
                cursor_content_line.saturating_sub(content_height.saturating_sub(1));
        } else if cursor_content_line < editor.content_scroll_offset {
            editor.content_scroll_offset = cursor_content_line;
        }

        let content_width = chunks[content_idx].width.saturating_sub(2) as usize;
        if content_active {
            editor.ensure_h_scroll(content_width);
        }

        let mut content_lines: Vec<Line> = vec![Line::from("")];
        let content_text: Vec<&str> = editor.content.lines().skip(1).collect();

        for (line_idx, line) in content_text
            .iter()
            .enumerate()
            .skip(editor.content_scroll_offset)
            .take(content_height)
        {
            let actual_line = line_idx + 2;

            if editor.has_selection {
                let mut spans = vec![];

                for (current_col, ch) in
                    (editor.content_h_scroll..).zip(line.chars().skip(editor.content_h_scroll))
                {
                    let in_selection = editor.is_in_selection(actual_line, current_col);
                    let style = if in_selection {
                        Style::default().bg(primary).fg(Color::Black)
                    } else {
                        Style::default().fg(text)
                    };
                    spans.push(Span::styled(ch.to_string(), style));
                }
                content_lines.push(Line::from(spans));
            } else {
                content_lines.push(Line::from(hslice(line, editor.content_h_scroll)));
            }
        }

        let content_para = Paragraph::new(Text::from(content_lines)).block(content_box);
        f.render_widget(content_para, chunks[content_idx]);

        if is_path_active && is_path_editable {
            let col = editor.cursor_col.saturating_sub(editor.path_h_scroll);
            let cursor_x = chunks[path_idx].x + 1 + col as u16;
            let cursor_y = chunks[path_idx].y + 1;
            f.set_cursor(
                cursor_x.min(chunks[path_idx].x + chunks[path_idx].width - 2),
                cursor_y,
            );
        } else if is_password_active {
            let col = editor.cursor_col.saturating_sub(editor.password_h_scroll);
            let cursor_x = chunks[password_idx].x + 1 + col as u16;
            let cursor_y = chunks[password_idx].y + 1;
            f.set_cursor(cursor_x, cursor_y);
        } else if content_active {
            let line_in_content = editor.cursor_line.saturating_sub(2);
            let vis_line = line_in_content.saturating_sub(editor.content_scroll_offset);
            let lines_in_box: Vec<&str> = editor.content.lines().skip(1).collect();
            let current_line_text = lines_in_box.get(line_in_content).unwrap_or(&"");
            let col = editor
                .cursor_col
                .min(current_line_text.len())
                .saturating_sub(editor.content_h_scroll);
            let cursor_x = chunks[content_idx].x + 1 + col as u16;
            let cursor_y = chunks[content_idx].y + 2 + vis_line as u16;
            let cursor_y = cursor_y.min(chunks[content_idx].y + chunks[content_idx].height - 2);
            f.set_cursor(cursor_x, cursor_y);
        }
    }
}

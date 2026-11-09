use super::get_colors;
use crate::app::App;
use crate::models::NewStoreStage;
use crate::pass::store_path;
use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub(crate) fn draw_new_store(f: &mut Frame, app: &App, area: Rect) {
    let (primary, secondary, text, _) = get_colors(app);

    let block = Block::default()
        .title(" New Password Store ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(secondary));

    let inner = block.inner(area);
    f.render_widget(block, area);

    match app.new_store_stage {
        NewStoreStage::Confirm => {
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!(
                        "No pass password store was found at {}.",
                        store_path().display()
                    ),
                    Style::default().fg(text),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "Would you like to set one up?",
                    Style::default().fg(text),
                )),
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        "Y",
                        Style::default().fg(primary).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(":set it up  ", Style::default().fg(secondary)),
                    Span::styled(
                        "Esc",
                        Style::default().fg(primary).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(":quit", Style::default().fg(secondary)),
                ]),
            ];
            f.render_widget(
                Paragraph::new(Text::from(lines)).alignment(Alignment::Center),
                inner,
            );
        }
        NewStoreStage::AskGpgId => {
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "Enter your GPG ID (key fingerprint or email):",
                    Style::default().fg(secondary),
                )),
                Line::from(Span::styled(
                    app.command_buffer.clone(),
                    Style::default().fg(primary),
                )),
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        "Enter",
                        Style::default().fg(primary).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(":create store   ", Style::default().fg(secondary)),
                    Span::styled(
                        "Esc",
                        Style::default().fg(primary).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(":back", Style::default().fg(secondary)),
                ]),
            ];
            f.render_widget(
                Paragraph::new(Text::from(lines)).alignment(Alignment::Center),
                inner,
            );
            // The input line is centered ON ITS OWN (the prompt above is a
            // separate line with its own centering), so the text starts at
            // (inner_w - buffer_len) / 2. The logical cursor (command_cursor,
            // a character index that Left/Right/Home/End move within the
            // text) is drawn at lead + cursor, so it can sit anywhere in -
            // or immediately after - the entered text, including mid-text
            // after a paste.
            let buf_len = app.command_buffer.chars().count();
            let cur = app.command_cursor.min(buf_len);
            let inner_w = inner.width as usize;
            let lead = inner_w.saturating_sub(buf_len) / 2;
            let cursor_x = inner.x + lead.min(inner_w.saturating_sub(1)) as u16 + cur as u16;
            let cursor_y = inner.y + 2;
            f.set_cursor(
                cursor_x.min(inner.x + inner.width.saturating_sub(1)),
                cursor_y,
            );
        }
    }
}

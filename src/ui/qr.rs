use super::get_colors;
use crate::app::App;
use crate::models::{AppMode, DecryptedContent};
use crate::utils::hslice;
use qrcode::QrCode;
use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

fn qr_code_lines(qr: &QrCode, inner_width: usize, inner_height: usize) -> Vec<Line<'static>> {
    const QUIET_ZONE: usize = 1;

    let colors = qr.to_colors();
    let side = (colors.len() as f64).sqrt().round() as usize;
    if side == 0 {
        return Vec::new();
    }
    let total = side + QUIET_ZONE * 2;
    // The solid renderer emits TWO cells per module (two spaces with bg),
    // so it needs total * 2 columns as well as total rows. The half-block
    // fallback is one cell per module horizontally and fits narrow panes
    // while keeping modules roughly square.
    if inner_height >= total && inner_width >= total * 2 {
        qr_code_lines_solid(&colors, side, total, QUIET_ZONE)
    } else {
        qr_code_lines_halfblock(qr)
    }
}

/// The solid one-module-per-cell renderer. Each module is a run of TWO
/// cells (two spaces, bg only) so modules stay roughly square on an
/// ~8x19 character cell. No even-height padding is needed: there are no
/// glyph pairs, so every row is a full module row.
fn qr_code_lines_solid(
    colors: &[qrcode::Color],
    side: usize,
    total: usize,
    quiet_zone: usize,
) -> Vec<Line<'static>> {
    let at = |r: usize, c: usize| {
        r >= quiet_zone
            && r < quiet_zone + side
            && c >= quiet_zone
            && c < quiet_zone + side
            && colors[(r - quiet_zone) * side + (c - quiet_zone)] == qrcode::Color::Dark
    };

    let dark_style = Style::default().fg(Color::Black).bg(Color::Black);
    let light_style = Style::default().fg(Color::White).bg(Color::White);

    let mut lines = Vec::new();
    for r in 0..total {
        let mut spans = Vec::new();
        for c in 0..total {
            let style = if at(r, c) { dark_style } else { light_style };
            spans.push(Span::styled("  ".to_string(), style));
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// Half-block fallback for short panes: two module rows per terminal row
/// using half-block characters. Dark modules are black on a white
/// background (and light modules white) so scanners get the contrast they
/// expect regardless of the terminal theme. The QR is left-aligned like the
/// other preview content.
///
/// A quiet zone (white margin) is rendered on all four sides - scanners
/// need it to locate the code's edges. The total height is padded to an
/// even number of module rows so the last terminal row is always a full
/// module pair, avoiding a stray half-module white strip at the bottom.
fn qr_code_lines_halfblock(qr: &QrCode) -> Vec<Line<'static>> {
    const QUIET_ZONE: usize = 2;

    let colors = qr.to_colors();
    let side = (colors.len() as f64).sqrt().round() as usize;
    if side == 0 {
        return Vec::new();
    }

    let total = side + QUIET_ZONE * 2;
    let height = total + (total % 2);

    // Out-of-bounds coordinates (the quiet zone and any bottom padding)
    // count as light modules.
    let at = |r: usize, c: usize| {
        r >= QUIET_ZONE
            && r < QUIET_ZONE + side
            && c >= QUIET_ZONE
            && c < QUIET_ZONE + side
            && colors[(r - QUIET_ZONE) * side + (c - QUIET_ZONE)] == qrcode::Color::Dark
    };

    let dark_style = Style::default().fg(Color::Black).bg(Color::White);
    // Uniform dark cell: fg AND bg Black, so the glyph's line-box insets
    // expose Black (the module's own colour) instead of the White
    // background. Terminal.app's insets would otherwise stripe every solid
    // dark module white.
    let solid_dark_style = Style::default().fg(Color::Black).bg(Color::Black);
    // Inverse half-block style (fg White, bg Black), shared by the mixed
    // cell variants whose exposed edge band must show the opposite
    // module's colour.
    let inverse_style = Style::default().fg(Color::White).bg(Color::Black);
    let light_style = Style::default().fg(Color::White).bg(Color::White);

    let mut lines = Vec::new();
    let mut r = 0;
    while r < height {
        let mut spans = Vec::new();
        for c in 0..total {
            let top = at(r, c);
            // height is even, so r + 1 is always a valid row here.
            let bottom = at(r + 1, c);
            // Row above the cell: r - 1 underflows at r == 0, where the
            // quiet zone above counts as light.
            let above = r > 0 && at(r - 1, c);
            // height is even, so r + 2 is always a valid row here (at()
            // returns false for the out-of-range final row).
            let below = at(r + 2, c);
            // Mixed cells: a half block's ink starts one inset below the
            // cell top (▀) or ends one inset above the cell bottom (▄),
            // exposing one band of the bg colour at that edge. Pick the
            // variant whose exposed band merges with the neighbouring
            // module: ▀ when the module above equals the bottom module,
            // else ▄ when the module below equals the top module, else ▀.
            let (ch, style) = match (top, bottom) {
                // Uniform cells are glyph-independent: the exposed insets
                // show the module's own colour.
                (true, true) => ('█', solid_dark_style),
                (false, false) => (' ', light_style),
                (true, false) => {
                    if !above {
                        // Band at the top shows the light bottom colour.
                        ('▀', dark_style)
                    } else if below {
                        // Band at the bottom shows the dark top colour.
                        ('▄', inverse_style)
                    } else {
                        ('▀', dark_style)
                    }
                }
                (false, true) => {
                    if above {
                        // Band at the top shows the dark bottom colour.
                        ('▀', inverse_style)
                    } else if !below {
                        // Band at the bottom shows the light top colour.
                        ('▄', dark_style)
                    } else {
                        ('▀', inverse_style)
                    }
                }
            };
            spans.push(Span::styled(ch.to_string(), style));
        }
        lines.push(Line::from(spans));
        r += 2;
    }
    lines
}

pub(crate) fn draw_qr_view(f: &mut Frame, app: &App, area: Rect, footer_area: Rect) {
    let (primary, secondary, _, _) = get_colors(app);

    let password = app
        .current_content
        .as_ref()
        .and_then(|c| c.lines.first().cloned())
        .unwrap_or_default();

    // Full available height/width of the QR area: the chooser compares
    // against these, so a QR that fits renders solid and never shares the
    // area with other content that could clip it.
    let qr_lines = QrCode::with_error_correction_level(password.as_bytes(), qrcode::EcLevel::L)
        .ok()
        .map(|code| qr_code_lines(&code, area.width as usize, area.height as usize))
        .unwrap_or_default();

    let mut lines: Vec<Line> = Vec::new();
    if qr_lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "Could not generate QR code for this password",
            Style::default().fg(Color::Red),
        )));
    } else {
        // Top-aligned: the paragraph starts at the area's first row.
        lines.extend(qr_lines);
    }
    f.render_widget(Paragraph::new(Text::from(lines)), area);

    // Footer: only the close hint (key:value, no space after the colon).
    // Only w is advertised: while the QR is shown it closes the view
    // (Esc does the same), r/c/e close it and then act in the normal
    // preview, and every other key is ignored.
    let key_style = Style::default().fg(primary).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(secondary);
    let footer = vec![Span::styled("w", key_style), Span::styled(":close", dim)];
    f.render_widget(Paragraph::new(Text::from(Line::from(footer))), footer_area);
}

pub(crate) fn draw_preview(f: &mut Frame, app: &mut App, area: Rect) {
    let (primary, secondary, text, _) = get_colors(app);

    let block = Block::default()
        .title(" Preview ")
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(secondary));

    let block = block
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(secondary));

    // While the editor is open (or its unsaved-changes confirmation is
    // pending), the preview shows the PENDING content being edited - parsed
    // from the editor's current state - instead of the on-disk original, so
    // the user sees exactly what they are about to save or discard. The
    // password stays masked by the usual show_password logic.
    // current_content is left untouched: on discard it still holds the
    // original on-disk content, which the preview falls back to.
    let pending_content = if matches!(app.mode, AppMode::Editor | AppMode::ConfirmDiscard) {
        app.editor
            .as_ref()
            .map(|e| DecryptedContent::parse(&e.to_content()))
    } else {
        None
    };
    // Owned copy: holding a borrow of app.current_content across the
    // preview_h_scroll / preview_scroll_offset mutations below would not
    // borrow-check.
    let effective_content: Option<DecryptedContent> =
        pending_content.or_else(|| app.current_content.clone());

    if let Some(content) = &effective_content {
        let inner_width = area.width.saturating_sub(2) as usize;
        let max_len = content
            .lines
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0);
        let max_h = max_len.saturating_sub(inner_width.max(1));
        if app.preview_h_scroll > max_h {
            app.preview_h_scroll = max_h;
        }
        let h = app.preview_h_scroll;
        let show_password = app.show_password;
        let show_qr = app.show_qr;
        let selected_line = app.preview_selected_line;

        // Pane size available to the QR block (the block's borders take
        // two rows and two columns), needed to pick the solid vs
        // half-block QR renderer.
        let inner_width = area.width.saturating_sub(2) as usize;
        let inner_height = area.height.saturating_sub(2) as usize;

        // Build the QR block once per frame when it is shown. Low error
        // correction keeps the code small (21x21 / 25x25 modules for typical
        // passwords) so it fits comfortably in the pane.
        let qr_block: Option<Vec<Line>> = if show_qr {
            content
                .lines
                .first()
                .cloned()
                .filter(|pwd| !pwd.is_empty())
                .and_then(|pwd| {
                    QrCode::with_error_correction_level(pwd.as_bytes(), qrcode::EcLevel::L)
                        .ok()
                        .map(|code| qr_code_lines(&code, inner_width, inner_height))
                })
        } else {
            None
        };

        let mut lines = vec![];
        {
            // `content` is the outer `if let` binding: either the editor's
            // pending content or the on-disk original.
            lines.push(Line::from(""));

            for (i, line) in content.lines.iter().enumerate() {
                let selected = selected_line == Some(i);
                let line_style = if selected {
                    Style::default().bg(primary).fg(Color::Black)
                } else {
                    Style::default().fg(text)
                };

                if i == 0 {
                    let password = if show_password {
                        hslice(line, h)
                    } else {
                        hslice(&"*".repeat(line.len()), h)
                    };
                    lines.push(Line::from(vec![
                        Span::styled(
                            "pwd: ",
                            Style::default().fg(primary).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(password, line_style),
                    ]));
                    lines.push(Line::from(""));

                    // QR display: left-aligned like the other preview content.
                    // The additional information lines below are shifted down
                    // by the height of the QR block.
                    if show_qr {
                        if let Some(qr_lines) = &qr_block {
                            lines.push(Line::from(""));
                            lines.extend(qr_lines.iter().cloned());
                            lines.push(Line::from(""));
                        } else {
                            lines.push(Line::from(Span::styled(
                                "Could not generate QR code for this password",
                                Style::default().fg(Color::Red),
                            )));
                            lines.push(Line::from(""));
                        }
                    }
                } else {
                    lines.push(Line::from(Span::styled(hslice(line, h), line_style)));
                }
            }

            if let Some(totp) = &content.totp {
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled(
                        "TOTP: ",
                        Style::default().fg(primary).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(totp.generate(), Style::default().fg(Color::Green)),
                    Span::styled(
                        format!(" ({}s)", totp.time_remaining()),
                        Style::default().fg(secondary),
                    ),
                ]));
            }
        }

        let max_scroll = lines.len().saturating_sub(inner_height);
        if app.preview_scroll_offset > max_scroll {
            app.preview_scroll_offset = max_scroll;
        }

        let visible_lines: Vec<Line> = lines
            .into_iter()
            .skip(app.preview_scroll_offset)
            .take(inner_height)
            .collect();

        let paragraph = Paragraph::new(Text::from(visible_lines)).block(block);

        f.render_widget(paragraph, area);
    } else {
        let has_selection = !app.filtered_entries.is_empty();
        let selected_is_file = app
            .filtered_entries
            .get(app.selected)
            .map(|&idx| !app.entries[idx].is_dir)
            .unwrap_or(false);

        if !has_selection {
            let paragraph = Paragraph::new("No entries found")
                .style(Style::default().fg(secondary))
                .block(block)
                .alignment(Alignment::Center);
            f.render_widget(paragraph, area);
        } else if selected_is_file {
            let line = Line::from(vec![
                Span::styled("Press ", Style::default().fg(secondary)),
                Span::styled(
                    "Enter",
                    Style::default().fg(primary).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" to decrypt", Style::default().fg(secondary)),
            ]);
            let paragraph = Paragraph::new(line)
                .block(block)
                .alignment(Alignment::Center);
            f.render_widget(paragraph, area);
        } else {
            f.render_widget(block, area);
        }
    }
}

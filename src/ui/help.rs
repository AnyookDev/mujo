use super::get_colors;
use super::hints::chord_pair;
use crate::app::App;
use crate::keys::{self, Action};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

/// Marks a line drawn entirely in the value colour (no separate key
/// colour). The rest of the row keeps the 14-cell key column layout.
const VALUE_ROW: char = '\u{1}';

/// A text line shown entirely in the value colour. The key is followed by a
/// single space, with no padding to the keybinding columns.
fn value_row(key: &str, value: &str) -> String {
    format!("{VALUE_ROW}{} {}", key, value)
}

fn push_help_group(lines: &mut Vec<String>, heading: &str, entries: &[(&str, &str)]) {
    lines.push(String::new());
    lines.push(heading.to_string());
    for (key, desc) in entries {
        lines.push(format!("{:<14}{}", key, desc));
    }
}

/// Every binding in the app, grouped into NORMAL, PREVIEW, EDITOR and
/// GIT. Entry keys stay under 14 cells so the padded key/description
/// split is always clean. Keyed on the active keyboard platform.
pub(crate) fn help_entries() -> Vec<String> {
    help_entries_for(keys::keyboard())
}

/// help_entries keyed on an explicit platform, so tests can exercise both
/// platforms (ACTIVE_KEYBOARD is a OnceLock and can only be set once per
/// process) - the same pattern as keys::advertised_for.
pub(crate) fn help_entries_for(platform: keys::Platform) -> Vec<String> {
    let mut lines = Vec::new();

    // Chord labels are platform-aware: a Mac user sees Ctrl+U/D, Ctrl+A/E,
    // b/Space and g/G where a Linux user sees Ctrl+arrows, Home/End and
    // PgUp/PgDn. Every chord works on every platform either way; only the
    // advertising differs.
    let jump = chord_pair(
        keys::advertised_for(Action::JumpUp, platform).unwrap_or("Ctrl+\u{2191}"),
        keys::advertised_for(Action::JumpDown, platform).unwrap_or("Ctrl+\u{2193}"),
    );
    let line_nav = chord_pair(
        keys::advertised_for(Action::LineStart, platform).unwrap_or("Home"),
        keys::advertised_for(Action::LineEnd, platform).unwrap_or("End"),
    );
    let page = chord_pair(
        keys::advertised_for(Action::PageUp, platform).unwrap_or("PgUp"),
        keys::advertised_for(Action::PageDown, platform).unwrap_or("PgDn"),
    );
    let top_bottom = chord_pair(
        keys::advertised_for(Action::Top, platform).unwrap_or("Home"),
        keys::advertised_for(Action::Bottom, platform).unwrap_or("End"),
    );
    // Delete is Fn+Backspace on a Mac keyboard; the chord is identical.
    let delete_key = keys::advertised_for(Action::DeleteForward, platform).unwrap_or("Delete");

    push_help_group(
        &mut lines,
        "NORMAL",
        &[
            ("Up/Down, k/j", "move selection in the tree"),
            ("Left/h", "collapse folder, or go to parent"),
            ("Right/l", "expand folder, or move into it"),
            ("Enter", "decrypt entry (or expand folder)"),
            (jump.as_str(), "jump 5 siblings up/down"),
            ("Shift+Ltr", "jump to next entry starting with letter"),
            ("/", "search/filter entries"),
            ("n", "new entry"),
            ("e", "edit selected entry"),
            ("d", "delete selected entry"),
            ("m", "move/rename entry"),
            ("f", "add folder"),
            ("c", "copy password to clipboard"),
            ("t", "copy TOTP code to clipboard"),
            ("u", "copy username to clipboard"),
            ("Ctrl+c", "copy password, or the selected line"),
            ("r", "reveal/hide password"),
            ("w", "show/hide password QR code"),
            ("p", "git pull"),
            ("Ctrl+p", "git push"),
            ("Ctrl+r", "reload config.toml"),
            ("g", "open the git pane"),
            ("q / Ctrl+q", "quit"),
            ("?", "open this help view"),
        ],
    );

    push_help_group(
        &mut lines,
        "PREVIEW",
        &[
            ("Shift+↑↓", "scroll preview vertically"),
            ("Shift+←→", "scroll preview horizontally"),
            (page.as_str(), "scroll preview by page"),
            ("Esc", "close preview (clears a filter first)"),
        ],
    );

    push_help_group(
        &mut lines,
        "EDITOR",
        &[
            ("Up/Down", "move between fields and lines"),
            ("Left/Right", "move cursor (with Shift: select)"),
            ("Shift+↑↓", "extend selection by line"),
            (line_nav.as_str(), "start/end of the current line"),
            ("Enter", "newline (in Additional Content)"),
            ("Backspace", "delete character before cursor"),
            (delete_key, "delete character after cursor"),
            ("Tab", "cycle charset (Password) / indent (content)"),
            ("Shift+Tab", "dedent line (Additional Content)"),
            (
                jump.as_str(),
                "generated password length -/+ (Password field)",
            ),
            ("Ctrl+g", "generate password into the Password field"),
            ("Ctrl+s", "save entry"),
            ("Ctrl+c", "copy selection"),
            ("Ctrl+v", "paste from clipboard"),
            ("Esc", "close editor (asks when unsaved)"),
        ],
    );

    push_help_group(
        &mut lines,
        "GIT",
        &[
            ("Ctrl+p", "push"),
            ("p", "pull"),
            ("r", "refresh status and log"),
            (page.as_str(), "scroll log by page"),
            (top_bottom.as_str(), "jump to top/bottom of the log"),
            ("Shift+↑↓", "scroll log vertically"),
            ("Shift+←→", "scroll pane horizontally"),
            ("Esc / q", "close the git pane"),
        ],
    );

    // Entry formatting: how to write an entry so the t and u copy keys have
    // something to copy. Prose lines (two-space prefix) render as notes.
    push_help_group(&mut lines, "ENTRY FORMATTING", &[]);
    lines.push("  Entries can have the following special key/value pairs:".to_string());
    lines.push(String::new());
    lines.push(value_row("totp:", "<your totp secret>"));
    lines.push(value_row("username:", "<your username>"));
    lines.push(String::new());
    lines.push("  Press t to copy the 6-digit totp code".to_string());
    lines.push("  Press u to copy the username".to_string());

    // Platform caveats and the universal help key, as prose lines. The
    // leading two spaces make draw_help render them as note paragraphs
    // (secondary color) instead of key/description pairs. The macOS
    // caveats are gated on the resolved keyboard, not the compile target:
    // a Linux user SSHing from a Mac with LC_MUJO_KEYBOARD=mac still sees
    // them.
    lines.push(String::new());
    if platform == keys::Platform::Mac {
        lines.push("  macOS captures Ctrl+arrow for Mission Control and Spaces.".to_string());
        lines.push(
            "  Home/End/PgUp/PgDn need Fn on a Mac keyboard; the alternatives above do not."
                .to_string(),
        );
    }
    lines.push(String::new());
    lines.push("  Ctrl+/ or F1 opens this help from any mode.".to_string());

    lines
}

/// A group heading: uppercase letters and spaces only ("NORMAL", "ENTRY
/// FORMATTING"). Key/description rows and prose notes never match.
pub(crate) fn is_group_heading(line: &str) -> bool {
    !line.is_empty() && line.chars().all(|c| c.is_ascii_uppercase() || c == ' ')
}

/// Full-screen help view (AppMode::Help): a titled block with a scrollable
/// body listing every binding, modelled on draw_git's log section. The
/// whole main area is given over to it (see draw), so the sidebar steps
/// aside.
pub(crate) fn draw_help(f: &mut Frame, app: &mut App, area: Rect) {
    let (primary, secondary, text, _) = get_colors(app);

    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(secondary));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let body_height = inner.height as usize;
    let all_lines = help_entries();
    let max_scroll = all_lines.len().saturating_sub(body_height.max(1));
    if app.help_scroll_offset > max_scroll {
        app.help_scroll_offset = max_scroll;
    }

    let lines: Vec<Line> = all_lines
        .iter()
        .skip(app.help_scroll_offset)
        .take(body_height)
        .map(|l| {
            if l.is_empty() {
                Line::from("")
            } else if is_group_heading(l) {
                // Group heading: bold text colour, so the headings read by
                // weight rather than by accent.
                Line::from(Span::styled(
                    l.clone(),
                    Style::default().fg(text).add_modifier(Modifier::BOLD),
                ))
            } else if let Some(row) = l.strip_prefix(VALUE_ROW) {
                // Value row: the whole row in the value (text) colour.
                Line::from(Span::styled(row.to_string(), Style::default().fg(text)))
            } else if l.starts_with("  ") {
                // Note paragraph. The leading spaces are only the note marker
                // and are not drawn, so notes sit flush left like key rows.
                Line::from(Span::styled(
                    l.trim_start().to_string(),
                    Style::default().fg(secondary),
                ))
            } else {
                // Key/description pair: the key is padded to 14 cells.
                let key: String = l.chars().take(14).collect();
                let desc: String = l.chars().skip(14).collect();
                Line::from(vec![
                    Span::styled(
                        key,
                        Style::default().fg(primary).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(desc, Style::default().fg(text)),
                ])
            }
        })
        .collect();

    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ENTRY FORMATTING section sits after GIT and before the platform
    /// caveats, is recognised as a heading (it contains a space), and tells
    /// users the exact keys the t and u copy bindings read.
    #[test]
    fn entry_formatting_section_is_placed_and_documents_keys() {
        let lines = help_entries_for(keys::Platform::Linux);
        let pos = |needle: &str| {
            lines
                .iter()
                .position(|l| l == needle)
                .unwrap_or_else(|| panic!("missing line: {needle}"))
        };
        let git = pos("GIT");
        let fmt = pos("ENTRY FORMATTING");
        let caveat = pos("  Ctrl+/ or F1 opens this help from any mode.");
        assert!(git < fmt && fmt < caveat, "section out of order");
        assert!(is_group_heading(&lines[fmt]));
        let text = lines.join("\n");
        // Value rows: the key is padded to the 14-cell key column and the
        // row is marked for the value colour.
        assert!(lines.contains(&value_row("totp:", "<your totp secret>")));
        assert!(lines.contains(&value_row("username:", "<your username>")));
        assert!(text.contains("Press t to copy the 6-digit totp code"));
        assert!(text.contains("Press u to copy the username"));
    }

    #[test]
    fn group_heading_detection() {
        assert!(is_group_heading("NORMAL"));
        assert!(is_group_heading("ENTRY FORMATTING"));
        assert!(!is_group_heading(""));
        assert!(!is_group_heading("Enter"));
        assert!(!is_group_heading("  Ctrl+/ or F1 opens this help"));
    }
}

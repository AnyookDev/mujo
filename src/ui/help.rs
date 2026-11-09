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
            } else if l.chars().all(|c| c.is_ascii_uppercase()) {
                // Group heading: bold text colour, so the headings read by
                // weight rather than by accent.
                Line::from(Span::styled(
                    l.clone(),
                    Style::default().fg(text).add_modifier(Modifier::BOLD),
                ))
            } else if l.starts_with("  ") {
                // Note paragraph.
                Line::from(Span::styled(l.clone(), Style::default().fg(secondary)))
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

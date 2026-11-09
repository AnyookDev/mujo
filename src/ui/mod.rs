pub mod glyphs;
pub(crate) mod git_pane;
pub(crate) mod hints;
pub(crate) mod qr;
pub(crate) mod sidebar;
pub(crate) mod new_store;
pub(crate) mod editor;
pub(crate) mod help;
pub(crate) mod status_bar;

pub use glyphs::{
    cap_style, downgrade, effective_glyph_mode, effective_truecolor_mode,
    glyphs, resolve_glyphs, resolve_truecolor, set_glyphs, set_truecolor,
    Glyphs, GLYPHS_ASCII, GLYPHS_BMP, GLYPHS_NERD,
};
pub(crate) use glyphs::pick_mode;

use crate::app::App;
use crate::models::AppMode;
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::Span,
    widgets::Block,
    Frame,
};

pub fn draw(f: &mut Frame, app: &mut App) {
    // While the password QR is shown in Normal mode, the QR gets the whole
    // window: search box, sidebar, status bar and helpbar are hidden and
    // the preview pane expands to the full terminal area, giving the QR
    // renderer the largest possible inner size for its render-path
    // selection. No new AppMode: this is preview mode with the chrome
    // hidden, so the existing w (toggle QR) and Esc (close preview)
    // bindings keep working unchanged and the layout restores cleanly
    // when the QR is toggled off.
    let qr_fullscreen = app.show_qr && app.mode == AppMode::Normal;

    let status_constraint = if app.mode == AppMode::AddFolder || app.mode == AppMode::Move {
        Constraint::Length(2)
    } else {
        Constraint::Max(2)
    };

    let main_chunks = if qr_fullscreen {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0)])
            .split(f.size())
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(5), status_constraint])
            .split(f.size())
    };

    let main_panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(main_chunks[0]);

    let left_column = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(main_panes[0]);

    // Paint the themed background across the whole frame first (if a theme
    // with a background is active). Widgets drawn afterwards with a default
    // style leave this background visible.
    let (_, _, text, bg) = get_colors(app);
    f.render_widget(
        Block::default().style(Style::default().fg(text).bg(bg)),
        f.size(),
    );

    if app.mode == AppMode::Help {
        // Full-screen help: rendered across the whole main area so the
        // sidebar steps aside and there is room for key + description
        // pairs. Pane replacement (like Editor/Git), not an overlay.
        help::draw_help(f, app, main_chunks[0]);
    } else if qr_fullscreen {
        // QR fullscreen: a borderless QR area over a single footer row.
        // Borderless keeps the row budget to QR rows + 1 footer, which
        // fits a 25-row window; the footer carries the close hint the
        // hidden status bar can no longer show.
        let qr_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(main_chunks[0]);
        qr::draw_qr_view(f, app, qr_chunks[0], qr_chunks[1]);
    } else {
        sidebar::draw_search_bar(f, app, left_column[0]);
        sidebar::draw_sidebar(f, app, left_column[1]);

        if app.mode == AppMode::Editor {
            editor::draw_editor(f, app, main_panes[1]);
        } else if app.mode == AppMode::Git {
            git_pane::draw_git(f, app, main_panes[1]);
        } else if app.mode == AppMode::NewStore {
            new_store::draw_new_store(f, app, main_panes[1]);
        } else {
            qr::draw_preview(f, app, main_panes[1]);
        }
    }

    // The status bar (which carries the helpbar) is hidden while the QR
    // owns the window; the exit affordance moves into the pane's title.
    if !qr_fullscreen {
        status_bar::draw_status_bar(f, app, main_chunks[1]);
    }

    if let Some(ref menu) = app.context_menu {
        let (primary, secondary, text, bg) = get_colors(app);
        status_bar::draw_context_menu(f, menu, primary, secondary, text, bg);
    }
}

pub(crate) fn get_colors(app: &App) -> (Color, Color, Color, Color) {
    let theme = app.config.effective_theme();
    let primary = downgrade(app.config.parse_color(&theme.accent));
    let secondary = downgrade(app.config.parse_color(&theme.secondary));
    let text = downgrade(app.config.parse_color(&theme.foreground));
    let background = if theme.background.trim().is_empty() {
        Color::Reset
    } else {
        downgrade(app.config.parse_color(&theme.background))
    };
    (primary, secondary, text, background)
}

pub(crate) fn message_style(msg: &str, text: Color) -> Style {
    if msg.starts_with("Error") || msg.starts_with("Decrypt error") {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(text)
    }
}


pub(crate) fn relative_time(elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 30 {
        "just now".to_string()
    } else if secs < 3600 {
        let mins = secs / 60;
        if mins <= 1 {
            "1 min ago".to_string()
        } else {
            format!("{} min ago", mins)
        }
    } else {
        let hours = secs / 3600;
        if hours <= 1 {
            "1 hour ago".to_string()
        } else {
            format!("{} hours ago", hours)
        }
    }
}

pub(crate) fn hslice_spans(spans: Vec<Span>, off: usize) -> Vec<Span> {
    if off == 0 {
        return spans;
    }
    let mut result: Vec<Span> = Vec::new();
    let mut skip = off;
    for span in spans {
        let len = span.content.chars().count();
        if skip >= len {
            skip -= len;
            continue;
        }
        let content: String = span.content.chars().skip(skip).collect();
        skip = 0;
        result.push(Span::styled(content, span.style));
    }
    result
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_relative_time_granularity() {
        assert_eq!(
            relative_time(std::time::Duration::from_secs(10)),
            "just now"
        );
        assert_eq!(
            relative_time(std::time::Duration::from_secs(90)),
            "1 min ago"
        );
        assert_eq!(
            relative_time(std::time::Duration::from_secs(7200)),
            "2 hours ago"
        );
    }

    #[test]
    fn hslice_empty_offset_returns_original() {
        let spans = vec![Span::raw("hello"), Span::raw(" world")];
        let result = hslice_spans(spans.clone(), 0);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn hslice_offset_clips_first_span() {
        let spans = vec![Span::raw("hello"), Span::raw(" world")];
        let result = hslice_spans(spans, 3);
        assert_eq!(result[0].content, "lo");
    }
}

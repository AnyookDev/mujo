use super::{get_colors, hslice_spans, relative_time};
use crate::app::App;
use crate::git;
use crate::models::{GitPaneState, SyncStatus};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

fn tracking_label(
    ahead: u32,
    behind: u32,
    has_upstream: bool,
    has_remote: bool,
    uncommitted: usize,
) -> String {
    let base = if has_upstream {
        format!("{} ahead, {} behind", ahead, behind)
    } else if has_remote {
        "no upstream".to_string()
    } else {
        "no remote".to_string()
    };
    if uncommitted > 0 {
        format!("{}, {} uncommitted", base, uncommitted)
    } else {
        base
    }
}

/// Spans for the Git pane's Status row: "{remote}  {tracking}" with the
/// remote name in the muted secondary colour and the tracking label in
/// the foreground. When no remote is configured there is no name to
/// print, so the row is the tracking label alone - exactly "no remote",
/// never "origin no remote". The name is also omitted when none resolved
/// (several remotes, no upstream), where the label alone is likewise the
/// only honest content.
fn status_row_spans(
    remote_name: &str,
    has_remote: bool,
    tracking: &str,
    secondary: Color,
    text: Color,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if has_remote && !remote_name.is_empty() {
        spans.push(Span::styled(
            format!("{}  ", remote_name),
            Style::default().fg(secondary),
        ));
    }
    spans.push(Span::styled(
        tracking.to_string(),
        Style::default().fg(text),
    ));
    spans
}

/// The branch row's display name: "detached" when HEAD is detached (git
/// reports "## HEAD (no branch)", which parses as the branch "HEAD"),
/// otherwise the branch name unchanged.
fn display_branch(branch: &str) -> &str {
    if branch == "HEAD" {
        "detached"
    } else {
        branch
    }
}

/// The Status block's hint line, three-way on remote/upstream state: a
/// fix-it for a store with no remote at all, reassurance that the first
/// push establishes tracking when a remote exists but nothing tracks the
/// branch, and no hint at all once an upstream is tracked.
fn upstream_hint(has_remote: bool, has_upstream: bool) -> Option<&'static str> {
    if has_upstream {
        None
    } else if has_remote {
        Some("no upstream - set on first push")
    } else {
        // Short enough to fit the half-width Status block without
        // truncating; the config route stays discoverable.
        Some("git remote add <name> <url> (or config git.remote_url)")
    }
}

/// Spans for the Git pane's Sync line on success: "{action} {remote}
/// ({url})" with the URL in the muted secondary colour, so a local-path
/// remote or a multi-remote store says which remote was used. Degrades to
/// the previous "{action} {url}" shape when no remote name is known (a
/// record whose remote could not be resolved). The caller appends the
/// relative age in its own parens.
fn sync_success_spans(
    sync: &SyncStatus,
    age: &str,
    text: Color,
    secondary: Color,
) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(
        format!("{} ", sync.action),
        Style::default().fg(text),
    )];
    if !sync.remote.is_empty() {
        spans.push(Span::styled(
            format!("{} ", sync.remote),
            Style::default().fg(text),
        ));
    }
    if !sync.url.is_empty() {
        let url_text = if sync.remote.is_empty() {
            sync.url.clone()
        } else {
            format!("({})", sync.url)
        };
        spans.push(Span::styled(url_text, Style::default().fg(secondary)));
        spans.push(Span::styled(" ", Style::default().fg(text)));
    }
    spans.push(Span::styled(
        format!("({})", age),
        Style::default().fg(text),
    ));
    spans
}
pub(crate) fn draw_git(f: &mut Frame, app: &mut App, area: Rect) {
    let (primary, secondary, text, _) = get_colors(app);

    let block = Block::default()
        .title(" Git ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(secondary));

    let inner = block.inner(area);
    f.render_widget(block, area);

    match app.git_pane {
        GitPaneState::Uninitialized => {
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "Git is not set up for this password store.",
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
                    Span::styled(":cancel", Style::default().fg(secondary)),
                ]),
            ];
            f.render_widget(Paragraph::new(Text::from(lines)), inner);
        }
        GitPaneState::AskRemote => {
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled("Git initialized.", Style::default().fg(text))),
                Line::from(""),
                Line::from(Span::styled(
                    "Enter remote URL:",
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
                    Span::styled(":add   ", Style::default().fg(secondary)),
                    Span::styled(
                        "Esc",
                        Style::default().fg(primary).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(":skip", Style::default().fg(secondary)),
                ]),
            ];
            f.render_widget(Paragraph::new(Text::from(lines)), inner);
            let cursor_x = inner.x + app.command_buffer.len() as u16;
            let cursor_y = inner.y + 4;
            f.set_cursor(
                cursor_x.min(inner.x + inner.width.saturating_sub(1)),
                cursor_y,
            );
        }
        GitPaneState::AskIdentityName => {
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "Git needs an identity to create the initial commit.",
                    Style::default().fg(text),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "Enter your name (user.name):",
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
                    Span::styled(":next   ", Style::default().fg(secondary)),
                    Span::styled(
                        "Esc",
                        Style::default().fg(primary).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(":skip commit", Style::default().fg(secondary)),
                ]),
            ];
            f.render_widget(Paragraph::new(Text::from(lines)), inner);
            let cursor_x = inner.x + app.command_buffer.len() as u16;
            let cursor_y = inner.y + 4;
            f.set_cursor(
                cursor_x.min(inner.x + inner.width.saturating_sub(1)),
                cursor_y,
            );
        }
        GitPaneState::AskIdentityEmail => {
            let lines = vec![
                Line::from(""),
                Line::from(vec![
                    Span::styled("Name: ", Style::default().fg(secondary)),
                    Span::styled(app.git_identity_name.clone(), Style::default().fg(text)),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    "Enter your email (user.email):",
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
                    Span::styled(":set & commit   ", Style::default().fg(secondary)),
                    Span::styled(
                        "Esc",
                        Style::default().fg(primary).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(":skip commit", Style::default().fg(secondary)),
                ]),
            ];
            f.render_widget(Paragraph::new(Text::from(lines)), inner);
            let cursor_x = inner.x + app.command_buffer.len() as u16;
            let cursor_y = inner.y + 4;
            f.set_cursor(
                cursor_x.min(inner.x + inner.width.saturating_sub(1)),
                cursor_y,
            );
        }
        GitPaneState::Ready => {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(8),
                    Constraint::Length(4),
                    Constraint::Min(3),
                ])
                .split(inner);

            let tracking = tracking_label(
                app.git_ahead,
                app.git_behind,
                app.git_has_upstream,
                app.git_has_remote,
                app.git_uncommitted,
            );
            let hint = upstream_hint(app.git_has_remote, app.git_has_upstream);

            // Whole-pane horizontal scroll: Shift+Left/Right moves Status,
            // Sync and Log together as one unit, so all three sections stay
            // aligned. The offset is clamped against the longest line across
            // all three sections and the visible content width.
            let pane_width = inner.width.saturating_sub(2) as usize;
            let longest_line = app
                .git_log
                .lines()
                .map(|l| l.chars().count())
                .chain(app.last_sync.as_ref().map(|s| {
                    let age = relative_time(s.at.elapsed());
                    if s.success {
                        format!("{} {} ({})", s.action, s.url, age).chars().count()
                    } else {
                        format!(
                            "{} failed: {} ({})",
                            s.action,
                            s.detail.lines().next().unwrap_or(""),
                            age
                        )
                        .chars()
                        .count()
                    }
                }))
                .chain(
                    app.git_last_remote_update
                        .as_ref()
                        .map(|s| s.chars().count()),
                )
                .chain([
                    "branch  ".chars().count() + display_branch(&app.git_branch).chars().count(),
                    tracking.chars().count(),
                ])
                .chain(hint.iter().map(|hint| hint.chars().count()))
                .max()
                .unwrap_or(0);
            let max_h = longest_line.saturating_sub(pane_width.max(1));
            if app.git_h_scroll > max_h {
                app.git_h_scroll = max_h;
            }
            let h = app.git_h_scroll;

            let mut status_lines = vec![
                // White space below the pane title, matching the Preview pane.
                Line::from(""),
                Line::from(Span::styled(
                    "Status",
                    Style::default().fg(primary).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                // The three data rows participate in the pane-wide
                // horizontal scroll, like the Sync and Log rows below.
                Line::from(hslice_spans(
                    vec![
                        Span::styled("branch  ", Style::default().fg(secondary)),
                        Span::styled(display_branch(&app.git_branch), Style::default().fg(text)),
                    ],
                    h,
                )),
                Line::from(hslice_spans(
                    status_row_spans(
                        &app.git_remote_name,
                        app.git_has_remote,
                        &tracking,
                        secondary,
                        text,
                    ),
                    h,
                )),
            ];
            if let Some(hint) = hint {
                status_lines.push(Line::from(hslice_spans(
                    vec![Span::styled(hint, Style::default().fg(Color::DarkGray))],
                    h,
                )));
            }
            f.render_widget(Paragraph::new(Text::from(status_lines)), chunks[0]);

            // Sync section: the outcome of the last push/pull. Only theme
            // colors are used - action verb and timestamp in foreground, URL
            // in secondary - with accent reserved for the pane headings. The
            // message participates in the pane-wide horizontal scroll.
            let mut sync_lines = vec![
                Line::from(Span::styled(
                    "Sync",
                    Style::default().fg(primary).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
            ];
            match &app.last_sync {
                Some(sync) => {
                    let age = relative_time(sync.at.elapsed());
                    if sync.success {
                        sync_lines.push(Line::from(hslice_spans(
                            sync_success_spans(sync, &age, text, secondary),
                            h,
                        )));
                    } else {
                        sync_lines.push(Line::from(hslice_spans(
                            vec![
                                Span::styled(
                                    format!("{} failed", sync.action),
                                    Style::default().fg(text),
                                ),
                                Span::styled(
                                    format!(": {}", sync.detail.lines().next().unwrap_or("")),
                                    Style::default().fg(secondary),
                                ),
                                Span::styled(format!(" ({})", age), Style::default().fg(text)),
                            ],
                            h,
                        )));
                    }
                }
                None => {
                    // No in-session record: fall back to the reflog-derived
                    // label from the last refresh ("last remote update ...",
                    // or "(unknown)" when the reflog was empty), or "(no sync
                    // yet)" when there is no upstream to have one from.
                    let label = app
                        .git_last_remote_update
                        .clone()
                        .unwrap_or_else(|| "(no sync yet)".to_string());
                    sync_lines.push(Line::from(Span::styled(label, Style::default().fg(text))));
                }
            }
            f.render_widget(Paragraph::new(Text::from(sync_lines)), chunks[1]);

            let log_height = chunks[2].height.saturating_sub(2) as usize;
            let log_all: Vec<&str> = app.git_log.lines().collect();
            let max_scroll = log_all.len().saturating_sub(log_height.max(1));
            if app.git_scroll > max_scroll {
                app.git_scroll = max_scroll;
            }

            let mut log_lines = vec![
                Line::from(Span::styled(
                    "Log",
                    Style::default().fg(primary).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
            ];
            for line in log_all.iter().skip(app.git_scroll).take(log_height) {
                // Commit lines look like "<hash> <date> <time> <message>".
                // git::parse_log_line validates the timestamp format; anything
                // else (e.g. "No commits yet") is rendered plain.
                match git::parse_log_line(line) {
                    Some(entry) => {
                        let spans = vec![
                            Span::styled(format!("{} ", entry.hash), Style::default().fg(text)),
                            Span::styled(
                                format!("{} {} ", entry.date, entry.time),
                                Style::default().fg(secondary),
                            ),
                            Span::styled(entry.message, Style::default().fg(text)),
                        ];
                        log_lines.push(Line::from(hslice_spans(spans, h)));
                    }
                    None => {
                        let spans =
                            vec![Span::styled((*line).to_string(), Style::default().fg(text))];
                        log_lines.push(Line::from(hslice_spans(spans, h)));
                    }
                }
            }
            f.render_widget(Paragraph::new(Text::from(log_lines)), chunks[2]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::status_bar::confirm_delete_units;
    use ratatui::style::{Color, Style};
    use ratatui::text::Span;

    fn spans_text(spans: &[Span<'static>]) -> String {
        spans.iter().map(|s| s.content.to_string()).collect()
    }

    #[test]
    fn test_tracking_label_uncommitted_clause_only_when_non_zero() {
        assert_eq!(
            tracking_label(0, 0, true, true, 0),
            "0 ahead, 0 behind",
            "clean store must render unchanged"
        );
        assert_eq!(
            tracking_label(0, 0, true, true, 3),
            "0 ahead, 0 behind, 3 uncommitted"
        );
        assert_eq!(
            tracking_label(2, 1, true, true, 7),
            "2 ahead, 1 behind, 7 uncommitted"
        );
        // No upstream: the clause still surfaces a dirty tree.
        assert_eq!(
            tracking_label(0, 0, false, true, 1),
            "no upstream, 1 uncommitted"
        );
        assert_eq!(tracking_label(0, 0, false, true, 0), "no upstream");
        // No remote at all: the label says so instead of naming one, and
        // the uncommitted clause still applies.
        assert_eq!(tracking_label(0, 0, false, false, 0), "no remote");
        assert_eq!(
            tracking_label(0, 0, false, false, 2),
            "no remote, 2 uncommitted"
        );
    }

    /// The Status row omits the remote-name prefix when there is no
    /// remote: the row must read exactly "no remote", never
    /// "origin no remote". A configured remote keeps its name and the
    /// two-space separator ahead of the tracking label.
    #[test]
    fn test_status_row_omits_remote_prefix_without_remote() {
        let spans = status_row_spans("", false, "no remote", Color::Blue, Color::White);
        assert_eq!(spans_text(&spans), "no remote");

        let spans = status_row_spans(
            "origin",
            true,
            "1 ahead, 0 behind",
            Color::Blue,
            Color::White,
        );
        assert_eq!(spans_text(&spans), "origin  1 ahead, 0 behind");
    }

    /// A detached HEAD renders as "detached"; any real branch name passes
    /// through unchanged.
    #[test]
    fn test_display_branch_detached() {
        assert_eq!(display_branch("HEAD"), "detached");
        assert_eq!(display_branch("master"), "master");
    }

    /// The ConfirmDelete bar's sentence follows pending_pull: the pull
    /// confirmation when a merge pull is awaiting y/n, the delete sentence
    /// otherwise. The y/n key units are common to both.
    #[test]
    fn confirm_delete_bar_sentence_follows_pending_pull() {
        let key = Style::default();
        let dim = Style::default();
        let text = Color::White;

        let delete = confirm_delete_units(false, Some("work/github".to_string()), text, key, dim);
        let rendered = delete
            .iter()
            .map(|u| spans_text(u))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            rendered.contains("Delete work/github? "),
            "delete sentence expected: {rendered}"
        );
        assert!(rendered.contains("y:confirm"), "{rendered}");
        assert!(rendered.contains("n:cancel"), "{rendered}");

        let pull = confirm_delete_units(true, None, text, key, dim);
        let rendered = pull
            .iter()
            .map(|u| spans_text(u))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            rendered.starts_with("Pull would merge - continue? "),
            "pull sentence expected: {rendered}"
        );
        assert!(!rendered.contains("Delete"), "{rendered}");
        assert!(rendered.contains("y:confirm"), "{rendered}");
        assert!(rendered.contains("n:cancel"), "{rendered}");
    }
    #[test]
    fn test_upstream_hint_three_way() {
        assert_eq!(
            upstream_hint(false, false),
            Some("git remote add <name> <url> (or config git.remote_url)")
        );
        assert_eq!(
            upstream_hint(true, false),
            Some("no upstream - set on first push")
        );
        assert_eq!(upstream_hint(true, true), None);
        assert_eq!(upstream_hint(false, true), None);
    }

    /// The Sync line names the remote alongside the URL, so a local-path
    /// remote (or a multi-remote store) says which one was used.
    #[test]
    fn sync_line_names_remote() {
        let sync = SyncStatus {
            action: "Pushed to".to_string(),
            remote: "fudge".to_string(),
            url: "/home/me/pass-store-fudge.git".to_string(),
            at: std::time::Instant::now(),
            success: true,
            detail: String::new(),
        };
        let line = spans_text(&sync_success_spans(
            &sync,
            "5 min ago",
            Color::White,
            Color::Blue,
        ));
        assert!(
            line.contains("fudge"),
            "Sync line must name the remote: {line}"
        );
        assert_eq!(
            line,
            "Pushed to fudge (/home/me/pass-store-fudge.git) (5 min ago)"
        );
    }

    /// Without a remote name the line degrades to the previous
    /// "{action} {url} ({age})" shape.
    #[test]
    fn sync_line_degrades_without_remote() {
        let sync = SyncStatus {
            action: "Pulled from".to_string(),
            remote: String::new(),
            url: "/home/me/pass-store.git".to_string(),
            at: std::time::Instant::now(),
            success: true,
            detail: String::new(),
        };
        let line = spans_text(&sync_success_spans(
            &sync,
            "just now",
            Color::White,
            Color::Blue,
        ));
        assert_eq!(line, "Pulled from /home/me/pass-store.git (just now)");
    }

    /// The Status rows participate in the same pane-wide horizontal scroll
    /// as the Sync line: hslice_spans with a shared offset drops exactly
    /// that many leading cells from every row, so Status, Sync and Log
    /// stay aligned when git_h_scroll is non-zero.
    #[test]
    fn status_rows_share_the_pane_h_scroll_offset() {
        let branch = vec![
            Span::styled("branch  ", Style::default()),
            Span::styled("master", Style::default()),
        ];
        let tracking = status_row_spans(
            "origin",
            true,
            "1 ahead, 0 behind",
            Color::Blue,
            Color::White,
        );
        let sync = SyncStatus {
            action: "Pushed to".to_string(),
            remote: "fudge".to_string(),
            url: "/home/me/store.git".to_string(),
            at: std::time::Instant::now(),
            success: true,
            detail: String::new(),
        };
        let sync_spans = sync_success_spans(&sync, "5 min ago", Color::White, Color::Blue);

        let branch_text = spans_text(&branch);
        let tracking_text = spans_text(&tracking);
        let sync_text = spans_text(&sync_spans);

        for off in [1usize, 4, 9] {
            assert_eq!(
                spans_text(&hslice_spans(branch.clone(), off)),
                &branch_text[off..]
            );
            assert_eq!(
                spans_text(&hslice_spans(tracking.clone(), off)),
                &tracking_text[off..]
            );
            assert_eq!(
                spans_text(&hslice_spans(sync_spans.clone(), off)),
                &sync_text[off..]
            );
        }
    }

    /// The reflog-derived fallback label renders through the same
    /// relative-time formatter as the in-session sync age.
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
}

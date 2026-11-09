use crate::keys::{self, Action};
use ratatui::style::Style;
use ratatui::text::Span;

pub(crate) fn key_unit(
    key: &str,
    key_style: Style,
    action: &str,
    action_style: Style,
) -> Vec<Span<'static>> {
    vec![
        Span::styled(key.to_string(), key_style),
        Span::styled(action.trim_end().to_string(), action_style),
    ]
}

/// Visible width of a single span.
fn span_width(span: &Span) -> usize {
    span.content.chars().count()
}

/// Visible width of a group of spans (one helpbar unit).
fn unit_width(unit: &[Span]) -> usize {
    unit.iter().map(span_width).sum()
}

/// Break a unit into word-level pieces (each piece keeps its trailing
/// space, preserving its style). Only used for units wider than a whole
/// line, where word wrapping is the only way to keep them visible.
fn split_unit_words(unit: &[Span]) -> Vec<Vec<Span<'static>>> {
    let mut pieces: Vec<Vec<Span<'static>>> = Vec::new();
    for span in unit {
        let mut current = String::new();
        for c in span.content.chars() {
            current.push(c);
            if c == ' ' {
                pieces.push(vec![Span::styled(std::mem::take(&mut current), span.style)]);
            }
        }
        if !current.is_empty() {
            pieces.push(vec![Span::styled(current, span.style)]);
        }
    }
    pieces
}
pub(crate) fn pack_units(
    units: Vec<Vec<Span<'static>>>,
    width: usize,
    first_capacity: usize,
) -> Vec<Vec<Span<'static>>> {
    let mut lines: Vec<Vec<Span<'static>>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut cur_len = 0usize;
    let mut capacity = first_capacity;

    for unit in units {
        let pieces = if unit_width(&unit) > width {
            split_unit_words(&unit)
        } else {
            vec![unit]
        };
        for piece in pieces {
            let piece = trim_unit(piece);
            let piece_len = unit_width(&piece);
            if piece_len == 0 {
                continue;
            }
            let sep_len = if current.is_empty() { 0 } else { 1 };
            if cur_len + sep_len + piece_len > capacity
                && (!current.is_empty() || piece_len <= width)
            {
                // Move to the next line. The line is pushed even when
                // empty: line 1 must remain line 1 (the mode prefix is
                // prepended to it by the caller), so its first unit
                // starts line 2 instead of overflowing past the prefix.
                lines.push(std::mem::take(&mut current));
                cur_len = 0;
                capacity = width;
                // A single piece wider than a full line still does not fit
                // anywhere; rendering it as-is is the only option.
            }
            if !current.is_empty() {
                // Exactly one space between adjacent units on a line,
                // styled like the incoming unit to keep backgrounds continuous.
                let style = piece.first().map(|s| s.style).unwrap_or_default();
                current.push(Span::styled(" ".to_string(), style));
                cur_len += 1;
            }
            cur_len += piece_len;
            current.extend(piece);
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

/// Priority tier for a Normal-mode helpbar hint. When the two visible
/// helpbar lines cannot hold every hint, hints are emitted in tier order
/// and lower tiers are dropped first; Essential hints are never dropped.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum HintTier {
    Essential,
    Important,
    Secondary,
    /// Only shown while a preview is open.
    Contextual,
}

/// One Normal-mode helpbar hint: its priority tier, its visible width in
/// display cells (the labels contain multi-byte arrows, so measuring
/// bytes would overcount), and its styled spans.
pub(crate) struct HintUnit {
    tier: HintTier,
    len: usize,
    unit: Vec<Span<'static>>,
}

/// Strip trailing spaces from a unit's last span, matching key_unit's
/// label discipline for hand-built units.
fn trim_unit(mut unit: Vec<Span<'static>>) -> Vec<Span<'static>> {
    if let Some(last) = unit.last_mut() {
        last.content = last.content.trim_end().to_string().into();
    }
    unit
}

/// Wrap key_unit with a priority tier and a precomputed display width.
fn hint_unit(
    tier: HintTier,
    key: &str,
    key_style: Style,
    action: &str,
    action_style: Style,
) -> HintUnit {
    hint_unit_spans(tier, key_unit(key, key_style, action, action_style))
}

/// HintUnit constructor for hand-built span groups (the multi-span nav
/// unit), so every hint goes through the same trim-and-measure path and
/// participates in the same spacing and capacity accounting.
fn hint_unit_spans(tier: HintTier, unit: Vec<Span<'static>>) -> HintUnit {
    let unit = trim_unit(unit);
    let len = unit_width(&unit);
    HintUnit { tier, len, unit }
}

/// Greedily place prioritized hints onto the visible helpbar lines,
/// mirroring pack_units' line filling: line 1 shares its width with the
/// mode prefix and context slot, later lines get the full width, adjacent
/// units on a line are joined by exactly one space, and only `line_count`
/// lines are visible - anything beyond is clipped. `reserve_last` is
/// subtracted from the last visible line's capacity up front, leaving
/// room for the truncation marker so it can never push a placed hint off
/// the line. Returns the indices of the placed hints (in priority order)
/// and whether any hint was dropped. Essential hints are never dropped:
/// they are placed even when they overflow the last line.
pub(crate) fn select_hints(
    hints: &[HintUnit],
    width: usize,
    first_capacity: usize,
    line_count: usize,
    reserve_last: usize,
) -> (Vec<usize>, bool) {
    let cap_of = |line: usize| -> usize {
        let base = if line == 0 { first_capacity } else { width };
        if line + 1 == line_count {
            base.saturating_sub(reserve_last)
        } else {
            base
        }
    };
    let mut keep = Vec::new();
    let mut dropped = false;
    let mut line = 0usize;
    let mut cur_len = 0usize;
    for (i, h) in hints.iter().enumerate() {
        let essential = h.tier == HintTier::Essential;
        let sep_len = if cur_len == 0 { 0 } else { 1 };
        let mut placed = false;
        if cur_len + sep_len + h.len <= cap_of(line) {
            cur_len += sep_len + h.len;
            placed = true;
        } else if line + 1 < line_count && h.len <= cap_of(line + 1) {
            // Start the next visible line (no separator at a line start).
            // Like pack_units, the fill never returns to an earlier line
            // once it has moved on.
            line += 1;
            cur_len = h.len;
            placed = true;
        }
        if !placed && essential {
            // Essential hints are never dropped: Enter :open and q :quit
            // must never disappear. Placed even when they overflow, the
            // same trade pack_units makes for an oversized piece.
            if cur_len > 0 && line + 1 < line_count {
                line += 1;
                cur_len = 0;
            }
            cur_len += h.len;
            placed = true;
        }
        if placed {
            keep.push(i);
        } else {
            dropped = true;
        }
    }
    (keep, dropped)
}

/// Compact paired label for two advertised chords that share a "Ctrl+"
/// prefix: "Ctrl+\u{2191}"/"Ctrl+\u{2193}" becomes "Ctrl+\u{2191}/\u{2193}"
/// and "Ctrl+U"/"Ctrl+D" becomes "Ctrl+U/D". Pairs without the shared
/// prefix ("PgUp"/"PgDn", "b"/"Space") are joined with a slash as-is.
pub(crate) fn chord_pair(up: &str, down: &str) -> String {
    match (up.strip_prefix("Ctrl+"), down.strip_prefix("Ctrl+")) {
        (Some(u), Some(d)) => format!("Ctrl+{u}/{d}"),
        _ => format!("{up}/{down}"),
    }
}

/// Build the Normal-mode helpbar units, width-aware: hints are emitted in
/// priority order (see HintTier) and lower-tier hints are dropped first
/// when the `height` visible helpbar lines would otherwise overflow. The
/// other Essential hints (Enter :open, q :quit) are never dropped. The
/// "?:help" pointer to the full help view is always appended last, and
/// when any unit was dropped a literal "…" unit is emitted immediately
/// before it, so the tail reads "… ?:help". Both tail units are reserved
/// on the last visible line before ordinary units are placed, so they are
/// never pushed off the line nor dropped themselves.
pub(crate) fn normal_mode_hints(
    preview_active: bool,
    width: usize,
    height: usize,
    prefix_width: usize,
    key_style: Style,
    dim: Style,
) -> Vec<Vec<Span<'static>>> {
    let mut hints: Vec<HintUnit> = Vec::new();
    if preview_active {
        // Preview state keeps its own, smaller hint set: the entry-level
        // controls plus the contextual scroll/close hints. The tree-only
        // bindings (search, new, delete, ...) do not apply here.
        hints.push(hint_unit(
            HintTier::Important,
            "e",
            key_style,
            ":edit ",
            dim,
        ));
        hints.push(hint_unit(
            HintTier::Secondary,
            "r",
            key_style,
            ":reveal/hide-pwd ",
            dim,
        ));
        hints.push(hint_unit(
            HintTier::Secondary,
            "c",
            key_style,
            ":copy-pwd ",
            dim,
        ));
        hints.push(hint_unit(
            HintTier::Secondary,
            "w",
            key_style,
            ":reveal/hide-pwd-qr ",
            dim,
        ));
        // Contextual: only meaningful while a preview is open.
        hints.push(hint_unit(
            HintTier::Contextual,
            "Shift+↑↓",
            key_style,
            ":vscroll ",
            dim,
        ));
        hints.push(hint_unit(
            HintTier::Contextual,
            "Shift+←→",
            key_style,
            ":hscroll ",
            dim,
        ));
        hints.push(hint_unit(
            HintTier::Contextual,
            "Esc",
            key_style,
            ":close",
            dim,
        ));
    } else {
        // Essential: never dropped.
        hints.push(hint_unit(
            HintTier::Essential,
            "Enter",
            key_style,
            ":open ",
            dim,
        ));
        hints.push(hint_unit(
            HintTier::Essential,
            "/",
            key_style,
            ":search ",
            dim,
        ));
        hints.push(hint_unit(HintTier::Essential, "n", key_style, ":new ", dim));
        hints.push(hint_unit(
            HintTier::Essential,
            "d",
            key_style,
            ":delete ",
            dim,
        ));
        hints.push(hint_unit(HintTier::Essential, "q", key_style, ":quit", dim));
        // Important.
        let nav = vec![
            Span::styled("↑↓".to_string(), key_style),
            Span::styled("/".to_string(), dim),
            Span::styled("jkhl".to_string(), key_style),
            Span::styled(":nav".to_string(), dim),
        ];
        hints.push(hint_unit_spans(HintTier::Important, nav));
        hints.push(hint_unit(
            HintTier::Important,
            "Shift+Ltr",
            key_style,
            ":jump to entry ",
            dim,
        ));
        hints.push(hint_unit(
            HintTier::Important,
            "e",
            key_style,
            ":edit ",
            dim,
        ));
        hints.push(hint_unit(
            HintTier::Important,
            "m",
            key_style,
            ":move ",
            dim,
        ));
        hints.push(hint_unit(
            HintTier::Important,
            "f",
            key_style,
            ":add-folder ",
            dim,
        ));
        hints.push(hint_unit(HintTier::Important, "g", key_style, ":git ", dim));
        // Secondary. The jump chord label is platform-aware: a Mac user
        // sees "Ctrl+U/D" where a Linux user sees "Ctrl+↑/↓".
        let jump_label = chord_pair(
            keys::advertised(Action::JumpUp).unwrap_or("Ctrl+\u{2191}"),
            keys::advertised(Action::JumpDown).unwrap_or("Ctrl+\u{2193}"),
        );
        hints.push(hint_unit(
            HintTier::Secondary,
            &jump_label,
            key_style,
            ":jump ",
            dim,
        ));
        hints.push(hint_unit(
            HintTier::Secondary,
            "Ctrl+p",
            key_style,
            ":push ",
            dim,
        ));
        hints.push(hint_unit(
            HintTier::Secondary,
            "p",
            key_style,
            ":pull ",
            dim,
        ));
        hints.push(hint_unit(
            HintTier::Secondary,
            "Ctrl+r",
            key_style,
            ":reload-cfg ",
            dim,
        ));
    }

    hints.sort_by_key(|h| h.tier);

    // The tail is reserved before any ordinary unit is placed: the "…"
    // drop marker (emitted only when something was dropped) and the
    // "?:help" pointer to the full help view, joined by the packer's
    // single space. Neither tail unit is ever dropped; reserving
    // separator + ellipsis + separator + pointer covers both the dropped
    // and the not-dropped tail.
    let help_unit = vec![
        Span::styled("?".to_string(), key_style),
        Span::styled(":help".to_string(), dim),
    ];
    let ellipsis_unit = vec![Span::styled("…".to_string(), dim)];
    let tail_reserve = 1 + unit_width(&ellipsis_unit) + 1 + unit_width(&help_unit);

    let line_count = height.max(1);
    let first_capacity = width.saturating_sub(prefix_width);
    let (keep, dropped) = select_hints(&hints, width, first_capacity, line_count, tail_reserve);

    let mut units: Vec<Vec<Span<'static>>> =
        keep.into_iter().map(|i| hints[i].unit.clone()).collect();
    if dropped {
        units.push(ellipsis_unit);
    }
    // Always last, never dropped.
    units.push(help_unit);
    units
}

/// Hint-bar units for the Editor status bar. Extracted from
/// draw_status_bar so tests can render the real units rather than a copy
/// of their literals.
pub(crate) fn editor_hints(cyan_bold: Style, dim: Style) -> Vec<Vec<Span<'static>>> {
    let jump_label = chord_pair(
        keys::advertised(Action::JumpUp).unwrap_or("Ctrl+\u{2191}"),
        keys::advertised(Action::JumpDown).unwrap_or("Ctrl+\u{2193}"),
    );
    vec![
        key_unit("↑↓", cyan_bold, ":navigate ", dim),
        key_unit(&jump_label, cyan_bold, ":pwd-length ", dim),
        key_unit("Tab", cyan_bold, ":charset ", dim),
        key_unit("Ctrl+g", cyan_bold, ":generate-pwd ", dim),
        key_unit("Ctrl+s", cyan_bold, ":save ", dim),
        key_unit("Esc", cyan_bold, ":cancel", dim),
    ]
}

/// Hint-bar units for the Git pane's Ready state (the only Git state with
/// key hints). Extracted from draw_status_bar for the same reason as
/// editor_hints.
pub(crate) fn git_ready_hints(cyan_bold: Style, dim: Style) -> Vec<Vec<Span<'static>>> {
    vec![
        key_unit("Ctrl+p", cyan_bold, ":push ", dim),
        key_unit("p", cyan_bold, ":pull ", dim),
        key_unit("r", cyan_bold, ":refresh ", dim),
        key_unit("Shift+↑↓", cyan_bold, ":log vscroll ", dim),
        key_unit("Shift+←→", cyan_bold, ":pane hscroll ", dim),
        key_unit("Esc", cyan_bold, ":close", dim),
    ]
}

/// Hint-bar units for the Help mode's status bar. Extracted from
/// draw_status_bar for the same reason as editor_hints.
pub(crate) fn help_hints(cyan_bold: Style, dim: Style) -> Vec<Vec<Span<'static>>> {
    let page_label = chord_pair(
        keys::advertised(Action::PageUp).unwrap_or("PgUp"),
        keys::advertised(Action::PageDown).unwrap_or("PgDn"),
    );
    vec![
        key_unit("Esc", cyan_bold, ":close ", dim),
        key_unit("Shift+↑↓", cyan_bold, ":line ", dim),
        key_unit(&page_label, cyan_bold, ":page", dim),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::help::{help_entries, help_entries_for};
    use ratatui::style::Style;
    use ratatui::text::Span;

    fn spans_text(spans: &[Span<'static>]) -> String {
        spans.iter().map(|s| s.content.to_string()).collect()
    }

    fn render_helpbar(
        units: Vec<Vec<Span<'static>>>,
        width: usize,
        height: usize,
        prefix_width: usize,
    ) -> Vec<String> {
        let first_capacity = width.saturating_sub(prefix_width);
        let packed = pack_units(units, width, first_capacity);
        let mut line1 = vec![Span::styled(" ".repeat(prefix_width), Style::default())];
        if let Some(first) = packed.first() {
            line1.extend(first.iter().cloned());
        }
        let mut out = vec![spans_text(&line1)];
        for line in packed.iter().skip(1).take(height.saturating_sub(1)) {
            out.push(spans_text(line));
        }
        out
    }
    #[test]
    fn test_normal_hints_fit_80x24() {
        let key = Style::default();
        let dim = Style::default();
        // Plain Normal mode prefix: cap + " Normal " + cap + space = 11
        // cells; the bar is two rows tall at a 24-row terminal.
        let lines = render_helpbar(normal_mode_hints(false, 80, 2, 11, key, dim), 80, 2, 11);
        assert!(lines.len() <= 2, "helpbar must fit its two rows");
        for line in &lines {
            assert!(
                line.chars().count() <= 80,
                "line overflows 80 cells: {line}"
            );
        }
        let text = lines.join("\n");
        // q:quit survives, space-separated from the unit that follows it.
        assert!(
            text.contains("q:quit "),
            "q:quit must be present and space-separated: {text}"
        );
        // Nothing was clipped past the two rows: the last placed hint is
        // still visible. The jump label is platform-aware, so build the
        // needle from the same advertised chords the hint uses.
        assert!(
            text.contains(&format!(
                "{}:jump",
                chord_pair(
                    keys::advertised(Action::JumpUp).unwrap_or("Ctrl+\u{2191}"),
                    keys::advertised(Action::JumpDown).unwrap_or("Ctrl+\u{2193}"),
                )
            )),
            "last placed hint must be visible: {text}"
        );
        // Units are dropped at 80 columns, so the tail signals it: the
        // last row ends with "… ?:help", and the pointer carries no space
        // after the colon.
        assert!(
            lines.last().unwrap().ends_with("… ?:help"),
            "dropped units must be signalled by the tail: {text}"
        );
        assert!(
            text.contains("?:help"),
            "help pointer must be visible: {text}"
        );
        assert!(
            !text.contains("? :help"),
            "no space after the colon: {text}"
        );

        // The preview state's hint set also fits (prefix 23 with the
        // Preview sub-pill).
        let lines = render_helpbar(normal_mode_hints(true, 80, 2, 23, key, dim), 80, 2, 23);
        assert!(lines.len() <= 2);
        for line in &lines {
            assert!(
                line.chars().count() <= 80,
                "line overflows 80 cells: {line}"
            );
        }
    }

    #[test]
    fn test_normal_hints_single_line_bar_keeps_essentials() {
        let key = Style::default();
        let dim = Style::default();
        // A 12-row terminal leaves the status bar a single row.
        let lines = render_helpbar(normal_mode_hints(false, 80, 1, 11, key, dim), 80, 1, 11);
        assert!(lines.len() <= 1, "single-row bar must render one line");
        for line in &lines {
            assert!(
                line.chars().count() <= 80,
                "line overflows 80 cells: {line}"
            );
        }
        let text = lines.join("\n");
        assert!(
            text.contains("Enter:open"),
            "essential Enter:open must survive: {text}"
        );
        assert!(
            text.contains("q:quit"),
            "essential q:quit must survive: {text}"
        );
    }

    #[test]
    fn test_normal_hints_complete_at_200_columns() {
        let key = Style::default();
        let dim = Style::default();
        let lines = render_helpbar(normal_mode_hints(false, 200, 2, 11, key, dim), 200, 2, 11);
        let text = lines.join("\n");
        assert!(
            !text.contains('…'),
            "nothing should be dropped at 200 columns"
        );
        // The jump chord label is platform-aware; build the expected needle
        // from the same advertised chords the hint uses.
        let jump_needle = format!(
            "{}:jump",
            chord_pair(
                keys::advertised(Action::JumpUp).unwrap_or("Ctrl+\u{2191}"),
                keys::advertised(Action::JumpDown).unwrap_or("Ctrl+\u{2193}"),
            )
        );
        for needle in [
            "Enter:open",
            "/:search",
            "n:new",
            "d:delete",
            "q:quit",
            "↑↓/jkhl:nav",
            "Shift+Ltr:jump to entry",
            "e:edit",
            "m:move",
            "f:add-folder",
            "g:git",
            jump_needle.as_str(),
            "Ctrl+p:push",
            "Ctrl+r:reload-cfg",
            "?:help",
        ] {
            assert!(text.contains(needle), "missing hint: {needle}");
        }
        // The joiner inserts exactly one space between adjacent units.
        assert!(
            text.contains("q:quit ↑↓/jkhl:nav"),
            "units must be single-space joined: {text}"
        );
        // The help pointer is always last; nothing is dropped at 200
        // columns, so there is no "…" marker.
        assert!(
            text.ends_with("?:help"),
            "help pointer must be last: {text}"
        );
        assert!(!text.contains("…"), "no drop marker at 200 columns: {text}");

        // The preview state keeps its own hints and carries none of the
        // tree-only set.
        let lines = render_helpbar(normal_mode_hints(true, 200, 2, 23, key, dim), 200, 2, 23);
        let text = lines.join("\n");
        assert!(!text.contains('…'));
        assert!(
            text.ends_with("?:help"),
            "help pointer must be last: {text}"
        );
        for needle in [
            "e:edit",
            "r:reveal/hide-pwd",
            "c:copy-pwd",
            "w:reveal/hide-pwd-qr",
            "Shift+↑↓:vscroll",
            "Shift+←→:hscroll",
            "Esc:close",
        ] {
            assert!(text.contains(needle), "missing hint: {needle}");
        }
        assert!(!text.contains(":quit"));
    }

    #[test]
    fn test_help_hint_survives_narrow_bar() {
        let key = Style::default();
        let dim = Style::default();
        let lines = render_helpbar(normal_mode_hints(false, 80, 2, 11, key, dim), 80, 2, 11);
        let text = lines.join("\n");
        assert!(
            text.contains("?:help"),
            "help pointer must survive at 80 columns: {text}"
        );
    }

    /// Regression guard for the key:value hint rule: no hint label may
    /// render with a space after the colon. Asserts on rendered output,
    /// so a ": " literal added to any covered builder fails here.
    #[test]
    fn hint_labels_have_no_space_after_colon() {
        let key = Style::default();
        let dim = Style::default();
        // Width 200 / height 3: the tiered packer drops nothing, so every
        // unit of each bar is present in the rendered text.
        let bars = [
            (
                "normal tree",
                render_helpbar(normal_mode_hints(false, 200, 3, 11, key, dim), 200, 3, 11),
            ),
            (
                "normal preview",
                render_helpbar(normal_mode_hints(true, 200, 3, 23, key, dim), 200, 3, 23),
            ),
            ("editor", render_helpbar(editor_hints(key, dim), 200, 3, 11)),
            (
                "git pane",
                render_helpbar(git_ready_hints(key, dim), 200, 3, 11),
            ),
            (
                "help mode",
                render_helpbar(help_hints(key, dim), 200, 3, 11),
            ),
        ];
        for (name, lines) in bars {
            let text = lines.join("\n");
            assert!(
                !text.contains(": "),
                "{name} hint bar renders a label with a space after the colon: {text:?}"
            );
        }
    }

    /// Regression guard for the Ctrl-chord casing rule: holding Ctrl and
    /// pressing a letter produces the lowercase control code, so hints
    /// must read "Ctrl+r", never "Ctrl+R". Scans every key string the
    /// status-bar hint builders emit (at a width where nothing is dropped)
    /// and every help_entries() line for "Ctrl+" followed by an uppercase
    /// ASCII letter. Non-letter chords (Ctrl+↑↓, Ctrl+/) are unaffected.
    #[test]
    fn ctrl_chords_use_lowercase_letters() {
        let key = Style::default();
        let dim = Style::default();
        let mut texts: Vec<String> = Vec::new();
        for units in [
            normal_mode_hints(false, 200, 3, 11, key, dim),
            normal_mode_hints(true, 200, 3, 23, key, dim),
            editor_hints(key, dim),
            git_ready_hints(key, dim),
            help_hints(key, dim),
        ] {
            for unit in units {
                texts.push(spans_text(&unit));
            }
        }
        texts.extend(help_entries());
        for text in &texts {
            let mut rest = text.as_str();
            while let Some(pos) = rest.find("Ctrl+") {
                let after = &rest[pos + "Ctrl+".len()..];
                if let Some(c) = after.chars().next() {
                    assert!(
                        !c.is_ascii_uppercase(),
                        "Ctrl chord uses an uppercase letter: {text}"
                    );
                }
                rest = after;
            }
        }
    }

    #[test]
    fn test_help_entries_mention_key_bindings() {
        let text = help_entries().join("\n");
        assert!(text.contains("Ctrl+p"), "help must list Ctrl+p: {text}");
        assert!(
            text.contains("Shift+Ltr"),
            "help must list Shift+Ltr: {text}"
        );
        for group in ["NORMAL", "PREVIEW", "EDITOR", "GIT"] {
            assert!(
                text.contains(group),
                "help must have a {group} group: {text}"
            );
        }
    }

    /// The git pane advertises Ctrl+p for push everywhere the binding is
    /// shown: the Ready-state footer and the GIT help group. Uppercase P
    /// keeps working as an undocumented alias but is never advertised.
    #[test]
    fn test_git_push_hint_advertises_ctrl_p() {
        let key = Style::default();
        let dim = Style::default();

        // Footer: the git pane's Ready-state hint bar.
        let footer = git_ready_hints(key, dim)
            .iter()
            .map(|u| spans_text(u))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            footer.contains("Ctrl+p:push"),
            "git footer must advertise Ctrl+p:push: {footer}"
        );
        assert!(
            !footer.contains("P:push"),
            "git footer must not advertise a bare P: {footer}"
        );

        // Help page: the GIT group entry. The key column is padded to 14
        // cells, so "Ctrl+p" is followed by 8 spaces before "push"; a bare
        // "P" entry would render as "P" + 13 spaces + "push".
        let text = help_entries().join("\n");
        assert!(
            text.contains("Ctrl+p        push"),
            "GIT help group must advertise Ctrl+p: {text}"
        );
        assert!(
            !text.contains("P             push"),
            "GIT help group must not advertise a bare P: {text}"
        );
    }

    /// The Normal-mode hint bar advertises both git operations: Ctrl+p for
    /// push and p for pull, matching the git pane's Ready-state footer.
    #[test]
    fn test_normal_hint_advertises_push_and_pull() {
        let key = Style::default();
        let dim = Style::default();

        // Width 200 / height 3: the tiered packer drops nothing, so both
        // units are present in the rendered text.
        let footer = normal_mode_hints(false, 200, 3, 11, key, dim)
            .iter()
            .map(|u| spans_text(u))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            footer.contains("Ctrl+p:push"),
            "normal footer must advertise Ctrl+p:push: {footer}"
        );
        assert!(
            footer.contains("p:pull"),
            "normal footer must advertise p:pull: {footer}"
        );
    }

    #[test]
    fn test_help_delete_label_follows_platform() {
        // The Delete chord is identical on both platforms; only the
        // advertisement differs (Delete is Fn+Backspace on a Mac keyboard).
        let mac = help_entries_for(keys::Platform::Mac).join("\n");
        assert!(
            mac.contains("Fn+Backspace"),
            "mac help must advertise Fn+Backspace: {mac}"
        );
        let linux = help_entries_for(keys::Platform::Linux).join("\n");
        assert!(
            linux.contains("Delete"),
            "linux help must advertise Delete: {linux}"
        );
        assert!(!linux.contains("Fn+Backspace"));
    }

    #[test]
    fn test_help_body_documents_caveats_and_universal_key() {
        // The macOS caveats are gated on the resolved keyboard, so assert
        // them against an explicitly Mac platform.
        let mac = help_entries_for(keys::Platform::Mac).join("\n");
        assert!(
            mac.contains("macOS captures Ctrl+arrow for Mission Control and Spaces."),
            "{mac}"
        );
        assert!(
            mac.contains(
                "Home/End/PgUp/PgDn need Fn on a Mac keyboard; the alternatives above do not."
            ),
            "{mac}"
        );
        // The universal help key applies on every platform.
        let text = help_entries().join("\n");
        assert!(
            text.contains("Ctrl+/ or F1 opens this help from any mode."),
            "{text}"
        );
    }

    #[test]
    fn test_help_mac_caveats_follow_platform() {
        let mac = help_entries_for(keys::Platform::Mac).join("\n");
        assert!(
            mac.contains("Mission Control"),
            "mac help must mention Mission Control: {mac}"
        );
        assert!(mac.contains("Fn"), "mac help must mention Fn: {mac}");

        let linux = help_entries_for(keys::Platform::Linux).join("\n");
        assert!(
            !linux.contains("Mission Control"),
            "linux help must not mention Mission Control: {linux}"
        );
        assert!(
            !linux.contains("Fn"),
            "linux help must not mention Fn: {linux}"
        );
    }
}

use qrcode::QrCode;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::app::{App, PULL_CONFIRM_SENTENCE};
use crate::git;
use crate::keys::{self, Action};
use crate::models::{AppMode, DecryptedContent, GitPaneState, NewStoreStage, SyncStatus};
use crate::pass::store_path;
use crate::utils::hslice;

/// Glyph tiers for the UI's pill caps and expand/collapse markers. The
/// Powerline caps (U+E0B4/U+E0B6) live in the Private Use Area, which the
/// default font of Apple Terminal (Menlo) does not cover - they render as
/// '?'. The BMP tier substitutes half-block characters; the ASCII tier is
/// plain punctuation.
#[derive(Clone, Copy)]
pub struct Glyphs {
    pub cap_left: &'static str,
    pub cap_right: &'static str,
    pub tri_closed: &'static str,
    pub tri_open: &'static str,
    /// True when the cap cell should be painted entirely in the accent
    /// colour instead of relying on the glyph. The bmp tier's half-block
    /// caps are inset from the cell edge and shorter than the line box in
    /// Apple Terminal/Menlo, so the background shows through the inset and
    /// Terminal.app's line leading leaves the cap short of the label's
    /// background rectangle; filling the cell hides both artifacts. ascii
    /// must stay false: its literal "["/"]" glyphs would vanish against an
    /// accent background. nerd must stay false: its rounded corners need
    /// the outside colour as bg to read as rounded at all.
    pub cap_fills_cell: bool,
}

pub const GLYPHS_NERD: Glyphs = Glyphs {
    cap_left: "\u{e0b6}",
    cap_right: "\u{e0b4}",
    tri_closed: "\u{25be} ",
    tri_open: "\u{25b8} ",
    cap_fills_cell: false,
};
pub const GLYPHS_BMP: Glyphs = Glyphs {
    cap_left: "\u{2590}",
    cap_right: "\u{258c}",
    tri_closed: "\u{25be} ",
    tri_open: "\u{25b8} ",
    cap_fills_cell: true,
};
pub const GLYPHS_ASCII: Glyphs = Glyphs {
    cap_left: "[",
    cap_right: "]",
    tri_closed: "v ",
    tri_open: "> ",
    cap_fills_cell: false,
};

static ACTIVE_GLYPHS: std::sync::OnceLock<Glyphs> = std::sync::OnceLock::new();
pub fn set_glyphs(g: Glyphs) {
    let _ = ACTIVE_GLYPHS.set(g);
}
pub fn glyphs() -> Glyphs {
    *ACTIVE_GLYPHS.get().unwrap_or(&GLYPHS_NERD)
}

/// Style for a pill cap cell. When the active tier fills the cell (bmp),
/// the whole cell is painted accent-colored so Menlo's inset half-block
/// glyphs cannot leak the background through; otherwise the glyph is drawn
/// in accent over the outside colour (nerd's rounded corners, ascii's
/// literal brackets).
pub fn cap_style(accent: Color, outside: Color) -> Style {
    cap_style_for(glyphs(), accent, outside)
}

/// cap_style keyed on an explicit tier, so tests can exercise tiers other
/// than the process-wide active one (ACTIVE_GLYPHS is a OnceLock and can
/// only be set once per process).
fn cap_style_for(g: Glyphs, accent: Color, outside: Color) -> Style {
    if g.cap_fills_cell {
        Style::default().fg(accent).bg(accent)
    } else {
        Style::default().fg(accent).bg(outside)
    }
}

/// Resolve the glyph set from a mode string ("nerd", "bmp", "ascii", or
/// "auto"/anything unknown). The auto path detects in this order:
///
/// 1. A CJK locale ($LC_ALL, else $LC_CTYPE, else $LANG): both nerd and
///    bmp use East-Asian-Ambiguous codepoints that CJK-locale terminals
///    render double-width, causing column drift; ascii is immune.
/// 2. An SSH session ($SSH_CONNECTION or $SSH_TTY): nerd_fonts() scans the
///    LOCAL machine, so over SSH it describes the server's fonts rather
///    than the client's and cannot be trusted. Users who know their client
///    font push $LC_MUJO_GLYPHS=nerd instead (the LC_ prefix rides ssh's
///    default SendEnv "LANG LC_*" with no server-side sshd_config change).
/// 3. A non-empty local Nerd Font scan: GLYPHS_NERD.
/// 4. Otherwise GLYPHS_BMP, the safe fallback.
pub fn resolve_glyphs(mode: &str) -> Glyphs {
    match mode {
        "nerd" => GLYPHS_NERD,
        "bmp" => GLYPHS_BMP,
        "ascii" => GLYPHS_ASCII,
        // "auto" and anything unknown.
        _ => auto_glyphs(),
    }
}

/// The "auto"/unknown arm of resolve_glyphs; see its doc comment.
fn auto_glyphs() -> Glyphs {
    if cjk_locale() {
        return GLYPHS_ASCII;
    }
    if std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some() {
        return GLYPHS_BMP;
    }
    if !crate::diagnose::nerd_fonts().is_empty() {
        return GLYPHS_NERD;
    }
    GLYPHS_BMP
}

/// True when the resolved locale ($LC_ALL, else $LC_CTYPE, else $LANG)
/// is Japanese, Chinese, or Korean.
fn cjk_locale() -> bool {
    let locale = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_CTYPE"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default()
        .to_lowercase();
    ["ja", "zh", "ko"].iter().any(|l| locale.starts_with(l))
}

/// Resolve the effective glyph mode: the --glyphs CLI flag wins, then
/// $MUJO_GLYPHS, then $LC_MUJO_GLYPHS, then the config value. Empty values
/// are treated as unset at every level.
///
/// The LC_-prefixed alias exists for SSH sessions: ssh only forwards
/// variables matching its SendEnv list, which defaults to "LANG LC_*" on
/// both client and server. Naming the override with an LC_ prefix means it
/// forwards with no server-side sshd_config change.
pub fn effective_glyph_mode(flag: Option<&str>, config: &str) -> String {
    pick_mode(flag, "MUJO_GLYPHS", "LC_MUJO_GLYPHS", config)
}

/// Resolve the effective truecolor mode: the --truecolor CLI flag wins,
/// then $MUJO_TRUECOLOR, then $LC_MUJO_TRUECOLOR, then the config value.
/// Empty values are treated as unset at every level.
///
/// As with glyphs, the LC_-prefixed alias forwards over ssh's default
/// SendEnv ("LANG LC_*") with no server-side sshd_config change.
pub fn effective_truecolor_mode(flag: Option<&str>, config: &str) -> String {
    pick_mode(flag, "MUJO_TRUECOLOR", "LC_MUJO_TRUECOLOR", config)
}

/// Shared precedence chain behind the effective_* functions and
/// keys::effective_keyboard_mode: flag, then the two environment variables
/// in order, then the config value.
pub(crate) fn pick_mode(flag: Option<&str>, primary: &str, alias: &str, config: &str) -> String {
    if let Some(f) = flag {
        if !f.trim().is_empty() {
            return f.to_string();
        }
    }
    for key in [primary, alias] {
        if let Ok(v) = std::env::var(key) {
            if !v.trim().is_empty() {
                return v;
            }
        }
    }
    config.to_string()
}

static TRUECOLOR: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Resolve truecolor from a mode string ("true", "false", or
/// "auto"/anything unknown). "auto" enables RGB where COLORTERM says it is
/// honoured. Over SSH, COLORTERM is not forwarded by default, so truecolor
/// degrades to the 256-color palette unless the user pushes
/// $LC_MUJO_TRUECOLOR=true (LC_-prefixed variables ride ssh's default
/// SendEnv "LANG LC_*"). Apple Terminal sets no COLORTERM and drops
/// SGR 38;2, so it is guarded explicitly even when COLORTERM is exported
/// globally in a shell rc.
pub fn resolve_truecolor(mode: &str) -> bool {
    match mode {
        "true" => true,
        "false" => false,
        _ => {
            // Apple Terminal sets no COLORTERM and drops SGR 38;2. Guard
            // anyway, in case COLORTERM is exported globally in a shell rc.
            if std::env::var("TERM_PROGRAM").as_deref() == Ok("Apple_Terminal") {
                return false;
            }
            matches!(
                std::env::var("COLORTERM").as_deref(),
                Ok("truecolor") | Ok("24bit")
            )
        }
    }
}

pub fn set_truecolor(mode: &str) {
    TRUECOLOR.store(
        resolve_truecolor(mode),
        std::sync::atomic::Ordering::Relaxed,
    );
}

fn rgb_to_xterm256(r: u8, g: u8, b: u8) -> u8 {
    if r == g && g == b {
        if r < 8 {
            return 16;
        }
        if r > 248 {
            return 231;
        }
        return 232 + ((r as u16 - 8) * 24 / 247) as u8;
    }
    let to6 = |v: u8| -> u16 { ((v as u16 * 5 + 127) / 255).min(5) };
    (16 + 36 * to6(r) + 6 * to6(g) + to6(b)) as u8
}

pub fn downgrade(c: Color) -> Color {
    if TRUECOLOR.load(std::sync::atomic::Ordering::Relaxed) {
        return c;
    }
    match c {
        Color::Rgb(r, g, b) => Color::Indexed(rgb_to_xterm256(r, g, b)),
        other => other,
    }
}

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
        draw_help(f, app, main_chunks[0]);
    } else if qr_fullscreen {
        // QR fullscreen: a borderless QR area over a single footer row.
        // Borderless keeps the row budget to QR rows + 1 footer, which
        // fits a 25-row window; the footer carries the close hint the
        // hidden status bar can no longer show.
        let qr_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(main_chunks[0]);
        draw_qr_view(f, app, qr_chunks[0], qr_chunks[1]);
    } else {
        draw_search_bar(f, app, left_column[0]);
        draw_sidebar(f, app, left_column[1]);

        if app.mode == AppMode::Editor {
            draw_editor(f, app, main_panes[1]);
        } else if app.mode == AppMode::Git {
            draw_git(f, app, main_panes[1]);
        } else if app.mode == AppMode::NewStore {
            draw_new_store(f, app, main_panes[1]);
        } else {
            draw_preview(f, app, main_panes[1]);
        }
    }

    // The status bar (which carries the helpbar) is hidden while the QR
    // owns the window; the exit affordance moves into the pane's title.
    if !qr_fullscreen {
        draw_status_bar(f, app, main_chunks[1]);
    }

    if let Some(ref menu) = app.context_menu {
        let (primary, secondary, text, bg) = get_colors(app);
        draw_context_menu(f, menu, primary, secondary, text, bg);
    }
}

/// Resolved UI colors: (accent/primary, secondary, foreground/text, background).
/// An empty theme background resolves to Color::Reset, leaving the
/// terminal's default background untouched. All four values pass through
/// downgrade(), so RGB hex colors fall back to the 256-color palette when
/// truecolor output is disabled.
fn get_colors(app: &App) -> (Color, Color, Color, Color) {
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

/// Message styling: only errors are colored, and only by prefix - messages
/// starting with "Error" or "Decrypt error" render red. Every other
/// message (successes, notices) uses the theme's foreground color instead
/// of a hardcoded green/yellow.
fn message_style(msg: &str, text: Color) -> Style {
    if msg.starts_with("Error") || msg.starts_with("Decrypt error") {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(text)
    }
}

/// The Git pane's tracking label: "N ahead, M behind" when an upstream is
/// tracked, "no upstream" when a remote exists but none is tracked, and
/// "no remote" when the store has no remote at all - ahead/behind can
/// never move without one, and the label must not name a remote that does
/// not exist. A non-zero uncommitted count (staged, modified and
/// untracked entries together) is appended as ", K uncommitted" -
/// ahead/behind are commit counts and can both read zero while the tree
/// is dirty, so the clause is the only signal short of the raw status
/// line. Omitted entirely when zero, so a clean store renders exactly as
/// before.
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

/// Human-readable age of a completed operation, at minute granularity:
/// "just now" for the first 30 seconds, then whole minutes ("1 min ago",
/// "2 min ago"), then whole hours ("1 hour ago", "2 hours ago"). No
/// second-by-second display, so the label stays stable between redraws.
/// Used for the Git pane's sync timestamp.
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

/// Horizontally slice a list of styled spans, skipping `off` characters
/// across span boundaries while preserving each span's style. Used for the
/// Git pane's whole-pane horizontal scrolling (Status, Sync and Log scroll
/// as one unit).
fn hslice_spans(spans: Vec<Span>, off: usize) -> Vec<Span> {
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

/// One unbreakable helpbar unit: a key (or key sequence) plus its action
/// label. Units are kept intact when the helpbar wraps, so a binding like
/// "Ctrl+s:save" never splits across the two lines.
/// Labels carry no trailing spaces: the packer owns inter-unit spacing,
/// inserting exactly one space between adjacent units on a line (see
/// pack_units).
fn key_unit(key: &str, key_style: Style, action: &str, action_style: Style) -> Vec<Span<'static>> {
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

/// Greedily pack unbreakable units into lines of at most `width` visible
/// cells. Line 1 shares its width with the mode prefix, so it starts with
/// `first_capacity` cells already used; later lines get the full width.
/// A unit only starts a new line when it does not fit whole; units wider
/// than a full line are word-wrapped so nothing flows off-screen.
///
/// The packer owns inter-unit spacing: labels carry no trailing spaces,
/// and exactly one space is inserted between adjacent units on a line. A
/// piece that does not fit an empty line 1 moves to line 2 (an empty line
/// 1 is still emitted, because the mode prefix is prepended to it by the
/// caller) rather than overflowing past the prefix; a single piece wider
/// than a full line is rendered as-is, the only alternative to dropping
/// it.
fn pack_units(
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
struct HintUnit {
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
fn select_hints(
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
fn chord_pair(up: &str, down: &str) -> String {
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
fn normal_mode_hints(
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
fn editor_hints(cyan_bold: Style, dim: Style) -> Vec<Vec<Span<'static>>> {
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
fn git_ready_hints(cyan_bold: Style, dim: Style) -> Vec<Vec<Span<'static>>> {
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
fn help_hints(cyan_bold: Style, dim: Style) -> Vec<Vec<Span<'static>>> {
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

/// Render a QR code as terminal lines. When the pane is tall enough, one
/// module per terminal cell rendered with background colour only - no
/// glyphs - so the line-box insets show the module's own colour (fg == bg)
/// and every module is a full, gap-free rectangle. Block glyphs do not
/// fill the cell (Terminal.app's insets leave ~30% of the cell showing the
/// background), which breaks the module grid for scanners; the half-block
/// fallback below stays available for short or narrow panes.
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

fn draw_search_bar(f: &mut Frame, app: &App, area: Rect) {
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

fn draw_sidebar(f: &mut Frame, app: &mut App, area: Rect) {
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

/// Fullscreen QR view: the QR alone, top-aligned and borderless, with a
/// single footer row. No pane title and no borders: QR rows + 1 footer
/// must fit the available rows (a 25-row window leaves 24 QR rows), and
/// the footer carries the close hint the hidden status bar can no longer
/// show. The QR chooser receives the full QR-area height (no borders, no
/// preceding lines), so the QR is never clipped.
fn draw_qr_view(f: &mut Frame, app: &App, area: Rect, footer_area: Rect) {
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

fn draw_preview(f: &mut Frame, app: &mut App, area: Rect) {
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

/// First-run setup pane, shown when no ~/.password-store directory exists.
/// Stage 1 asks whether to set up a pass store; stage 2 asks for the GPG
/// ID used by `pass init`. Centered text, like the other prompt panes.
fn draw_new_store(f: &mut Frame, app: &App, area: Rect) {
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

fn draw_editor(f: &mut Frame, app: &mut App, area: Rect) {
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

fn draw_git(f: &mut Frame, app: &mut App, area: Rect) {
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

/// One group of the help view: a blank spacer, a heading line, then
/// key/description entries with the key left-aligned and padded to 14
/// cells.
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
fn help_entries() -> Vec<String> {
    help_entries_for(keys::keyboard())
}

/// help_entries keyed on an explicit platform, so tests can exercise both
/// platforms (ACTIVE_KEYBOARD is a OnceLock and can only be set once per
/// process) - the same pattern as keys::advertised_for.
fn help_entries_for(platform: keys::Platform) -> Vec<String> {
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
fn draw_help(f: &mut Frame, app: &mut App, area: Rect) {
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

/// Clamp a context-menu rectangle to the frame. The size is clamped down to
/// the frame first, then the position is clamped so the (possibly shrunk)
/// menu fits entirely inside the frame: clicking near the right/bottom edge
/// would otherwise push the rectangle past the buffer and panic in
/// ratatui's buffer indexing.
fn clamp_menu_rect(click_x: u16, click_y: u16, menu_w: u16, menu_h: u16, frame: Rect) -> Rect {
    let menu_width = menu_w.min(frame.width);
    let menu_height = menu_h.min(frame.height);
    let menu_x = click_x.min(frame.width.saturating_sub(menu_width));
    let menu_y = click_y.min(frame.height.saturating_sub(menu_height));
    Rect::new(menu_x, menu_y, menu_width, menu_height)
}

fn draw_context_menu(
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
fn confirm_delete_units(
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

fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
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

    /// Join the visible text of spans.
    fn spans_text(spans: &[Span<'static>]) -> String {
        spans.iter().map(|s| s.content.to_string()).collect()
    }

    /// Mimic draw_status_bar's helpbar assembly: a prefix of
    /// `prefix_width` cells followed by the packed units, one rendered
    /// line per available status-bar row.
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

    /// The uncommitted clause appears only when the count is non-zero: a
    /// clean store must render the row exactly as it did before the count
    /// existed.
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

    #[test]
    fn test_cap_style_fills_cell_only_for_bmp() {
        // bmp fills the whole cell with the accent colour: fg == bg.
        let style = cap_style_for(GLYPHS_BMP, Color::Red, Color::Blue);
        assert_eq!(style.fg, Some(Color::Red));
        assert_eq!(style.bg, Some(Color::Red));

        // nerd keeps the outside colour behind the glyph: fg != bg.
        let style = cap_style_for(GLYPHS_NERD, Color::Red, Color::Blue);
        assert_eq!(style.fg, Some(Color::Red));
        assert_eq!(style.bg, Some(Color::Blue));
        assert_ne!(style.fg, style.bg);

        // ascii behaves like nerd: literal brackets need the outside bg.
        let style = cap_style_for(GLYPHS_ASCII, Color::Red, Color::Blue);
        assert_eq!(style.fg, Some(Color::Red));
        assert_eq!(style.bg, Some(Color::Blue));

        // The process-wide helper agrees with whichever tier is active.
        let style = cap_style(Color::Red, Color::Blue);
        if glyphs().cap_fills_cell {
            assert_eq!(style.fg, style.bg);
        } else {
            assert_ne!(style.fg, style.bg);
        }
    }

    /// Serialize env mutation across tests: env vars are process-global,
    /// so parallel test threads would otherwise see each other's values.
    /// Delegates to testenv::lock so ui tests exclude pass/handlers and
    /// each other on the SAME process-global lock. The lock is
    /// non-reentrant: never hold two guards at once.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::testenv::lock()
    }

    /// Take the env lock, snapshot the variables the effective_* functions
    /// read, and clear them; restores the snapshot on drop.
    struct EnvGuard {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn take() -> Self {
            let lock = env_lock();
            let mut saved = Vec::new();
            for key in [
                "MUJO_GLYPHS",
                "LC_MUJO_GLYPHS",
                "MUJO_TRUECOLOR",
                "LC_MUJO_TRUECOLOR",
            ] {
                saved.push((key, std::env::var_os(key)));
                std::env::remove_var(key);
            }
            EnvGuard { saved, _lock: lock }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.saved {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    #[test]
    fn test_effective_glyph_mode_flag_beats_env() {
        let _env = EnvGuard::take();
        std::env::set_var("MUJO_GLYPHS", "nerd");
        std::env::set_var("LC_MUJO_GLYPHS", "ascii");
        assert_eq!(effective_glyph_mode(Some("bmp"), "nerd"), "bmp");
    }

    #[test]
    fn test_effective_glyph_mode_mujo_beats_lc_mujo() {
        let _env = EnvGuard::take();
        std::env::set_var("MUJO_GLYPHS", "bmp");
        std::env::set_var("LC_MUJO_GLYPHS", "ascii");
        assert_eq!(effective_glyph_mode(None, "nerd"), "bmp");
    }

    #[test]
    fn test_effective_glyph_mode_lc_mujo_beats_config() {
        let _env = EnvGuard::take();
        std::env::set_var("LC_MUJO_GLYPHS", "ascii");
        assert_eq!(effective_glyph_mode(None, "nerd"), "ascii");
    }

    #[test]
    fn test_effective_glyph_mode_config_used_when_none_set() {
        let _env = EnvGuard::take();
        assert_eq!(effective_glyph_mode(None, "bmp"), "bmp");
    }

    /// The Status block's hint line is three-way on remote/upstream
    /// state: a fix-it for a remote-less store, reassurance when a
    /// remote exists but nothing tracks the branch, and silence once an
    /// upstream is tracked.
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

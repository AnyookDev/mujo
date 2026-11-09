use ratatui::style::{Color, Style};

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
pub(crate) fn cap_style_for(g: Glyphs, accent: Color, outside: Color) -> Style {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::testenv::lock()
    }

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
}

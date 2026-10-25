//! Environment diagnostics for `mujo --diagnose`.
//!
//! `report()` renders a plain-text summary of everything that affects how
//! mujo behaves - terminal capabilities, config lookup chain, store
//! location, glyph/color resolution - without initializing the terminal
//! and without requiring a password store to exist.

use std::path::PathBuf;

use crate::config::Config;
use crate::keys;
use crate::pass;
use crate::ui;

/// One report entry: a label and its value, rendered later with the label
/// column sized to the longest label actually present.
fn field(label: &str, value: &str) -> (String, String) {
    (label.to_string(), value.to_string())
}

/// A continuation entry aligned under the value column of [`field`].
fn continuation(value: &str) -> (String, String) {
    (String::new(), value.to_string())
}

/// Render label/value pairs: the label column is padded to the longest
/// label present, so every value starts at the same column (width + 2).
/// Continuation entries (empty label) are indented to that same column.
fn render(pairs: &[(String, String)]) -> String {
    let width = pairs.iter().map(|(l, _)| l.len()).max().unwrap_or(0);
    let mut out = String::new();
    for (label, value) in pairs {
        if label.is_empty() {
            out.push_str(&format!("{:<width$}  {}", "", value, width = width));
        } else {
            out.push_str(&format!("{label:<width$}: {value}"));
        }
        out.push('\n');
    }
    out
}

/// The value of an environment variable, or "-" when unset or empty.
fn env_or_dash(key: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "-".to_string())
}

/// The value following a `--flag value` pair on the command line, mirroring
/// main's arg_value helper so the report sees the same overrides the TUI
/// would.
fn arg_value(flag: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Path of the gpg-agent config file: $GNUPGHOME/gpg-agent.conf when
/// GNUPGHOME is set and non-empty, else $HOME/.gnupg/gpg-agent.conf.
/// None when neither variable is set.
fn gpg_agent_conf_path() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("GNUPGHOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(home).join("gpg-agent.conf"));
    }
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(|home| PathBuf::from(home).join(".gnupg").join("gpg-agent.conf"))
}

/// The pinentry program configured in gpg-agent.conf: the value of the
/// LAST pinentry-program line, since gpg-agent honours the last
/// occurrence. None when the file is missing, unreadable, or has no such
/// line.
fn pinentry_from_conf() -> Option<String> {
    let text = std::fs::read_to_string(gpg_agent_conf_path()?).ok()?;
    let mut configured = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        if parts.next() == Some("pinentry-program") {
            if let Some(value) = parts.next() {
                configured = Some(value.to_string());
            }
        }
    }
    configured
}

/// Expand a pinentry-program value from gpg-agent.conf: a leading "~/"
/// is expanded against $HOME, a bare program name (no path separator) is
/// resolved to its full PATH location, and absolute paths are kept
/// as-is.
fn expand_pinentry_value(value: &str) -> String {
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
            return PathBuf::from(home).join(rest).to_string_lossy().to_string();
        }
        return value.to_string();
    }
    if !value.contains('/') {
        let search_dirs = std::env::var_os("PATH")
            .map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
            .unwrap_or_default();
        if let Some(dir) = search_dirs
            .iter()
            .find(|dir| is_executable(&dir.join(value)))
        {
            return dir.join(value).to_string_lossy().to_string();
        }
    }
    value.to_string()
}

/// The pinentry program configured for gpg-agent per `gpgconf
/// --list-options gpg-agent`, as a fallback for systems where the config
/// file cannot be read directly. The value lives in one of the value
/// fields after the description (which itself mentions "pinentry", so it
/// must be skipped); empty, a failed gpgconf, a missing gpgconf, or a
/// gpgconf that does not list pinentry-program all return None.
fn pinentry_from_gpgconf() -> Option<String> {
    let output = std::process::Command::new("gpgconf")
        .arg("--list-options")
        .arg("gpg-agent")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.first().copied() != Some("pinentry-program") {
            continue;
        }
        return fields
            .iter()
            .skip(5)
            .take(4)
            .map(|f| f.trim().trim_matches('"'))
            .find(|f| f.contains("pinentry"))
            .map(str::to_string);
    }
    None
}

/// Whether the path exists and is executable; on non-Unix platforms any
/// regular file counts.
fn is_executable(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Which of the known pinentry frontends exist (and are executable) on
/// PATH, in a fixed candidate order, as (name, full path) pairs.
fn pinentry_on_path() -> Vec<(String, String)> {
    const CANDIDATES: [&str; 4] = [
        "pinentry",
        "pinentry-tty",
        "pinentry-curses",
        "pinentry-mac",
    ];
    let search_dirs = std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .unwrap_or_default();

    CANDIDATES
        .iter()
        .filter_map(|candidate| {
            search_dirs
                .iter()
                .find(|dir| is_executable(&dir.join(candidate)))
                .map(|dir| {
                    (
                        candidate.to_string(),
                        dir.join(candidate).to_string_lossy().to_string(),
                    )
                })
        })
        .collect()
}

/// Classification of a resolved pinentry program.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum PinentryKind {
    Terminal,
    Graphical,
    Unknown,
}

/// Classify a resolved pinentry program by its basename: curses/tty
/// frontends prompt in the terminal; mac/gnome/gtk/qt/kwallet frontends
/// open a window. The un-suffixed `pinentry` counts as terminal: when it
/// is an alternatives-style symlink, canonicalization has already
/// resolved the real frontend, so a bare basename means a standalone
/// curses-style default build.
fn classify_pinentry(resolved: &str) -> PinentryKind {
    let base = resolved
        .rsplit('/')
        .next()
        .unwrap_or(resolved)
        .to_lowercase();
    if base.contains("curses") || base.contains("tty") || base == "pinentry" {
        PinentryKind::Terminal
    } else if ["mac", "gnome", "gtk", "qt", "kwallet"]
        .iter()
        .any(|marker| base.contains(marker))
    {
        PinentryKind::Graphical
    } else {
        PinentryKind::Unknown
    }
}

/// Where the effective pinentry program came from.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum PinentrySource {
    /// pinentry-program in gpg-agent.conf (last occurrence wins).
    Config,
    /// pinentry-program as exposed by `gpgconf --list-options gpg-agent`.
    Gpgconf,
    /// The agent's built-in default: the first known frontend on PATH.
    AgentDefault,
}

impl PinentrySource {
    /// The source label shown next to the program in the report.
    fn label(self) -> &'static str {
        match self {
            PinentrySource::Config => "config",
            PinentrySource::Gpgconf => "config (gpgconf)",
            PinentrySource::AgentDefault => "default (agent PATH lookup)",
        }
    }
}

/// The effective pinentry program: the configured program wins; otherwise
/// the agent's built-in default, the first known frontend on PATH. None
/// only when nothing is configured and no frontend exists on PATH.
fn effective_pinentry(
    configured: Option<(String, PinentrySource)>,
    available: &[(String, String)],
) -> Option<(String, PinentrySource)> {
    configured.or_else(|| {
        available
            .first()
            .map(|(_, path)| (path.clone(), PinentrySource::AgentDefault))
    })
}

/// Platform-specific session probe gating the graphical-pinentry warning:
/// pinentry-mac is a Cocoa app that never reads DISPLAY, so macOS checks
/// for an Aqua GUI session instead.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum SessionProbe {
    /// Non-macOS: the value says whether DISPLAY is set.
    Display(bool),
    /// macOS: the value says whether an Aqua GUI session is active.
    Aqua(bool),
}

/// Whether a macOS GUI (Aqua) session is active: `launchctl managername`
/// prints "Aqua" inside a GUI session, and SECURITYSESSIONID is set in
/// one. Neither check alone is authoritative, so either counts.
fn macos_aqua_session() -> bool {
    if std::env::var_os("SECURITYSESSIONID").is_some() {
        return true;
    }
    std::process::Command::new("launchctl")
        .arg("managername")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "Aqua")
        .unwrap_or(false)
}

/// Resolve a pinentry path to its final target with `readlink -f`
/// (following the alternatives-style symlinks); when readlink is
/// unavailable or the path does not exist, the path itself is used.
fn resolve_path(path: &str) -> String {
    std::process::Command::new("readlink")
        .arg("-f")
        .arg(path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| path.to_string())
}

/// The pinentry package to install when none exists, per platform.
fn pinentry_package() -> &'static str {
    if cfg!(target_os = "macos") {
        "pinentry"
    } else {
        "pinentry-tty"
    }
}

/// Build the pinentry report lines from the effective program (with its
/// source label), the raw and canonicalized paths, the frontends found on
/// PATH, and the platform session probe. Split out from `report()` so
/// tests can exercise the wording without gpgconf, readlink, launchctl or
/// a real PATH.
fn pinentry_lines(
    program: Option<(&str, PinentrySource)>,
    raw: &str,
    canonical: &str,
    available: &[String],
    probe: SessionProbe,
) -> Vec<(String, String)> {
    let mut lines = Vec::new();

    // The effective program with its source label; "unset" only when the
    // config, gpgconf and the agent's PATH default all came up empty.
    let program_text = match program {
        Some((path, source)) => format!("{path} ({})", source.label()),
        None => "unset".to_string(),
    };
    lines.push(field("pinentry program", &program_text));

    // The canonicalized path; when it differs from the raw path, show the
    // symlink hop explicitly.
    let resolved_text = if canonical.is_empty() {
        "-".to_string()
    } else if !raw.is_empty() && raw != canonical {
        format!("{raw} -> {canonical}")
    } else {
        canonical.to_string()
    };
    lines.push(field("pinentry resolved", &resolved_text));

    let kind = classify_pinentry(canonical);
    let kind_text = match kind {
        PinentryKind::Terminal => "terminal",
        PinentryKind::Graphical => "graphical",
        PinentryKind::Unknown => "unknown",
    };
    lines.push(field("pinentry kind", kind_text));

    // Mark the entry the agent's built-in default would pick: the first
    // known frontend in the candidate order.
    let found_text = if available.is_empty() {
        "-".to_string()
    } else {
        let mut parts: Vec<String> = available.to_vec();
        parts[0] = format!("{} (agent default)", parts[0]);
        parts.join(", ")
    };
    lines.push(field("pinentry on path", &found_text));

    let is_mac = matches!(probe, SessionProbe::Aqua(_));
    let session_ok = match probe {
        SessionProbe::Display(set) => set,
        SessionProbe::Aqua(aqua) => aqua,
    };

    let mut notes: Vec<String> = Vec::new();
    if kind == PinentryKind::Graphical && !session_ok {
        // A graphical pinentry with no session to draw on may still fall
        // back to a terminal prompt (pinentry-gnome3 does), but only when
        // a terminal frontend is actually available.
        let terminal_available = available
            .iter()
            .any(|a| a == "pinentry-curses" || a == "pinentry-tty");
        let outcome = if terminal_available {
            "may fall back to a terminal prompt"
        } else {
            "will hang with no prompt"
        };
        if is_mac {
            // pinentry-mac is a Cocoa app and never reads DISPLAY; what
            // matters on macOS is an Aqua GUI session.
            notes.push(format!(
                "warning: graphical pinentry but no Aqua GUI session - the window \
                 would open on another session's desktop, or {outcome}; set \
                 pinentry-program, e.g. /opt/homebrew/bin/pinentry-mac, then run \
                 gpgconf --kill gpg-agent"
            ));
        } else {
            notes.push(format!(
                "warning: graphical pinentry but DISPLAY is unset - {outcome}; \
                 set pinentry-program, e.g. /usr/bin/pinentry-curses, then run \
                 gpgconf --kill gpg-agent"
            ));
        }
    }
    if canonical.is_empty() && available.is_empty() {
        notes.push(format!(
            "warning: no pinentry found - gpg will hang with no prompt; install {}",
            pinentry_package()
        ));
    }
    if is_mac && kind != PinentryKind::Graphical && !available.iter().any(|a| a == "pinentry-mac") {
        notes.push(
            "info: no pinentry-mac on PATH - the agent will prompt in the terminal".to_string(),
        );
    }

    for (i, note) in notes.iter().enumerate() {
        if i == 0 {
            lines.push(field("pinentry note", note));
        } else {
            lines.push(continuation(note));
        }
    }

    lines
}

/// Human-readable name of the glyph tier that `resolve_glyphs` produced.
/// The tiers are distinguished by their left caps, which differ per tier.
fn glyphs_tier_name(g: ui::Glyphs) -> &'static str {
    if g.cap_left == ui::GLYPHS_NERD.cap_left {
        "nerd"
    } else if g.cap_left == ui::GLYPHS_BMP.cap_left {
        "bmp"
    } else {
        "ascii"
    }
}

/// Build the full `--diagnose` report.
///
/// The `--glyphs`/`--truecolor`/`--keyboard` CLI flags and the
/// `MUJO_GLYPHS`/`MUJO_TRUECOLOR`/`MUJO_KEYBOARD` environment variables
/// are resolved exactly as main
/// does before the display lines are rendered, so the report reflects
/// what a normal run would actually use. Nothing secret is printed:
/// paths and terminal capabilities only.
pub fn report() -> String {
    let mut entries: Vec<(String, String)> = Vec::new();

    // Read the config without side effects: unlike startup, a missing
    // config file is NOT created here.
    let mut config = Config::config_path()
        .ok()
        .filter(|p| p.exists())
        .and_then(|p| Config::load_or_create(&p).ok())
        .unwrap_or_default();

    // Capture the raw config values for the "config" lines, then resolve
    // the effective modes exactly as main does: env override wins over the
    // config, CLI flag wins over both.
    let raw_glyphs = config.display.glyphs.clone();
    let raw_truecolor = config.display.truecolor.clone();
    let raw_keyboard = config.display.keyboard.clone();
    config.display.glyphs = ui::effective_glyph_mode(arg_value("--glyphs").as_deref(), &raw_glyphs);
    config.display.truecolor =
        ui::effective_truecolor_mode(arg_value("--truecolor").as_deref(), &raw_truecolor);
    config.display.keyboard =
        keys::effective_keyboard_mode(arg_value("--keyboard").as_deref(), &raw_keyboard);

    entries.push(field("mujo version", env!("CARGO_PKG_VERSION")));

    let term_program = std::env::var("TERM_PROGRAM").ok().filter(|v| !v.is_empty());
    let term_version = std::env::var("TERM_PROGRAM_VERSION")
        .ok()
        .filter(|v| !v.is_empty());
    let terminal = match (term_program, term_version) {
        (Some(p), Some(v)) => format!("{} {}", p, v),
        (Some(p), None) => p,
        (None, _) => "-".to_string(),
    };
    entries.push(field("terminal", &terminal));

    entries.push(field("TERM", &env_or_dash("TERM")));
    entries.push(field("COLORTERM", &env_or_dash("COLORTERM")));

    let ssh =
        if std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some() {
            "yes"
        } else {
            "no"
        };
    entries.push(field("ssh", ssh));

    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| "-".to_string());
    entries.push(field("locale", &locale));

    let config_file = Config::config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "-".to_string());
    entries.push(field("config file", &config_file));

    match Config::config_candidates() {
        Ok(candidates) if !candidates.is_empty() => {
            for (i, path) in candidates.iter().enumerate() {
                let entry = if path.exists() {
                    format!("{} (exists)", path.display())
                } else {
                    path.display().to_string()
                };
                if i == 0 {
                    entries.push(field("config chain", &entry));
                } else {
                    entries.push(continuation(&entry));
                }
            }
        }
        _ => {
            entries.push(field("config chain", "-"));
        }
    }

    let store = pass::store_path();
    let store_value = format!(
        "{} ({})",
        store.display(),
        if store.exists() {
            "exists"
        } else {
            "not found"
        }
    );
    entries.push(field("store", &store_value));

    entries.push(field("glyphs config", &raw_glyphs));
    entries.push(field(
        "glyphs effective",
        glyphs_tier_name(ui::resolve_glyphs(&config.display.glyphs)),
    ));
    entries.push(field("truecolor config", &raw_truecolor));
    entries.push(field(
        "truecolor effective",
        if ui::resolve_truecolor(&config.display.truecolor) {
            "true"
        } else {
            "false"
        },
    ));

    // Keyboard chord advertising: the raw config value plus the platform
    // the mode resolves to. Only the advertising differs per platform;
    // every chord works everywhere, so this line is informational.
    let keyboard_platform = match keys::resolve_keyboard(&config.display.keyboard) {
        keys::Platform::Mac => "mac",
        keys::Platform::Linux => "linux",
    };
    entries.push(field("keyboard config", &raw_keyboard));
    entries.push(field("keyboard effective", keyboard_platform));

    // GPG_TTY: gpg uses it to find the terminal for passphrase prompts.
    // mujo sets it for gpg-invoking children when it is unset, so an
    // unset value here is handled, not broken.
    let gpg_tty = std::env::var("GPG_TTY").ok().filter(|v| !v.is_empty());
    entries.push(field(
        "GPG_TTY",
        &gpg_tty.unwrap_or_else(|| "unset - mujo sets it for gpg-invoking children".to_string()),
    ));

    // Pinentry: what gpg-agent will use for passphrase prompts. The
    // resolution chain is: pinentry-program from gpg-agent.conf (the last
    // line wins), then gpgconf where it exposes the option, then the
    // agent's built-in default - the first known frontend on PATH - and
    // only "unset" when all three come up empty. The resolved path is
    // canonicalized so alternatives-style symlinks show their target, and
    // the PATH scan shows which frontends could actually serve.
    let found = pinentry_on_path();
    let available: Vec<String> = found.iter().map(|(name, _)| name.clone()).collect();
    let configured = if let Some(value) = pinentry_from_conf().map(|v| expand_pinentry_value(&v)) {
        Some((value, PinentrySource::Config))
    } else {
        pinentry_from_gpgconf().map(|value| (value, PinentrySource::Gpgconf))
    };
    let program = effective_pinentry(configured, &found);
    let raw = program.as_ref().map(|(p, _)| p.clone()).unwrap_or_default();
    let canonical = if raw.is_empty() {
        String::new()
    } else {
        resolve_path(&raw)
    };
    let probe = if cfg!(target_os = "macos") {
        SessionProbe::Aqua(macos_aqua_session())
    } else {
        SessionProbe::Display(matches!(std::env::var("DISPLAY"), Ok(v) if !v.is_empty()))
    };
    entries.extend(pinentry_lines(
        program.as_ref().map(|(p, s)| (p.as_str(), *s)),
        &raw,
        &canonical,
        &available,
        probe,
    ));

    let fonts = nerd_fonts();
    let fonts_value = if fonts.is_empty() {
        "none found".to_string()
    } else {
        fonts.join(", ")
    };
    entries.push(field("nerd fonts", &fonts_value));

    render(&entries)
}

/// Substrings (matched case-insensitively) that identify a Nerd Font
/// family or filename.
const NERD_MARKERS: [&str; 8] = [
    "nerd font",
    "nerdfont",
    " nf",
    "meslolgs",
    "hack nerd",
    "jetbrainsmono nerd",
    "firacode nerd",
    "caskaydiacove",
];

fn is_nerd_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    NERD_MARKERS.iter().any(|m| lower.contains(m))
}

/// Font directories scanned when fontconfig is not available,
/// non-recursively.
fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join("Library/Fonts"));
        dirs.push(home.join(".local/share/fonts"));
    }
    dirs.push(PathBuf::from("/Library/Fonts"));
    dirs.push(PathBuf::from("/System/Library/Fonts"));
    dirs.push(PathBuf::from("/usr/share/fonts"));
    dirs
}

/// Font file extensions stripped from candidates, case-insensitively.
const FONT_EXTENSIONS: [&str; 4] = [".ttf", ".otf", ".ttc", ".dfont"];

/// Trailing weight/style words stripped from candidates, compared
/// case-insensitively. Mono and Propo are deliberately absent: they
/// distinguish real Nerd Font variants and must survive stripping.
const STYLE_WORDS: [&str; 16] = [
    "thin",
    "extralight",
    "light",
    "regular",
    "med",
    "medium",
    "ret",
    "semibold",
    "sembd",
    "bold",
    "extrabold",
    "black",
    "italic",
    "oblique",
    "condensed",
    "expanded",
];

/// Reduce a font candidate (family name or filename) to its base family:
/// trailing extension removed, anything from the first '-' on dropped
/// (PostScript-style suffixes), and trailing weight/style words stripped
/// repeatedly. The name is never stripped down to nothing.
fn normalize_font_name(name: &str) -> String {
    let mut base = name.trim().to_string();

    let lower = base.to_lowercase();
    if let Some(ext) = FONT_EXTENSIONS.iter().find(|e| lower.ends_with(*e)) {
        base.truncate(base.len() - ext.len());
    }

    if let Some(idx) = base.find('-') {
        base.truncate(idx);
    }

    loop {
        let words: Vec<&str> = base.split_whitespace().collect();
        let Some((last, rest)) = words.split_last() else {
            break;
        };
        // Never strip the only remaining word: the base must stay non-empty.
        let last_lower = last.to_lowercase();
        if rest.is_empty() || !STYLE_WORDS.contains(&last_lower.as_str()) {
            break;
        }
        base = rest.join(" ");
    }

    base
}

/// Normalize candidates, keep the Nerd ones, deduplicate
/// case-insensitively (first spelling wins), sort, and cap at 5.
fn collect_fonts<I>(candidates: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let mut found: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    for candidate in candidates {
        let name = normalize_font_name(&candidate);
        if name.is_empty() || !is_nerd_name(&name) {
            continue;
        }
        let key = name.to_lowercase();
        if !seen.contains(&key) {
            seen.push(key);
            found.push(name);
        }
    }

    found.sort();
    found.truncate(5);
    found
}

/// Detect installed Nerd Fonts, for the report's "nerd fonts" line.
///
/// On Linux, fontconfig (`fc-list : family`) is used when available;
/// otherwise (including when fc-list is missing) the usual font
/// directories are scanned non-recursively. Candidates are normalized to
/// base family names before matching and deduplication. Returns a
/// deduplicated, sorted list capped at 5 entries; never panics - failures
/// simply yield fewer or no results.
pub fn nerd_fonts() -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();

    let fc_list = if cfg!(target_os = "linux") {
        std::process::Command::new("fc-list")
            .arg(":")
            .arg("family")
            .output()
            .ok()
    } else {
        None
    };

    if let Some(output) = fc_list.filter(|o| o.status.success()) {
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            // fc-list separates families within one entry with commas.
            for family in line.split(',') {
                candidates.push(family.trim().to_string());
            }
        }
    } else {
        for dir in font_dirs() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                candidates.push(entry.file_name().to_string_lossy().to_string());
            }
        }
    }

    collect_fonts(candidates)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialize env access across tests: report() reads environment
    /// variables, so parallel test threads must not observe partial
    /// updates while another test is mutating them. Delegates to
    /// testenv::lock so diagnose tests exclude pass/handlers and each
    /// other on the SAME process-global lock. The lock is non-reentrant:
    /// never hold two guards at once.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::testenv::lock()
    }

    /// Override environment variables for the duration of the test,
    /// restoring the previous values on drop.
    struct EnvGuard {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        /// Each entry is (variable, Some(value) to set or None to remove).
        fn set(vars: &[(&'static str, Option<&str>)]) -> Self {
            let lock = env_lock();
            let saved = vars
                .iter()
                .map(|(k, _)| (*k, std::env::var_os(k)))
                .collect();
            for (k, v) in vars {
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
            Self { saved, _lock: lock }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in &self.saved {
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    /// Extract the value of a labeled line ("label: value") from a report.
    fn field_value(report: &str, label: &str) -> String {
        report
            .lines()
            .find_map(|line| {
                let rest = line.trim_start().strip_prefix(label)?;
                rest.trim_start().strip_prefix(": ").map(str::to_string)
            })
            .unwrap_or_else(|| panic!("report missing line for label: {label}"))
    }

    #[test]
    fn test_is_nerd_name() {
        assert!(is_nerd_name("MesloLGS NF"));
        assert!(is_nerd_name("Hack Nerd Font"));
        assert!(is_nerd_name("JetBrainsMono Nerd Font"));
        assert!(is_nerd_name("CaskaydiaCove Nerd Font"));
        assert!(is_nerd_name("FiraCode Nerd Font"));
        assert!(!is_nerd_name("DejaVu Sans"));
        assert!(!is_nerd_name("Arial"));
        assert!(!is_nerd_name(""));
    }

    #[test]
    fn test_field_padding() {
        let out = render(&[
            field("TERM", "xterm"),
            field("pinentry resolved", "/usr/bin/pinentry"),
        ]);
        // The width comes from the longest label present, so every value
        // starts at the same column.
        assert_eq!(out.lines().next(), Some("TERM             : xterm"));
        assert_eq!(
            out.lines().nth(1),
            Some("pinentry resolved: /usr/bin/pinentry")
        );
    }

    #[test]
    fn test_report_values_all_share_one_column() {
        let _lock = env_lock();
        let report = report();
        let separators: Vec<usize> = report.lines().filter_map(|line| line.find(": ")).collect();
        assert!(!separators.is_empty(), "report has no labeled lines");
        assert!(
            separators.iter().all(|&i| i == separators[0]),
            "': ' separators at inconsistent columns: {separators:?}"
        );
    }

    #[test]
    fn test_nerd_fonts_sorted_deduped_capped() {
        let fonts = nerd_fonts();
        assert!(fonts.len() <= 5);
        let mut sorted = fonts.clone();
        sorted.sort();
        assert_eq!(fonts, sorted);
        let unique: std::collections::HashSet<_> = fonts.iter().collect();
        assert_eq!(fonts.len(), unique.len());
    }

    #[test]
    fn test_collect_fonts_normalizes_candidates() {
        let fonts = collect_fonts([
            // Weight/style suffixes collapse into one entry; the first
            // spelling wins on case-insensitive duplicates.
            "FiraCode Nerd Font Mono Light".to_string(),
            "FiraCode Nerd Font Mono Bold Italic".to_string(),
            "firacode nerd font mono".to_string(),
            // Mono stays a distinct family from its non-Mono sibling.
            "Hack Nerd Font".to_string(),
            "Hack Nerd Font Mono".to_string(),
            // Filenames: extension and PostScript dash suffix removed.
            "HackNerdFontMono-BoldItalic.ttf".to_string(),
            "HackNerdFont-Bold.ttf".to_string(),
            // Non-Nerd fonts are filtered out even after normalization.
            "DejaVu Sans.ttf".to_string(),
            "Arial Bold.otf".to_string(),
        ]);

        assert_eq!(
            fonts,
            vec![
                "FiraCode Nerd Font Mono",
                "Hack Nerd Font",
                "Hack Nerd Font Mono",
                "HackNerdFont",
                "HackNerdFontMono",
            ]
        );
    }

    #[test]
    fn test_mujo_glyphs_env_override_differs_from_config_line() {
        let _guard = EnvGuard::set(&[("MUJO_GLYPHS", None)]);
        let baseline = report();
        let raw = field_value(&baseline, "glyphs config");

        // Pick an override that differs from the configured value.
        let override_value = if raw == "ascii" { "bmp" } else { "ascii" };
        std::env::set_var("MUJO_GLYPHS", override_value);
        let overridden = report();

        // The config line keeps showing the raw config; the effective
        // line reflects the env override.
        assert_eq!(field_value(&overridden, "glyphs config"), raw);
        assert_eq!(field_value(&overridden, "glyphs effective"), override_value);
        assert_ne!(
            field_value(&overridden, "glyphs config"),
            field_value(&overridden, "glyphs effective")
        );
    }

    #[test]
    fn test_mujo_keyboard_env_override_differs_from_config_line() {
        let _guard = EnvGuard::set(&[("MUJO_KEYBOARD", None)]);
        let baseline = report();
        let raw = field_value(&baseline, "keyboard config");

        // Pick an override that resolves to a different platform than the
        // configured value advertises.
        let override_value = if keys::resolve_keyboard(&raw) == keys::Platform::Mac {
            "linux"
        } else {
            "mac"
        };
        std::env::set_var("MUJO_KEYBOARD", override_value);
        let overridden = report();

        // The config line keeps showing the raw config; the platform line
        // reflects the env override.
        assert_eq!(field_value(&overridden, "keyboard config"), raw);
        assert_eq!(
            field_value(&overridden, "keyboard effective"),
            override_value
        );
    }

    #[test]
    fn test_auto_glyphs_with_cjk_locale_resolves_to_ascii() {
        let _guard = EnvGuard::set(&[
            ("MUJO_GLYPHS", Some("auto")),
            ("LC_ALL", Some("ja_JP.UTF-8")),
        ]);
        let rep = report();
        assert_eq!(field_value(&rep, "glyphs effective"), "ascii");
    }

    #[test]
    fn test_report_contains_all_sections() {
        // Read-only: report() must not create a config file or require a
        // store, so it is safe to call against the real environment.
        let _lock = env_lock();
        let report = report();
        for label in [
            "mujo version",
            "terminal",
            "TERM",
            "COLORTERM",
            "ssh",
            "locale",
            "config file",
            "config chain",
            "store",
            "glyphs config",
            "glyphs effective",
            "truecolor config",
            "truecolor effective",
            "keyboard config",
            "keyboard effective",
            "GPG_TTY",
            "pinentry program",
            "pinentry resolved",
            "pinentry kind",
            "pinentry on path",
            "nerd fonts",
        ] {
            assert!(report.contains(label), "missing label: {}", label);
        }
    }

    #[test]
    fn test_classify_pinentry() {
        assert_eq!(
            classify_pinentry("/usr/bin/pinentry-tty"),
            PinentryKind::Terminal
        );
        assert_eq!(
            classify_pinentry("/usr/bin/pinentry-curses"),
            PinentryKind::Terminal
        );
        // The un-suffixed default build counts as terminal.
        assert_eq!(
            classify_pinentry("/usr/bin/pinentry"),
            PinentryKind::Terminal
        );
        assert_eq!(
            classify_pinentry("/usr/bin/pinentry-qt"),
            PinentryKind::Graphical
        );
        assert_eq!(
            classify_pinentry("/opt/homebrew/bin/pinentry-mac"),
            PinentryKind::Graphical
        );
        assert_eq!(
            classify_pinentry("/usr/bin/pinentry-gtk"),
            PinentryKind::Graphical
        );
        assert_eq!(
            classify_pinentry("/usr/bin/pinentry-kwallet"),
            PinentryKind::Graphical
        );
        assert_eq!(classify_pinentry(""), PinentryKind::Unknown);
    }

    #[test]
    fn test_pinentry_section_names_configured_program() {
        let lines = pinentry_lines(
            Some(("/usr/bin/pinentry-curses", PinentrySource::Config)),
            "/usr/bin/pinentry-curses",
            "/usr/bin/pinentry-curses",
            &["pinentry".to_string(), "pinentry-curses".to_string()],
            SessionProbe::Display(true),
        );
        let text = render(&lines);
        assert_eq!(
            field_value(&text, "pinentry program"),
            "/usr/bin/pinentry-curses (config)"
        );
        assert_eq!(field_value(&text, "pinentry kind"), "terminal");
        // The agent default mark lands on the first PATH entry.
        assert_eq!(
            field_value(&text, "pinentry on path"),
            "pinentry (agent default), pinentry-curses"
        );
        // A terminal pinentry needs no session warning regardless of DISPLAY.
        assert!(!text.contains("pinentry note"));
    }

    #[test]
    fn test_pinentry_section_warns_graphical_without_display() {
        let lines = pinentry_lines(
            Some(("/usr/bin/pinentry-qt", PinentrySource::Config)),
            "/usr/bin/pinentry-qt",
            "/usr/bin/pinentry-qt",
            &["pinentry-qt".to_string()],
            SessionProbe::Display(false),
        );
        let text = render(&lines);
        assert!(text.contains("graphical"));
        assert!(text.contains("DISPLAY is unset"));
        // No terminal pinentry on PATH: the harsh wording applies.
        assert!(text.contains("will hang with no prompt"));
        assert!(text.contains("gpgconf --kill gpg-agent"));
        // The macOS wording never appears under the DISPLAY probe.
        assert!(!text.contains("Aqua"));
        assert!(!text.contains("prompt in the terminal"));
    }

    #[test]
    fn test_pinentry_section_fallback_wording_with_terminal_available() {
        let lines = pinentry_lines(
            Some(("/usr/bin/pinentry-gnome3", PinentrySource::Config)),
            "/usr/bin/pinentry-gnome3",
            "/usr/bin/pinentry-gnome3",
            &["pinentry-curses".to_string(), "pinentry-gnome3".to_string()],
            SessionProbe::Display(false),
        );
        let text = render(&lines);
        assert!(text.contains("may fall back to a terminal prompt"));
        assert!(!text.contains("will hang"));
        assert!(text.contains("gpgconf --kill gpg-agent"));
    }

    /// A fresh temp dir whose gpg-agent.conf holds `content`.
    fn conf_dir_with(content: &str) -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(dir.path().join("gpg-agent.conf"), content).expect("write conf");
        dir
    }

    #[test]
    fn test_pinentry_conf_last_line_wins_and_is_terminal() {
        let dir = conf_dir_with(
            "pinentry-program /usr/bin/pinentry-qt\npinentry-program /usr/bin/pinentry-curses\n",
        );
        let gnupghome: &'static str =
            Box::leak(dir.path().to_string_lossy().to_string().into_boxed_str());
        let _guard = EnvGuard::set(&[("GNUPGHOME", Some(gnupghome))]);
        let rep = report();
        let program = field_value(&rep, "pinentry program");
        assert!(
            program.starts_with("/usr/bin/pinentry-curses") && program.contains("(config)"),
            "{program}"
        );
        assert_eq!(field_value(&rep, "pinentry kind"), "terminal");
        // A terminal pinentry needs no warning regardless of session; an
        // informational note (macOS) is allowed.
        assert!(!rep.contains("warning: "));
    }

    #[test]
    fn test_pinentry_empty_conf_reports_unset() {
        let dir = conf_dir_with("# no pinentry-program line\n");
        let gnupghome: &'static str =
            Box::leak(dir.path().to_string_lossy().to_string().into_boxed_str());
        let _guard = EnvGuard::set(&[("GNUPGHOME", Some(gnupghome))]);
        let rep = report();
        // An empty conf must not claim the config source: whatever the
        // gpgconf fallback or the agent default finds, it is not from
        // this file.
        let program = field_value(&rep, "pinentry program");
        assert!(!program.contains("(config)"), "{program}");
    }

    // The DISPLAY-shaped warning is Linux-shaped; macOS uses the Aqua
    // probe instead, so this report-level test is non-macOS only.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn test_pinentry_conf_graphical_without_display_warns_fallback() {
        let dir = conf_dir_with("pinentry-program /usr/bin/pinentry-gnome3\n");
        let gnupghome: &'static str =
            Box::leak(dir.path().to_string_lossy().to_string().into_boxed_str());
        let _guard = EnvGuard::set(&[("GNUPGHOME", Some(gnupghome)), ("DISPLAY", None)]);
        let rep = report();
        assert_eq!(field_value(&rep, "pinentry kind"), "graphical");
        let note = field_value(&rep, "pinentry note");
        assert!(note.contains("DISPLAY is unset"));
        assert!(note.contains("may fall back to a terminal prompt"));
        assert!(note.contains("gpgconf --kill gpg-agent"));
    }

    #[test]
    fn test_pinentry_section_warns_when_nothing_available() {
        let lines = pinentry_lines(None, "", "", &[], SessionProbe::Display(false));
        let text = render(&lines);
        assert_eq!(field_value(&text, "pinentry program"), "unset");
        assert!(text.contains("gpg will hang"));
        // The package name is platform-specific; both spellings start
        // with "pinentry".
        assert!(text.contains("install pinentry"));
    }

    #[test]
    fn test_resolution_chain_default_when_nothing_configured() {
        // Invariant: with nothing configured and a non-empty on-PATH
        // list, the agent default resolves to Some with an inferred kind.
        let found = vec![
            ("pinentry".to_string(), "/usr/bin/pinentry".to_string()),
            (
                "pinentry-curses".to_string(),
                "/usr/bin/pinentry-curses".to_string(),
            ),
        ];
        let (path, source) = effective_pinentry(None, &found).expect("default resolves");
        assert_eq!(source, PinentrySource::AgentDefault);
        assert!(!path.is_empty());
        assert_ne!(classify_pinentry(&path), PinentryKind::Unknown);

        // A configured program beats the agent default, whatever its source.
        for source in [PinentrySource::Config, PinentrySource::Gpgconf] {
            let configured = Some(("/usr/bin/pinentry-qt".to_string(), source));
            let (path, got) = effective_pinentry(configured, &found).unwrap();
            assert_eq!(got, source);
            assert_eq!(path, "/usr/bin/pinentry-qt");
        }

        // Nothing configured and nothing on PATH: genuinely unset.
        assert_eq!(effective_pinentry(None, &[]), None);
    }

    #[test]
    fn test_pinentry_resolved_shows_symlink_hop() {
        let lines = pinentry_lines(
            Some(("/usr/bin/pinentry", PinentrySource::Config)),
            "/usr/bin/pinentry",
            "/usr/bin/pinentry-curses",
            &["pinentry".to_string()],
            SessionProbe::Display(true),
        );
        let text = render(&lines);
        assert!(text.contains("/usr/bin/pinentry -> /usr/bin/pinentry-curses"));
        // The kind is inferred from the canonical basename.
        assert_eq!(field_value(&text, "pinentry kind"), "terminal");
    }

    #[test]
    fn test_pinentry_warning_gated_by_platform_probe() {
        let configured_mac = Some(("/opt/homebrew/bin/pinentry-mac", PinentrySource::Config));
        // macOS: a graphical pinentry with no Aqua session warns, with the
        // macOS wording and never the DISPLAY wording.
        let mac_lines = pinentry_lines(
            configured_mac,
            "/opt/homebrew/bin/pinentry-mac",
            "/opt/homebrew/bin/pinentry-mac",
            &["pinentry-mac".to_string()],
            SessionProbe::Aqua(false),
        );
        let mac_text = render(&mac_lines);
        assert!(mac_text.contains("no Aqua GUI session"));
        assert!(!mac_text.contains("DISPLAY is unset"));
        // No terminal frontend on PATH: the harsh wording applies.
        assert!(mac_text.contains("will hang with no prompt"));

        // An Aqua session silences the warning.
        let ok_lines = pinentry_lines(
            configured_mac,
            "/opt/homebrew/bin/pinentry-mac",
            "/opt/homebrew/bin/pinentry-mac",
            &["pinentry-mac".to_string()],
            SessionProbe::Aqua(true),
        );
        assert!(!render(&ok_lines).contains("pinentry note"));

        // Non-macOS: DISPLAY set silences the warning.
        let linux_lines = pinentry_lines(
            Some(("/usr/bin/pinentry-qt", PinentrySource::Config)),
            "/usr/bin/pinentry-qt",
            "/usr/bin/pinentry-qt",
            &["pinentry-qt".to_string()],
            SessionProbe::Display(true),
        );
        assert!(!render(&linux_lines).contains("pinentry note"));
    }

    #[test]
    fn test_pinentry_mac_info_when_no_pinentry_mac() {
        let program = Some(("/usr/bin/pinentry-curses", PinentrySource::AgentDefault));
        let available = vec!["pinentry-curses".to_string()];

        // On macOS without pinentry-mac, note that the agent prompts in
        // the terminal.
        let lines = pinentry_lines(
            program,
            "/usr/bin/pinentry-curses",
            "/usr/bin/pinentry-curses",
            &available,
            SessionProbe::Aqua(true),
        );
        let text = render(&lines);
        assert!(text.contains("no pinentry-mac on PATH"));
        assert!(text.contains("prompt in the terminal"));

        // pinentry-mac on PATH: no note at all.
        let lines = pinentry_lines(
            Some(("/opt/homebrew/bin/pinentry-mac", PinentrySource::Config)),
            "/opt/homebrew/bin/pinentry-mac",
            "/opt/homebrew/bin/pinentry-mac",
            &["pinentry-mac".to_string()],
            SessionProbe::Aqua(true),
        );
        assert!(!render(&lines).contains("pinentry note"));

        // The info is macOS-specific: a DISPLAY probe never shows it.
        let lines = pinentry_lines(
            program,
            "/usr/bin/pinentry-curses",
            "/usr/bin/pinentry-curses",
            &available,
            SessionProbe::Display(true),
        );
        assert!(!render(&lines).contains("prompt in the terminal"));
    }
}

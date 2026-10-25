use anyhow::{Context, Result};
use ratatui::style::Color;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub password_generation: PasswordGeneration,
    #[serde(default)]
    pub colors: ColorConfig,
    #[serde(default)]
    pub git: GitConfig,
    #[serde(default)]
    pub clipboard: ClipboardConfig,
    #[serde(default)]
    pub display: DisplayConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasswordGeneration {
    #[serde(default = "default_length")]
    pub default_length: usize,

    #[serde(default = "default_charset")]
    pub charset: String,

    #[serde(default = "default_custom_charset")]
    pub custom_charset: String,
}

impl Default for PasswordGeneration {
    fn default() -> Self {
        Self {
            default_length: default_length(),
            charset: default_charset(),
            custom_charset: default_custom_charset(),
        }
    }
}

/// The single [colors] section. `theme` selects one of the built-in presets
/// or "custom". When "custom" is selected, the foreground/secondary/accent/
/// background values below are used.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColorConfig {
    #[serde(default = "default_theme")]
    pub theme: String,
    /// When true, the terminal's own background shows through instead of
    /// the theme's background color. Works with any theme.
    #[serde(default = "default_transparent")]
    pub transparent: bool,
    #[serde(default)]
    pub foreground: String,
    #[serde(default)]
    pub secondary: String,
    #[serde(default)]
    pub accent: String,
    #[serde(default)]
    pub background: String,
}

impl Default for ColorConfig {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            transparent: false,
            foreground: String::new(),
            secondary: String::new(),
            accent: String::new(),
            background: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GitConfig {
    #[serde(default)]
    pub remote_url: String,
}

impl GitConfig {
    pub fn remote_opt(&self) -> Option<&str> {
        let s = self.remote_url.trim();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }
}

/// Clipboard auto-clear settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardConfig {
    /// Seconds after a copy before the clipboard is automatically cleared.
    /// 0 disables auto-clear; values above 3600 are clamped to 3600;
    /// negative values fall back to the default of 45.
    #[serde(
        default = "default_clipboard_timeout",
        deserialize_with = "deserialize_clipboard_timeout"
    )]
    pub timeout_seconds: u64,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self {
            timeout_seconds: default_clipboard_timeout(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayConfig {
    /// Glyph set for UI markers: auto, nerd, bmp, ascii.
    ///
    /// "bmp" uses half-block characters present in every system font and
    /// renders correctly with no setup, so it is the safe default: neither
    /// Apple Terminal's Menlo nor iTerm2's default font has Private Use
    /// Area coverage. "nerd" only renders correctly when the user has
    /// installed a Nerd Font. "auto" resolves to bmp on Apple Terminal and
    /// to nerd elsewhere - which means "auto" requires a Nerd Font on
    /// non-Apple terminals.
    #[serde(default = "default_glyphs")]
    pub glyphs: String,
    #[serde(default = "default_truecolor")]
    pub truecolor: String,
    #[serde(default = "default_keyboard")]
    pub keyboard: String,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            glyphs: default_glyphs(),
            truecolor: default_truecolor(),
            keyboard: default_keyboard(),
        }
    }
}

fn default_glyphs() -> String {
    "bmp".to_string()
}
fn default_truecolor() -> String {
    "auto".to_string()
}
fn default_keyboard() -> String {
    "auto".to_string()
}

/// Upper bound for the clipboard timeout (1 hour).
const MAX_CLIPBOARD_TIMEOUT: u64 = 3600;
/// Default clipboard auto-clear delay in seconds.
const DEFAULT_CLIPBOARD_TIMEOUT: u64 = 45;

fn default_clipboard_timeout() -> u64 {
    DEFAULT_CLIPBOARD_TIMEOUT
}

/// Deserialize the timeout leniently: any integer is accepted. Negative
/// values (invalid for u64) fall back to the default instead of failing
/// the whole config parse; values above the maximum are clamped.
fn deserialize_clipboard_timeout<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: i64 = serde::Deserialize::deserialize(deserializer)?;
    Ok(clamp_clipboard_timeout(raw))
}

fn clamp_clipboard_timeout(raw: i64) -> u64 {
    if raw < 0 {
        DEFAULT_CLIPBOARD_TIMEOUT
    } else if raw > MAX_CLIPBOARD_TIMEOUT as i64 {
        MAX_CLIPBOARD_TIMEOUT
    } else {
        raw as u64
    }
}

/// Fully resolved theme colors.
#[derive(Debug, Clone)]
pub struct ThemeColors {
    pub foreground: String,
    pub secondary: String,
    pub accent: String,
    pub background: String,
}

fn default_theme() -> String {
    "lime".to_string()
}

fn default_transparent() -> bool {
    false
}

fn preset_colors(name: &str) -> Option<ThemeColors> {
    match name {
        "dracula" => Some(ThemeColors {
            foreground: "#F8F8F2".to_string(),
            secondary: "#6272A4".to_string(),
            accent: "#BD93F9".to_string(),
            background: "#282A36".to_string(),
        }),
        "solarized" => Some(ThemeColors {
            foreground: "#839496".to_string(),
            secondary: "#586E75".to_string(),
            accent: "#268BD2".to_string(),
            background: "#002B36".to_string(),
        }),
        "gruvbox" => Some(ThemeColors {
            foreground: "#EBDBB2".to_string(),
            secondary: "#928374".to_string(),
            accent: "#FABD2F".to_string(),
            background: "#282828".to_string(),
        }),
        "nord" => Some(ThemeColors {
            foreground: "#D8DEE9".to_string(),
            secondary: "#4C566A".to_string(),
            accent: "#88C0D0".to_string(),
            background: "#2E3440".to_string(),
        }),
        "catppuccin" => Some(ThemeColors {
            foreground: "#CDD6F4".to_string(),
            secondary: "#6C7086".to_string(),
            accent: "#CBA6F7".to_string(),
            background: "#1E1E2E".to_string(),
        }),
        "onedark" => Some(ThemeColors {
            foreground: "#ABB2BF".to_string(),
            secondary: "#5C6370".to_string(),
            accent: "#C678DD".to_string(),
            background: "#282C34".to_string(),
        }),
        "tokyonight" => Some(ThemeColors {
            foreground: "#A9B1D6".to_string(),
            secondary: "#565F89".to_string(),
            accent: "#7AA2F7".to_string(),
            background: "#1A1B26".to_string(),
        }),
        "rosepine" => Some(ThemeColors {
            foreground: "#E0DEF4".to_string(),
            secondary: "#6E6A86".to_string(),
            accent: "#C4A7E7".to_string(),
            background: "#191724".to_string(),
        }),
        "monokai" => Some(ThemeColors {
            foreground: "#F8F8F2".to_string(),
            secondary: "#75715E".to_string(),
            accent: "#FD971F".to_string(),
            background: "#272822".to_string(),
        }),
        "nightowl" => Some(ThemeColors {
            foreground: "#D6DEEB".to_string(),
            secondary: "#4B6479".to_string(),
            accent: "#C792EA".to_string(),
            background: "#011627".to_string(),
        }),
        // Lime: dark gray-green background with a bright lime accent.
        // As in the other presets, the accent doubles as the primary color
        // and the secondary doubles as the border color.
        "lime" => Some(ThemeColors {
            foreground: "#DCE4D2".to_string(),
            secondary: "#7CA57F".to_string(),
            accent: "#C7F542".to_string(),
            background: "#2A2A2A".to_string(),
        }),
        _ => None,
    }
}

fn non_empty_or<'a>(value: &'a str, fallback: &'a str) -> String {
    if value.trim().is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

fn default_length() -> usize {
    30
}
fn default_charset() -> String {
    "all".to_string()
}

fn default_custom_charset() -> String {
    "abcdefghjkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789!@#$%^&*()_+-=[]{};:,.?".to_string()
}

impl Config {
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;
        Self::load_or_create(&config_path)
    }

    /// Load the config from an explicit path, creating a default config file
    /// (and returning the defaults) when the file does not exist. Split out
    /// from `load()` so tests can exercise the real loading logic against
    /// temporary files.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if !path.exists() {
            let default_config = Self::default();
            default_config.save_to(path)?;
            return Ok(default_config);
        }

        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config at {:?}", path))?;

        let config: Config = toml::from_str(&content).context("Failed to parse config file")?;

        Ok(config)
    }

    /// Resolve the colors actually used by the UI.
    ///
    /// - A named preset: dracula, solarized, gruvbox, nord, catppuccin,
    ///   onedark, tokyonight, rosepine, monokai, nightowl, lime.
    /// - "custom": the foreground/secondary/accent/background values in the
    ///   [colors] section (empty fields fall back to the tokyonight preset).
    /// - Unknown theme names fall back to tokyonight.
    /// - `transparent = true` overrides the background with "transparent",
    ///   which parse_color resolves to ratatui's Reset color.
    pub fn effective_theme(&self) -> ThemeColors {
        let theme = self.colors.theme.trim();
        let mut colors = if theme.eq_ignore_ascii_case("custom") {
            let fallback = preset_colors("tokyonight").unwrap();
            ThemeColors {
                foreground: non_empty_or(&self.colors.foreground, &fallback.foreground),
                secondary: non_empty_or(&self.colors.secondary, &fallback.secondary),
                accent: non_empty_or(&self.colors.accent, &fallback.accent),
                background: non_empty_or(&self.colors.background, &fallback.background),
            }
        } else {
            preset_colors(&theme.to_lowercase())
                .unwrap_or_else(|| preset_colors("tokyonight").unwrap())
        };
        if self.colors.transparent {
            colors.background = "transparent".to_string();
        }
        colors
    }

    /// Persist the current config values to config.toml (regenerating the
    /// commented template). Used to keep e.g. the git remote URL in sync
    /// with changes made in the TUI.
    pub fn save(&self) -> Result<()> {
        // Always write to a user-writable path, never a system path, so an
        // admin-provided config is never modified: the $MUJO_CONFIG
        // override when set, otherwise the user config (XDG_CONFIG_HOME or
        // ~/.config). When only a system config exists, this writes a NEW
        // user config holding the merged values (this Config was loaded
        // from the system file plus any in-memory changes).
        let path = match std::env::var_os("MUJO_CONFIG") {
            Some(raw) => PathBuf::from(raw),
            None => Self::user_config_path()?,
        };
        self.save_to(&path)
    }

    fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = format!(
            r##"[password_generation]
# Default password length
default_length = {length}

# Default charset: "all", "alphanumeric", "letters", or "custom"
charset = "{charset}"

# Custom charset used when charset = "custom"
# Includes: lowercase (no l), uppercase (no I, O), numbers (no 0, 1), symbols
# Excludes ambiguous: 0, O, 1, l, I
# 
# Symbols chosen to be web-safe and commonly accepted:
# ! @ # $ % ^ & * ( ) _ + - = [ ] {{ }} ; : , . ?
# Excludes: < > / \ ` ~ | " ' (can break HTML, SQL, shells, or paths)
custom_charset = "{custom_charset}"

[clipboard]
# Clipboard auto-clear timeout in seconds.
# The clipboard is cleared this many seconds after each copy.
# Set to 0 to disable auto-clear. Values above 3600 are clamped to 3600.
timeout_seconds = {timeout_seconds}

[colors]
# Choose a theme: dracula, solarized, gruvbox, nord, catppuccin, onedark,
#              tokyonight, rosepine, monokai, nightowl, lime, custom
theme = "{theme}"

# Specify custom above and uncomment the following to create your own theme:
#foreground = "#F8F8F2"
#secondary = "#6272A4"
#accent = "#BD93F9"
#background = "#282A36"

# UI Colors
# Use named colors: black, red, green, yellow, blue, magenta, cyan, gray, white
# Or hex: #00FFFF (cyan), #FF0000 (red), #808080 (gray)
# "transparent" (or "default"/"reset") shows the terminal's own background.
# Suggested colors:
#   #6B9BFA - Cornflower Blue
#   #A78BFA - Lavender
#   #C4B5FD - Periwinkle
#   #F9A8D4 - Pastel Pink
#   #FB7185 - Coral
#   #FB923C - Peach
#   #FDE68A - Butter
#   #86EFAC - Mint Green

# Set true to let the terminal background show through; works with any theme
transparent = {transparent}

[display]
# Glyph set for UI markers: auto, nerd, bmp, ascii.
# Default is bmp: half-block characters present in every system font,
# rendering correctly with no setup. "nerd" needs a Nerd Font installed
# (neither Apple Terminal's Menlo nor iTerm2's default font has Private
# Use Area coverage). "auto" resolves to bmp on Apple Terminal and to
# nerd elsewhere, so it requires a Nerd Font on non-Apple terminals.
glyphs = "{glyphs}"

# Truecolor output: auto, true, false.
# auto enables RGB where it is honoured and falls back to 256-colour on
# Apple Terminal, which advertises truecolor but discards SGR 38;2.
truecolor = "{truecolor}"

# Keyboard chord advertising: auto, mac, linux.
# auto follows the platform mujo runs on. This only changes which chords
# are ADVERTISED in the help body and status bar (a Mac advertises Ctrl+U,
# Ctrl+A/E, b/Space and g/G instead of Ctrl+arrows, Home/End and PgUp/PgDn);
# every chord works on every platform either way.
keyboard = "{keyboard}"

[git]
# Git (optional)
# If you skipped the remote in the Git pane, set remote_url here,
# then press S (Git pane) or r (refresh). mujo will run:
#   pass git remote add origin <url>
# if origin does not exist yet.
#
# This value is kept in sync with the TUI: entering a remote during
# Git setup saves it here, and editing it here updates git on refresh.
#
# SSH (recommended): git@github.com:USERNAME/REPO.git
#   Auth is your SSH key. Create an empty repository on GitHub first (no
#   README), so the initial push is accepted.
# SSH over 443 (if port 22 is blocked):
#   ssh://git@ssh.github.com:443/USERNAME/REPO.git
# HTTPS: https://github.com/USERNAME/REPO.git
#   Needs a credential helper in a normal terminal; mujo cannot prompt
#   for tokens reliably.
remote_url = "{remote_url}"
"##,
            length = self.password_generation.default_length,
            charset = self.password_generation.charset,
            custom_charset = self.password_generation.custom_charset,
            theme = self.colors.theme,
            transparent = self.colors.transparent,
            remote_url = self.git.remote_url,
            timeout_seconds = self.clipboard.timeout_seconds,
            glyphs = self.display.glyphs,
            truecolor = self.display.truecolor,
            keyboard = self.display.keyboard
        );

        std::fs::write(path, content)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(path)?.permissions();
            perms.set_mode(0o600);
            std::fs::set_permissions(path, perms)?;
        }

        Ok(())
    }

    /// Resolve the config path to READ from, following the lookup chain:
    ///
    /// 1. $MUJO_CONFIG, used verbatim (a missing file there is created
    ///    with defaults, matching the user-path behaviour)
    /// 2. $XDG_CONFIG_HOME/mujo/config.toml, when XDG_CONFIG_HOME is set
    /// 3. ~/.config/mujo/config.toml
    /// 4. /usr/local/etc/mujo/config.toml
    /// 5. $XDG_CONFIG_DIRS/mujo/config.toml for each entry (default /etc/xdg)
    /// 6. /etc/mujo/config.toml
    ///
    /// The first path that EXISTS is returned. If none exist, the user path
    /// (2 or 3, whichever applies) is returned so a default config can be
    /// created there.
    pub fn config_path() -> Result<PathBuf> {
        if let Some(raw) = std::env::var_os("MUJO_CONFIG") {
            return Ok(PathBuf::from(raw));
        }
        let candidates = Self::config_candidates()?;
        if let Some(found) = candidates.into_iter().find(|p| p.exists()) {
            return Ok(found);
        }
        Self::user_config_path()
    }

    /// The ordered candidate list behind [`Config::config_path`], split out
    /// so tests can assert the exact order without depending on whether the
    /// absolute system paths (/usr/local/etc, /etc) exist on the machine.
    ///
    /// When $MUJO_CONFIG is set the list contains only that path: it is
    /// honoured verbatim and the rest of the chain is not consulted.
    /// Otherwise the user path comes first (the XDG path when
    /// XDG_CONFIG_HOME is set, else ~/.config - matching user_config_path,
    /// so the home path is never a candidate alongside the XDG path),
    /// followed by the system candidates in lookup order.
    pub fn config_candidates() -> Result<Vec<PathBuf>> {
        if let Some(raw) = std::env::var_os("MUJO_CONFIG") {
            return Ok(vec![PathBuf::from(raw)]);
        }
        let user = Self::user_config_path()?;

        let mut candidates = vec![user, PathBuf::from("/usr/local/etc/mujo/config.toml")];
        match std::env::var_os("XDG_CONFIG_DIRS") {
            Some(dirs) => {
                for dir in std::env::split_paths(&dirs) {
                    if dir.as_os_str().is_empty() {
                        continue;
                    }
                    candidates.push(dir.join("mujo").join("config.toml"));
                }
            }
            None => candidates.push(PathBuf::from("/etc/xdg/mujo/config.toml")),
        }
        candidates.push(PathBuf::from("/etc/mujo/config.toml"));
        Ok(candidates)
    }

    /// The user-owned config path: $XDG_CONFIG_HOME/mujo/config.toml when
    /// XDG_CONFIG_HOME is set, otherwise ~/.config/mujo/config.toml.
    ///
    /// ~/.config/mujo/config.toml on ALL platforms (when XDG_CONFIG_HOME is
    /// unset). dirs::config_dir() resolves to ~/Library/Application Support
    /// on macOS, which would split mujo's config across platform-specific
    /// locations; the home relative path keeps one location everywhere (and
    /// is unchanged on Linux).
    ///
    /// This is also the only path ever WRITTEN: the system-wide candidates
    /// (/usr/local/etc, XDG_CONFIG_DIRS, /etc) are read-only to mujo so an
    /// admin-provided config is never modified; changed settings are saved
    /// as a new user config instead.
    fn user_config_path() -> Result<PathBuf> {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return Ok(PathBuf::from(xdg).join("mujo").join("config.toml"));
            }
        }
        let home = dirs::home_dir().context("Could not find home directory")?;
        Ok(home.join(".config").join("mujo").join("config.toml"))
    }

    pub fn parse_color(&self, color_str: &str) -> Color {
        match color_str.to_lowercase().as_str() {
            "black" => Color::Black,
            "red" => Color::Red,
            "green" => Color::Green,
            "yellow" => Color::Yellow,
            "blue" => Color::Blue,
            "magenta" => Color::Magenta,
            "cyan" => Color::Cyan,
            "gray" | "grey" => Color::Gray,
            "darkgray" | "darkgrey" => Color::DarkGray,
            "white" => Color::White,
            // Transparent background: ratatui's Reset color leaves the
            // terminal's own (possibly translucent) background visible.
            "transparent" | "default" | "reset" => Color::Reset,
            hex if hex.starts_with('#') && hex.len() == 7 => {
                let r = u8::from_str_radix(&hex[1..3], 16).unwrap_or(0);
                let g = u8::from_str_radix(&hex[3..5], 16).unwrap_or(0);
                let b = u8::from_str_radix(&hex[5..7], 16).unwrap_or(0);
                Color::Rgb(r, g, b)
            }
            _ => Color::Cyan,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Write the given TOML content to a config.toml inside a fresh temp
    /// directory and return the path.
    fn write_config(dir: &TempDir, content: &str) -> PathBuf {
        let path = dir.path().join("config.toml");
        std::fs::write(&path, content).expect("failed to write test config");
        path
    }

    #[test]
    fn test_parse_full_config() {
        let dir = TempDir::new().unwrap();
        // Note: r##"..."## is required here because the TOML contains hex
        // colors like "#FFFFFF", and a r#"..."# raw string would terminate
        // at the first `"#` sequence.
        let content = r##"
[password_generation]
default_length = 42
charset = "custom"
custom_charset = "abcABC123!@#"

[colors]
theme = "custom"
foreground = "#FFFFFF"
secondary = "#888888"
accent = "#FF0000"
background = "#000000"

[git]
remote_url = "git@github.com:user/repo.git"
"##;
        let config = Config::load_or_create(&write_config(&dir, content)).unwrap();

        assert_eq!(config.password_generation.default_length, 42);
        assert_eq!(config.password_generation.charset, "custom");
        assert_eq!(config.password_generation.custom_charset, "abcABC123!@#");

        assert_eq!(config.colors.theme, "custom");
        assert_eq!(config.colors.foreground, "#FFFFFF");
        assert_eq!(config.colors.secondary, "#888888");
        assert_eq!(config.colors.accent, "#FF0000");
        assert_eq!(config.colors.background, "#000000");

        assert_eq!(config.git.remote_url, "git@github.com:user/repo.git");

        // The resolved theme must use the custom values verbatim.
        let theme = config.effective_theme();
        assert_eq!(theme.foreground, "#FFFFFF");
        assert_eq!(theme.secondary, "#888888");
        assert_eq!(theme.accent, "#FF0000");
        assert_eq!(theme.background, "#000000");

        // The remote_opt accessor treats a non-empty value as Some.
        assert_eq!(
            config.git.remote_opt(),
            Some("git@github.com:user/repo.git")
        );
    }

    #[test]
    fn test_parse_minimal_config() {
        let dir = TempDir::new().unwrap();
        // An empty file is valid: every field has a serde default.
        let config = Config::load_or_create(&write_config(&dir, "")).unwrap();

        assert_eq!(config.password_generation.default_length, default_length());
        assert_eq!(config.password_generation.charset, "all");
        assert_eq!(
            config.password_generation.custom_charset,
            default_custom_charset()
        );

        assert_eq!(config.colors.theme, "lime");
        assert!(!config.colors.transparent);
        assert!(config.colors.foreground.is_empty());
        assert!(config.colors.secondary.is_empty());
        assert!(config.colors.accent.is_empty());
        assert!(config.colors.background.is_empty());

        assert!(config.git.remote_url.is_empty());
        assert_eq!(config.git.remote_opt(), None);

        // With no custom colors, the default preset is used.
        let theme = config.effective_theme();
        let lime = preset_colors("lime").unwrap();
        assert_eq!(theme.foreground, lime.foreground);
        assert_eq!(theme.accent, lime.accent);
        // With transparent defaulting to false, the preset background hex
        // survives.
        assert_eq!(theme.background, "#2A2A2A");
        assert_eq!(
            config.parse_color(&theme.background),
            Color::Rgb(0x2A, 0x2A, 0x2A)
        );
    }

    #[test]
    fn test_parse_theme_preset() {
        let dir = TempDir::new().unwrap();
        let content = r#"
[colors]
theme = "dracula"
"#;
        let config = Config::load_or_create(&write_config(&dir, content)).unwrap();

        assert_eq!(config.colors.theme, "dracula");
        let theme = config.effective_theme();
        assert_eq!(theme.foreground, "#F8F8F2");
        assert_eq!(theme.secondary, "#6272A4");
        assert_eq!(theme.accent, "#BD93F9");
        assert_eq!(theme.background, "#282A36");
    }

    #[test]
    fn test_parse_custom_theme() {
        let dir = TempDir::new().unwrap();
        // r##"..."## for the same reason as in test_parse_full_config: the
        // TOML contains hex colors with the `"#` sequence.
        let content = r##"
[colors]
theme = "custom"
foreground = "#123456"
accent = "#654321"
"##;
        let config = Config::load_or_create(&write_config(&dir, content)).unwrap();

        let theme = config.effective_theme();
        // Explicitly set custom values are used...
        assert_eq!(theme.foreground, "#123456");
        assert_eq!(theme.accent, "#654321");
        // ...while empty fields fall back to the tokyonight preset.
        let tokyonight = preset_colors("tokyonight").unwrap();
        assert_eq!(theme.secondary, tokyonight.secondary);
        assert_eq!(theme.background, tokyonight.background);
    }

    #[test]
    fn test_parse_invalid_config() {
        let dir = TempDir::new().unwrap();
        // Malformed TOML: the [colors] table header is never closed before
        // the file ends mid-key.
        let content = "[colors\ntheme = \"dracula\"";
        let result = Config::load_or_create(&write_config(&dir, content));

        let err = result.expect_err("malformed TOML must produce an error");
        let msg = format!("{}", err);
        assert!(
            msg.contains("Failed to parse config file"),
            "error message should be useful, got: {}",
            msg
        );
    }

    #[test]
    fn test_missing_config_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");

        // No file exists yet: defaults are returned and a default config
        // file is created at the given path.
        assert!(!path.exists());
        let config = Config::load_or_create(&path).unwrap();
        assert!(path.exists(), "a missing config file should be created");

        assert_eq!(config.password_generation.default_length, default_length());
        assert_eq!(config.colors.theme, "lime");
        assert!(!config.colors.transparent);
        assert!(config.git.remote_url.is_empty());

        // The created file must itself be parseable.
        let reloaded = Config::load_or_create(&path).unwrap();
        assert_eq!(reloaded.colors.theme, "lime");
        assert_eq!(
            reloaded.password_generation.default_length,
            default_length()
        );
    }

    #[test]
    fn test_partial_config() {
        let dir = TempDir::new().unwrap();
        // Only one section present; the others must be filled with defaults.
        let content = r#"
[colors]
theme = "nord"
"#;
        let config = Config::load_or_create(&write_config(&dir, content)).unwrap();

        assert_eq!(config.colors.theme, "nord");
        // Missing sections fall back to their defaults.
        assert_eq!(config.password_generation.default_length, default_length());
        assert_eq!(config.password_generation.charset, "all");
        assert!(config.git.remote_url.is_empty());

        let theme = config.effective_theme();
        assert_eq!(theme.accent, "#88C0D0");
        assert_eq!(theme.background, "#2E3440");
    }

    #[test]
    fn test_unknown_theme_falls_back_to_tokyonight() {
        let dir = TempDir::new().unwrap();
        let content = r#"
[colors]
theme = "not-a-real-theme"
"#;
        let config = Config::load_or_create(&write_config(&dir, content)).unwrap();

        let theme = config.effective_theme();
        let tokyonight = preset_colors("tokyonight").unwrap();
        assert_eq!(theme.foreground, tokyonight.foreground);
        assert_eq!(theme.secondary, tokyonight.secondary);
        assert_eq!(theme.accent, tokyonight.accent);
        assert_eq!(theme.background, tokyonight.background);
    }

    #[test]
    fn test_transparent_flag_overrides_background() {
        let dir = TempDir::new().unwrap();
        let content = r#"
[colors]
theme = "nord"
transparent = true
"#;
        let config = Config::load_or_create(&write_config(&dir, content)).unwrap();

        let theme = config.effective_theme();
        // The flag overrides the preset background...
        assert_eq!(theme.background, "transparent");
        assert_eq!(config.parse_color(&theme.background), Color::Reset);
        // ...while all other colors come from the preset unchanged.
        assert_eq!(theme.foreground, "#D8DEE9");
        assert_eq!(theme.secondary, "#4C566A");
        assert_eq!(theme.accent, "#88C0D0");

        // Flag off (or absent): the preset background is kept.
        let content = r#"
[colors]
theme = "nord"
"#;
        let config = Config::load_or_create(&write_config(&dir, content)).unwrap();
        let theme = config.effective_theme();
        assert_eq!(theme.background, "#2E3440");
        assert_eq!(
            config.parse_color(&theme.background),
            Color::Rgb(0x2E, 0x34, 0x40)
        );
    }

    #[test]
    fn test_clipboard_timeout_validation() {
        let dir = TempDir::new().unwrap();

        // Missing section: default 45.
        let config = Config::load_or_create(&write_config(&dir, "")).unwrap();
        assert_eq!(config.clipboard.timeout_seconds, 45);

        // Negative value: falls back to the default instead of erroring.
        let config =
            Config::load_or_create(&write_config(&dir, "[clipboard]\ntimeout_seconds = -5\n"))
                .unwrap();
        assert_eq!(config.clipboard.timeout_seconds, 45);

        // Zero is kept: it disables auto-clear.
        let config =
            Config::load_or_create(&write_config(&dir, "[clipboard]\ntimeout_seconds = 0\n"))
                .unwrap();
        assert_eq!(config.clipboard.timeout_seconds, 0);

        // Above the maximum: clamped to 3600.
        let config = Config::load_or_create(&write_config(
            &dir,
            "[clipboard]\ntimeout_seconds = 99999\n",
        ))
        .unwrap();
        assert_eq!(config.clipboard.timeout_seconds, 3600);
    }

    #[test]
    fn test_clipboard_timeout_default() {
        let dir = TempDir::new().unwrap();
        // No [clipboard] section at all: the serde default of 45 applies.
        let config = Config::load_or_create(&write_config(&dir, "")).unwrap();
        assert_eq!(config.clipboard.timeout_seconds, 45);
    }

    #[test]
    fn test_clipboard_timeout_valid() {
        let dir = TempDir::new().unwrap();
        let config =
            Config::load_or_create(&write_config(&dir, "[clipboard]\ntimeout_seconds = 30\n"))
                .unwrap();
        assert_eq!(config.clipboard.timeout_seconds, 30);
    }

    #[test]
    fn test_clipboard_timeout_zero() {
        let dir = TempDir::new().unwrap();
        // Zero is valid and disables auto-clear.
        let config =
            Config::load_or_create(&write_config(&dir, "[clipboard]\ntimeout_seconds = 0\n"))
                .unwrap();
        assert_eq!(config.clipboard.timeout_seconds, 0);
    }

    #[test]
    fn test_clipboard_timeout_negative_falls_back() {
        let dir = TempDir::new().unwrap();
        // Negative values are invalid for u64; they fall back to the
        // default instead of failing the parse.
        let config =
            Config::load_or_create(&write_config(&dir, "[clipboard]\ntimeout_seconds = -1\n"))
                .unwrap();
        assert_eq!(config.clipboard.timeout_seconds, 45);
    }

    #[test]
    fn test_clipboard_timeout_clamped() {
        let dir = TempDir::new().unwrap();
        // Values above the 3600 maximum are clamped.
        let config = Config::load_or_create(&write_config(
            &dir,
            "[clipboard]\ntimeout_seconds = 99999\n",
        ))
        .unwrap();
        assert_eq!(config.clipboard.timeout_seconds, 3600);
    }

    #[test]
    fn test_transparent_background() {
        let dir = TempDir::new().unwrap();
        let content = r#"
[colors]
theme = "custom"
background = "transparent"
"#;
        let config = Config::load_or_create(&write_config(&dir, content)).unwrap();

        // The value passes through the theme verbatim and resolves to
        // ratatui's Reset color, as do its aliases.
        let theme = config.effective_theme();
        assert_eq!(theme.background, "transparent");
        assert_eq!(config.parse_color(&theme.background), Color::Reset);
        assert_eq!(config.parse_color("default"), Color::Reset);
        assert_eq!(config.parse_color("reset"), Color::Reset);

        // Hex and named colors keep working alongside the new aliases.
        assert_eq!(config.parse_color("#FF0000"), Color::Rgb(255, 0, 0));
        assert_eq!(config.parse_color("cyan"), Color::Cyan);
    }

    #[test]
    fn test_git_section_without_proxy_still_parses() {
        let dir = TempDir::new().unwrap();
        // A config written by an older mujo version may still contain a
        // proxy key in [git]. It must be ignored (not an error), and the
        // remaining git settings must parse as before.
        let content = r#"
[git]
remote_url = "git@github.com:user/repo.git"
proxy = "socks5h://127.0.0.1:9050"
"#;
        let config = Config::load_or_create(&write_config(&dir, content)).unwrap();

        assert_eq!(config.git.remote_url, "git@github.com:user/repo.git");
        assert_eq!(
            config.git.remote_opt(),
            Some("git@github.com:user/repo.git")
        );
    }

    #[test]
    fn test_transparent_default_agrees_between_paths() {
        // The serde attribute default and the impl Default must agree: an
        // empty config file (built from Default) and a config containing a
        // bare [colors] table (built by serde) must both resolve
        // transparent to false.
        let dir_empty = TempDir::new().unwrap();
        let empty = Config::load_or_create(&write_config(&dir_empty, "")).unwrap();
        let dir_bare = TempDir::new().unwrap();
        let bare = Config::load_or_create(&write_config(&dir_bare, "[colors]\n")).unwrap();

        assert!(!empty.colors.transparent);
        assert!(!bare.colors.transparent);
    }

    /// Serialize tests that mutate process-wide environment variables:
    /// env changes are visible to every thread in the process. Delegates
    /// to testenv::lock so config tests exclude pass/handlers and each
    /// other on the SAME process-global lock. The lock is non-reentrant:
    /// never hold two guards at once.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::testenv::lock()
    }

    /// Take the env lock, snapshot the variables the config lookup chain
    /// reads, and clear them; restores the snapshot on drop.
    struct EnvGuard {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn take() -> Self {
            let lock = env_lock();
            let mut saved = Vec::new();
            for key in ["MUJO_CONFIG", "XDG_CONFIG_HOME", "XDG_CONFIG_DIRS", "HOME"] {
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
    fn test_xdg_config_home_selects_user_path() {
        let _env = EnvGuard::take();
        let dir = TempDir::new().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path());
        // Keep a real ~/.config/mujo out of the picture.
        std::env::set_var("HOME", dir.path());

        // With no config file present anywhere in the chain, the user path
        // under XDG_CONFIG_HOME is the fallback.
        assert_eq!(
            Config::config_path().unwrap(),
            dir.path().join("mujo").join("config.toml")
        );
    }

    #[test]
    fn test_mujo_config_used_verbatim() {
        let _env = EnvGuard::take();
        let dir = TempDir::new().unwrap();
        // Deliberately a path whose file does not exist: it must still be
        // returned verbatim so a typo is visible rather than silently
        // ignored.
        let missing = dir.path().join("custom").join("config.toml");
        std::env::set_var("MUJO_CONFIG", &missing);

        assert_eq!(Config::config_path().unwrap(), missing);

        // A missing MUJO_CONFIG file is treated as "the config path to use
        // and to create": defaults are returned and the file is created at
        // that path.
        let config = Config::load().unwrap();
        assert_eq!(config.password_generation.default_length, default_length());
        assert_eq!(config.colors.theme, "lime");
        assert!(missing.exists(), "load must create the MUJO_CONFIG file");
    }

    #[test]
    fn test_save_writes_to_mujo_config_path() {
        let _env = EnvGuard::take();
        let dir = TempDir::new().unwrap();
        std::env::set_var("HOME", dir.path());

        let override_path = dir.path().join("override").join("config.toml");
        std::env::set_var("MUJO_CONFIG", &override_path);

        Config::default().save().unwrap();

        assert!(
            override_path.exists(),
            "save must write the MUJO_CONFIG path"
        );
        let user_path = dir.path().join(".config").join("mujo").join("config.toml");
        assert!(
            !user_path.exists(),
            "save must not touch ~/.config when MUJO_CONFIG is set"
        );
    }

    #[test]
    fn test_fallback_to_home_config_path() {
        let _env = EnvGuard::take();
        let dir = TempDir::new().unwrap();
        // Neither override set; point HOME at a fresh temp dir so the
        // fallback is deterministic. Assumes no system config exists at
        // /usr/local/etc/mujo, /etc/xdg/mujo or /etc/mujo.
        std::env::set_var("HOME", dir.path());

        assert_eq!(
            Config::config_path().unwrap(),
            dir.path().join(".config").join("mujo").join("config.toml")
        );
    }

    #[test]
    fn test_candidate_order() {
        let _env = EnvGuard::take();
        let dir = TempDir::new().unwrap();
        std::env::set_var("HOME", dir.path());

        // Nothing but HOME set: user path, then the system chain with the
        // XDG_CONFIG_DIRS default of /etc/xdg.
        assert_eq!(
            Config::config_candidates().unwrap(),
            vec![
                dir.path().join(".config").join("mujo").join("config.toml"),
                PathBuf::from("/usr/local/etc/mujo/config.toml"),
                PathBuf::from("/etc/xdg/mujo/config.toml"),
                PathBuf::from("/etc/mujo/config.toml"),
            ]
        );

        // XDG_CONFIG_HOME set: the XDG user path replaces the home path.
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("xdg"));
        assert_eq!(
            Config::config_candidates().unwrap(),
            vec![
                dir.path().join("xdg").join("mujo").join("config.toml"),
                PathBuf::from("/usr/local/etc/mujo/config.toml"),
                PathBuf::from("/etc/xdg/mujo/config.toml"),
                PathBuf::from("/etc/mujo/config.toml"),
            ]
        );

        // XDG_CONFIG_DIRS entries follow /usr/local/etc, in order.
        std::env::set_var("XDG_CONFIG_DIRS", "/opt/etc:/srv/etc");
        assert_eq!(
            Config::config_candidates().unwrap(),
            vec![
                dir.path().join("xdg").join("mujo").join("config.toml"),
                PathBuf::from("/usr/local/etc/mujo/config.toml"),
                PathBuf::from("/opt/etc/mujo/config.toml"),
                PathBuf::from("/srv/etc/mujo/config.toml"),
                PathBuf::from("/etc/mujo/config.toml"),
            ]
        );

        // MUJO_CONFIG set: it alone is the candidate list.
        let mujo = dir.path().join("override").join("config.toml");
        std::env::set_var("MUJO_CONFIG", &mujo);
        assert_eq!(Config::config_candidates().unwrap(), vec![mujo]);
    }

    #[test]
    fn test_xdg_config_dirs_system_path_selected() {
        let _env = EnvGuard::take();
        let dir = TempDir::new().unwrap();
        std::env::set_var("HOME", dir.path());

        // A config file exists under an XDG_CONFIG_DIRS entry; no user
        // config exists anywhere.
        let sys_root = TempDir::new().unwrap();
        let sys_config = sys_root.path().join("mujo").join("config.toml");
        std::fs::create_dir_all(sys_config.parent().unwrap()).unwrap();
        std::fs::write(&sys_config, "").unwrap();
        std::env::set_var("XDG_CONFIG_DIRS", sys_root.path());

        assert_eq!(Config::config_path().unwrap(), sys_config);
    }

    #[test]
    fn test_save_never_targets_system_path() {
        let _env = EnvGuard::take();
        let home = TempDir::new().unwrap();
        std::env::set_var("HOME", home.path());

        // Only a system config exists; save() must write a NEW user config
        // and leave the system file byte-identical.
        let sys_root = TempDir::new().unwrap();
        let sys_config = sys_root.path().join("mujo").join("config.toml");
        std::fs::create_dir_all(sys_config.parent().unwrap()).unwrap();
        std::fs::write(&sys_config, "system content").unwrap();
        std::env::set_var("XDG_CONFIG_DIRS", sys_root.path());

        // Precondition: reading resolves to the system file.
        assert_eq!(Config::config_path().unwrap(), sys_config);

        Config::default().save().unwrap();

        let user_path = home.path().join(".config").join("mujo").join("config.toml");
        assert!(user_path.exists(), "save must write the user config path");
        assert_eq!(
            std::fs::read(&sys_config).unwrap(),
            b"system content",
            "the system config must not be modified by save()"
        );
    }
}

//! Platform-aware keyboard layer.
//!
//! Every chord stays LIVE on every platform: [`accepts`] matches bindings
//! while IGNORING the platform, so Ctrl+Up still jumps on a Mac even
//! though it is not advertised there. The platform only decides which
//! chord is ADVERTISED in the help body and status bar ([`advertised`]).
//! This avoids dead keys and keeps key matching platform-independent.
//!
//! Why some bindings are Linux-only in the table: macOS captures
//! Ctrl+arrow for Mission Control and Spaces, and Home/End/PgUp/PgDn have
//! no key on a Mac keyboard and need Fn. Those chords still match
//! everywhere - only their advertising is gated.
//!
//! The $LC_MUJO_KEYBOARD alias of $MUJO_KEYBOARD exists for SSH sessions:
//! ssh only forwards variables matching its SendEnv list, which defaults
//! to "LANG LC_*" on both client and server. Naming the override with an
//! LC_ prefix means a Mac user SSHing into a Linux box can advertise Mac
//! chords with no server-side sshd_config change.

use crossterm::event::{KeyCode, KeyModifiers};

/// The platforms whose keyboards differ in what mujo advertises.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Platform {
    Mac,
    Linux,
}

/// The chord groups this layer manages.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// Tree: jump 5 siblings; editor: generated password length.
    JumpUp,
    JumpDown,
    /// Editor: start/end of the current line.
    LineStart,
    LineEnd,
    /// Preview, git log and help paging.
    PageUp,
    PageDown,
    /// Git log and help: jump to the top/bottom.
    Top,
    Bottom,
    /// Open the full-screen help view from any mode.
    Help,
    /// Editor: delete the character after the cursor.
    DeleteForward,
}

/// One chord: what it does, how the key event is matched, how it is
/// advertised and on which platforms it is advertised.
struct Binding {
    action: Action,
    code: KeyCode,
    mods: KeyModifiers,
    label: &'static str,
    platforms: &'static [Platform],
}

const MAC_LINUX: &[Platform] = &[Platform::Mac, Platform::Linux];
const LINUX_ONLY: &[Platform] = &[Platform::Linux];

/// One entry per chord. Order matters only for `advertised`, which returns
/// the first binding whose platforms contain the active platform.
static BINDINGS: &[Binding] = &[
    Binding {
        action: Action::JumpUp,
        code: KeyCode::Up,
        mods: KeyModifiers::CONTROL,
        label: "Ctrl+\u{2191}",
        platforms: LINUX_ONLY,
    },
    Binding {
        action: Action::JumpUp,
        code: KeyCode::Char('u'),
        mods: KeyModifiers::CONTROL,
        label: "Ctrl+U",
        platforms: MAC_LINUX,
    },
    Binding {
        action: Action::JumpDown,
        code: KeyCode::Down,
        mods: KeyModifiers::CONTROL,
        label: "Ctrl+\u{2193}",
        platforms: LINUX_ONLY,
    },
    Binding {
        action: Action::JumpDown,
        code: KeyCode::Char('d'),
        mods: KeyModifiers::CONTROL,
        label: "Ctrl+D",
        platforms: MAC_LINUX,
    },
    Binding {
        action: Action::LineStart,
        code: KeyCode::Home,
        mods: KeyModifiers::NONE,
        label: "Home",
        platforms: LINUX_ONLY,
    },
    Binding {
        action: Action::LineStart,
        code: KeyCode::Char('a'),
        mods: KeyModifiers::CONTROL,
        label: "Ctrl+A",
        platforms: MAC_LINUX,
    },
    Binding {
        action: Action::LineEnd,
        code: KeyCode::End,
        mods: KeyModifiers::NONE,
        label: "End",
        platforms: LINUX_ONLY,
    },
    Binding {
        action: Action::LineEnd,
        code: KeyCode::Char('e'),
        mods: KeyModifiers::CONTROL,
        label: "Ctrl+E",
        platforms: MAC_LINUX,
    },
    Binding {
        action: Action::PageUp,
        code: KeyCode::PageUp,
        mods: KeyModifiers::NONE,
        label: "PgUp",
        platforms: LINUX_ONLY,
    },
    Binding {
        action: Action::PageUp,
        code: KeyCode::Char('b'),
        mods: KeyModifiers::NONE,
        label: "b",
        platforms: MAC_LINUX,
    },
    Binding {
        action: Action::PageDown,
        code: KeyCode::PageDown,
        mods: KeyModifiers::NONE,
        label: "PgDn",
        platforms: LINUX_ONLY,
    },
    Binding {
        action: Action::PageDown,
        code: KeyCode::Char(' '),
        mods: KeyModifiers::NONE,
        label: "Space",
        platforms: MAC_LINUX,
    },
    Binding {
        action: Action::Top,
        code: KeyCode::Home,
        mods: KeyModifiers::NONE,
        label: "Home",
        platforms: LINUX_ONLY,
    },
    Binding {
        action: Action::Top,
        code: KeyCode::Char('g'),
        mods: KeyModifiers::NONE,
        label: "g",
        platforms: MAC_LINUX,
    },
    Binding {
        action: Action::Bottom,
        code: KeyCode::End,
        mods: KeyModifiers::NONE,
        label: "End",
        platforms: LINUX_ONLY,
    },
    Binding {
        action: Action::Bottom,
        code: KeyCode::Char('G'),
        mods: KeyModifiers::NONE,
        label: "G",
        platforms: MAC_LINUX,
    },
    // Help: Ctrl+/ everywhere. Ctrl+/ transmits 0x1F, which crossterm
    // reports as Char('7') with CONTROL (0x1F is also Ctrl+7, and
    // crossterm names control bytes 0x1C-0x1F by their Ctrl+digit chords)
    // - the code must be Char('7') while the label advertises the key the
    // user actually presses. F1 matches via an EMPTY platforms slice:
    // always live, never advertised (on a Mac keyboard F1 needs Fn, which
    // undercuts it as the universal help key).
    Binding {
        action: Action::Help,
        code: KeyCode::Char('7'),
        mods: KeyModifiers::CONTROL,
        label: "Ctrl+/",
        platforms: MAC_LINUX,
    },
    // Some terminals/crossterm versions report 0x1F as Char('_') with
    // CONTROL instead; keep that spelling live too, unadvertised.
    Binding {
        action: Action::Help,
        code: KeyCode::Char('_'),
        mods: KeyModifiers::CONTROL,
        label: "Ctrl+/",
        platforms: &[],
    },
    Binding {
        action: Action::Help,
        code: KeyCode::F(1),
        mods: KeyModifiers::NONE,
        label: "F1",
        platforms: &[],
    },
    // DeleteForward: the same keycode on both platforms, two entries with
    // different labels so the Mac advertisement reads "Fn+Backspace"
    // (Delete is Fn+Backspace there) while the chord itself is unchanged.
    Binding {
        action: Action::DeleteForward,
        code: KeyCode::Delete,
        mods: KeyModifiers::NONE,
        label: "Delete",
        platforms: LINUX_ONLY,
    },
    Binding {
        action: Action::DeleteForward,
        code: KeyCode::Delete,
        mods: KeyModifiers::NONE,
        label: "Fn+Backspace",
        platforms: MAC_LINUX,
    },
];

static ACTIVE_KEYBOARD: std::sync::OnceLock<Platform> = std::sync::OnceLock::new();

/// Resolve the keyboard platform from a mode string ("mac", "linux", or
/// "auto"/anything unknown). "auto" follows the platform mujo was compiled
/// for.
pub fn resolve_keyboard(mode: &str) -> Platform {
    match mode {
        "mac" => Platform::Mac,
        "linux" => Platform::Linux,
        // "auto" and anything unknown.
        _ => {
            if cfg!(target_os = "macos") {
                Platform::Mac
            } else {
                Platform::Linux
            }
        }
    }
}

/// Resolve the effective keyboard mode: the --keyboard CLI flag wins, then
/// $MUJO_KEYBOARD, then $LC_MUJO_KEYBOARD, then the config value. Empty
/// values are treated as unset at every level.
///
/// As with glyphs and truecolor, the LC_-prefixed alias rides ssh's
/// default SendEnv ("LANG LC_*") with no server-side sshd_config change,
/// so a Mac user SSHing into a Linux box can advertise Mac chords by
/// pushing $LC_MUJO_KEYBOARD=mac.
pub fn effective_keyboard_mode(flag: Option<&str>, config: &str) -> String {
    crate::ui::pick_mode(flag, "MUJO_KEYBOARD", "LC_MUJO_KEYBOARD", config)
}

pub fn set_keyboard(p: Platform) {
    let _ = ACTIVE_KEYBOARD.set(p);
}

/// The active platform, defaulting to resolve_keyboard("auto") when
/// set_keyboard has not been called yet.
pub fn keyboard() -> Platform {
    *ACTIVE_KEYBOARD.get().unwrap_or(&resolve_keyboard("auto"))
}

/// True when any binding for the action matches the key event, IGNORING
/// the platform: every chord stays live everywhere, advertised or not.
///
/// A binding with no modifiers also matches a SHIFT-only event: crossterm
/// reports Shift+G as Char('G') with SHIFT, and the chord must still work.
pub fn accepts(action: Action, code: KeyCode, mods: KeyModifiers) -> bool {
    BINDINGS.iter().any(|b| {
        b.action == action
            && b.code == code
            && (b.mods == mods || (b.mods | KeyModifiers::SHIFT) == mods)
    })
}

/// The advertised label for the action on the active platform: the first
/// binding whose platforms contain it. Chords without a binding for the
/// platform still work (see accepts) - they are just not advertised.
pub fn advertised(action: Action) -> Option<&'static str> {
    advertised_for(action, keyboard())
}

/// advertised keyed on an explicit platform, so tests and the help body
/// (ui::help_entries_for) can exercise platforms other than the active
/// one (ACTIVE_KEYBOARD is a OnceLock and can only be set once per
/// process).
pub(crate) fn advertised_for(action: Action, platform: Platform) -> Option<&'static str> {
    BINDINGS
        .iter()
        .find(|b| b.action == action && b.platforms.contains(&platform))
        .map(|b| b.label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_keyboard_modes() {
        assert_eq!(resolve_keyboard("mac"), Platform::Mac);
        assert_eq!(resolve_keyboard("linux"), Platform::Linux);
        // "auto" and anything unknown follow the compilation target.
        let expected = if cfg!(target_os = "macos") {
            Platform::Mac
        } else {
            Platform::Linux
        };
        assert_eq!(resolve_keyboard("auto"), expected);
        assert_eq!(resolve_keyboard("nonsense"), expected);
    }

    /// Serialize env mutation across tests: env vars are process-global,
    /// so parallel test threads would otherwise see each other's values.
    /// Delegates to testenv::lock so keys tests exclude pass/handlers and
    /// each other on the SAME process-global lock. The lock is
    /// non-reentrant: never hold two guards at once.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::testenv::lock()
    }

    /// Take the env lock, snapshot the variables effective_keyboard_mode
    /// reads, and clear them; restores the snapshot on drop.
    struct EnvGuard {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn take() -> Self {
            let lock = env_lock();
            let mut saved = Vec::new();
            for key in ["MUJO_KEYBOARD", "LC_MUJO_KEYBOARD"] {
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
    fn test_effective_keyboard_mode_flag_beats_env() {
        let _env = EnvGuard::take();
        std::env::set_var("MUJO_KEYBOARD", "mac");
        std::env::set_var("LC_MUJO_KEYBOARD", "linux");
        assert_eq!(effective_keyboard_mode(Some("linux"), "mac"), "linux");
    }

    #[test]
    fn test_effective_keyboard_mode_mujo_beats_lc_mujo() {
        let _env = EnvGuard::take();
        std::env::set_var("MUJO_KEYBOARD", "mac");
        std::env::set_var("LC_MUJO_KEYBOARD", "linux");
        assert_eq!(effective_keyboard_mode(None, "linux"), "mac");
    }

    #[test]
    fn test_effective_keyboard_mode_lc_mujo_beats_config() {
        let _env = EnvGuard::take();
        std::env::set_var("LC_MUJO_KEYBOARD", "mac");
        assert_eq!(effective_keyboard_mode(None, "linux"), "mac");
    }

    #[test]
    fn test_effective_keyboard_mode_config_used_when_none_set() {
        let _env = EnvGuard::take();
        assert_eq!(effective_keyboard_mode(None, "mac"), "mac");
    }

    #[test]
    fn test_accepts_is_platform_independent() {
        // Every chord matches everywhere, regardless of the active
        // platform: Ctrl+Up (advertised on Linux only) and Ctrl+U
        // (advertised on Mac too) both jump, on any platform.
        assert!(accepts(Action::JumpUp, KeyCode::Up, KeyModifiers::CONTROL));
        assert!(accepts(
            Action::JumpUp,
            KeyCode::Char('u'),
            KeyModifiers::CONTROL
        ));
        assert!(accepts(
            Action::JumpDown,
            KeyCode::Down,
            KeyModifiers::CONTROL
        ));
        assert!(accepts(
            Action::JumpDown,
            KeyCode::Char('d'),
            KeyModifiers::CONTROL
        ));
        assert!(accepts(
            Action::LineStart,
            KeyCode::Home,
            KeyModifiers::NONE
        ));
        assert!(accepts(
            Action::LineStart,
            KeyCode::Char('a'),
            KeyModifiers::CONTROL
        ));
        assert!(accepts(Action::LineEnd, KeyCode::End, KeyModifiers::NONE));
        assert!(accepts(
            Action::LineEnd,
            KeyCode::Char('e'),
            KeyModifiers::CONTROL
        ));
        assert!(accepts(Action::PageUp, KeyCode::PageUp, KeyModifiers::NONE));
        assert!(accepts(
            Action::PageUp,
            KeyCode::Char('b'),
            KeyModifiers::NONE
        ));
        assert!(accepts(
            Action::PageDown,
            KeyCode::PageDown,
            KeyModifiers::NONE
        ));
        assert!(accepts(
            Action::PageDown,
            KeyCode::Char(' '),
            KeyModifiers::NONE
        ));
        assert!(accepts(Action::Top, KeyCode::Home, KeyModifiers::NONE));
        assert!(accepts(Action::Top, KeyCode::Char('g'), KeyModifiers::NONE));
        assert!(accepts(Action::Bottom, KeyCode::End, KeyModifiers::NONE));
        assert!(accepts(
            Action::Bottom,
            KeyCode::Char('G'),
            KeyModifiers::NONE
        ));

        // crossterm reports Shift+G as Char('G') with SHIFT; unmodified
        // bindings also match SHIFT-only events so the chord still works.
        assert!(accepts(
            Action::Bottom,
            KeyCode::Char('G'),
            KeyModifiers::SHIFT
        ));

        // Wrong modifiers do not match: a bare Up is tree navigation, not
        // JumpUp, and an unbound letter is never PageDown.
        assert!(!accepts(Action::JumpUp, KeyCode::Up, KeyModifiers::NONE));
        assert!(!accepts(
            Action::JumpUp,
            KeyCode::Char('u'),
            KeyModifiers::NONE
        ));
        assert!(!accepts(
            Action::PageDown,
            KeyCode::Char('c'),
            KeyModifiers::NONE
        ));
    }

    #[test]
    fn test_advertised_is_platform_filtered() {
        // Ctrl+Up is advertised on Linux only; Mac users are told Ctrl+U.
        assert_eq!(
            advertised_for(Action::JumpUp, Platform::Linux),
            Some("Ctrl+\u{2191}")
        );
        assert_eq!(
            advertised_for(Action::JumpUp, Platform::Mac),
            Some("Ctrl+U")
        );
        assert_eq!(
            advertised_for(Action::JumpDown, Platform::Mac),
            Some("Ctrl+D")
        );
        assert_eq!(
            advertised_for(Action::LineStart, Platform::Mac),
            Some("Ctrl+A")
        );
        assert_eq!(
            advertised_for(Action::LineEnd, Platform::Linux),
            Some("End")
        );
        assert_eq!(advertised_for(Action::PageUp, Platform::Mac), Some("b"));
        assert_eq!(
            advertised_for(Action::PageDown, Platform::Linux),
            Some("PgDn")
        );
        assert_eq!(advertised_for(Action::Top, Platform::Mac), Some("g"));
        assert_eq!(advertised_for(Action::Bottom, Platform::Linux), Some("End"));
    }
}

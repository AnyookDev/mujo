use anyhow::{bail, Result};
use rand::seq::SliceRandom;

/// Resolve the character set for a named charset option. Supported names:
/// "letters", "alphanumeric", "numbers", "symbols", "custom" (uses
/// `custom_charset`) and "all" (the default). Ambiguous characters
/// (0, O, 1, l, I) are always removed, and an empty result falls back to
/// a safe lowercase set.
fn charset_for(charset: &str, custom_charset: &str) -> String {
    let mut chars = match charset {
        "letters" => "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ".to_string(),
        "alphanumeric" => "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789".to_string(),
        "numbers" => "0123456789".to_string(),
        "symbols" => "!@#$%^&*()_+-=[]{}|;:,.<>?".to_string(),
        "custom" => {
            if custom_charset.trim().is_empty() {
                "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789".to_string()
            } else {
                custom_charset.to_string()
            }
        }
        // "all" and anything unrecognized.
        _ => "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!@#$%^&*()_+-=[]{}|;:,.<>?".to_string(),
    };

    // Remove characters that are easy to misread.
    chars = chars.replace(['0', 'O', '1', 'l', 'I'], "");

    if chars.is_empty() {
        chars = "abcdefghjkmnpqrstuvwxyz".to_string();
    }
    chars
}

/// Generate a random password of the requested length from the named
/// charset. Pure function: no filesystem, no subprocesses, no global
/// state - safe to call from tests.
pub fn generate_password(length: usize, charset: &str, custom_charset: &str) -> String {
    let chars: Vec<char> = charset_for(charset, custom_charset).chars().collect();
    let mut rng = rand::thread_rng();
    (0..length)
        .map(|_| *chars.choose(&mut rng).unwrap())
        .collect()
}

/// Qualitative password strength.
// Currently only exercised by the unit tests; remove the allow attribute
// once the strength indicator is wired into the UI.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strength {
    Weak,
    Medium,
    Strong,
}

#[allow(dead_code)]
impl Strength {
    pub fn as_str(&self) -> &'static str {
        match self {
            Strength::Weak => "weak",
            Strength::Medium => "medium",
            Strength::Strong => "strong",
        }
    }
}

/// Heuristic strength check based on length and character-class variety:
/// - weak:   fewer than 8 characters, or only one character class
/// - strong: at least 16 characters and at least 3 character classes
/// - medium: everything else
// Currently only exercised by the unit tests; remove the allow attribute
// once the strength indicator is wired into the UI.
#[allow(dead_code)]
pub fn password_strength(password: &str) -> Strength {
    let len = password.chars().count();
    let has_lower = password.chars().any(|c| c.is_ascii_lowercase());
    let has_upper = password.chars().any(|c| c.is_ascii_uppercase());
    let has_digit = password.chars().any(|c| c.is_ascii_digit());
    let has_symbol = password.chars().any(|c| !c.is_ascii_alphanumeric());
    let classes = [has_lower, has_upper, has_digit, has_symbol]
        .iter()
        .filter(|b| **b)
        .count();

    if len < 8 || classes <= 1 {
        Strength::Weak
    } else if len >= 16 && classes >= 3 {
        Strength::Strong
    } else {
        Strength::Medium
    }
}

/// Validate an entry path for create/move operations. Valid paths are
/// non-empty, relative, and made of non-empty segments without any `.` or
/// `..` traversal. Examples:
/// - "work/github"  -> ok (nested entry)
/// - "/work"        -> err (leading slash)
/// - "work/"        -> err (trailing slash)
/// - "a//b"         -> err (empty segment)
/// - "a/../b"       -> err (traversal)
// Currently only exercised by the unit tests; remove the allow attribute
// once path validation is wired into save/move flows.
#[allow(dead_code)]
pub fn validate_entry_path(path: &str) -> Result<()> {
    let p = path.trim();
    if p.is_empty() {
        bail!("Path cannot be empty");
    }
    if p.starts_with('/') {
        bail!("Path cannot start with '/'");
    }
    if p.ends_with('/') {
        bail!("Path cannot end with '/'");
    }
    if p.contains("//") {
        bail!("Path cannot contain empty segments");
    }
    if p.split('/').any(|seg| seg == "." || seg == "..") {
        bail!("Path cannot contain '.' or '..' segments");
    }
    if p.chars().any(|c| c.is_control()) {
        bail!("Path cannot contain control characters");
    }
    Ok(())
}

/// Strip newlines and other control characters from a password. `pass`
/// stores the first line as the password, so embedded newlines would
/// silently truncate or corrupt entries.
// Currently only exercised by the unit tests; remove the allow attribute
// once sanitization is wired into the decrypt flow.
#[allow(dead_code)]
pub fn sanitize_password(password: &str) -> String {
    password.chars().filter(|c| !c.is_control()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_password_length() {
        for length in [8, 20, 50] {
            let pwd = generate_password(length, "all", "");
            assert_eq!(
                pwd.chars().count(),
                length,
                "password should have exactly {} characters",
                length
            );
        }
    }

    #[test]
    fn test_generate_password_characters() {
        // A custom charset with exactly one representative of each class.
        // Note: '1' would be stripped as ambiguous, so '4' stands in for
        // the digit class. With 100 draws from 4 characters, the chance of
        // any class being missing is ~(3/4)^100, i.e. negligible.
        let pwd = generate_password(100, "custom", "aA4!");
        assert!(pwd.contains('a'), "should contain lowercase: {}", pwd);
        assert!(pwd.contains('A'), "should contain uppercase: {}", pwd);
        assert!(pwd.contains('4'), "should contain a digit: {}", pwd);
        assert!(pwd.contains('!'), "should contain a symbol: {}", pwd);

        // Every generated character must come from the resolved "all"
        // charset. Deriving the allowed set from charset_for() (rather than
        // hand-writing a second copy) keeps this test in sync with the
        // generator, including symbols like '|' that are easy to forget.
        let allowed: Vec<char> = charset_for("all", "").chars().collect();
        assert!(!allowed.is_empty());
        let pwd = generate_password(100, "all", "");
        for c in pwd.chars() {
            assert!(
                allowed.contains(&c),
                "character {:?} is not in the allowed charset",
                c
            );
        }
    }

    #[test]
    fn test_generate_password_randomness() {
        let passwords: Vec<String> = (0..10).map(|_| generate_password(24, "all", "")).collect();

        // All 10 passwords must differ from each other. With a ~80-character
        // alphabet and 24-character length, a collision is astronomically
        // unlikely; identical output would indicate a deterministic (broken)
        // generator.
        for i in 0..passwords.len() {
            for j in (i + 1)..passwords.len() {
                assert_ne!(
                    passwords[i], passwords[j],
                    "two generated passwords were identical - generator is not random"
                );
            }
        }
    }

    #[test]
    fn test_generate_password_numbers_and_symbols() {
        // Numbers: digits only, and never the ambiguous 0/1.
        let pwd = generate_password(50, "numbers", "");
        assert!(
            pwd.chars()
                .all(|c| c.is_ascii_digit() && c != '0' && c != '1'),
            "numbers charset produced unexpected characters: {}",
            pwd
        );

        // Symbols: only characters from the symbol set.
        let symbols = "!@#$%^&*()_+-=[]{}|;:,.<>?";
        let pwd = generate_password(50, "symbols", "");
        assert!(
            pwd.chars().all(|c| symbols.contains(c)),
            "symbols charset produced unexpected characters: {}",
            pwd
        );
    }

    #[test]
    fn test_password_strength_weak() {
        // Too short, even with mixed classes.
        assert_eq!(password_strength("aB1!").as_str(), "weak");
        // Long enough but a single character class.
        assert_eq!(password_strength("aaaaaaaa").as_str(), "weak");
        assert_eq!(password_strength("12345678").as_str(), "weak");
        // Empty input.
        assert_eq!(password_strength("").as_str(), "weak");
    }

    #[test]
    fn test_password_strength_strong() {
        // 21 chars, 4 classes.
        assert_eq!(
            password_strength("CorrectHorse9!Battery").as_str(),
            "strong"
        );
        // 16 chars, 3 classes (lower, upper, digit) - exactly at the boundary.
        assert_eq!(password_strength("abcdefGHIJKLMN45").as_str(), "strong");
    }

    #[test]
    fn test_password_strength_mixed() {
        // 9 chars, 3 classes: long enough to not be weak, too short for strong.
        assert_eq!(password_strength("abc123XYZ").as_str(), "medium");
        // 2 classes, 8+ chars.
        assert_eq!(password_strength("password1").as_str(), "medium");
        // 15 chars, 4 classes: one short of the strong length boundary.
        assert_eq!(password_strength("Abcdefgh1!xyz").as_str(), "medium");
    }

    #[test]
    fn test_validate_password_path() {
        // Valid: plain and nested paths.
        assert!(validate_entry_path("work/github").is_ok());
        assert!(validate_entry_path("single").is_ok());
        assert!(validate_entry_path("a/b/c/deep").is_ok());

        // Invalid: empty, leading/trailing slashes, empty segments, traversal.
        assert!(validate_entry_path("").is_err());
        assert!(validate_entry_path("   ").is_err());
        assert!(validate_entry_path("/work").is_err());
        assert!(validate_entry_path("work/").is_err());
        assert!(validate_entry_path("work//github").is_err());
        assert!(validate_entry_path("..").is_err());
        assert!(validate_entry_path("work/../github").is_err());
        assert!(validate_entry_path("./work").is_err());
    }

    #[test]
    fn test_sanitize_password() {
        // Newlines are stripped (pass treats the first line as the password).
        assert_eq!(sanitize_password("pass\nword"), "password");
        assert_eq!(sanitize_password("secret\n"), "secret");
        // Other control characters (tab, NUL) are removed too.
        assert_eq!(sanitize_password("ab\tc\u{0}d"), "abcd");
        // Printable characters, including symbols and spaces, are preserved.
        assert_eq!(sanitize_password("P@ss w0rd!#$"), "P@ss w0rd!#$");
        assert_eq!(sanitize_password(""), "");
    }

    #[test]
    fn test_charset_excludes_ambiguous_characters() {
        // The generated password must never contain the ambiguous
        // characters 0, O, 1, l, I regardless of charset.
        for charset in ["all", "alphanumeric", "letters", "numbers"] {
            let pwd = generate_password(200, charset, "");
            for ambiguous in ['0', 'O', '1', 'l', 'I'] {
                assert!(
                    !pwd.contains(ambiguous),
                    "charset {:?} produced ambiguous character {:?}",
                    charset,
                    ambiguous
                );
            }
        }
    }
}

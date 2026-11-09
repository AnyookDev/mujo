use std::path::PathBuf;

use super::Totp;

#[derive(Clone, Debug)]
pub struct PassEntry {
    pub path: PathBuf,
    pub name: String,
    pub display_name: String,
    pub is_dir: bool,
    pub depth: usize,
    pub expanded: bool,
}

impl PassEntry {
    pub fn new(path: PathBuf, _store_root: &std::path::Path, depth: usize) -> Self {
        let is_dir = path.is_dir();
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        let display_name = if is_dir {
            path.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string()
        } else {
            name.clone()
        };
        Self {
            path,
            name,
            display_name,
            is_dir,
            depth,
            expanded: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DecryptedContent {
    pub lines: Vec<String>,
    pub totp: Option<Totp>,
}

impl DecryptedContent {
    pub fn parse(content: &str) -> Self {
        let lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
        let mut totp = None;
        for line in &lines {
            if let Some((key, value)) = line.split_once(": ") {
                if key.to_lowercase() == "totp" || key.to_lowercase() == "otp" {
                    if let Some(t) = Totp::from_secret(value.trim()) {
                        totp = Some(t);
                        break;
                    }
                }
            }
        }
        Self { lines, totp }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_pass_entry_file_name_strips_gpg() {
        let path = PathBuf::from("/store/work/github.gpg");
        let entry = PassEntry::new(path, std::path::Path::new("/store"), 1);
        assert!(!entry.is_dir);
        assert_eq!(entry.name, "github");
        assert_eq!(entry.display_name, "github");
        assert!(!entry.expanded);
    }

    #[test]
    fn test_pass_entry_dir_display_name() {
        let dir = TempDir::new().unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir(&work).unwrap();

        let entry = PassEntry::new(work.clone(), dir.path(), 0);
        assert!(entry.is_dir);
        assert_eq!(entry.display_name, "work");
        assert!(!entry.expanded);
    }

    #[test]
    fn test_parse_splits_lines() {
        let content = DecryptedContent::parse("secret\nusername: bob\nurl: https://x.example");
        assert_eq!(
            content.lines,
            vec!["secret", "username: bob", "url: https://x.example"]
        );
        assert!(content.totp.is_none());
    }

    #[test]
    fn test_parse_detects_totp_key() {
        let content = DecryptedContent::parse("secret\ntotp: JBSWY3DPEHPK3PXP");
        assert!(content.totp.is_some(), "'totp:' line must yield a Totp");
    }

    #[test]
    fn test_parse_detects_otp_key_case_insensitive() {
        let content = DecryptedContent::parse("secret\nOTP: JBSWY3DPEHPK3PXP");
        assert!(
            content.totp.is_some(),
            "'OTP:' (any case) must yield a Totp"
        );
    }

    #[test]
    fn test_parse_ignores_non_totp_keys() {
        let content = DecryptedContent::parse("secret\nnote: totp-like text");
        assert!(content.totp.is_none());
    }

    #[test]
    fn test_parse_skips_invalid_totp_secret() {
        let content = DecryptedContent::parse("secret\ntotp: !!!");
        assert!(content.totp.is_none());
    }
}

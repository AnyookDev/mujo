use hmac::{Hmac, Mac};
use sha1::Sha1;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

type HmacSha1 = Hmac<Sha1>;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum AppMode {
    Normal,
    Search,
    Editor,
    Git,
    Help,
    ConfirmDelete,
    ConfirmSave,
    ConfirmDiscard,
    AddFolder,
    Move,
    ContextMenu,
    NewStore,
}

/// Stages of the first-run "New Password Store" setup pane, shown when no
/// store directory exists.
#[derive(Clone, Copy, PartialEq)]
pub enum NewStoreStage {
    /// Ask whether to set up a pass password store.
    Confirm,
    /// Ask for the GPG ID to initialize the store with.
    AskGpgId,
}

#[derive(Clone, Copy, PartialEq)]
pub enum GitPaneState {
    Uninitialized,
    AskRemote,
    AskIdentityName,
    AskIdentityEmail,
    Ready,
}

#[derive(Clone, Copy)]
pub enum ClipboardTool {
    WlCopy,
    Pbcopy,
    Xclip,
}

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

#[derive(Clone, Debug)]
pub struct Totp {
    secret: Vec<u8>,
    period: u64,
    digits: usize,
}

impl Totp {
    pub fn from_secret(secret_b32: &str) -> Option<Self> {
        let secret = base32::decode(
            base32::Alphabet::RFC4648 { padding: false },
            &secret_b32.to_uppercase().replace(" ", ""),
        )?;
        // base32::decode("") succeeds with an empty vec, but an empty
        // secret is not a valid TOTP key.
        if secret.is_empty() {
            return None;
        }
        Some(Self {
            secret,
            period: 30,
            digits: 6,
        })
    }

    pub fn generate(&self) -> String {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let counter = timestamp / self.period;
        self.generate_for_counter(counter)
    }

    fn generate_for_counter(&self, counter: u64) -> String {
        let mut mac =
            HmacSha1::new_from_slice(&self.secret).expect("HMAC can take key of any size");
        mac.update(&counter.to_be_bytes());
        let result = mac.finalize();
        let hash = result.into_bytes();
        let offset = (hash[hash.len() - 1] & 0x0f) as usize;
        let code = ((hash[offset] & 0x7f) as u32) << 24
            | (hash[offset + 1] as u32) << 16
            | (hash[offset + 2] as u32) << 8
            | hash[offset + 3] as u32;
        let code = code % 10_u32.pow(self.digits as u32);
        format!("{:0width$}", code, width = self.digits)
    }

    pub fn time_remaining(&self) -> u64 {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        self.period - (timestamp % self.period)
    }
}

#[derive(Clone)]
pub struct EntryEditor {
    pub path: String,
    pub original_path: String,
    pub content: String,
    pub original_content: String,
    pub cursor_line: usize,
    pub cursor_col: usize,
    pub is_new: bool,
    pub sel_start_line: usize,
    pub sel_start_col: usize,
    pub sel_end_line: usize,
    pub sel_end_col: usize,
    pub has_selection: bool,
    pub selecting: bool,
    pub content_scroll_offset: usize,
    pub path_h_scroll: usize,
    pub password_h_scroll: usize,
    pub content_h_scroll: usize,
    /// Settings for the in-editor password generator (the "Password
    /// Generation" section). Seeded from config when the editor opens and
    /// adjusted in place with Ctrl+Up/Down (length) and Tab (charset) while
    /// the Password field is active.
    pub gen_length: usize,
    pub gen_charset: String,
    pub gen_custom_charset: String,
}

impl EntryEditor {
    pub fn new(base_path: &str) -> Self {
        let path = if base_path.ends_with('/') {
            base_path.to_string()
        } else {
            format!("{}/", base_path)
        };
        let path_len = path.len();

        Self {
            path: path.clone(),
            original_path: path,
            content: "\n".to_string(),
            original_content: "\n".to_string(),
            cursor_line: 0,
            cursor_col: path_len,
            is_new: true,
            sel_start_line: 0,
            sel_start_col: 0,
            sel_end_line: 0,
            sel_end_col: 0,
            has_selection: false,
            selecting: false,
            content_scroll_offset: 0,
            path_h_scroll: 0,
            password_h_scroll: 0,
            content_h_scroll: 0,
            gen_length: 30,
            gen_charset: "all".to_string(),
            gen_custom_charset: String::new(),
        }
    }

    pub fn from_decrypted(path: &str, content: &str) -> Self {
        let password_len = content.lines().next().map(|l| l.len()).unwrap_or(0);
        Self {
            path: path.to_string(),
            original_path: path.to_string(),
            content: content.to_string(),
            original_content: content.to_string(),
            cursor_line: 1,
            cursor_col: password_len,
            is_new: false,
            sel_start_line: 0,
            sel_start_col: 0,
            sel_end_line: 0,
            sel_end_col: 0,
            has_selection: false,
            selecting: false,
            content_scroll_offset: 0,
            path_h_scroll: 0,
            password_h_scroll: 0,
            content_h_scroll: 0,
            gen_length: 30,
            gen_charset: "all".to_string(),
            gen_custom_charset: String::new(),
        }
    }

    pub fn to_content(&self) -> String {
        self.content.clone()
    }

    pub fn get_password(&self) -> Option<&str> {
        self.content.lines().next()
    }

    pub fn set_password(&mut self, password: &str) {
        let lines: Vec<&str> = self.content.lines().collect();
        let rest: Vec<&str> = lines.iter().skip(1).copied().collect();
        let mut new_content = password.to_string();
        if !rest.is_empty() {
            new_content.push('\n');
            new_content.push_str(&rest.join("\n"));
        }
        self.content = new_content;
        self.cursor_line = 1;
        self.cursor_col = password.len();
    }

    pub fn clear_selection(&mut self) {
        self.has_selection = false;
        self.selecting = false;
    }

    pub fn start_selection(&mut self) {
        self.sel_start_line = self.cursor_line;
        self.sel_start_col = self.cursor_col;
        self.sel_end_line = self.cursor_line;
        self.sel_end_col = self.cursor_col;
        self.has_selection = true;
        self.selecting = true;
    }

    pub fn update_selection(&mut self) {
        if self.selecting {
            self.sel_end_line = self.cursor_line;
            self.sel_end_col = self.cursor_col;
        }
    }

    pub fn get_selected_text(&self) -> Option<String> {
        if !self.has_selection {
            return None;
        }

        let (start_line, start_col, end_line, end_col) = if self.sel_start_line < self.sel_end_line
            || (self.sel_start_line == self.sel_end_line && self.sel_start_col <= self.sel_end_col)
        {
            (
                self.sel_start_line,
                self.sel_start_col,
                self.sel_end_line,
                self.sel_end_col,
            )
        } else {
            (
                self.sel_end_line,
                self.sel_end_col,
                self.sel_start_line,
                self.sel_start_col,
            )
        };

        let mut result = String::new();

        // Selections may only span content lines (editor lines 2+). The
        // path (line 0) and password (line 1) fields are not selectable:
        // including them would misalign the column mapping below, since
        // content lines are offset by 2.
        if start_line < 2 || end_line < 2 {
            return None;
        }

        let content_start = start_line - 2;
        let content_end = end_line - 2;
        let lines: Vec<&str> = self.content.lines().skip(1).collect();

        for (i, line) in lines
            .iter()
            .enumerate()
            .skip(content_start)
            .take(content_end - content_start + 1)
        {
            if i == content_start && i == content_end {
                let start = start_col.min(line.len());
                let end = end_col.min(line.len());
                if start < end {
                    result.push_str(&line[start..end]);
                }
            } else if i == content_start {
                let start = start_col.min(line.len());
                result.push_str(&line[start..]);
                result.push('\n');
            } else if i == content_end {
                let end = end_col.min(line.len());
                result.push_str(&line[..end]);
            } else {
                result.push_str(line);
                result.push('\n');
            }
        }

        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }

    pub fn is_in_selection(&self, line: usize, col: usize) -> bool {
        if !self.has_selection {
            return false;
        }

        let (start_line, start_col, end_line, end_col) = if self.sel_start_line < self.sel_end_line
            || (self.sel_start_line == self.sel_end_line && self.sel_start_col <= self.sel_end_col)
        {
            (
                self.sel_start_line,
                self.sel_start_col,
                self.sel_end_line,
                self.sel_end_col,
            )
        } else {
            (
                self.sel_end_line,
                self.sel_end_col,
                self.sel_start_line,
                self.sel_start_col,
            )
        };

        let pos = (line, col);
        pos >= (start_line, start_col) && pos < (end_line, end_col)
    }

    pub fn insert_char(&mut self, c: char) {
        if c == '\n' {
            self.insert_newline();
            return;
        }

        if self.cursor_line == 0 && !self.is_new {
            return;
        }

        self.clear_selection();

        if self.cursor_line == 0 {
            let mut path_chars: Vec<char> = self.path.chars().collect();
            if self.cursor_col > path_chars.len() {
                self.cursor_col = path_chars.len();
            }
            path_chars.insert(self.cursor_col, c);
            self.path = path_chars.into_iter().collect();
            self.cursor_col += 1;
        } else {
            let content_line = self.cursor_line - 1;
            let lines: Vec<String> = self.content.lines().map(|s| s.to_string()).collect();
            let mut new_lines = lines;

            while new_lines.len() <= content_line {
                new_lines.push(String::new());
            }

            let line = &mut new_lines[content_line];
            if self.cursor_col > line.len() {
                self.cursor_col = line.len();
            }

            line.insert(self.cursor_col, c);
            self.cursor_col += 1;
            self.content = new_lines.join("\n");
        }
    }

    fn insert_newline(&mut self) {
        if self.cursor_line == 0 {
            if self.is_new {
                self.cursor_line = 1;
                self.cursor_col = 0;
            }
            return;
        }

        self.clear_selection();

        let content_line = self.cursor_line - 1;
        let lines: Vec<String> = self.content.lines().map(|s| s.to_string()).collect();
        let mut new_lines = lines;

        while new_lines.len() <= content_line {
            new_lines.push(String::new());
        }

        let current_line = &mut new_lines[content_line];
        let after_cursor = current_line.split_off(self.cursor_col);

        self.cursor_line += 1;
        self.cursor_col = 0;
        new_lines.insert(content_line + 1, after_cursor);
        self.content = new_lines.join("\n");
    }

    pub fn backspace(&mut self) {
        if self.cursor_line == 0 && !self.is_new {
            return;
        }

        if self.has_selection {
            self.clear_selection();
            return;
        }

        if self.cursor_line == 0 {
            if self.cursor_col > 0 {
                let mut path_chars: Vec<char> = self.path.chars().collect();
                path_chars.remove(self.cursor_col - 1);
                self.path = path_chars.into_iter().collect();
                self.cursor_col -= 1;
            }
        } else {
            let content_line = self.cursor_line - 1;
            if self.cursor_col > 0 {
                let lines: Vec<String> = self.content.lines().map(|s| s.to_string()).collect();
                let mut new_lines = lines;

                while new_lines.len() <= content_line {
                    new_lines.push(String::new());
                }

                let line = &mut new_lines[content_line];
                if self.cursor_col <= line.len() {
                    line.remove(self.cursor_col - 1);
                    self.cursor_col -= 1;
                    self.content = new_lines.join("\n");
                }
            } else if self.cursor_line > 2 {
                // Join with the previous line only inside Additional
                // Content (cursor_line 3+). Backspace is local to the
                // current field: at the top line of the box (cursor_line
                // == 2) joining would merge into the Password field, and
                // from the Password field (cursor_line == 1) it must not
                // jump back into the Path field. Only Up/Down move
                // between fields.
                let lines: Vec<String> = self.content.lines().map(|s| s.to_string()).collect();
                let mut new_lines = lines;

                if content_line < new_lines.len() {
                    let current = new_lines.remove(content_line);
                    self.cursor_line -= 1;
                    let prev_line = content_line - 1;
                    self.cursor_col = new_lines[prev_line].len();
                    new_lines[prev_line].push_str(&current);
                    self.content = new_lines.join("\n");
                }
            }
            // cursor_line == 2 at col 0 (top of Additional Content) and
            // cursor_line == 1 at col 0 (start of Password): start of the
            // field - do nothing.
        }
    }

    pub fn delete(&mut self) {
        if self.cursor_line == 0 && !self.is_new {
            return;
        }

        if self.has_selection {
            self.clear_selection();
            return;
        }

        if self.cursor_line == 0 {
            let mut path_chars: Vec<char> = self.path.chars().collect();
            if self.cursor_col < path_chars.len() {
                path_chars.remove(self.cursor_col);
                self.path = path_chars.into_iter().collect();
            }
        } else {
            let content_line = self.cursor_line - 1;
            let lines: Vec<String> = self.content.lines().map(|s| s.to_string()).collect();
            let mut new_lines = lines;

            while new_lines.len() <= content_line {
                new_lines.push(String::new());
            }

            let line = &mut new_lines[content_line];
            if self.cursor_col < line.len() {
                line.remove(self.cursor_col);
                self.content = new_lines.join("\n");
            } else if self.cursor_line > 1 && content_line < new_lines.len() - 1 {
                // Join the next line only inside Additional Content:
                // Delete at the end of the Password field must not pull
                // the box's first line into the password.
                let next = new_lines.remove(content_line + 1);
                new_lines[content_line].push_str(&next);
                self.content = new_lines.join("\n");
            }
        }
    }

    pub fn move_cursor_left(&mut self) {
        if self.cursor_line == 0 {
            if self.cursor_col > 0 {
                self.cursor_col -= 1;
            }
        } else if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_line > 2 {
            // Wrap to the previous line only inside Additional Content.
            // Left at the start of the Password field stays put: Up/Down
            // are the only field navigation.
            self.cursor_line -= 1;
            let content_line = self.cursor_line - 1;
            let lines: Vec<&str> = self.content.lines().collect();
            self.cursor_col = lines.get(content_line).map(|l| l.len()).unwrap_or(0);
        }
    }

    pub fn move_cursor_right(&mut self) {
        if self.cursor_line == 0 {
            // Right at the end of the path stays put: Up/Down are the
            // only field navigation.
            if self.cursor_col < self.path.len() {
                self.cursor_col += 1;
            }
        } else {
            let content_line = self.cursor_line - 1;
            let lines: Vec<&str> = self.content.lines().collect();
            let current_len = lines.get(content_line).map(|l| l.len()).unwrap_or(0);

            if self.cursor_col < current_len {
                self.cursor_col += 1;
            } else if self.cursor_line > 1 {
                // Wrap to the next line only inside Additional Content:
                // Right at the end of the Password field stays put.
                let total_content_lines = self.content.lines().count();
                if content_line < total_content_lines {
                    self.cursor_line += 1;
                    self.cursor_col = 0;
                }
            }
        }
    }

    pub fn move_cursor_up(&mut self) {
        // Bounded to Additional Content (lines 2+): field switching
        // happens only via the plain Up/Down arms in handle_editor_mode.
        if self.cursor_line > 2 {
            self.cursor_line -= 1;
            let content_line = self.cursor_line - 1;
            let lines: Vec<&str> = self.content.lines().collect();
            if let Some(line) = lines.get(content_line) {
                self.cursor_col = self.cursor_col.min(line.len());
            }
        }
    }

    pub fn move_cursor_down(&mut self) {
        let total_content_lines = self.content.lines().count();

        // Bounded to Additional Content (lines 2..=total): field switching
        // happens only via the plain Up/Down arms in handle_editor_mode.
        if self.cursor_line >= 2 && self.cursor_line < total_content_lines {
            self.cursor_line += 1;
            let content_line = self.cursor_line - 1;
            let lines: Vec<&str> = self.content.lines().collect();
            if let Some(line) = lines.get(content_line) {
                self.cursor_col = self.cursor_col.min(line.len());
            } else {
                self.cursor_col = 0;
            }
        }
    }

    pub fn current_line_len(&self) -> usize {
        if self.cursor_line == 0 {
            self.path.len()
        } else {
            let content_line = self.cursor_line - 1;
            self.content
                .lines()
                .nth(content_line)
                .map(|l| l.len())
                .unwrap_or(0)
        }
    }

    pub fn ensure_h_scroll(&mut self, visible_width: usize) {
        if visible_width == 0 {
            return;
        }
        let scroll = match self.cursor_line {
            0 => &mut self.path_h_scroll,
            1 => &mut self.password_h_scroll,
            _ => &mut self.content_h_scroll,
        };
        if self.cursor_col >= *scroll + visible_width {
            *scroll = self
                .cursor_col
                .saturating_sub(visible_width.saturating_sub(1));
        } else if self.cursor_col < *scroll {
            *scroll = self.cursor_col;
        }
    }

    /// Tab in Additional Content (cursor_line 2+): insert 4 literal spaces
    /// at the cursor and advance past them. Literal spaces (not a tab
    /// character) keep the width predictable when rendered and when saved
    /// into the entry. No-op outside Additional Content.
    pub fn insert_indent(&mut self) {
        if self.cursor_line < 2 {
            return;
        }
        for _ in 0..4 {
            self.insert_char(' ');
        }
    }

    /// Shift+Tab in Additional Content (cursor_line 2+): remove leading
    /// spaces from the current line back to the previous tab stop (a
    /// multiple of 4), at most 4 spaces. The cursor shifts left by the
    /// same amount, clamped to the shortened line. No-op when the line
    /// has no leading spaces or outside Additional Content.
    pub fn dedent(&mut self) {
        if self.cursor_line < 2 {
            return;
        }
        let content_line = self.cursor_line - 1;
        let lines: Vec<String> = self.content.lines().map(|s| s.to_string()).collect();
        let mut new_lines = lines;

        while new_lines.len() <= content_line {
            new_lines.push(String::new());
        }

        let line = &mut new_lines[content_line];
        let indent = line.len() - line.trim_start_matches(' ').len();
        let remove = if indent == 0 {
            0
        } else if indent.is_multiple_of(4) {
            4
        } else {
            indent % 4
        };

        if remove > 0 {
            line.drain(..remove);
            self.cursor_col = self.cursor_col.saturating_sub(remove).min(line.len());
            self.content = new_lines.join("\n");
        }
    }
}

#[derive(Clone)]
pub struct ContextMenu {
    pub x: u16,
    pub y: u16,
    pub items: Vec<MenuItem>,
    pub selected: usize,
}

#[derive(Clone)]
pub struct MenuItem {
    pub label: String,
    pub action: MenuAction,
    pub enabled: bool,
}

#[derive(Clone)]
pub enum MenuAction {
    Open,
    Edit,
    New,
    Delete,
    CopyPassword,
    Reveal,
    ShowQr,
    Expand,
    Collapse,
    AddFolder,
    Move,
    GitPush,
    GitPull,
    Cancel,
}

/// Outcome of the most recent sync operation (push/pull), rendered in the
/// Git pane's Sync section. On success `action` holds the completed verb
/// ("Pushed to" / "Pulled from"); on failure it holds the bare verb
/// ("Push" / "Pull") and `detail` carries the error output.
#[derive(Clone, Debug)]
pub struct SyncStatus {
    pub action: String,
    /// Name of the remote the operation targeted ("origin", "fudge", ...),
    /// empty when unknown (e.g. a blocked sync that never resolved one).
    /// Rendered by the Git pane's Sync line so a local-path URL or a
    /// multi-remote store still says which remote was used.
    pub remote: String,
    /// Remote URL the operation targeted (empty when unknown).
    pub url: String,
    /// When the operation completed.
    pub at: Instant,
    pub success: bool,
    /// Error output when `success` is false.
    pub detail: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ------------------------------------------------------------------
    // Totp
    // ------------------------------------------------------------------

    /// The base32 encoding of "12345678901234567890", the secret used by
    /// the RFC 4226 HOTP test vectors.
    fn rfc_secret() -> &'static str {
        "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"
    }

    #[test]
    fn test_totp_from_secret_valid() {
        let totp = Totp::from_secret(rfc_secret());
        assert!(totp.is_some(), "valid base32 secret must parse");
    }

    #[test]
    fn test_totp_from_secret_invalid() {
        // '!' and '@' are not in the base32 alphabet, and an empty string
        // decodes to an empty (useless) secret - both must be rejected.
        assert!(Totp::from_secret("!!!").is_none());
        assert!(Totp::from_secret("").is_none());
    }

    #[test]
    fn test_totp_whitespace_only_secret_rejected() {
        // Spaces are stripped before decoding, so a whitespace-only secret
        // decodes to an empty secret and must be rejected like "".
        assert!(Totp::from_secret("   ").is_none());
    }

    #[test]
    fn test_totp_valid_secret_accepted() {
        assert!(Totp::from_secret("JBSWY3DPEHPK3PXP").is_some());
    }

    #[test]
    fn test_totp_from_secret_normalizes() {
        // Lowercase and spaced input must decode to the same secret as the
        // canonical uppercase form, producing identical codes.
        let canonical = Totp::from_secret(rfc_secret()).unwrap();
        let messy = Totp::from_secret("gezd gnbv gy3t qojq gezd gnbv gy3t qojq").unwrap();
        assert_eq!(
            canonical.generate_for_counter(1),
            messy.generate_for_counter(1)
        );
    }

    #[test]
    fn test_totp_rfc4226_vectors() {
        // The RFC 4226 HOTP test vectors for the 6-digit truncation of the
        // "12345678901234567890" secret. generate_for_counter is private,
        // but tests in this module may call it directly.
        let totp = Totp::from_secret(rfc_secret()).unwrap();
        let expected = [
            (0, "755224"),
            (1, "287082"),
            (2, "359152"),
            (3, "969429"),
            (4, "338314"),
            (5, "254676"),
        ];
        for (counter, code) in expected {
            assert_eq!(
                totp.generate_for_counter(counter),
                code,
                "HOTP value at counter {} mismatched",
                counter
            );
        }
    }

    #[test]
    fn test_totp_generate_shape() {
        // generate() depends on the wall clock, so only its shape can be
        // asserted: exactly `digits` numeric characters.
        let totp = Totp::from_secret(rfc_secret()).unwrap();
        let code = totp.generate();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()), "code: {}", code);
    }

    #[test]
    fn test_totp_time_remaining_in_period() {
        let totp = Totp::from_secret(rfc_secret()).unwrap();
        let remaining = totp.time_remaining();
        assert!((1..=30).contains(&remaining), "remaining: {}", remaining);
    }

    // ------------------------------------------------------------------
    // DecryptedContent
    // ------------------------------------------------------------------

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
        // An unparseable secret must not produce a Totp.
        let content = DecryptedContent::parse("secret\ntotp: !!!");
        assert!(content.totp.is_none());
    }

    // ------------------------------------------------------------------
    // PassEntry
    // ------------------------------------------------------------------

    #[test]
    fn test_pass_entry_file_name_strips_gpg() {
        let path = PathBuf::from("/store/work/github.gpg");
        let entry = PassEntry::new(path, std::path::Path::new("/store"), 1);
        // The path does not exist on disk, so it is not a directory.
        assert!(!entry.is_dir);
        assert_eq!(entry.name, "github");
        assert_eq!(entry.display_name, "github");
        assert!(!entry.expanded);
    }

    #[test]
    fn test_pass_entry_dir_display_name() {
        // PassEntry::new consults the filesystem for is_dir, so the test
        // needs a real directory to observe the directory branch.
        let dir = TempDir::new().unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir(&work).unwrap();

        let entry = PassEntry::new(work.clone(), dir.path(), 0);
        assert!(entry.is_dir);
        // Directories keep their full file name (no extension to strip).
        assert_eq!(entry.display_name, "work");
        assert!(!entry.expanded);
    }

    // ------------------------------------------------------------------
    // EntryEditor: construction
    // ------------------------------------------------------------------

    #[test]
    fn test_editor_new_appends_trailing_slash() {
        let editor = EntryEditor::new("work");
        assert_eq!(editor.path, "work/");
        assert_eq!(editor.original_path, "work/");
        assert!(editor.is_new);
        // Cursor starts in the path field, after the slash.
        assert_eq!(editor.cursor_line, 0);
        assert_eq!(editor.cursor_col, "work/".len());
        // Generator defaults.
        assert_eq!(editor.gen_length, 30);
        assert_eq!(editor.gen_charset, "all");
    }

    #[test]
    fn test_editor_new_keeps_trailing_slash() {
        let editor = EntryEditor::new("work/");
        assert_eq!(editor.path, "work/");
    }

    #[test]
    fn test_editor_from_decrypted_positions_cursor() {
        let editor = EntryEditor::from_decrypted("work/github", "hunter2\nusername: octocat");
        assert!(!editor.is_new);
        assert_eq!(editor.path, "work/github");
        assert_eq!(editor.content, "hunter2\nusername: octocat");
        // Cursor sits at the end of the password line.
        assert_eq!(editor.cursor_line, 1);
        assert_eq!(editor.cursor_col, "hunter2".len());
    }

    // ------------------------------------------------------------------
    // EntryEditor: password accessors
    // ------------------------------------------------------------------

    #[test]
    fn test_get_password_returns_first_line() {
        let editor = EntryEditor::from_decrypted("x", "hunter2\nusername: octocat");
        assert_eq!(editor.get_password(), Some("hunter2"));
    }

    #[test]
    fn test_set_password_replaces_first_line_keeps_rest() {
        let mut editor = EntryEditor::from_decrypted("x", "old\nusername: octocat");
        editor.set_password("new-pass");
        assert_eq!(editor.content, "new-pass\nusername: octocat");
        assert_eq!(editor.get_password(), Some("new-pass"));
        assert_eq!(editor.cursor_line, 1);
        assert_eq!(editor.cursor_col, "new-pass".len());
    }

    #[test]
    fn test_set_password_on_single_line_content() {
        let mut editor = EntryEditor::from_decrypted("x", "only");
        editor.set_password("replacement");
        assert_eq!(editor.content, "replacement");
    }

    // ------------------------------------------------------------------
    // EntryEditor: text insertion and deletion
    // ------------------------------------------------------------------

    #[test]
    fn test_insert_char_into_content() {
        let mut editor = EntryEditor::from_decrypted("x", "ab\ncd");
        // cursor_line 1 is the password line ("ab"); insert after 'a'.
        editor.cursor_line = 1;
        editor.cursor_col = 1;
        editor.insert_char('X');
        assert_eq!(editor.content, "aXb\ncd");
        assert_eq!(editor.cursor_col, 2);
    }

    #[test]
    fn test_insert_char_into_path_when_new() {
        let mut editor = EntryEditor::new("work/");
        editor.insert_char('a');
        assert_eq!(editor.path, "work/a");
        assert_eq!(editor.cursor_col, "work/a".len());
    }

    #[test]
    fn test_insert_char_path_locked_for_existing() {
        // For an existing entry the path field is read-only.
        let mut editor = EntryEditor::from_decrypted("work/github", "pw");
        editor.cursor_line = 0;
        editor.cursor_col = 5;
        editor.insert_char('X');
        assert_eq!(editor.path, "work/github");
    }

    #[test]
    fn test_insert_newline_splits_line() {
        let mut editor = EntryEditor::from_decrypted("x", "one\ntwo");
        editor.cursor_line = 1;
        editor.cursor_col = 1; // between 'o' and 'n' of "one"
        editor.insert_char('\n');
        assert_eq!(editor.content, "o\nne\ntwo");
        assert_eq!(editor.cursor_line, 2);
        assert_eq!(editor.cursor_col, 0);
    }

    #[test]
    fn test_backspace_removes_char() {
        let mut editor = EntryEditor::from_decrypted("x", "abc");
        editor.cursor_col = 2; // after 'b'
        editor.backspace();
        assert_eq!(editor.content, "ac");
        assert_eq!(editor.cursor_col, 1);
    }

    #[test]
    fn test_backspace_joins_with_previous_line() {
        // Joining happens only within Additional Content (cursor_line 3+):
        // the box's top line must not merge into the Password field.
        let mut editor = EntryEditor::from_decrypted("x", "pw\none\ntwo");
        editor.cursor_line = 3;
        editor.cursor_col = 0;
        editor.backspace();
        assert_eq!(editor.content, "pw\nonetwo");
        assert_eq!(editor.cursor_line, 2);
        assert_eq!(editor.cursor_col, 3);
    }

    #[test]
    fn test_backspace_at_content_top_is_noop() {
        // Fields are bounded: backspace at the top line of Additional
        // Content must not merge the line into the Password field.
        let mut editor = EntryEditor::from_decrypted("x", "pw\nfirst\nsecond");
        editor.cursor_line = 2;
        editor.cursor_col = 0;
        editor.backspace();
        assert_eq!(editor.content, "pw\nfirst\nsecond");
        assert_eq!(editor.cursor_line, 2);
        assert_eq!(editor.cursor_col, 0);
    }

    #[test]
    fn test_backspace_at_password_start_is_noop() {
        // Fields are bounded: backspace at the very start of the Password
        // field must not move into the Path field.
        let mut editor = EntryEditor::new("work/");
        editor.cursor_line = 1;
        editor.cursor_col = 0;
        editor.backspace();
        assert_eq!(editor.cursor_line, 1);
        assert_eq!(editor.cursor_col, 0);
        assert_eq!(editor.path, "work/");
    }

    #[test]
    fn test_delete_removes_char_before_next() {
        let mut editor = EntryEditor::from_decrypted("x", "abc");
        editor.cursor_col = 1; // before 'b'
        editor.delete();
        assert_eq!(editor.content, "ac");
        assert_eq!(editor.cursor_col, 1);
    }

    #[test]
    fn test_delete_joins_with_next_line() {
        // Joining happens only within Additional Content: Delete at the
        // end of the Password field must not pull the box's first line
        // into the password.
        let mut editor = EntryEditor::from_decrypted("x", "one\ntwo");
        editor.cursor_line = 1;
        editor.cursor_col = 3; // end of "one"
        editor.delete();
        assert_eq!(editor.content, "one\ntwo");

        let mut editor = EntryEditor::from_decrypted("x", "pw\none\ntwo");
        editor.cursor_line = 2;
        editor.cursor_col = 3; // end of "one"
        editor.delete();
        assert_eq!(editor.content, "pw\nonetwo");
    }

    // ------------------------------------------------------------------
    // EntryEditor: cursor movement
    // ------------------------------------------------------------------

    #[test]
    fn test_move_cursor_left_at_password_start_is_noop() {
        // Up/Down are the only field navigation: Left at the start of the
        // Password field must not wrap into the Path field.
        let mut editor = EntryEditor::from_decrypted("work/github", "pw");
        editor.cursor_line = 1;
        editor.cursor_col = 0;
        editor.move_cursor_left();
        assert_eq!(editor.cursor_line, 1);
        assert_eq!(editor.cursor_col, 0);
    }

    #[test]
    fn test_move_cursor_right_advances_to_next_line() {
        // Wrapping happens only within Additional Content: Right at the
        // end of the Password field stays put (Up/Down are the only field
        // navigation).
        let mut editor = EntryEditor::from_decrypted("x", "ab\ncd");
        editor.cursor_line = 1;
        editor.cursor_col = 2; // end of "ab"
        editor.move_cursor_right();
        assert_eq!(editor.cursor_line, 1);
        assert_eq!(editor.cursor_col, 2);

        let mut editor = EntryEditor::from_decrypted("x", "pw\nab\ncd");
        editor.cursor_line = 2;
        editor.cursor_col = 2; // end of "ab"
        editor.move_cursor_right();
        assert_eq!(editor.cursor_line, 3);
        assert_eq!(editor.cursor_col, 0);
    }

    #[test]
    fn test_move_cursor_up_down_clamp_column() {
        // move_cursor_up/down stay within Additional Content (lines 2+).
        let mut editor = EntryEditor::from_decrypted("x", "pw\nshort\na-much-longer-line");
        editor.cursor_line = 3;
        editor.cursor_col = 10;
        editor.move_cursor_up();
        assert_eq!(editor.cursor_line, 2);
        assert_eq!(
            editor.cursor_col, 5,
            "column must clamp to the shorter line"
        );

        editor.move_cursor_down();
        assert_eq!(editor.cursor_line, 3);
        assert_eq!(
            editor.cursor_col, 5,
            "column must be preserved when possible"
        );
    }

    #[test]
    fn test_move_cursor_up_down_do_not_cross_fields() {
        // Field switching happens only via the plain Up/Down arms in
        // handle_editor_mode; the model methods are bounded to the box.
        let mut editor = EntryEditor::from_decrypted("x", "pw\nonly");
        editor.cursor_line = 2;
        editor.move_cursor_up();
        assert_eq!(
            editor.cursor_line, 2,
            "Up from the top content line must not enter Password"
        );

        editor.cursor_line = 1;
        editor.move_cursor_down();
        assert_eq!(
            editor.cursor_line, 1,
            "Down from Password must not enter Additional Content"
        );
    }

    #[test]
    fn test_current_line_len() {
        let mut editor = EntryEditor::from_decrypted("work/github", "ab\nlonger");
        // Line 0 is the path field.
        editor.cursor_line = 0;
        assert_eq!(editor.current_line_len(), "work/github".len());
        // Line 1 is the password line ("ab").
        editor.cursor_line = 1;
        assert_eq!(editor.current_line_len(), 2);
        // Line 2 is the first content line ("longer").
        editor.cursor_line = 2;
        assert_eq!(editor.current_line_len(), "longer".len());
    }

    // ------------------------------------------------------------------
    // EntryEditor: selection
    // ------------------------------------------------------------------

    #[test]
    fn test_selection_within_one_line() {
        let mut editor = EntryEditor::from_decrypted("x", "pw\nhello world");
        editor.cursor_line = 2;
        editor.cursor_col = 0;
        editor.start_selection();
        editor.cursor_col = 5;
        editor.update_selection();
        assert_eq!(editor.get_selected_text().as_deref(), Some("hello"));
        assert!(editor.is_in_selection(2, 0));
        assert!(editor.is_in_selection(2, 4));
        assert!(!editor.is_in_selection(2, 5));
    }

    #[test]
    fn test_selection_reversed_endpoints() {
        // Selecting right-to-left must yield the same text.
        let mut editor = EntryEditor::from_decrypted("x", "pw\nhello world");
        editor.cursor_line = 2;
        editor.cursor_col = 5;
        editor.start_selection();
        editor.cursor_col = 0;
        editor.update_selection();
        assert_eq!(editor.get_selected_text().as_deref(), Some("hello"));
    }

    #[test]
    fn test_selection_across_lines() {
        let mut editor = EntryEditor::from_decrypted("x", "pw\nhello\nworld");
        editor.cursor_line = 2;
        editor.cursor_col = 1;
        editor.start_selection();
        editor.cursor_line = 3;
        editor.cursor_col = 3;
        editor.update_selection();
        assert_eq!(editor.get_selected_text().as_deref(), Some("ello\nwor"));
    }

    #[test]
    fn test_selection_cannot_include_path_or_password_lines() {
        // Selections touching line 0 (path) or line 1 (password) are
        // rejected by design: those fields are not selectable content.
        let mut editor = EntryEditor::from_decrypted("x", "pw\nhello");
        editor.cursor_line = 1;
        editor.cursor_col = 0;
        editor.start_selection();
        editor.cursor_line = 2;
        editor.cursor_col = 2;
        editor.update_selection();
        assert_eq!(editor.get_selected_text(), None);
    }

    #[test]
    fn test_clear_selection() {
        let mut editor = EntryEditor::from_decrypted("x", "pw\nhello");
        editor.cursor_line = 2;
        editor.start_selection();
        assert!(editor.has_selection);
        editor.clear_selection();
        assert!(!editor.has_selection);
        assert!(!editor.selecting);
        assert_eq!(editor.get_selected_text(), None);
    }

    #[test]
    fn test_insert_char_clears_selection() {
        let mut editor = EntryEditor::from_decrypted("x", "pw\nhello");
        editor.cursor_line = 2;
        editor.start_selection();
        editor.insert_char('X');
        assert!(!editor.has_selection);
    }

    // ------------------------------------------------------------------
    // EntryEditor: horizontal scrolling
    // ------------------------------------------------------------------

    #[test]
    fn test_ensure_h_scroll_scrolls_right() {
        let mut editor = EntryEditor::from_decrypted("x", "pw");
        editor.cursor_line = 1;
        editor.cursor_col = 10;
        editor.ensure_h_scroll(5);
        // The cursor must stay visible with room to spare on the right.
        assert_eq!(editor.password_h_scroll, 10 - (5 - 1));
    }

    #[test]
    fn test_ensure_h_scroll_scrolls_left() {
        let mut editor = EntryEditor::from_decrypted("x", "pw");
        editor.cursor_line = 1;
        editor.password_h_scroll = 10;
        editor.cursor_col = 4;
        editor.ensure_h_scroll(5);
        assert_eq!(editor.password_h_scroll, 4);
    }

    #[test]
    fn test_ensure_h_scroll_noop_when_visible() {
        let mut editor = EntryEditor::from_decrypted("x", "pw");
        editor.cursor_line = 1;
        editor.password_h_scroll = 2;
        editor.cursor_col = 4;
        editor.ensure_h_scroll(5);
        assert_eq!(editor.password_h_scroll, 2);
    }

    #[test]
    fn test_ensure_h_scroll_targets_correct_field() {
        let mut editor = EntryEditor::new("work/");
        editor.cursor_line = 0;
        editor.cursor_col = 6;
        editor.ensure_h_scroll(3);
        assert!(editor.path_h_scroll > 0);
        assert_eq!(editor.password_h_scroll, 0);
        assert_eq!(editor.content_h_scroll, 0);
    }

    #[test]
    fn test_ensure_h_scroll_zero_width_is_noop() {
        let mut editor = EntryEditor::from_decrypted("x", "pw");
        editor.cursor_line = 1;
        editor.cursor_col = 10;
        editor.ensure_h_scroll(0);
        assert_eq!(editor.password_h_scroll, 0);
    }

    // ------------------------------------------------------------------
    // EntryEditor: Tab indent / Shift+Tab dedent
    // ------------------------------------------------------------------

    #[test]
    fn test_insert_indent_adds_four_spaces() {
        let mut editor = EntryEditor::from_decrypted("x", "pw\nhello");
        editor.cursor_line = 2;
        editor.cursor_col = 0;
        editor.insert_indent();
        assert_eq!(editor.content, "pw\n    hello");
        assert_eq!(editor.cursor_col, 4);
    }

    #[test]
    fn test_insert_indent_mid_line() {
        let mut editor = EntryEditor::from_decrypted("x", "pw\nab");
        editor.cursor_line = 2;
        editor.cursor_col = 1;
        editor.insert_indent();
        assert_eq!(editor.content, "pw\na    b");
        assert_eq!(editor.cursor_col, 5);
    }

    #[test]
    fn test_insert_indent_ignored_outside_content() {
        // Tab handling outside Additional Content is unchanged: the
        // helper must be a no-op there.
        let mut editor = EntryEditor::from_decrypted("x", "pw");
        editor.cursor_line = 1;
        editor.cursor_col = 2;
        editor.insert_indent();
        assert_eq!(editor.content, "pw");
        assert_eq!(editor.cursor_col, 2);
    }

    #[test]
    fn test_dedent_removes_to_tab_stop() {
        // 8 leading spaces dedent by 4, back to the previous tab stop.
        let mut editor = EntryEditor::from_decrypted("x", "pw\n        deep");
        editor.cursor_line = 2;
        editor.cursor_col = 8;
        editor.dedent();
        assert_eq!(editor.content, "pw\n    deep");
        assert_eq!(editor.cursor_col, 4);
    }

    #[test]
    fn test_dedent_partial_indent() {
        // 2 leading spaces are not a tab stop: all of them are removed.
        let mut editor = EntryEditor::from_decrypted("x", "pw\n  two");
        editor.cursor_line = 2;
        editor.cursor_col = 2;
        editor.dedent();
        assert_eq!(editor.content, "pw\ntwo");
        assert_eq!(editor.cursor_col, 0);
    }

    #[test]
    fn test_dedent_noop_without_leading_spaces() {
        let mut editor = EntryEditor::from_decrypted("x", "pw\nnone");
        editor.cursor_line = 2;
        editor.cursor_col = 1;
        editor.dedent();
        assert_eq!(editor.content, "pw\nnone");
        assert_eq!(editor.cursor_col, 1);
    }

    #[test]
    fn test_dedent_ignored_outside_content() {
        let mut editor = EntryEditor::from_decrypted("x", "pw");
        editor.cursor_line = 1;
        editor.cursor_col = 2;
        editor.dedent();
        assert_eq!(editor.content, "pw");
    }
}

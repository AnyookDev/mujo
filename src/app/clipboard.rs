use super::App;
use crate::models::ClipboardTool;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Standard base64 (RFC 4648, with padding), used to encode OSC 52 payloads.
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// True when mujo is running inside an SSH session, where the local
/// clipboard tools cannot reach the user's machine.
#[cfg(not(test))]
fn in_ssh_session() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some()
}

/// Run a clipboard backend, feeding `stdin_text` on its stdin when given.
/// Returns true only when the program started and exited with status 0.
/// The exit status is the authority: a tool that exits early without
/// reading its input may close the pipe, and that write error is ignored.
/// A missing program or a non-zero exit counts as failure.
fn run_copy_tool(program: &str, args: &[&str], stdin_text: Option<&str>) -> bool {
    use std::io::Write;
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(if stdin_text.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    let Ok(mut child) = cmd.spawn() else {
        return false;
    };
    if let Some(text) = stdin_text {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
            // stdin is dropped here, closing the pipe so the tool sees EOF.
        }
    }
    child.wait().map(|status| status.success()).unwrap_or(false)
}

/// Write an OSC 52 "set clipboard" sequence to the controlling terminal.
/// Writes to /dev/tty rather than stdout, because stdout belongs to the
/// TUI's backend. Returns true when the sequence was written.
fn osc52_copy(text: &str) -> bool {
    use std::io::Write;
    let seq = format!("\x1b]52;c;{}\x07", base64_encode(text.as_bytes()));
    match std::fs::OpenOptions::new().write(true).open("/dev/tty") {
        Ok(mut tty) => tty
            .write_all(seq.as_bytes())
            .and_then(|_| tty.flush())
            .is_ok(),
        Err(_) => false,
    }
}

impl App {
    pub fn copy_ctrl_c(&mut self) {
        let to_copy: Option<(String, bool)> = if let Some(content) = &self.current_content {
            if let Some(i) = self.preview_selected_line {
                content.lines.get(i).cloned().map(|l| (l, i == 0))
            } else {
                content.lines.first().cloned().map(|l| (l, true))
            }
        } else {
            None
        };

        if let Some((text, is_password)) = to_copy {
            if self.copy_to_clipboard(&text) {
                // Keep the "(Ns)" hint in sync with the configured timeout; when
                // auto-clear is disabled no countdown is shown.
                let msg = match (is_password, self.clipboard_timeout()) {
                    (true, Some(d)) => format!("Password copied to clipboard ({}s)", d.as_secs()),
                    (false, Some(d)) => format!("Line copied to clipboard ({}s)", d.as_secs()),
                    (true, None) => "Password copied to clipboard".to_string(),
                    (false, None) => "Line copied to clipboard".to_string(),
                };
                self.message = Some((msg, Instant::now()));
            }
        }
    }

    /// Copy `text` with the first backend that reports success. Returns
    /// true only when a backend exited successfully. On success it arms the
    /// auto-clear timer and sets a generic success message; on failure it
    /// sets the failure message. Callers that have a more specific message
    /// overwrite it, and only do so when this returns true.
    pub fn copy_to_clipboard(&mut self, text: &str) -> bool {
        let tool = self.copy_backend(text);

        match tool {
            Some(tool) => {
                self.clipboard_tool = Some(tool);
                self.message = Some(("Copied to clipboard!".to_string(), Instant::now()));
                self.security_clear_at = self.clipboard_timeout().map(|d| Instant::now() + d);
                true
            }
            None => {
                self.message = Some(("Failed to copy to clipboard".to_string(), Instant::now()));
                false
            }
        }
    }

    /// Pick the first backend that accepts `text`. Only called by
    /// copy_to_clipboard.
    #[cfg(not(test))]
    fn copy_backend(&self, text: &str) -> Option<ClipboardTool> {
        // OSC 52 is written to the real terminal, so only do it when the
        // TUI is running. Tests never reach this and never touch /dev/tty.
        if self.tui_active && in_ssh_session() && osc52_copy(text) {
            // The local clipboard tools cannot reach the user's machine
            // over SSH, so the text goes out as an OSC 52 escape for the
            // local terminal to apply.
            return Some(ClipboardTool::Osc52);
        }
        if cfg!(target_os = "macos") {
            return run_copy_tool("pbcopy", &[], Some(text)).then_some(ClipboardTool::Pbcopy);
        }
        if run_copy_tool("wl-copy", &[], Some(text)) {
            return Some(ClipboardTool::WlCopy);
        }
        if run_copy_tool("xclip", &["-selection", "clipboard"], Some(text)) {
            return Some(ClipboardTool::Xclip);
        }
        None
    }

    /// Tests must not depend on the machine's clipboard tools or write to
    /// a real terminal. The tool-running behaviour is covered by the
    /// run_copy_tool tests, so this stub always reports a successful copy.
    #[cfg(test)]
    fn copy_backend(&self, _text: &str) -> Option<ClipboardTool> {
        Some(ClipboardTool::WlCopy)
    }

    pub fn paste_from_clipboard() -> Option<String> {
        // Mirror copy_to_clipboard's OS-based backend choice: pbpaste on
        // macOS, wl-paste with an xclip fallback on Linux.
        if cfg!(target_os = "macos") {
            if let Ok(output) = std::process::Command::new("pbpaste").output() {
                if output.status.success() {
                    return Some(String::from_utf8_lossy(&output.stdout).to_string());
                }
            }
            return None;
        }

        if let Ok(output) = std::process::Command::new("wl-paste")
            .arg("--no-newline")
            .output()
        {
            if output.status.success() {
                return Some(String::from_utf8_lossy(&output.stdout).to_string());
            }
        }

        if let Ok(output) = std::process::Command::new("xclip")
            .args(["-selection", "clipboard", "-o"])
            .output()
        {
            if output.status.success() {
                return Some(String::from_utf8_lossy(&output.stdout).to_string());
            }
        }

        None
    }

    /// The configured clipboard auto-clear delay. `None` when auto-clear
    /// is disabled (timeout_seconds = 0).
    pub fn clipboard_timeout(&self) -> Option<Duration> {
        if self.config.clipboard.timeout_seconds == 0 {
            None
        } else {
            Some(Duration::from_secs(self.config.clipboard.timeout_seconds))
        }
    }

    pub fn clear_clipboard(&mut self) {
        match self.clipboard_tool {
            Some(ClipboardTool::WlCopy) => {
                let _ = Command::new("wl-copy")
                    .arg("--clear")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
            }
            Some(ClipboardTool::Pbcopy) => {
                let _ = Command::new("sh")
                    .arg("-c")
                    .arg("echo -n | pbcopy")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
            }
            Some(ClipboardTool::Xclip) => {
                let _ = Command::new("sh")
                    .arg("-c")
                    .arg("echo -n | xclip -selection clipboard")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
                let _ = Command::new("sh")
                    .arg("-c")
                    .arg("echo -n | xclip -selection primary")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
            }
            // Clearing the clipboard over OSC 52 means writing an empty
            // payload. Only effective if the terminal honours the write.
            Some(ClipboardTool::Osc52) => {
                let _ = osc52_copy("");
            }
            None => {}
        }

        self.clipboard_tool = None;
        self.security_clear_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::{base64_encode, run_copy_tool};

    /// RFC 4648 section 10 test vectors.
    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_handles_high_bytes_and_plus_slash() {
        // 0xfb 0xff encodes to "+/8=" (exercises the top two alphabet entries).
        assert_eq!(base64_encode(&[0xfb, 0xff]), "+/8=");
    }

    /// A tool that exits 0 counts as a successful copy.
    #[test]
    fn copy_tool_exit_zero_is_success() {
        assert!(run_copy_tool("true", &[], Some("secret")));
    }

    /// A tool that exits non-zero counts as failure, even though it spawned.
    /// This is the case that made "Copied!" lie when wl-copy had no display.
    #[test]
    fn copy_tool_exit_nonzero_is_failure() {
        assert!(!run_copy_tool("false", &[], Some("secret")));
    }

    /// A missing program counts as failure.
    #[test]
    fn copy_tool_missing_program_is_failure() {
        assert!(!run_copy_tool("mujo-no-such-clipboard-tool", &[], Some("x")));
    }
}

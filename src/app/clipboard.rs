use super::App;
use crate::models::ClipboardTool;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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
            self.copy_to_clipboard(&text);
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

    pub fn copy_to_clipboard(&mut self, text: &str) {
        // Backend selection is by OS, not by spawn failure: on macOS
        // pbcopy is the native backend; on Linux Wayland's wl-copy is
        // preferred, with xclip as the X11 fallback.
        if cfg!(target_os = "macos") {
            if Command::new("pbcopy")
                .stdin(Stdio::piped())
                .spawn()
                .map(|mut child| {
                    if let Some(stdin) = child.stdin.as_mut() {
                        use std::io::Write;
                        let _ = stdin.write_all(text.as_bytes());
                    }
                    child.wait()
                })
                .is_ok()
            {
                self.clipboard_tool = Some(ClipboardTool::Pbcopy);
                self.message = Some(("Copied to clipboard!".to_string(), Instant::now()));
                self.security_clear_at = self.clipboard_timeout().map(|d| Instant::now() + d);
                return;
            }
        } else {
            if Command::new("wl-copy").arg(text).spawn().is_ok() {
                self.clipboard_tool = Some(ClipboardTool::WlCopy);
                self.message = Some(("Copied to clipboard!".to_string(), Instant::now()));
                self.security_clear_at = self.clipboard_timeout().map(|d| Instant::now() + d);
                return;
            }

            if Command::new("xclip")
                .args(["-selection", "clipboard"])
                .stdin(Stdio::piped())
                .spawn()
                .map(|mut child| {
                    if let Some(stdin) = child.stdin.as_mut() {
                        use std::io::Write;
                        let _ = stdin.write_all(text.as_bytes());
                    }
                    child.wait()
                })
                .is_ok()
            {
                self.clipboard_tool = Some(ClipboardTool::Xclip);
                self.message = Some(("Copied to clipboard!".to_string(), Instant::now()));
                self.security_clear_at = self.clipboard_timeout().map(|d| Instant::now() + d);
                return;
            }
        }

        self.message = Some(("Failed to copy to clipboard".to_string(), Instant::now()));
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
                let _ = Command::new("wl-copy").arg("--clear").spawn();
            }
            Some(ClipboardTool::Pbcopy) => {
                let _ = Command::new("sh").arg("-c").arg("echo -n | pbcopy").spawn();
            }
            Some(ClipboardTool::Xclip) => {
                let _ = Command::new("sh")
                    .arg("-c")
                    .arg("echo -n | xclip -selection clipboard")
                    .spawn();
                let _ = Command::new("sh")
                    .arg("-c")
                    .arg("echo -n | xclip -selection primary")
                    .spawn();
            }
            None => {}
        }

        self.clipboard_tool = None;
        self.security_clear_at = None;
    }
}

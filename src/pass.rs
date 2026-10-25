use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// Resolve the password store directory. $PASSWORD_STORE_DIR is used
/// verbatim when set and non-empty (no tilde expansion, matching how pass
/// itself treats it); otherwise the default ~/.password-store is returned.
pub fn store_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("PASSWORD_STORE_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    dirs::home_dir()
        .map(|home| home.join(".password-store"))
        .unwrap_or_else(|| PathBuf::from(".password-store"))
}

pub struct PassCommand;

/// Build an error for a failed external command, including the command's
/// captured stderr so the UI can show *why* it failed. Falls back to a
/// generic "<context> failed" when stderr is empty.
fn cmd_err(context: &str, stderr: &str) -> anyhow::Error {
    let stderr = stderr.trim();
    if stderr.is_empty() {
        anyhow::anyhow!("{} failed", context)
    } else {
        anyhow::anyhow!("{} failed: {}", context, stderr)
    }
}

/// Map an io::Error from spawning an external command into a user-facing
/// error, distinguishing "binary missing" from any other failure.
fn spawn_err(program: &str, e: &std::io::Error) -> anyhow::Error {
    if e.kind() == std::io::ErrorKind::NotFound {
        let pkg = match program {
            "gpg" => "gnupg",
            other => other,
        };
        anyhow::anyhow!("{program} not found on PATH — install {pkg}")
    } else {
        anyhow::anyhow!("failed to run {program}: {e}")
    }
}

/// The controlling terminal path to hand a gpg-invoking child via
/// $GPG_TTY, or None when the parent already has a usable value.
///
/// mujo owns the terminal (alternate screen, raw mode) and captures child
/// stdout/stderr through pipes, so the child's stdin is NOT the tty.
/// pinentry-curses (and pinentry-tty) locate the terminal via
/// ttyname(stdin) and fall back to $GPG_TTY; with a piped stdin and
/// GPG_TTY unset, pinentry fails and gpg reports "public key decryption
/// failed: No such file or directory". Setting GPG_TTY on the child to
/// the real controlling terminal makes the curses overlay appear and
/// work. An existing non-empty GPG_TTY in the parent environment is
/// never overridden.
fn gpg_tty_for_child() -> Option<String> {
    if let Some(v) = std::env::var_os("GPG_TTY") {
        if !v.is_empty() {
            return None;
        }
    }
    // Linux: fd 0's symlink target is the controlling terminal when one
    // exists (a pipe under capture resolves to "pipe:[...]" and is
    // rejected by the /dev/ prefix check).
    if let Ok(target) = std::fs::read_link("/proc/self/fd/0") {
        if let Some(s) = target.to_str() {
            if s.starts_with("/dev/") {
                return Some(s.to_string());
            }
        }
    }
    Some("/dev/tty".to_string())
}

/// Build a Command for a program that may invoke gpg (pass itself, or
/// gpg directly), applying gpg_tty_for_child() so pinentry can find the
/// terminal. The single place gpg-invoking children are constructed, so
/// the GPG_TTY fix lives here rather than at every call site.
fn gpg_capable_cmd(program: &str) -> Command {
    let mut cmd = Command::new(program);
    if let Some(tty) = gpg_tty_for_child() {
        cmd.env("GPG_TTY", tty);
    }
    cmd
}

impl PassCommand {
    pub fn show(path: &str) -> Result<String> {
        let output = gpg_capable_cmd("pass")
            .arg("show")
            .arg(path)
            .output()
            .map_err(|e| spawn_err("pass", &e))?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(cmd_err(
                "pass show",
                &String::from_utf8_lossy(&output.stderr),
            ))
        }
    }

    pub fn insert(path: &str, content: &str) -> Result<()> {
        let mut child = gpg_capable_cmd("pass")
            .args(["insert", "-m", path])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;

        if let Some(stdin) = child.stdin.as_mut() {
            use std::io::Write;
            stdin.write_all(content.as_bytes())?;
        }

        // wait_with_output() drains the piped stderr (avoiding a deadlock
        // if pass fills the pipe buffer) and gives us the exit status.
        let output = child.wait_with_output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(cmd_err(
                "pass insert",
                &String::from_utf8_lossy(&output.stderr),
            ))
        }
    }

    pub fn generate(path: &str, length: usize, no_symbols: bool) -> Result<String> {
        let mut cmd = gpg_capable_cmd("pass");
        cmd.arg("generate");
        if no_symbols {
            cmd.arg("-n");
        }
        cmd.arg(path).arg(length.to_string());

        let output = cmd.output().map_err(|e| spawn_err("pass", &e))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(cmd_err(
                "pass generate",
                &String::from_utf8_lossy(&output.stderr),
            ))
        }
    }

    /// Initialize a new password store with the given GPG ID
    /// (`pass init <gpg-id>`). Creates the store directory and writes the
    /// .gpg-id file. Returns the command output (or the stderr text on
    /// failure, so the UI can show why init did not work).
    pub fn init(gpg_id: &str) -> Result<String> {
        let output = gpg_capable_cmd("pass")
            .arg("init")
            .arg(gpg_id)
            .output()
            .map_err(|e| spawn_err("pass", &e))?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if stderr.is_empty() {
                Err(anyhow::anyhow!("pass init failed"))
            } else {
                Err(anyhow::anyhow!("pass init failed: {}", stderr))
            }
        }
    }

    /// Check whether `id` refers to an existing GPG key, suitable for
    /// initializing a password store. Accepts an email address, a short
    /// (8 hex chars) or long (16 hex chars) key ID, or a full 40-char
    /// fingerprint (spaces between fingerprint groups are tolerated).
    ///
    /// Two-stage check: a cheap pure format plausibility test first (so
    /// obvious nonsense never spawns a subprocess), then an actual key
    /// lookup via `gpg --list-keys <id>`, which exits non-zero when no key
    /// matches. Returns Ok(false) for anything that is not a valid key;
    /// Err only when the gpg invocation itself fails (e.g. gpg missing).
    pub fn validate_gpg_key(id: &str) -> Result<bool> {
        // Pass the raw string to the plausibility check so control
        // characters (e.g. a trailing newline) are rejected before any
        // trimming can hide them.
        if id.trim().is_empty() {
            return Err(anyhow::anyhow!("enter a GPG key ID"));
        }
        if !is_plausible_gpg_id(id) {
            let t = id.trim();
            let detail = if t.contains('@') {
                "expected a valid email address".to_string()
            } else if let Some(bad) = t.chars().find(|c| !c.is_ascii_hexdigit()) {
                format!("unexpected character '{}'", bad)
            } else {
                format!("expected 8, 16 or 40 hex chars, got {}", t.len())
            };
            return Err(anyhow::anyhow!("invalid GPG key ID: {} ({})", t, detail));
        }
        let output = gpg_capable_cmd("gpg")
            .args(["--list-keys", id.trim()])
            .output()
            .map_err(|e| spawn_err("gpg", &e))?;
        Ok(output.status.success())
    }

    /// Delete an entry (`pass rm -f <path>`) or a folder
    /// (`pass rm -f -r <path>/`).
    ///
    /// The trailing slash on a folder deletion is REQUIRED: pass's
    /// cmd_delete checks for the *file* (`-f $PREFIX/$path.gpg`) before
    /// the *directory* (`-d $PREFIX/$path`), so when a folder `X` and a
    /// file `X.gpg` coexist, `pass rm -f -r X` takes the file branch and
    /// deletes `X.gpg` while leaving the folder behind. Appending `/`
    /// makes the file check look for `X/.gpg` (never a regular file), so
    /// the directory branch runs instead. We deliberately keep going
    /// through `pass` (rather than removing the directory ourselves with
    /// fs::remove_dir_all) so its git rm and commit handling still runs.
    pub fn remove(path: &str, is_dir: bool) -> Result<()> {
        use std::process::Stdio;

        let target = if is_dir {
            format!("{}/", path.trim_end_matches('/'))
        } else {
            path.to_string()
        };

        let mut cmd = gpg_capable_cmd("pass");
        cmd.arg("rm").arg("-f");
        if is_dir {
            cmd.arg("-r");
        }
        cmd.arg(target);

        cmd.stdout(Stdio::null());

        let output = cmd.output().map_err(|e| spawn_err("pass", &e))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(cmd_err("pass rm", &String::from_utf8_lossy(&output.stderr)))
        }
    }

    pub fn mv(from: &str, to: &str, is_dest_dir: bool) -> Result<()> {
        use std::process::Stdio;

        let from = from.strip_suffix(".gpg").unwrap_or(from);
        let to = to.strip_suffix(".gpg").unwrap_or(to);

        // `pass mv` builds its destination as "$PREFIX/<to>", so an
        // absolute `to` would be recreated *inside* the store: moving an
        // entry to "/home/me/nested1" produced
        // ~/.password-store/home/me/nested1. Normalize an absolute
        // destination that points into the store back to a store-relative
        // path, and refuse destinations outside the store.
        let store_path = store_path();
        let store_prefix = store_path.to_string_lossy().to_string();
        let to: String = if to.starts_with('/') {
            let rest = to
                .strip_prefix(store_prefix.as_str())
                .ok_or_else(|| anyhow::anyhow!("Destination is outside the password store"))?;
            let rest = rest.trim_start_matches('/');
            if rest.is_empty() {
                return Err(anyhow::anyhow!("Destination cannot be the store root"));
            }
            rest.to_string()
        } else {
            to.to_string()
        };

        if is_dest_dir {
            let dest_dir = store_path.join(&to);
            if !dest_dir.exists() {
                std::fs::create_dir_all(&dest_dir)?;
            }
        }

        // .output() (not .status()) so the piped stderr is actually read
        // instead of discarded.
        let output = Command::new("pass")
            .args(["mv", from, &to])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| spawn_err("pass", &e))?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if stderr.is_empty() {
                Err(anyhow::anyhow!(
                    "pass mv failed: '{}' may not exist, or destination may already exist",
                    from
                ))
            } else {
                Err(anyhow::anyhow!("pass mv failed: {}", stderr))
            }
        }
    }

    /// Run `pass git <args>`. Only the `init` subcommand still goes through
    /// this wrapper: pass's cmd_git adds .gitattributes and makes the
    /// initial commit, and its status is reliable on that branch because it
    /// does `git "$@" || exit 1`. Every other subcommand must use git_in(),
    /// because `pass git` exits 0 regardless of what git did, which made
    /// every failure invisible.
    pub fn git(args: &[&str]) -> Result<String> {
        let output = Command::new("pass")
            .arg("git")
            .args(args)
            .output()
            .map_err(|e| spawn_err("pass", &e))?;

        Self::git_result(output.status, &output.stdout, &output.stderr)
    }

    /// Run git directly in the store directory (`git -C <store> ...`),
    /// which is all pass's `git` wrapper does (GIT_DIR=$PREFIX git "$@")
    /// minus the wrapper's swallowed exit status. The real exit status
    /// propagates through git_result(), so failures are reportable.
    pub fn git_in(store: &Path, args: &[&str]) -> Result<String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(store)
            .args(args)
            .output()
            .map_err(|e| spawn_err("git", &e))?;

        Self::git_result(output.status, &output.stdout, &output.stderr)
    }

    /// Interactive variant of git_in() for the tty handover: the child's
    /// stderr is teed - each chunk is written verbatim (and flushed) to
    /// this process's stderr as it arrives, so OpenSSH's "Confirm user
    /// presence for key ..." touch prompt is visible while the alternate
    /// screen is down - AND captured, so the returned string is
    /// byte-identical to git_in()'s.
    pub fn git_in_tee(store: &Path, args: &[&str]) -> Result<String> {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(store).args(args);
        let output = spawn_with_tee(cmd, "git")?;
        Self::git_result(output.status, &output.stdout, &output.stderr)
    }

    /// Shared tail of git()/git_tee(): clean the raw streams and turn the
    /// exit status into Ok(combined) / Err(combined). One implementation
    /// for both paths keeps their output byte-identical.
    fn git_result(
        status: std::process::ExitStatus,
        raw_stdout: &[u8],
        raw_stderr: &[u8],
    ) -> Result<String> {
        let clean = |s: &str| -> String {
            s.lines()
                .map(|l| {
                    let t = l.trim();
                    t.strip_prefix("fatal:")
                        .or_else(|| t.strip_prefix("FATAL:"))
                        .unwrap_or(t)
                        .trim()
                        .to_string()
                })
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        };

        let stdout = clean(&String::from_utf8_lossy(raw_stdout));
        let stderr = clean(&String::from_utf8_lossy(raw_stderr));
        let combined = if stdout.is_empty() {
            stderr.clone()
        } else if stderr.is_empty() {
            stdout.clone()
        } else {
            format!("{}\n{}", stdout, stderr)
        };

        if status.success() {
            Ok(combined)
        } else {
            Err(anyhow::anyhow!(if combined.is_empty() {
                "pass git failed".to_string()
            } else {
                combined
            }))
        }
    }
}

/// Run `cmd` with both stdout and stderr piped, teeing the child's stderr
/// to this process's stderr (written and flushed per chunk, so prompts
/// like OpenSSH's key-touch confirmation appear immediately) while also
/// capturing it. Stdout is drained on the calling thread, stderr on a
/// helper thread that is joined before returning - so a caller that
/// restores the terminal right after this function can never race the
/// last stderr chunk. The returned Output has the same shape as
/// Command::output()'s.
fn spawn_with_tee(mut cmd: Command, program: &str) -> Result<Output> {
    use std::io::Read;

    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| spawn_err(program, &e))?;

    // Tee stderr: every chunk read from the child goes verbatim to this
    // process's stderr (flushed, so it is on screen at once) and into the
    // capture buffer.
    let stderr_thread = child.stderr.take().map(|stderr| {
        std::thread::spawn(move || {
            let mut reader = stderr;
            let mut captured = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        use std::io::Write;
                        let mut err = std::io::stderr();
                        let _ = err.write_all(&chunk[..n]);
                        let _ = err.flush();
                        captured.extend_from_slice(&chunk[..n]);
                    }
                    Err(_) => break,
                }
            }
            captured
        })
    });

    // Drain stdout on the calling thread so the child never blocks on a
    // full stdout pipe while stderr is being teed.
    let mut stdout_buf = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_end(&mut stdout_buf);
    }

    let status = child.wait()?;
    // Join the tee thread BEFORE returning: the caller restores the
    // alternate screen immediately after, and a not-yet-joined thread
    // could still be mid-write to the now-hidden terminal.
    let stderr_buf = stderr_thread
        .map(|h| h.join().unwrap_or_default())
        .unwrap_or_default();

    Ok(Output {
        status,
        stdout: stdout_buf,
        stderr: stderr_buf,
    })
}

/// Cheap plausibility check for a GPG key identifier, without invoking
/// gpg. Accepts:
/// - an email address: exactly one '@' with a non-empty local part and a
///   domain containing at least one dot, no whitespace/control characters
/// - a hex key ID or fingerprint: 8, 16 or 40 hex digits after removing
///   spaces (fingerprints are conventionally written in space-separated
///   groups)
fn is_plausible_gpg_id(id: &str) -> bool {
    // Control characters (e.g. an embedded or trailing newline) are never
    // valid. This check must run on the RAW input: trimming first would
    // strip a trailing '\n' and let "user@example.com\n" slip through as a
    // valid email.
    if id.chars().any(|c| c.is_control()) {
        return false;
    }

    let id = id.trim();
    if id.is_empty() {
        return false;
    }

    if id.contains('@') {
        // Email form: exactly one '@', a non-empty local part, a dotted
        // domain, and no whitespace anywhere (an internal space like
        // "us er@example.com" is invalid).
        if id.chars().any(|c| c.is_whitespace()) {
            return false;
        }
        let parts: Vec<&str> = id.split('@').collect();
        if parts.len() != 2 {
            return false;
        }
        let (local, domain) = (parts[0], parts[1]);
        return !local.is_empty() && !domain.is_empty() && domain.contains('.');
    }

    // Hex key ID / fingerprint form: tolerate the conventional
    // space-separated fingerprint groups by stripping spaces first, then
    // require one of the accepted lengths and hex digits only.
    let hex: String = id.chars().filter(|c| *c != ' ').collect();
    matches!(hex.len(), 8 | 16 | 40) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::MutexGuard;

    /// Set PASSWORD_STORE_DIR for the duration of a test (None removes
    /// it), restoring the previous value on drop. Holds the shared
    /// environment lock for its whole lifetime; the lock is non-reentrant,
    /// so never hold two guards at once.
    struct StoreDirGuard {
        saved: Option<OsString>,
        _lock: MutexGuard<'static, ()>,
    }

    impl StoreDirGuard {
        fn set(value: Option<&str>) -> Self {
            let lock = crate::testenv::lock();
            let saved = std::env::var_os("PASSWORD_STORE_DIR");
            match value {
                Some(v) => std::env::set_var("PASSWORD_STORE_DIR", v),
                None => std::env::remove_var("PASSWORD_STORE_DIR"),
            }
            Self { saved, _lock: lock }
        }
    }

    impl Drop for StoreDirGuard {
        fn drop(&mut self) {
            match self.saved.take() {
                Some(v) => std::env::set_var("PASSWORD_STORE_DIR", v),
                None => std::env::remove_var("PASSWORD_STORE_DIR"),
            }
        }
    }

    #[test]
    fn test_plausible_email_addresses() {
        assert!(is_plausible_gpg_id("user@example.com"));
        assert!(is_plausible_gpg_id("first.last@mail.example.org"));
        assert!(
            is_plausible_gpg_id("  user@example.com  "),
            "surrounding whitespace is trimmed"
        );
    }

    #[test]
    fn test_plausible_key_ids_and_fingerprints() {
        // Short key ID (8 hex chars).
        assert!(is_plausible_gpg_id("12345678"));
        assert!(is_plausible_gpg_id("ABCDEF01"));
        // Long key ID (16 hex chars).
        assert!(is_plausible_gpg_id("1234567890ABCDEF"));
        // Full 40-char fingerprint.
        assert!(is_plausible_gpg_id(
            "1234567890ABCDEF1234567890ABCDEF12345678"
        ));
        // Fingerprint with conventional space-separated groups.
        assert!(is_plausible_gpg_id(
            "1234 5678 90AB CDEF 1234 5678 90AB CDEF 1234 5678"
        ));
    }

    #[test]
    fn test_implausible_ids() {
        // Empty and whitespace-only.
        assert!(!is_plausible_gpg_id(""));
        assert!(!is_plausible_gpg_id("   "));
        // Not an email and not hex.
        assert!(!is_plausible_gpg_id("nonsense"));
        assert!(!is_plausible_gpg_id("!!!"));
        // Malformed emails.
        assert!(!is_plausible_gpg_id("@example.com"));
        assert!(!is_plausible_gpg_id("user@"));
        assert!(!is_plausible_gpg_id("user@@example.com"));
        assert!(!is_plausible_gpg_id("user@example"));
        assert!(!is_plausible_gpg_id("us er@example.com"));
        // Wrong hex lengths: too short, or between the accepted sizes.
        assert!(!is_plausible_gpg_id("1234567"));
        assert!(!is_plausible_gpg_id("123456789"));
        assert!(!is_plausible_gpg_id("1234567890ABCDEF1"));
        // Non-hex characters in a key-ID-shaped string.
        assert!(!is_plausible_gpg_id("1234567G"));
        // Control characters are rejected - including a trailing newline,
        // which must not be silently trimmed away first.
        assert!(!is_plausible_gpg_id("user@example.com\n"));
    }

    /// The tee path must capture exactly what the plain Command::output()
    /// path captures: run the same stream-writing command both ways and
    /// compare status and both byte streams, including \r characters.
    #[test]
    fn test_spawn_with_tee_matches_plain_output() {
        let script = "printf 'out\\nline2\\r\\n'; printf 'err\\nline2\\r' >&2; exit 3";
        let build = || {
            let mut c = Command::new("sh");
            c.arg("-c").arg(script);
            c
        };

        let plain = build().output().expect("plain run");
        let teed = spawn_with_tee(build(), "sh").expect("tee run");

        assert_eq!(plain.status.code(), teed.status.code());
        assert_eq!(plain.stdout, teed.stdout, "stdout must be identical");
        assert_eq!(plain.stderr, teed.stderr, "stderr must be identical");
    }

    /// git_in() must propagate git's real exit status: a failing command
    /// returns Err (pass git used to exit 0 and hide this) and a
    /// succeeding one returns Ok with its output.
    #[test]
    fn test_git_in_propagates_exit_status() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("create repo");
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(&repo)
            .status()
            .expect("git init");

        let err = PassCommand::git_in(&repo, &["rev-parse", "--verify", "nope"])
            .expect_err("failing git command must be an Err");
        assert!(
            !err.to_string().is_empty(),
            "error should carry git's stderr"
        );

        let ok = PassCommand::git_in(&repo, &["rev-parse", "--is-inside-work-tree"])
            .expect("succeeding git command must be Ok");
        assert_eq!(ok, "true");
    }

    /// With GPG_TTY already set to a non-empty value, the helper must
    /// return None so the parent's value is never overridden.
    #[test]
    fn test_gpg_tty_for_child_respects_existing_value() {
        let _guard = StoreDirGuard::set(None);
        let saved = std::env::var_os("GPG_TTY");
        std::env::set_var("GPG_TTY", "/dev/pts/42");
        assert!(gpg_tty_for_child().is_none());
        match saved {
            Some(v) => std::env::set_var("GPG_TTY", v),
            None => std::env::remove_var("GPG_TTY"),
        }
    }

    /// With GPG_TTY unset, the helper resolves to a terminal path. Under
    /// `cargo test` stdin is captured (a pipe, whose /proc target does not
    /// start with /dev/), so this exercises the /dev/tty fallback too.
    #[test]
    fn test_gpg_tty_for_child_falls_back_to_dev_path() {
        let _guard = StoreDirGuard::set(None);
        let saved = std::env::var_os("GPG_TTY");
        std::env::remove_var("GPG_TTY");
        let tty = gpg_tty_for_child();
        match saved {
            Some(v) => std::env::set_var("GPG_TTY", v),
            None => std::env::remove_var("GPG_TTY"),
        }
        let tty = tty.expect("unset GPG_TTY must resolve to a path");
        assert!(
            tty.starts_with("/dev/"),
            "expected a /dev/ path, got {tty:?}"
        );
    }

    /// End to end: with GPG_TTY unset in the parent, a child built by
    /// gpg_capable_cmd sees a /dev/ GPG_TTY in its environment.
    #[test]
    fn test_gpg_capable_cmd_sets_gpg_tty_when_unset() {
        let _guard = StoreDirGuard::set(None);
        let saved = std::env::var_os("GPG_TTY");
        std::env::remove_var("GPG_TTY");
        let out = gpg_capable_cmd("sh")
            .args(["-c", "printf '%s' \"$GPG_TTY\""])
            .output()
            .expect("sh");
        match saved {
            Some(v) => std::env::set_var("GPG_TTY", v),
            None => std::env::remove_var("GPG_TTY"),
        }
        let tty = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(
            tty.starts_with("/dev/"),
            "child must see a /dev/ GPG_TTY, got {tty:?}"
        );
    }

    /// An existing non-empty GPG_TTY must pass through to the child
    /// unchanged - the helper never overrides it.
    #[test]
    fn test_gpg_capable_cmd_keeps_existing_gpg_tty() {
        let _guard = StoreDirGuard::set(None);
        let saved = std::env::var_os("GPG_TTY");
        std::env::set_var("GPG_TTY", "/dev/pts/42");
        let out = gpg_capable_cmd("sh")
            .args(["-c", "printf '%s' \"$GPG_TTY\""])
            .output()
            .expect("sh");
        match saved {
            Some(v) => std::env::set_var("GPG_TTY", v),
            None => std::env::remove_var("GPG_TTY"),
        }
        assert_eq!(String::from_utf8_lossy(&out.stdout), "/dev/pts/42");
    }

    #[test]
    fn test_store_path_env_set() {
        let _guard = StoreDirGuard::set(Some("/tmp/custom-store"));
        assert_eq!(store_path(), PathBuf::from("/tmp/custom-store"));
    }

    #[test]
    fn test_store_path_env_unset() {
        let _guard = StoreDirGuard::set(None);
        assert!(store_path().ends_with(".password-store"));
    }

    #[test]
    fn test_store_path_env_empty_uses_fallback() {
        let _guard = StoreDirGuard::set(Some(""));
        assert!(store_path().ends_with(".password-store"));
    }

    /// A folder X and a file X.gpg coexisting in the store: pass checks
    /// the file branch before the directory branch, so the folder
    /// deletion must carry a trailing '/' (see remove()'s doc comment).
    /// Requires `pass` on PATH.
    #[test]
    fn test_remove_folder_vs_file_with_same_name() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let store = dir.path().join("store");
        std::fs::create_dir_all(store.join("X")).expect("create folder X");
        std::fs::write(store.join("X/inner.gpg"), "secret").expect("write inner entry");
        std::fs::write(store.join("X.gpg"), "secret").expect("write X.gpg");

        let _guard = StoreDirGuard::set(Some(store.to_str().unwrap()));

        // Folder deletion must remove the folder, not the coexisting
        // X.gpg file.
        PassCommand::remove("X", true).expect("remove folder");
        assert!(!store.join("X").exists(), "folder X should be deleted");
        assert!(store.join("X.gpg").exists(), "file X.gpg should survive");

        // File deletion removes the file.
        PassCommand::remove("X", false).expect("remove file");
        assert!(
            !store.join("X.gpg").exists(),
            "file X.gpg should be deleted"
        );
    }

    #[test]
    fn test_plausible_gpg_id_edge_cases() {
        // Valid shapes, repeated from the request for completeness.
        assert!(is_plausible_gpg_id("user@example.com"));
        assert!(is_plausible_gpg_id(
            "1234567890ABCDEF1234567890ABCDEF12345678"
        ));
        assert!(is_plausible_gpg_id("12345678"));
        assert!(is_plausible_gpg_id("1234567890ABCDEF"));
        // Invalid: empty, plain words, and hyphenated non-ids.
        assert!(!is_plausible_gpg_id(""));
        assert!(!is_plausible_gpg_id("not-an-id"));
        // A control character anywhere in the raw input is rejected,
        // even when the trimmed value would look like a valid email.
        assert!(!is_plausible_gpg_id("user@example.com\u{7}"));
        assert!(!is_plausible_gpg_id("us\ter@example.com"));
    }
}

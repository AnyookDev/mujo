//! Shared helpers for the integration tests: an isolated password store
//! with a fresh GPG key, created in a temp directory.

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Tests in this binary mutate process-global environment variables
/// (HOME, GNUPGHOME, PASSWORD_STORE_DIR, GIT_CONFIG_*), so every test that
/// touches the environment must hold this lock for its whole duration.
pub fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// An isolated password store backed by a real GPG key, plus the
/// environment lock keeping other env-mutating tests out.
pub struct TestStore {
    /// Root of the temp directory (also the test's HOME).
    root: PathBuf,
    /// The password store directory (PASSWORD_STORE_DIR).
    pub store: PathBuf,
    /// The store's private GPG home (GNUPGHOME). Kept for debugging and
    /// future tests that need to inspect the keyring directly.
    #[allow(dead_code)]
    pub gnupghome: PathBuf,
    /// Key id of the generated key (written into the store's .gpg-id).
    pub gpg_key_id: String,
    _dir: tempfile::TempDir,
    _lock: MutexGuard<'static, ()>,
}

impl TestStore {
    /// Set up a fresh password store with a generated GPG key. Returns
    /// None (after printing the reason) when `pass`/`gpg` are unavailable
    /// or the environment cannot run them, so tests skip gracefully.
    pub fn new() -> Option<Self> {
        let lock = env_lock();
        match Self::setup() {
            Ok((dir, root, store, gnupghome, key_id)) => Some(Self {
                root,
                store,
                gnupghome,
                gpg_key_id: key_id,
                _dir: dir,
                _lock: lock,
            }),
            Err(e) => {
                eprintln!("skipping integration test: {}", e);
                None
            }
        }
    }

    /// The temp root, for creating sibling directories (e.g. a bare remote).
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn setup() -> Result<(tempfile::TempDir, PathBuf, PathBuf, PathBuf, String)> {
        if !available("pass") {
            bail!("`pass` is not installed; cannot run pass integration test");
        }
        if !available("gpg") {
            bail!("`gpg` is not installed; cannot run pass integration test");
        }

        let dir = tempfile::TempDir::new()?;
        let root = dir.path().to_path_buf();
        let home = root.join("home");
        let store = home.join(".password-store");
        let gnupghome = home.join(".gnupg");
        std::fs::create_dir_all(&store)?;
        std::fs::create_dir_all(&gnupghome)?;

        // gpg refuses a group-writable GNUPGHOME.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&gnupghome, std::fs::Permissions::from_mode(0o700))?;
        }

        // Isolate git from the developer's global/system config (a global
        // commit.gpgsign, a different default branch, etc.).
        let gitconfig = root.join("gitconfig");
        std::fs::write(&gitconfig, "")?;
        std::env::set_var("GIT_CONFIG_GLOBAL", &gitconfig);
        std::env::set_var("GIT_CONFIG_SYSTEM", &gitconfig);

        // Point HOME, gpg and pass at the temp directories.
        std::env::set_var("HOME", &home);
        std::env::set_var("GNUPGHOME", &gnupghome);
        std::env::set_var("PASSWORD_STORE_DIR", &store);

        // Generate a throwaway RSA key (no passphrase) for encryption.
        let batch = root.join("keygen.batch");
        std::fs::write(
            &batch,
            "%no-protection\n\
             Key-Type: RSA\n\
             Key-Length: 2048\n\
             Name-Real: mujo test\n\
             Name-Email: mujo@example.com\n\
             Expire-Date: 0\n\
             %commit\n",
        )?;
        let out = Command::new("gpg")
            .args(["--batch", "--gen-key"])
            .arg(&batch)
            .output()?;
        if !out.status.success() {
            bail!(
                "gpg key generation failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }

        let key_id = secret_key_id(&gnupghome)?;

        // pass init writes .gpg-id so subsequent insert/show encrypt to the
        // generated key.
        let out = Command::new("pass").arg("init").arg(&key_id).output()?;
        if !out.status.success() {
            bail!(
                "pass init failed: {}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }

        Ok((dir, root, store, gnupghome, key_id))
    }
}

fn available(bin: &str) -> bool {
    Command::new(bin).arg("--version").output().is_ok()
}

/// Extract the key id of the first secret key in the given GPG home from
/// `gpg --with-colons` output ("sec:<...>:<...>:<...>:<keyid>:...").
fn secret_key_id(gnupghome: &Path) -> Result<String> {
    let out = Command::new("gpg")
        .env("GNUPGHOME", gnupghome)
        .args(["--batch", "--with-colons", "--list-secret-keys"])
        .output()?;
    if !out.status.success() {
        bail!(
            "gpg --list-secret-keys failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let fields: Vec<&str> = line.split(':').collect();
        if fields[0] == "sec" && fields.len() > 4 && !fields[4].is_empty() {
            return Ok(fields[4].to_string());
        }
    }
    bail!("no secret key found after generation")
}

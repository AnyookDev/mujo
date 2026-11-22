//! Test-only helpers shared across modules.
use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Serialises tests that mutate process-global environment variables.
/// Non-reentrant: a test must not hold two guards at once.
pub(crate) fn lock() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Set environment variables for the duration of a test, restoring the
/// previous values on drop. Holds the shared environment lock; the lock
/// is non-reentrant, so never hold two guards at once.
pub(crate) struct EnvGuard {
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    pub(crate) fn set(vars: &[(&'static str, Option<&str>)]) -> Self {
        let guard = lock();
        let saved = vars
            .iter()
            .map(|(k, _)| (*k, std::env::var_os(k)))
            .collect();
        for (k, v) in vars {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
        EnvGuard {
            saved,
            _lock: guard,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, v) in self.saved.drain(..) {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
    }
}

//! Test-only helpers shared across modules.
use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Serialises tests that mutate process-global environment variables.
/// Non-reentrant: a test must not hold two guards at once.
pub(crate) fn lock() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

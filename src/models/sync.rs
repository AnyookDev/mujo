use std::time::Instant;

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

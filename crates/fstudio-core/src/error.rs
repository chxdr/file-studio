//! The error type for core calls.

/// Everything a core call can fail with. Per-file operation errors are not
/// here: they arrive as [`crate::CoreEvent::OpFailed`] events.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// The database failed.
    #[error("database: {0}")]
    Db(#[from] fstudio_db::DbError),
    /// The operation queue refused a request.
    #[error("{0}")]
    Ops(#[from] fstudio_ops::OpsError),
    /// A path-engine call failed (frecency storage, zoxide import).
    #[error("{0}")]
    Path(#[from] fstudio_path::PathError),
    /// A filesystem call failed.
    #[error("{0}")]
    Fs(#[from] fstudio_fs::Error),
    /// Settings or session data could not be encoded or decoded.
    #[error("data: {0}")]
    Json(#[from] serde_json::Error),
    /// A setting is out of range or inconsistent.
    #[error("invalid setting: {0}")]
    InvalidSetting(String),
    /// A name the user typed can't be used (empty, contains `/`, too long).
    #[error("{0}")]
    InvalidName(String),
    /// No tab, pane, window or workspace with that id.
    #[error("not found: {0}")]
    NotFound(String),
    /// The command id is not registered.
    #[error("unknown command: {0}")]
    UnknownCommand(String),
    /// The command can't run right now (nothing selected, single pane…).
    #[error("{0}")]
    NotAvailable(String),
    /// An I/O error outside any one file.
    #[error("{}", fstudio_fs::plain_error(.0))]
    Io(#[from] std::io::Error),
}

/// Result alias for core calls.
pub type Result<T> = std::result::Result<T, CoreError>;

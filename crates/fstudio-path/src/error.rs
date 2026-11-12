//! Errors for the few fallible calls in this crate (database and import).

/// Everything that can go wrong in `fstudio-path`. Parsing and completion never fail.
#[derive(Debug, thiserror::Error)]
pub enum PathError {
    /// The database refused a read or write.
    #[error("database: {0}")]
    Db(#[from] fstudio_db::DbError),
    /// A file could not be read.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// A zoxide database or `query -ls` text could not be understood.
    #[error("zoxide import: {0}")]
    Zoxide(String),
}

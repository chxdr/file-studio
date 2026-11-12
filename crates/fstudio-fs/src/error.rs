//! The error type for this crate.

use std::path::{Path, PathBuf};

/// Everything that can go wrong in `fstudio-fs`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An OS call failed for a path. The path is the one the caller passed in.
    #[error("{path}: {source}", path = path.display())]
    Io {
        /// The path the operation was about.
        path: PathBuf,
        /// The underlying OS error.
        #[source]
        source: std::io::Error,
    },
    /// A stored property list (Finder tags) could not be read or written.
    #[error("{path}: bad property list: {message}", path = path.display())]
    Plist {
        /// The path whose attribute was being read or written.
        path: PathBuf,
        /// What went wrong.
        message: String,
    },
    /// The operation is not available on this platform.
    #[error("not supported on this platform: {0}")]
    Unsupported(&'static str),
}

impl Error {
    /// Wraps an I/O error together with the path it happened on.
    pub fn io(path: &Path, source: std::io::Error) -> Self {
        Error::Io {
            path: path.to_path_buf(),
            source,
        }
    }

    /// The OS error kind, when this is an I/O error.
    pub fn kind(&self) -> Option<std::io::ErrorKind> {
        match self {
            Error::Io { source, .. } => Some(source.kind()),
            _ => None,
        }
    }
}

/// Shorthand for results in this crate.
pub type Result<T> = std::result::Result<T, Error>;

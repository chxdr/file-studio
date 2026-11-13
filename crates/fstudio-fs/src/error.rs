//! The error type for this crate.

use std::path::{Path, PathBuf};

/// Everything that can go wrong in `fstudio-fs`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An OS call failed for a path. The path is the one the caller passed in.
    #[error("{path}: {}", plain_error(source), path = path.display())]
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

/// What went wrong, in plain words, never "(os error 13)": "permission
/// denied", "the disk isn't connected any more". Errors with no wording of
/// their own keep the system's sentence without the error number. The
/// phrases for access problems ("permission denied", "not permitted") are
/// the ones the shell's folder-error view recognises.
pub fn plain_error(e: &std::io::Error) -> String {
    let words = match e.raw_os_error() {
        Some(libc::EACCES) => "permission denied",
        Some(libc::EPERM) => {
            "the operation is not permitted (macOS privacy protection may be blocking it)"
        }
        Some(libc::ENOENT) => "the item no longer exists",
        Some(libc::ENOTDIR) => "a part of the path isn’t a folder",
        Some(libc::ELOOP) => "a symlink leads back to itself (a symlink loop)",
        Some(libc::ENOSPC) => "there isn’t enough free space",
        Some(libc::EDQUOT) => "the disk quota is exceeded",
        Some(libc::EROFS) => "the volume is read-only",
        Some(libc::ENAMETOOLONG) => "the name is too long",
        Some(libc::EEXIST) => "an item with that name already exists",
        Some(libc::ENOTEMPTY) => "the folder isn’t empty",
        Some(libc::EBUSY) => "it is in use",
        Some(libc::EIO) => "the disk couldn’t be read or written (it may have been disconnected)",
        Some(libc::ENXIO) | Some(libc::ENODEV) => "the disk isn’t connected any more",
        Some(libc::ETIMEDOUT) => "the server didn’t answer in time",
        Some(libc::EXDEV) => "the item is on another volume",
        Some(libc::EFBIG) => "the file is too large for this volume",
        Some(libc::EMFILE) | Some(libc::ENFILE) => "too many files are open",
        _ => "",
    };
    if !words.is_empty() {
        return words.to_string();
    }
    let s = e.to_string();
    match s.rfind(" (os error ") {
        Some(i) if s.ends_with(')') => s[..i].to_string(),
        _ => s,
    }
}

#[cfg(test)]
mod plain_tests {
    use super::*;

    #[test]
    fn errors_are_in_plain_words() {
        let e = |n| std::io::Error::from_raw_os_error(n);
        assert_eq!(plain_error(&e(libc::EACCES)), "permission denied");
        assert_eq!(
            plain_error(&e(libc::ENXIO)),
            "the disk isn’t connected any more"
        );
        assert!(plain_error(&e(libc::EPERM)).contains("not permitted"));
        // No wording of our own: the system's sentence, no error number.
        let other = plain_error(&e(libc::EDEADLK));
        assert!(!other.contains("os error"), "{other}");
        assert!(!other.is_empty());
        let f = Error::io(Path::new("/x/locked"), e(libc::EACCES)).to_string();
        assert_eq!(f, "/x/locked: permission denied");
    }
}

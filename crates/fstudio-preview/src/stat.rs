//! The `stat` facts a preview starts from.

use std::io;
use std::path::Path;

/// `SF_DATALESS`: an iCloud / File Provider placeholder (see
/// `fstudio_model::file_flags::DATALESS`).
pub const SF_DATALESS: u32 = fstudio_model::file_flags::DATALESS;

/// What `lstat` says about the path. Symlinks are described, not followed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStat {
    /// Logical size in bytes.
    pub size: u64,
    /// Modification time, ns since the Unix epoch.
    pub mtime_ns: i64,
    /// Creation time, ns since the Unix epoch, if the filesystem has one.
    pub btime_ns: Option<i64>,
    /// Access time, ns since the Unix epoch.
    pub atime_ns: Option<i64>,
    /// A directory.
    pub is_dir: bool,
    /// A symbolic link.
    pub is_symlink: bool,
    /// The content is an iCloud placeholder (`SF_DATALESS`): never read it.
    pub dataless: bool,
    /// The `st_dev:st_ino` pair, for telling files apart.
    pub inode: u64,
}

impl FileStat {
    /// `lstat`s `path`.
    pub fn of(path: &Path) -> io::Result<FileStat> {
        let md = std::fs::symlink_metadata(path)?;
        let mut st = Self::from_metadata(&md);
        if st.is_symlink {
            // Describe what the link points at; a broken or looping link
            // keeps its own facts.
            if let Ok(target) = std::fs::metadata(path) {
                st = Self::from_metadata(&target);
                st.is_symlink = true;
            }
        }
        Ok(st)
    }

    /// Reads the fields out of metadata.
    pub fn from_metadata(md: &std::fs::Metadata) -> FileStat {
        use std::os::unix::fs::MetadataExt;
        let ns = |s: i64, n: i64| s.saturating_mul(1_000_000_000).saturating_add(n);
        #[cfg(target_os = "macos")]
        let (flags, btime) = {
            use std::os::macos::fs::MetadataExt as Mac;
            (
                Mac::st_flags(md),
                Some(ns(Mac::st_birthtime(md), Mac::st_birthtime_nsec(md))),
            )
        };
        #[cfg(not(target_os = "macos"))]
        let (flags, btime) = (0u32, None);
        FileStat {
            size: md.len(),
            mtime_ns: ns(md.mtime(), md.mtime_nsec()),
            btime_ns: btime,
            atime_ns: Some(ns(md.atime(), md.atime_nsec())),
            is_dir: md.is_dir(),
            is_symlink: md.file_type().is_symlink(),
            dataless: flags & SF_DATALESS != 0,
            inode: md.ino(),
        }
    }
}

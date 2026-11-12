//! Small, safe metadata helpers on top of `lstat` (symlinks are never followed).

use std::fs::Metadata;
use std::io;
use std::os::macos::fs::MetadataExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

use fstudio_model::{EntryKind, file_flags};
use serde::{Deserialize, Serialize};

/// What identifies an item for "has it changed since?" checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ident {
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    pub mtime_ns: i64,
}

impl Ident {
    /// The same filesystem object (device and inode).
    pub fn same_object(&self, other: &Ident) -> bool {
        self.dev == other.dev && self.ino == other.ino
    }
    /// The same object, with the same size and modification time.
    pub fn unchanged(&self, other: &Ident) -> bool {
        self.same_object(other) && self.size == other.size && self.mtime_ns == other.mtime_ns
    }
}

/// `lstat`.
pub fn lstat(p: &Path) -> io::Result<Metadata> {
    std::fs::symlink_metadata(p)
}

/// Whether anything (including a dangling symlink) exists at `p`.
pub fn exists(p: &Path) -> bool {
    lstat(p).is_ok()
}

/// The entry kind, as `lstat` sees it.
pub fn kind_of(md: &Metadata) -> EntryKind {
    let ft = md.file_type();
    if ft.is_symlink() {
        EntryKind::Symlink
    } else if ft.is_dir() {
        EntryKind::Dir
    } else if ft.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    }
}

/// Modification time in ns since the epoch.
pub fn mtime_ns(md: &Metadata) -> i64 {
    md.mtime() * 1_000_000_000 + md.mtime_nsec()
}

/// The identity of an item.
pub fn ident(md: &Metadata) -> Ident {
    Ident {
        dev: md.dev(),
        ino: md.ino(),
        size: md.len(),
        mtime_ns: mtime_ns(md),
    }
}

/// BSD `st_flags`.
pub fn flags(md: &Metadata) -> u32 {
    md.st_flags()
}

/// An iCloud / File Provider placeholder whose content is not local (F-016).
pub fn is_dataless(md: &Metadata) -> bool {
    md.st_flags() & file_flags::DATALESS != 0
}

/// Flags that stop an item from being renamed or removed.
pub const IMMUTABLE_FLAGS: u32 = file_flags::USER_IMMUTABLE | 0x0000_0004 /* UF_APPEND */;

/// A short name for an entry kind, used in the journal.
pub fn kind_str(k: EntryKind) -> &'static str {
    match k {
        EntryKind::File => "file",
        EntryKind::Dir => "dir",
        EntryKind::Symlink => "symlink",
        EntryKind::Other => "other",
    }
}

/// The inverse of [`kind_str`].
pub fn parse_kind(s: &str) -> EntryKind {
    match s {
        "file" => EntryKind::File,
        "dir" => EntryKind::Dir,
        "symlink" => EntryKind::Symlink,
        _ => EntryKind::Other,
    }
}

//! Reading one folder: every entry with the metadata the scanner needs.
//!
//! On macOS this is one `getattrlistbulk` loop ([`crate::sys`]); elsewhere,
//! or on filesystems without the bulk call, `read_dir` plus `lstat` per entry.
//! Neither opens a file's contents.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

/// `SF_DATALESS`: the item is an iCloud / File Provider placeholder whose
/// contents are not on this Mac (F-016).
pub const SF_DATALESS: u32 = 0x4000_0000;

/// What kind of item an entry is. Symlinks are never followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EntryKind {
    /// A regular file.
    #[default]
    File,
    /// A folder.
    Dir,
    /// A symbolic link (sized as the link itself).
    Symlink,
    /// Anything else (sockets, pipes, devices).
    Other,
}

/// One folder entry as read from disk.
#[derive(Debug, Clone, Default)]
pub struct RawEntry {
    /// The name exactly as stored (F-017).
    pub name: Vec<u8>,
    /// The kind.
    pub kind: EntryKind,
    /// The device id.
    pub dev: u64,
    /// The inode number.
    pub ino: u64,
    /// Hard link count (files only; 0 when unknown).
    pub nlink: u32,
    /// Logical size in bytes (all forks).
    pub logical: u64,
    /// Bytes allocated on disk.
    pub allocated: u64,
    /// Modification time, ns since the epoch.
    pub mtime_ns: i64,
    /// `st_flags`.
    pub flags: u32,
    /// The Finder "has bundle" bit.
    pub has_bundle: bool,
    /// An `errno` when the kernel could not read this entry's attributes.
    pub error: i32,
}

/// Opens a folder for reading without following a symlink in its place.
pub fn open_dir(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

/// Which way folders are read; [`Method::Auto`] is the fast default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Method {
    /// `getattrlistbulk` on macOS, falling back per folder when unsupported.
    #[default]
    Auto,
    /// `read_dir` plus `lstat` for each entry (portable; for comparison).
    Portable,
}

/// Reads every entry of the folder at `path` into `out`.
pub fn read_dir(path: &Path, method: Method, out: &mut Vec<RawEntry>) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    if method == Method::Auto {
        let dir = open_dir(path)?;
        match crate::sys::read_dir_bulk(&dir, out) {
            Ok(()) => return Ok(()),
            Err(e) if is_unsupported(&e) => {}
            Err(e) => return Err(e),
        }
    }
    let _ = method;
    read_dir_portable(path, out)
}

#[cfg(target_os = "macos")]
fn is_unsupported(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::ENOTSUP) | Some(libc::ENOSYS) | Some(libc::EINVAL)
    )
}

/// `read_dir` + `symlink_metadata` per entry.
pub fn read_dir_portable(path: &Path, out: &mut Vec<RawEntry>) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    // Open first with O_NOFOLLOW so a symlink swapped in is refused.
    drop(open_dir(path)?);
    for de in std::fs::read_dir(path)? {
        let de = de?;
        let name = de.file_name().as_bytes().to_vec();
        let mut e = RawEntry {
            name,
            ..RawEntry::default()
        };
        match de.path().symlink_metadata() {
            Ok(m) => fill_from_metadata(&mut e, &m),
            Err(err) => e.error = err.raw_os_error().unwrap_or(libc::EIO),
        }
        out.push(e);
    }
    Ok(())
}

/// Fills an entry from `lstat` metadata.
pub fn fill_from_metadata(e: &mut RawEntry, m: &std::fs::Metadata) {
    let ft = m.file_type();
    e.kind = if ft.is_dir() {
        EntryKind::Dir
    } else if ft.is_symlink() {
        EntryKind::Symlink
    } else if ft.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    };
    // `st_dev` is a 32-bit `dev_t` on macOS; keep it unsigned like the bulk call.
    #[cfg(target_os = "macos")]
    {
        e.dev = u64::from(m.dev() as u32);
    }
    #[cfg(not(target_os = "macos"))]
    {
        e.dev = m.dev();
    }
    e.ino = m.ino();
    e.nlink = u32::try_from(m.nlink()).unwrap_or(u32::MAX);
    if e.kind != EntryKind::Dir {
        e.logical = m.len();
        e.allocated = m.blocks().saturating_mul(512);
    }
    e.mtime_ns = m
        .mtime()
        .saturating_mul(1_000_000_000)
        .saturating_add(m.mtime_nsec());
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt as _;
        e.flags = m.st_flags();
    }
}

//! Linux stub. Builds, flushes with `fsync`, and answers "unsupported" for
//! everything that needs a real port (`renameat2(RENAME_NOREPLACE)`,
//! `posix_fadvise(DONTNEED)`, `/dev/disk/by-uuid`, the freedesktop Trash).
//! Locking (`flock`) and random ids (`getrandom`) are real.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use crate::platform::{FlushLevel, RenameMode, Residency, VolumeInfo};

fn unsupported<T>(what: &str) -> io::Result<T> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!("{what} isn't implemented on Linux yet"),
    ))
}

/// Takes an exclusive advisory lock on `f` without waiting (`flock`).
/// `Ok(false)` means another process holds it.
pub fn try_lock(f: &File) -> io::Result<bool> {
    use std::os::fd::AsRawFd;
    // SAFETY: flock on a descriptor we own, with integer flags only.
    let r = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if r == 0 {
        return Ok(true);
    }
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(false)
    } else {
        Err(e)
    }
}
/// Not yet: needs `renameat2(RENAME_NOREPLACE)`.
pub fn rename_excl(_from: &Path, _to: &Path) -> io::Result<RenameMode> {
    unsupported("exclusive rename")
}
/// `fsync` (Linux has no separate drive-cache flush call).
pub fn full_sync(f: &File) -> io::Result<FlushLevel> {
    f.sync_all()?;
    Ok(FlushLevel::OsOnly)
}
/// The OS-level flush without a drive barrier (here: the same as `full_sync`).
pub fn os_sync(f: &File) -> io::Result<()> {
    f.sync_all()
}
/// `fsync` on the folder.
pub fn sync_dir(dir: &Path) -> io::Result<FlushLevel> {
    full_sync(&File::open(dir)?)
}
/// Not yet: needs `posix_fadvise(DONTNEED)`.
pub fn set_nocache(_f: &File) -> io::Result<()> {
    unsupported("uncached I/O")
}
/// Not yet.
pub fn residency(_path: &Path) -> io::Result<Residency> {
    unsupported("cache residency")
}
/// Not yet.
pub fn evict(_path: &Path) -> io::Result<Residency> {
    unsupported("cache eviction")
}
/// Not yet.
pub fn evict_open(_f: &File) -> io::Result<Residency> {
    unsupported("cache eviction")
}
/// Holes are not detected yet: the whole file is data.
pub fn seek_data(_f: &File, off: u64) -> io::Result<Option<u64>> {
    Ok(Some(off))
}
/// Holes are not detected yet: the end of the file is the only hole.
pub fn seek_hole(f: &File, _off: u64) -> io::Result<Option<u64>> {
    Ok(Some(f.metadata()?.len()))
}
/// Not yet: needs `ioctl(FICLONE)`.
pub fn clone_file(_src: &Path, _dst: &Path) -> io::Result<()> {
    unsupported("cloning")
}
/// Not yet: needs xattr copies.
pub fn copy_metadata(_src: &Path, _dst: &Path) -> io::Result<()> {
    unsupported("metadata copies")
}
/// Not yet.
pub fn copy_acl(_src: &Path, _dst: &Path) -> io::Result<()> {
    unsupported("ACL copies")
}
/// Not yet.
pub fn set_flags(_path: &Path, _flags: u32) -> io::Result<()> {
    unsupported("file flags")
}
/// Linux has no BSD flags.
pub fn file_flags(_md: &std::fs::Metadata) -> u32 {
    0
}
/// Not yet (`statx`).
pub fn birth_ns(_md: &std::fs::Metadata) -> Option<i64> {
    None
}
/// Not yet.
pub fn has_acl(_path: &Path) -> bool {
    false
}
/// Not yet.
pub fn clear_acl(_path: &Path) -> io::Result<()> {
    Ok(())
}
/// Not yet.
pub fn xattr_names(_path: &Path) -> io::Result<Vec<String>> {
    Ok(Vec::new())
}
/// Linux filesystems are case-sensitive by default.
pub fn case_sensitive(_dir: &Path) -> io::Result<bool> {
    Ok(true)
}
/// Fills `buf` with random bytes from the kernel (`getrandom`).
pub fn random_bytes(buf: &mut [u8]) -> io::Result<()> {
    // SAFETY: `buf` is writable for `buf.len()` bytes.
    let n = unsafe { libc::getrandom(buf.as_mut_ptr().cast(), buf.len(), 0) };
    if n == buf.len() as isize {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
/// Not yet: needs `statfs` and `/dev/disk/by-uuid`.
pub fn volume_info(_path: &Path) -> io::Result<VolumeInfo> {
    unsupported("volume identity")
}
/// Not yet.
pub fn mounted_volumes() -> io::Result<Vec<VolumeInfo>> {
    unsupported("volume listing")
}
/// Not yet: the freedesktop Trash spec.
pub fn trash_item(_path: &Path) -> io::Result<PathBuf> {
    unsupported("the Trash")
}

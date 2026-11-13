//! Windows stub. Builds, flushes with `FlushFileBuffers` (via `sync_all`), and
//! answers "unsupported" for everything that needs a real port (`MoveFileExW`
//! without `REPLACE_EXISTING`, `FILE_FLAG_NO_BUFFERING`, `GetVolumeInformationW`).
//! Nothing here is unsafe yet.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use crate::platform::{FlushLevel, RenameMode, Residency, VolumeInfo};

fn unsupported<T>(what: &str) -> io::Result<T> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!("{what} isn't implemented on Windows yet"),
    ))
}

/// Not yet: needs `MoveFileExW` without `REPLACE_EXISTING`.
pub fn rename_excl(_from: &Path, _to: &Path) -> io::Result<RenameMode> {
    unsupported("exclusive rename")
}
/// `fsync` (`FlushFileBuffers` reaches the drive).
pub fn full_sync(f: &File) -> io::Result<FlushLevel> {
    f.sync_all()?;
    Ok(FlushLevel::Device)
}
/// `fsync` on the folder.
pub fn sync_dir(dir: &Path) -> io::Result<FlushLevel> {
    full_sync(&File::open(dir)?)
}
/// Not yet: needs `FILE_FLAG_NO_BUFFERING`.
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
/// Holes are not detected yet: the whole file is data.
pub fn seek_data(_f: &File, off: u64) -> io::Result<Option<u64>> {
    Ok(Some(off))
}
/// Holes are not detected yet: the end of the file is the only hole.
pub fn seek_hole(f: &File, _off: u64) -> io::Result<Option<u64>> {
    Ok(Some(f.metadata()?.len()))
}
/// Not yet: needs ReFS block cloning.
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
/// Windows has no BSD flags.
pub fn file_flags(_md: &std::fs::Metadata) -> u32 {
    0
}
/// Not yet (`GetFileInformationByHandleEx`).
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
/// NTFS is case-insensitive by default.
pub fn case_sensitive(_dir: &Path) -> io::Result<bool> {
    Ok(false)
}
/// Not yet: needs `BCryptGenRandom`.
pub fn random_bytes(_buf: &mut [u8]) -> io::Result<()> {
    unsupported("random ids")
}
/// Not yet: needs `GetVolumeInformationW`.
pub fn volume_info(_path: &Path) -> io::Result<VolumeInfo> {
    unsupported("volume identity")
}
/// Not yet.
pub fn mounted_volumes() -> io::Result<Vec<VolumeInfo>> {
    unsupported("volume listing")
}
/// Not yet: `IFileOperation` to the Recycle Bin.
pub fn trash_item(_path: &Path) -> io::Result<PathBuf> {
    unsupported("the Trash")
}

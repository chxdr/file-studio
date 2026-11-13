//! The only place in this crate that uses `unsafe`: thin, documented wrappers
//! over libc and macOS system calls. Everything above this module is safe Rust.
#![allow(unsafe_code)]

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

#[cfg(target_os = "macos")]
pub mod bulk;
#[cfg(target_os = "macos")]
pub mod fsevents;
pub mod volume;
pub mod xattr;

/// Converts a path to a NUL-terminated C string.
pub fn cpath(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))
}

#[cfg(target_os = "macos")]
/// `fstatat(dirfd, name, AT_SYMLINK_NOFOLLOW)`: stats `name` inside an open directory without following links.
pub fn lstat_at(dirfd: i32, name: &[u8]) -> io::Result<libc::stat> {
    let c = CString::new(name)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name contains a NUL byte"))?;
    // SAFETY: a zeroed `stat` is a valid out-buffer; `c` is NUL-terminated and outlives the
    // call; `dirfd` is a valid directory descriptor owned by the caller for the call's duration.
    unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        if libc::fstatat(dirfd, c.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) == 0 {
            Ok(st)
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

/// `lstat(path)`: stats a path without following a final symlink.
pub fn lstat(path: &Path) -> io::Result<libc::stat> {
    let c = cpath(path)?;
    // SAFETY: zeroed `stat` is a valid out-buffer; `c` is a valid NUL-terminated path.
    unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        if libc::lstat(c.as_ptr(), &mut st) == 0 {
            Ok(st)
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

/// The name of the item at `path` exactly as the volume stores it
/// (`getattrlist(ATTR_CMN_NAME, FSOPT_NOFOLLOW)`). On a case- or
/// normalisation-insensitive volume, `lstat("…/notes.txt")` succeeds for a
/// file stored as `Notes.txt`; this returns `Notes.txt`.
#[cfg(target_os = "macos")]
pub fn name_on_disk(path: &Path) -> io::Result<Vec<u8>> {
    let c = cpath(path)?;
    let mut al = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: libc::ATTR_CMN_NAME,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    // u32 length, then an attrreference_t, then the name (NAME_MAX UTF-8 bytes + NUL).
    let mut buf = [0u8; 4 + 8 + 1024 + 8];
    // SAFETY: `c` is NUL-terminated and outlives the call; `al` is a valid
    // attrlist; `buf` is writable for `buf.len()` bytes, which we pass.
    let r = unsafe {
        libc::getattrlist(
            c.as_ptr(),
            (&mut al as *mut libc::attrlist).cast(),
            buf.as_mut_ptr().cast(),
            buf.len(),
            libc::FSOPT_NOFOLLOW,
        )
    };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    let bad = || io::Error::new(io::ErrorKind::InvalidData, "getattrlist: bad name record");
    let total = u32::from_ne_bytes(buf[0..4].try_into().map_err(|_| bad())?) as usize;
    let off = i32::from_ne_bytes(buf[4..8].try_into().map_err(|_| bad())?);
    let len = u32::from_ne_bytes(buf[8..12].try_into().map_err(|_| bad())?) as usize;
    // The offset is relative to the attrreference itself (at byte 4).
    let start = 4usize
        .checked_add(usize::try_from(off).map_err(|_| bad())?)
        .ok_or_else(bad)?;
    let end = start.checked_add(len).ok_or_else(bad)?;
    if end > total.min(buf.len()) || len == 0 {
        return Err(bad());
    }
    // `len` counts the trailing NUL.
    let name = &buf[start..end - 1];
    Ok(name.to_vec())
}

/// Elsewhere the name in the path is the name on disk.
#[cfg(not(target_os = "macos"))]
pub fn name_on_disk(path: &Path) -> io::Result<Vec<u8>> {
    lstat(path)?;
    Ok(path
        .file_name()
        .map(|n| n.as_bytes().to_vec())
        .unwrap_or_default())
}

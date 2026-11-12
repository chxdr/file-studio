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

//! The only place in this crate that talks to the OS directly (`statfs`).
#![allow(unsafe_code)]

use std::path::Path;

/// True when `path` is on a network or otherwise non-local volume (SMB, NFS, AFP,
/// WebDAV, FTP...). Listing such a folder on a keystroke could block, so completion
/// skips it. Returns false when it cannot tell (including on non-macOS targets).
#[cfg(target_os = "macos")]
pub fn is_non_local_volume(path: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `statfs` is a plain C struct of integers and byte arrays; all-zero is valid.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated string and `st` is a valid out-pointer.
    let rc = unsafe { libc::statfs(c.as_ptr(), &mut st) };
    if rc != 0 {
        return false;
    }
    (st.f_flags & libc::MNT_LOCAL as u32) == 0
}

/// Non-macOS stub: nothing is treated as slow.
#[cfg(not(target_os = "macos"))]
pub fn is_non_local_volume(_path: &Path) -> bool {
    false
}

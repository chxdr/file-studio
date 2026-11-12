//! Extended attributes. Symlinks are never followed: the attribute belongs to the link itself.

use super::cpath;
use std::ffi::CString;
use std::io;
use std::path::Path;

fn cname(name: &str) -> io::Result<CString> {
    CString::new(name)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "attribute name contains NUL"))
}

#[cfg(target_os = "macos")]
const NOFOLLOW: i32 = libc::XATTR_NOFOLLOW;

/// Lists the attribute names on `path` (raw NUL-separated bytes).
pub fn list_raw(path: &Path) -> io::Result<Vec<u8>> {
    let c = cpath(path)?;
    loop {
        // SAFETY: `c` is a valid path; a NULL buffer with size 0 asks for the needed length.
        #[cfg(target_os = "macos")]
        let need = unsafe { libc::listxattr(c.as_ptr(), std::ptr::null_mut(), 0, NOFOLLOW) };
        #[cfg(not(target_os = "macos"))]
        let need = unsafe { libc::llistxattr(c.as_ptr(), std::ptr::null_mut(), 0) };
        if need < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buf = vec![0u8; need as usize];
        if buf.is_empty() {
            return Ok(buf);
        }
        // SAFETY: `buf` is writable for `buf.len()` bytes.
        #[cfg(target_os = "macos")]
        let got =
            unsafe { libc::listxattr(c.as_ptr(), buf.as_mut_ptr().cast(), buf.len(), NOFOLLOW) };
        #[cfg(not(target_os = "macos"))]
        let got = unsafe { libc::llistxattr(c.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
        if got < 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::ERANGE) {
                continue; // the list grew between the two calls
            }
            return Err(e);
        }
        buf.truncate(got as usize);
        return Ok(buf);
    }
}

/// Reads one attribute. `Ok(None)` when the attribute does not exist.
pub fn get(path: &Path, name: &str) -> io::Result<Option<Vec<u8>>> {
    let c = cpath(path)?;
    let n = cname(name)?;
    loop {
        // SAFETY: valid NUL-terminated strings; NULL buffer with size 0 asks for the length.
        #[cfg(target_os = "macos")]
        let need =
            unsafe { libc::getxattr(c.as_ptr(), n.as_ptr(), std::ptr::null_mut(), 0, 0, NOFOLLOW) };
        #[cfg(not(target_os = "macos"))]
        let need = unsafe { libc::lgetxattr(c.as_ptr(), n.as_ptr(), std::ptr::null_mut(), 0) };
        if need < 0 {
            let e = io::Error::last_os_error();
            return match e.raw_os_error() {
                #[cfg(target_os = "macos")]
                Some(libc::ENOATTR) => Ok(None),
                #[cfg(not(target_os = "macos"))]
                Some(libc::ENODATA) => Ok(None),
                _ => Err(e),
            };
        }
        let mut buf = vec![0u8; need as usize];
        if buf.is_empty() {
            return Ok(Some(buf));
        }
        // SAFETY: `buf` is writable for `buf.len()` bytes.
        #[cfg(target_os = "macos")]
        let got = unsafe {
            libc::getxattr(
                c.as_ptr(),
                n.as_ptr(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                0,
                NOFOLLOW,
            )
        };
        #[cfg(not(target_os = "macos"))]
        let got =
            unsafe { libc::lgetxattr(c.as_ptr(), n.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
        if got < 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::ERANGE) {
                continue;
            }
            return Err(e);
        }
        buf.truncate(got as usize);
        return Ok(Some(buf));
    }
}

/// Writes (creates or replaces) one attribute.
pub fn set(path: &Path, name: &str, value: &[u8]) -> io::Result<()> {
    let c = cpath(path)?;
    let n = cname(name)?;
    // SAFETY: valid strings; `value` is readable for `value.len()` bytes.
    #[cfg(target_os = "macos")]
    let r = unsafe {
        libc::setxattr(
            c.as_ptr(),
            n.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
            NOFOLLOW,
        )
    };
    #[cfg(not(target_os = "macos"))]
    let r = unsafe {
        libc::lsetxattr(
            c.as_ptr(),
            n.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
        )
    };
    if r == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Removes one attribute. `Ok(false)` when it did not exist.
pub fn remove(path: &Path, name: &str) -> io::Result<bool> {
    let c = cpath(path)?;
    let n = cname(name)?;
    // SAFETY: valid NUL-terminated strings.
    #[cfg(target_os = "macos")]
    let r = unsafe { libc::removexattr(c.as_ptr(), n.as_ptr(), NOFOLLOW) };
    #[cfg(not(target_os = "macos"))]
    let r = unsafe { libc::lremovexattr(c.as_ptr(), n.as_ptr()) };
    if r == 0 {
        return Ok(true);
    }
    let e = io::Error::last_os_error();
    match e.raw_os_error() {
        #[cfg(target_os = "macos")]
        Some(libc::ENOATTR) => Ok(false),
        #[cfg(not(target_os = "macos"))]
        Some(libc::ENODATA) => Ok(false),
        _ => Err(e),
    }
}

//! Generic extended attributes. Symlinks are not followed.

use crate::error::{Error, Result};
use crate::sys::xattr as sx;
use std::path::Path;

/// The attribute names on `path`.
pub fn xattr_list(path: impl AsRef<Path>) -> Result<Vec<String>> {
    let path = path.as_ref();
    let raw = sx::list_raw(path).map_err(|e| Error::io(path, e))?;
    Ok(raw
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect())
}

/// One attribute's bytes, or `None` when it isn't set.
pub fn xattr_get(path: impl AsRef<Path>, name: &str) -> Result<Option<Vec<u8>>> {
    let path = path.as_ref();
    sx::get(path, name).map_err(|e| Error::io(path, e))
}

/// Sets (creates or replaces) an attribute.
pub fn xattr_set(path: impl AsRef<Path>, name: &str, value: &[u8]) -> Result<()> {
    let path = path.as_ref();
    sx::set(path, name, value).map_err(|e| Error::io(path, e))
}

/// Removes an attribute. Returns `false` if it wasn't there.
pub fn xattr_remove(path: impl AsRef<Path>, name: &str) -> Result<bool> {
    let path = path.as_ref();
    sx::remove(path, name).map_err(|e| Error::io(path, e))
}

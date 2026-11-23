//! Stand-ins for `sys.rs` where NetFS, dns_sd and the BSD mount table don't
//! exist (the Linux core build): nothing mounts, nothing is found, and no
//! network volumes are listed. Same names and shapes as the macOS module.

use std::ffi::c_int;
use std::io;
use std::path::PathBuf;

/// A pending mount (never made here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MountRequest(usize);

/// Mounting isn't supported here: always `ENOTSUP`, and `done` never runs.
pub(crate) fn mount_async(
    _url: &str,
    _done: Box<dyn FnOnce(c_int, Vec<PathBuf>) + Send>,
) -> Result<MountRequest, c_int> {
    Err(libc::ENOTSUP)
}

/// Nothing to cancel.
pub(crate) fn cancel_mount(_req: MountRequest) {}

/// One answer from a Bonjour browse (none arrive here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowseReply {
    /// Appeared (`true`) or went away.
    pub added: bool,
    /// The service's name.
    pub name: String,
    /// Its domain.
    pub domain: String,
    /// The network interface it was seen on.
    pub interface: u32,
}

/// A browse; it can't start here.
pub(crate) struct DnsBrowse;

impl DnsBrowse {
    /// Always fails: there is no dns_sd (`kDNSServiceErr_Unsupported`).
    pub(crate) fn start(_service: &str) -> Result<DnsBrowse, i32> {
        Err(-65544)
    }
    /// Never reached (a browse can't start), but kept for the same shape.
    pub(crate) fn poll(&mut self, _timeout_ms: i32) -> Result<Vec<BrowseReply>, i32> {
        Ok(Vec::new())
    }
}

/// One mounted filesystem.
#[derive(Debug, Clone)]
pub(crate) struct Mount {
    pub mount_on: PathBuf,
    pub mount_from: String,
    pub fs_type: String,
    /// Not a network volume.
    pub local: bool,
}

/// No mount table to read here: an empty list.
pub(crate) fn mounts() -> io::Result<Vec<Mount>> {
    Ok(Vec::new())
}

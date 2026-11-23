//! Mounting a share with macOS's own NetFS.
//!
//! The system does the sign-in: when a share needs a password, NetFS's
//! agent shows the standard window (with "Remember this password in my
//! keychain"), and for an address without a share it shows the share
//! picker. File Studio never sees the password.
//!
//! The mount is soft (NetFS's default): when the server goes away, calls
//! on the volume fail instead of hanging the app.

use std::ffi::c_int;
use std::fmt;
use std::path::PathBuf;

use crate::sys;

/// Why a mount didn't happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountError {
    /// The user pressed Cancel in the sign-in window or share picker.
    Cancelled,
    /// The name or password was refused, or the account may not sign in.
    SignIn,
    /// The server couldn't be reached (off, asleep, wrong name).
    Unreachable,
    /// The server has no share by that name.
    NoSuchShare,
    /// The server offers no shares to this account.
    NoShares,
    /// The server doesn't speak a version of SMB or sign-in that macOS does.
    Unsupported,
    /// The address isn't a URL NetFS can use.
    BadAddress,
    /// Anything else: NetFS's status (an errno, or a negative `OSStatus`).
    Other(i32),
}

/// BSD's "authentication error", which NetFS reports for a wrong password.
/// Linux's libc doesn't define it; there NetFS never reports anything.
#[cfg(target_os = "macos")]
const EAUTH: i32 = libc::EAUTH;
#[cfg(not(target_os = "macos"))]
const EAUTH: i32 = 80;

impl MountError {
    /// The error for a NetFS status, or `None` for success. `EEXIST` (the
    /// share is already mounted) counts as success.
    pub fn from_status(status: i32) -> Option<MountError> {
        Some(match status {
            0 | libc::EEXIST => return None,
            -128 | libc::ECANCELED => MountError::Cancelled,
            EAUTH | libc::EPERM | libc::EACCES | -5045 | -5046 | -5999 => MountError::SignIn,
            libc::ENOENT => MountError::NoSuchShare,
            -5998 | -6003 => MountError::NoShares,
            libc::EHOSTUNREACH
            | libc::EHOSTDOWN
            | libc::ETIMEDOUT
            | libc::ECONNREFUSED
            | libc::ENETUNREACH
            | libc::ENETDOWN => MountError::Unreachable,
            libc::ENOTSUP | libc::EPROTONOSUPPORT | -5997 | -5996 | -6004 => {
                MountError::Unsupported
            }
            libc::EINVAL => MountError::BadAddress,
            other => MountError::Other(other),
        })
    }

    /// A sentence for the user about connecting to `what` ("media on
    /// archbox"). `None` for [`MountError::Cancelled`], which needs none.
    pub fn message(&self, what: &str) -> Option<String> {
        Some(match self {
            MountError::Cancelled => return None,
            MountError::SignIn => {
                format!("Couldn’t sign in to “{what}”. Check the name and password.")
            }
            MountError::Unreachable => format!(
                "Couldn’t reach “{what}”. Check that the computer is on and on this network."
            ),
            MountError::NoSuchShare => format!("“{what}” has no share by that name."),
            MountError::NoShares => format!("“{what}” offers no shares to this account."),
            MountError::Unsupported => {
                format!("“{what}” uses a kind of SMB or sign-in that this Mac doesn’t support.")
            }
            MountError::BadAddress => {
                format!("“{what}” isn’t an address that can be connected to.")
            }
            MountError::Other(code) => format!("Couldn’t connect to “{what}” (error {code})."),
        })
    }
}

impl fmt::Display for MountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.message("the server") {
            Some(m) => f.write_str(&m),
            None => f.write_str("Cancelled"),
        }
    }
}

impl std::error::Error for MountError {}

/// A mount in progress. Dropping it does not cancel the mount.
#[derive(Debug)]
pub struct MountHandle {
    req: sys::MountRequest,
}

impl MountHandle {
    /// Stops waiting for the mount (the sign-in window closes); its
    /// completion is not called.
    pub fn cancel(&self) {
        sys::cancel_mount(self.req);
    }
}

/// Starts mounting `url` (a canonical `smb://` URL from
/// [`crate::ServerAddress::url`]). `done` runs once, on a system dispatch
/// queue (not the caller's thread), with the mount points (usually one;
/// none when the share was already mounted and NetFS said so), or why it
/// failed. It doesn't run when the mount is cancelled with
/// [`MountHandle::cancel`]. Returns at once; never blocks on the network.
pub fn mount_url(
    url: &str,
    done: impl FnOnce(Result<Vec<PathBuf>, MountError>) + Send + 'static,
) -> Result<MountHandle, MountError> {
    let report = move |status: c_int, paths: Vec<PathBuf>| {
        done(match MountError::from_status(status) {
            None => Ok(paths),
            Some(e) => Err(e),
        })
    };
    sys::mount_async(url, Box::new(report))
        .map(|req| MountHandle { req })
        .map_err(|s| MountError::from_status(s).unwrap_or(MountError::Other(s)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_become_errors_people_understand() {
        assert_eq!(MountError::from_status(0), None);
        assert_eq!(MountError::from_status(libc::EEXIST), None);
        assert_eq!(MountError::from_status(-128), Some(MountError::Cancelled));
        assert_eq!(MountError::from_status(EAUTH), Some(MountError::SignIn));
        assert_eq!(
            MountError::from_status(libc::EHOSTDOWN),
            Some(MountError::Unreachable)
        );
        assert_eq!(MountError::from_status(-6003), Some(MountError::NoShares));
        assert_eq!(
            MountError::from_status(-1234),
            Some(MountError::Other(-1234))
        );
        assert_eq!(MountError::Cancelled.message("x"), None);
        assert!(
            MountError::SignIn
                .message("media on archbox")
                .unwrap()
                .contains("media on archbox")
        );
    }

    /// Mounting a real share: set `FSTUDIO_SMB_URL` (e.g.
    /// `smb://archbox/media`) and run with `--ignored`. The system may show
    /// its sign-in window. The share is left mounted; eject it afterwards.
    #[test]
    #[ignore = "needs a real SMB server (FSTUDIO_SMB_URL)"]
    fn mounts_a_real_share() {
        let url = std::env::var("FSTUDIO_SMB_URL").expect("set FSTUDIO_SMB_URL");
        let (tx, rx) = std::sync::mpsc::channel();
        let _h = mount_url(&url, move |r| {
            let _ = tx.send(r);
        })
        .unwrap();
        let r = rx
            .recv_timeout(std::time::Duration::from_secs(120))
            .expect("NetFS answered");
        let paths = r.expect("mounted");
        for p in &paths {
            assert!(p.is_dir(), "{p:?}");
        }
    }

    /// A server that can't exist fails with a message instead of hanging.
    #[test]
    #[ignore = "waits on name resolution; run by hand"]
    fn an_unknown_server_fails() {
        let (tx, rx) = std::sync::mpsc::channel();
        let _h = mount_url("smb://fstudio-no-such-host.invalid/x", move |r| {
            let _ = tx.send(r);
        });
        if let Ok(r) = rx.recv_timeout(std::time::Duration::from_secs(90)) {
            assert!(r.is_err());
        }
    }
}

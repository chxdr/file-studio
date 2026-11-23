#![warn(missing_docs)]
//! Connect to Server, the small SMB version (M3 follow-up; the big version
//! with SFTP, WebDAV and S3 is M5).
//!
//! - [`address`]: what the user typed (`archbox/media`, `smb://archbox`,
//!   `\\archbox\media`) as one canonical `smb://` URL. Pure.
//! - [`mount`]: mounts a URL with macOS's own NetFS (`NetFSMountURLAsync`),
//!   so sign-in, the share picker and the Keychain are the system's.
//! - [`browse`]: SMB computers on the local network, through Bonjour
//!   (`_smb._tcp`).
//! - [`identity`]: a network volume's identity that stays the same when it
//!   is unmounted and mounted again (`st_dev` doesn't, and SMB volumes have
//!   no volume UUID).
//!
//! Passwords never pass through here: a password typed into an address is
//! dropped before anything is stored or mounted, and the system asks for it
//! (and offers to keep it in the Keychain).
//!
//! Threads: no async runtime (F-010). A mount reports on a system dispatch
//! queue; the browser runs on one thread of its own.

pub mod address;
pub mod browse;
pub mod identity;
pub mod mount;
mod sys;

pub use address::{AddressError, ServerAddress, parse_server};
pub use browse::{BrowseEvent, Browser, Computer};
pub use identity::{NetworkMount, mount_key, network_mounts};
pub use mount::{MountError, MountHandle, mount_url};

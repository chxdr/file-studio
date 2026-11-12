//! Plain shared types for the File Studio core. No I/O and no OS calls.
//!
//! Every other crate depends on this one, so it changes only through the
//! coordinator. If a crate needs a new shared type, ask; don't add it locally
//! under the same name.

pub mod entry;
pub mod keys;

pub use entry::{Entry, EntryKind, file_flags};
pub use keys::{Key, KeyChord, Mods, Via};

/// A screen corner, for toasts and overlays.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
}

/// How a path is written out by "Copy Path" and friends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum PathStyle {
    /// `/Users/me/My Folder`
    Posix,
    /// `/Users/me/My\ Folder` (Terminal style)
    Escaped,
    /// `~/My Folder`
    HomeRelative,
    /// `file:///Users/me/My%20Folder/`
    FileUrl,
    /// `'/Users/me/My Folder'` (single-quoted for a shell)
    Quoted,
}

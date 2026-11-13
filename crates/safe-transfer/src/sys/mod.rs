//! Everything below `std`: the only place in the crate with unsafe code.
//!
//! Each platform module exposes the same small set of functions. macOS is
//! implemented; Linux and Windows are stubs that build and answer
//! "unsupported" until a shell for them exists.
#![allow(unsafe_code)]

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod macos_space;
#[cfg(target_os = "macos")]
mod macos_trash;
#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(target_os = "macos")]
pub use macos_space::important_free;
#[cfg(target_os = "macos")]
pub use macos_trash::trash_item;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

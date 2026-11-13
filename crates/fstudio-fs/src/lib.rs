//! File-system access for File Studio: streaming directory listing, single-entry stat,
//! volumes, extended attributes, Finder tags and folder watching.
//!
//! All `unsafe` code lives in the private `sys` module. Names are returned exactly as the
//! filesystem stores them (F-017); nothing here normalises or case-folds.

pub mod error;
pub mod list;
pub mod package;
mod sys;
pub mod tags;
pub mod volume;
pub mod watch;
pub mod xattr;

pub use error::{Error, Result};
pub use list::{Lister, Skipped, list_all, on_disk_name, stat_entry};
pub use package::{PACKAGE_EXTENSIONS, is_package_name};
pub use tags::{FinderTag, TagColor, finder_tags, set_finder_tags};
pub use volume::{STARTUP_DISK_FALLBACK, VolumeInfo, display_name, volume_for, volumes};
#[cfg(target_os = "macos")]
pub use watch::FsEventsWatcher;
pub use watch::{FsEvent, FsEventKind, PollWatcher, WatchConfig, Watcher, default_watcher};
pub use xattr::{xattr_get, xattr_list, xattr_remove, xattr_set};

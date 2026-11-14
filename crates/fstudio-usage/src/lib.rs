#![warn(missing_docs)]
//! Disk usage for File Studio: how much space a folder or a whole volume
//! uses, and where it goes.
//!
//! - [`scan()`] walks a folder in parallel and builds a [`Tree`] with sizes
//!   (allocated and logical), item counts and newest change per folder.
//! - [`UsageSession`] owns one scan and the view state around it: the tree
//!   list's rows ([`Outline`]), the treemap ([`squarify::treemap`]) and the
//!   breakdown by file type ([`types::breakdown`]).
//! - [`folder_totals`] is the same scanner without the tree, for Get Folder
//!   Size.
//!
//! Rules it keeps (CLAUDE.md, F-016, F-017): nothing is ever opened for
//! reading, symlinks are never followed, iCloud placeholders are counted
//! but never read, names are kept exactly as stored, and unreadable folders
//! are reported rather than failing the scan. `unsafe` lives only in `sys`.

pub mod outline;
pub mod read;
pub mod scan;
pub mod session;
pub mod squarify;
mod sys;
pub mod tree;
pub mod types;

use std::io;
use std::path::Path;

pub use outline::{Move, Outline, OutlineRow};
pub use read::Method;
pub use scan::{Progress, ScanControl, ScanOptions, default_threads, scan};
pub use session::{NullObserver, ScanState, UsageChange, UsageObserver, UsageSession};
pub use squarify::{Rect, Tile, TreemapOptions};
pub use tree::{NO_NODE, Node, NodeId, NodeKind, SizeMode, Tree, flags};
pub use types::{Category, Palette, TypeStat};

/// What Get Folder Size reports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Totals {
    /// Logical bytes (iCloud placeholders included).
    pub logical: u64,
    /// Bytes allocated on disk.
    pub allocated: u64,
    /// Items inside (packages count as one).
    pub items: u64,
    /// Logical bytes that are iCloud placeholders.
    pub cloud: u64,
    /// Folders that couldn't be read.
    pub denied: u32,
    /// The scan was cancelled or a folder failed to read.
    pub incomplete: bool,
    /// How long it took, in ms.
    pub elapsed_ms: u64,
}

/// Sizes the folder at `path` with the usage scanner, keeping only totals.
/// Blocks until done; run it off the main thread. `ctl` cancels it.
pub fn folder_totals(path: &Path, ctl: &ScanControl) -> io::Result<Totals> {
    let opts = ScanOptions {
        keep_tree: false,
        ..ScanOptions::default()
    };
    let tree = scan(path, &opts, ctl)?;
    let r = tree.node(tree.root());
    Ok(Totals {
        logical: r.logical,
        allocated: r.allocated,
        items: r.items,
        cloud: r.cloud,
        denied: r.denied,
        incomplete: r.flags & (flags::INCOMPLETE | flags::ERROR) != 0 || ctl.is_cancelled(),
        elapsed_ms: ctl.progress().elapsed_ms,
    })
}

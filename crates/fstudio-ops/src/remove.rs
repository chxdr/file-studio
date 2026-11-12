//! **Every removal of a user-visible path lives in this file** (F-007). A test
//! (`tests/safety_grep.rs`) fails if `remove_file`, `remove_dir`,
//! `remove_dir_all`, `unlink` or `rmdir` appear anywhere else in the crate.
//!
//! There are exactly four ways to remove something:
//! - [`delete_immediately`]: the user's explicit, gated Delete Immediately.
//! - [`remove_partial`]: our own `.<name>.fstudio-partial` temporaries.
//! - [`remove_moved_source`]: the source of a cross-volume move, only with a
//!   [`Placed`] proof that its copy is in place and synced, and only if the
//!   source hasn't changed since it was copied.
//! - [`remove_moved_source_dir`]: an *empty* source folder of a move (`rmdir`).

use std::io;
use std::path::{Path, PathBuf};

use crate::meta::{self, Ident};
use crate::names::is_partial_name;
use crate::sys;

/// Proof that Delete Immediately is enabled. Only [`DeleteGate::new`] makes one.
pub struct DeleteGate(());

impl DeleteGate {
    /// `Some` only when the setting allows Delete Immediately.
    pub fn new(allowed: bool) -> Option<DeleteGate> {
        allowed.then_some(DeleteGate(()))
    }
}

/// Permanently deletes `path` (a folder with everything in it). Symlinks are
/// removed as links; their targets are untouched.
pub fn delete_immediately(path: &Path, _gate: &DeleteGate) -> io::Result<()> {
    let md = meta::lstat(path)?;
    if md.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Removes one of our temporary files or (empty) folders. Refuses any path
/// whose name isn't `.<name>.fstudio-partial`. Clears immutable flags and ACLs
/// first, because a copied `deny delete` ACL would block removal (SPIKES S2).
/// A missing path is not an error.
pub fn remove_partial(path: &Path) -> io::Result<()> {
    if !is_partial_name(path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "refusing to remove {}: not a temporary file",
                path.display()
            ),
        ));
    }
    let md = match meta::lstat(path) {
        Ok(md) => md,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if meta::flags(&md) != 0 {
        let _ = sys::set_flags(path, 0);
    }
    let _ = sys::clear_acl(path);
    if md.is_dir() {
        std::fs::remove_dir(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Proof that a copy of `source` is in place at its destination and synced.
/// Only the engine creates one, after the rename and the sync.
pub struct Placed {
    pub(crate) source: PathBuf,
    /// The source as it was when the copy started.
    pub(crate) source_ident: Ident,
    pub(crate) dest: PathBuf,
}

/// Removes the source of a cross-volume move whose copy is in place.
///
/// Refuses if the destination is gone, or if the source changed after it was
/// copied (it could hold data the copy doesn't have).
pub fn remove_moved_source(p: &Placed) -> io::Result<()> {
    meta::lstat(&p.dest)?;
    let now = meta::ident(&meta::lstat(&p.source)?);
    if !now.unchanged(&p.source_ident) {
        return Err(io::Error::other(
            "the original changed while it was being moved, so it was kept",
        ));
    }
    std::fs::remove_file(&p.source)
}

/// Removes an empty source folder after all of its items were moved.
/// `rmdir` can't remove a folder that still has anything in it.
pub fn remove_moved_source_dir(path: &Path) -> io::Result<()> {
    std::fs::remove_dir(path)
}

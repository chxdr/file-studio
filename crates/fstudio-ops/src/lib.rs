//! File operation queue: copy, move, trash, delete, create, rename and
//! duplicate, with a SQLite journal, crash recovery, conflicts and undo.
//! See DECISIONS F-007, F-008, F-015, F-016 and F-017.
//!
//! # Data safety, structurally
//!
//! - **Nothing is hard-deleted** except in `remove.rs`: the gated
//!   [`OpRequest::DeleteImmediately`], our own temporary files, and the source
//!   of a cross-volume move after its copy is placed and synced. A test greps
//!   the crate to keep it that way.
//! - **Nothing is overwritten**: data is written to `.<name>.fstudio-partial`
//!   and renamed into place with `renamex_np(RENAME_EXCL)`. A taken name is a
//!   conflict; "Replace" sends the old item to the Trash first.
//! - **Everything is journaled** before it happens, so an interrupted op can
//!   be resumed ([`OpQueue::interrupted`], [`OpQueue::resume`]) and undone.
//! - **Errors are per file**: an item fails, the batch goes on.
//!
//! # Use
//!
//! ```no_run
//! use std::sync::Arc;
//! use fstudio_ops::{OpQueue, OpRequest, OpsConfig, SystemTrasher, NullSink};
//! let db = fstudio_db::Db::open_in_memory().unwrap();
//! let q = OpQueue::open(db, OpsConfig::default(), Arc::new(SystemTrasher), Arc::new(NullSink)).unwrap();
//! let id = q.enqueue(OpRequest::Copy {
//!     sources: vec!["/Users/me/a.txt".into()],
//!     dest_dir: "/Volumes/Backup".into(),
//! }).unwrap();
//! q.wait(id, std::time::Duration::from_secs(60));
//! ```

mod engine;
mod journal;
mod meta;
pub mod names;
mod plan;
mod queue;
mod recover;
mod remove;
mod runner;
mod sys;
mod trash;
mod types;
mod undo;

pub use journal::MIGRATIONS;
pub use queue::OpQueue;
pub use trash::{DirTrasher, SystemTrasher, Trasher};
pub use types::*;

/// xxh3-128 of a file's content (hex), read with `F_NOCACHE`. The same hash
/// the verify pass uses.
pub fn hash_file(path: &std::path::Path) -> std::io::Result<String> {
    engine::hash_file(path)
}

/// Whether the volume holding `dir` has case-sensitive names (F-017).
pub fn is_case_sensitive(dir: &std::path::Path) -> std::io::Result<bool> {
    sys::case_sensitive(dir)
}

#![warn(missing_docs)]
//! **Safe Transfer**: a crash-proof copy engine. A file only ever gets its
//! real name once every byte has been flushed to the drive and read back
//! from the drive (not from memory) with matching hashes. Pull the card, cut
//! the power or crash at any moment: you'll find the real file, complete and
//! checked, or no file plus a hidden `.st-partial` that Safe Transfer cleans up.
//!
//! This crate has no File Studio types, no UI and no database; File Studio
//! and Import Studio use it the same way. The design is
//! `docs/SAFE-TRANSFER-PROPOSAL.md` (approved 2026-11-13).
//!
//! - [`copy_file`] copies one file (milestone 1).
//! - [`Engine`] runs multi-file jobs: folders, a journal on the Mac,
//!   resume after a crash or a pulled card (by volume UUID, with chunk
//!   reuse), and small files committed in groups behind one drive flush
//!   (milestone 2).
//!
//! # The promises, structurally
//!
//! - **The original is only read.** It is checked to be unchanged before
//!   its copy is named.
//! - **Nothing is replaced.** The rename is exclusive; a taken name is a
//!   [`CopyError::Conflict`].
//! - **Nothing of yours is removed.** `remove.rs` is the only file that
//!   removes anything, and it only removes this crate's own partials (a test
//!   greps the crate to keep it that way).
//! - **Errors are per file** and describe what happened in plain language.
//!
//! # Use
//!
//! ```no_run
//! use safe_transfer::{CopyRequest, Control, NoProgress, Options, Platform, copy_file};
//! let req = CopyRequest::into_dir("/Volumes/Untitled/DCIM/100MSDCF/DSC01234.ARW",
//!                                 std::path::Path::new("/Users/me/Pictures/Smith")).unwrap();
//! let report = copy_file(&Platform::native(), &req, &Options::default(),
//!                        &Control::new(), &NoProgress).unwrap();
//! println!("{} {}", report.outcome.as_str(), report.hashes.file_hex());
//! ```

pub mod chunk;
pub mod compare;
mod copy;
mod engine;
mod events;
#[cfg(feature = "fault-injection")]
pub mod fault;
mod job;
pub mod journal;
pub mod names;
mod options;
mod plan;
mod platform;
mod remove;
pub mod space;
mod sys;

pub use copy::{CopyError, CopyRequest, Failure, copy_file, describe, metadata_lost};
pub use engine::{
    Cancel, Engine, EngineConfig, GROUP_BYTES, GROUP_FILES, GROUP_TIME, JOURNAL_INTERVAL,
    SMALL_FILE,
};
pub use events::{FileProgress, FileReport, MetaLoss, NoProgress, Outcome, Phase, ProgressSink};
pub use job::{
    EngineError, Event, EventSink, Item, JobInfo, JobPlan, JobProgress, JobReport, JobSpec,
    MissingSource, NoEvents, Note, human,
};
pub use journal::JobState;
pub use names::JobId;
pub use options::{Control, Options, Symlinks};
pub use plan::{Plan, plan};
pub use platform::{
    AtomicRename, CloneFile, Flush, FlushLevel, IMMUTABLE_FLAGS, MetaSnapshot, Metadata, Native,
    Platform, RenameMode, Residency, Trash, Uncached, VolumeIdentity, VolumeInfo,
};
pub use remove::PartialOwner;

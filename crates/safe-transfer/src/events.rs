//! Progress and the per-file report.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::chunk::FileHashes;
use crate::platform::{FlushLevel, RenameMode};

/// What a file is going through right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    /// Checking the source and destination before anything is written.
    Preparing,
    /// Writing the partial file, hashing each chunk.
    Copying,
    /// Asking the drive to make the data durable.
    Flushing,
    /// Reading the partial back from the drive and comparing hashes.
    Verifying,
    /// Reading the original a second time (re-read mode).
    Rereading,
    /// Giving the file its real name, then flushing the folder.
    Renaming,
    /// Applying ACLs and flags after the rename, and checking what was kept.
    Metadata,
}

impl Phase {
    /// A short lowercase name ("copying"), for logs and JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Preparing => "preparing",
            Phase::Copying => "copying",
            Phase::Flushing => "flushing",
            Phase::Verifying => "verifying",
            Phase::Rereading => "rereading",
            Phase::Renaming => "renaming",
            Phase::Metadata => "metadata",
        }
    }
}

/// One progress report for one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileProgress {
    /// The phase.
    pub phase: Phase,
    /// Bytes done in this phase.
    pub done: u64,
    /// Bytes this phase has to do.
    pub total: u64,
}

/// Receives progress. Called on the copying thread; keep it quick.
pub trait ProgressSink: Send + Sync {
    /// A new report.
    fn progress(&self, p: &FileProgress);
}

/// Drops every report.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoProgress;

impl ProgressSink for NoProgress {
    fn progress(&self, _p: &FileProgress) {}
}

/// Rate-limits reports: a phase change always goes out, byte counts at most
/// once per interval.
pub(crate) struct Throttle<'a> {
    sink: &'a dyn ProgressSink,
    every: Duration,
    last: Option<(Phase, Instant)>,
}

impl<'a> Throttle<'a> {
    pub(crate) fn new(sink: &'a dyn ProgressSink, every: Duration) -> Self {
        Throttle {
            sink,
            every,
            last: None,
        }
    }

    pub(crate) fn report(&mut self, phase: Phase, done: u64, total: u64) {
        let now = Instant::now();
        let due = match self.last {
            Some((p, t)) => p != phase || done >= total || now.duration_since(t) >= self.every,
            None => true,
        };
        if due {
            self.last = Some((phase, now));
            self.sink.progress(&FileProgress { phase, done, total });
        }
    }
}

/// How a file ended up in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Copied byte for byte and read back from the drive with matching hashes.
    Verified,
    /// Copied and the hashes matched, but some of the partial's pages could
    /// not be evicted from memory, so part of the read-back may have come
    /// from RAM. Treated as a warning in reports, never as "verified".
    VerifiedFromMemory,
    /// An instant APFS clone: no bytes were copied, so there was nothing to
    /// read back (proposal §4.4).
    Cloned,
    /// A symlink, copied as a link; its target text was compared.
    Link,
}

impl Outcome {
    /// A short lowercase name, for logs and JSON.
    pub fn as_str(&self) -> &'static str {
        match self {
            Outcome::Verified => "verified",
            Outcome::VerifiedFromMemory => "verified_from_memory",
            Outcome::Cloned => "cloned",
            Outcome::Link => "link",
        }
    }
}

/// A kind of metadata a destination didn't keep.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MetaLoss {
    /// Permission bits.
    Permissions,
    /// The owner.
    Owner,
    /// The modification date, beyond the filesystem's precision.
    ModifiedDate,
    /// The creation date.
    CreatedDate,
    /// These extended attributes (tags, comments, resource fork…).
    Xattrs(Vec<String>),
    /// The extended ACL.
    Acl,
    /// BSD flags (locked, hidden…).
    Flags,
    /// Holes of a sparse file were written out as zeros.
    Sparseness,
    /// Applying a part of the metadata failed with this message.
    Failed(String),
}

impl MetaLoss {
    /// A plain-language description.
    pub fn describe(&self) -> String {
        match self {
            MetaLoss::Permissions => "permissions".into(),
            MetaLoss::Owner => "owner".into(),
            MetaLoss::ModifiedDate => "exact modification date".into(),
            MetaLoss::CreatedDate => "creation date".into(),
            MetaLoss::Xattrs(n) => format!("extended attributes ({})", n.join(", ")),
            MetaLoss::Acl => "access control list".into(),
            MetaLoss::Flags => "file flags".into(),
            MetaLoss::Sparseness => "sparse holes (written in full)".into(),
            MetaLoss::Failed(m) => m.clone(),
        }
    }
}

/// What happened to one copied file.
#[derive(Debug, Clone)]
pub struct FileReport {
    /// The original.
    pub source: PathBuf,
    /// Where it is now, under its real name.
    pub dest: PathBuf,
    /// How it got there.
    pub outcome: Outcome,
    /// Size in bytes.
    pub bytes: u64,
    /// Chunk and file hashes of the content (empty for clones and links).
    pub hashes: FileHashes,
    /// Whether the original was read twice and both reads agreed.
    pub source_reread: bool,
    /// The weakest flush that happened (`Device` unless a disk refused it).
    pub flush_level: FlushLevel,
    /// How the rename was done.
    pub rename: RenameMode,
    /// Metadata the destination didn't keep.
    pub metadata_lost: Vec<MetaLoss>,
    /// Retries after a read-back mismatch that was the copy's fault.
    pub retries: u32,
    /// Time spent writing.
    pub copy_time: Duration,
    /// Time spent flushing.
    pub flush_time: Duration,
    /// Time spent reading back.
    pub verify_time: Duration,
    /// Time spent re-reading the original.
    pub reread_time: Duration,
}

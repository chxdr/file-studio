//! What a copy may do, and the controls a running copy listens to.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::chunk::DEFAULT_CHUNK_SIZE;

/// Copy, or move (the original goes to the Trash once its copy is verified).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Copy; the originals stay where they are.
    #[default]
    Copy,
    /// Move: a rename on the same volume; across volumes copy, verify, then
    /// send the original to the Trash (never a hard delete, §7).
    Move,
}

/// What to do when the destination name is taken (proposal §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// Ask: the item waits for an answer; the rest of the job carries on.
    Ask,
    /// Leave the existing item; don't copy this one.
    Skip,
    /// Skip if the existing file has the same content; otherwise ask.
    SkipIfIdentical,
    /// The new item is written and verified first; only then does the old
    /// one go to the Trash, and the new one is renamed into place.
    Replace,
    /// Keep both: the new item gets Finder's next free name ("name 2").
    KeepBoth,
    /// Folders only: copy into the existing folder.
    Merge,
}

impl Decision {
    /// A short lowercase name.
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Ask => "ask",
            Decision::Skip => "skip",
            Decision::SkipIfIdentical => "skip_if_identical",
            Decision::Replace => "replace",
            Decision::KeepBoth => "keep_both",
            Decision::Merge => "merge",
        }
    }
}

/// Default answers for files and for folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictPolicy {
    /// For files and symlinks. Default: ask.
    pub files: Decision,
    /// For folders. Default: merge.
    pub folders: Decision,
}

impl Default for ConflictPolicy {
    fn default() -> Self {
        ConflictPolicy {
            files: Decision::Ask,
            folders: Decision::Merge,
        }
    }
}

/// What happens to a moved original.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposal {
    /// To the Trash, where Finder's Put Back works (default).
    #[default]
    Trash,
    /// Kept where it is (a move that only copies).
    Keep,
}

/// What to do with a symlink among the sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Symlinks {
    /// Copy the link itself; its target is never read (default).
    #[default]
    CopyLink,
    /// Copy what the link points to.
    Follow,
}

/// Options for a copy. Verifying is not optional: every copied file is read
/// back from the destination before it gets its real name (instant APFS
/// clones excepted, because they copy no bytes; decided 2026-11-13).
#[derive(Debug, Clone)]
pub struct Options {
    /// Bytes per chunk for hashing and flushing. 64 MiB by default; clamped
    /// to 1–256 MiB.
    pub chunk_size: u64,
    /// Flush after every chunk of files bigger than one chunk, so an
    /// interruption keeps the flushed chunks. Default on.
    pub flush_each_chunk: bool,
    /// Read the original a second time, separately, and require both reads
    /// to agree (proposal §4.6). Catches a failing card or reader handing
    /// over a bad byte. Costs roughly twice the reading time from the source.
    /// Default off.
    pub reread_source: bool,
    /// Symlink handling.
    pub symlinks: Symlinks,
    /// Copy iCloud placeholders (which downloads them). Default off (F-016).
    pub allow_dataless: bool,
    /// Use an instant clone when source and destination are on the same
    /// APFS volume. Default on.
    pub clone_same_volume: bool,
    /// Minimum time between progress reports. Default 16 ms (F-010).
    pub progress_interval: Duration,
    /// Copy or move (jobs only).
    pub mode: Mode,
    /// What to do when a name is taken (jobs only).
    pub on_conflict: ConflictPolicy,
    /// What happens to moved originals (jobs only).
    pub disposal: Disposal,
    /// Files worked on at once. `None`: 4 for small files and 2 for big
    /// ones (one copying while the previous is checked).
    pub workers: Option<usize>,
    /// Test hooks (crash and yank tests). Only with the `fault-injection` feature.
    #[cfg(feature = "fault-injection")]
    pub faults: Option<std::sync::Arc<dyn crate::fault::FaultHook>>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            chunk_size: DEFAULT_CHUNK_SIZE,
            flush_each_chunk: true,
            reread_source: false,
            symlinks: Symlinks::CopyLink,
            allow_dataless: false,
            clone_same_volume: true,
            progress_interval: Duration::from_millis(16),
            mode: Mode::Copy,
            on_conflict: ConflictPolicy::default(),
            disposal: Disposal::Trash,
            workers: None,
            #[cfg(feature = "fault-injection")]
            faults: None,
        }
    }
}

/// Pause and cancel for a running copy, shared with whoever controls it.
#[derive(Debug, Default)]
pub struct Control {
    paused: AtomicBool,
    cancelled: AtomicBool,
}

impl Control {
    /// A control that is neither paused nor cancelled.
    pub fn new() -> Control {
        Control::default()
    }
    /// Pauses at the next 4 MiB step.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::Release);
    }
    /// Continues a paused copy.
    pub fn resume(&self) {
        self.paused.store(false, Ordering::Release);
    }
    /// Cancels: the partial is removed and nothing else changes.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    /// Whether the copy was cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
    /// Whether the copy is paused.
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }
    /// Blocks while paused. Returns `false` once cancelled.
    pub(crate) fn wait(&self) -> bool {
        while self.is_paused() && !self.is_cancelled() {
            std::thread::sleep(Duration::from_millis(5));
        }
        !self.is_cancelled()
    }
}

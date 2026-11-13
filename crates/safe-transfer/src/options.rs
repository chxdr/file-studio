//! What a copy may do, and the controls a running copy listens to.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::chunk::DEFAULT_CHUNK_SIZE;

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

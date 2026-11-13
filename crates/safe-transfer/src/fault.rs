//! Named stop points for crash and yank tests (proposal §11.2, §4.7). Only built
//! with the `fault-injection` feature; release builds have none of this.
//!
//! A hook decides what happens at each point: carry on, fail the file,
//! block (so a test can pull the disk or `kill -9` at exactly that moment),
//! or abort the process.

use std::io;
use std::path::Path;

/// A point in the single-file pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultPoint {
    /// Before anything is written for the file.
    BeforeCopy,
    /// Inside the write loop, before writing the block that starts at `offset`.
    MidChunk {
        /// Byte offset in the file.
        offset: u64,
    },
    /// The partial is completely written; before it is flushed (every file,
    /// grouped or not).
    AfterWrite,
    /// Before a flush (`chunk` is `None` for the end-of-file flush).
    BeforeFlush {
        /// The chunk just completed, if this is a per-chunk flush.
        chunk: Option<usize>,
    },
    /// After a flush.
    AfterFlush {
        /// As for `BeforeFlush`.
        chunk: Option<usize>,
    },
    /// Inside the read-back, before reading at `offset`.
    MidVerify {
        /// Byte offset in the file.
        offset: u64,
    },
    /// Before the original is read a second time (re-read mode).
    BeforeReread,
    /// Before the partial is renamed to its real name.
    BeforeRename,
    /// After the rename, before the folder is flushed.
    AfterRename,
    /// A group of small files is written; before its one drive flush.
    BeforeGroupBarrier,
    /// After a group's drive flush, before any of it is checked or renamed.
    AfterGroupBarrier,
    /// Before renaming file `index` (0-based) of a group.
    MidGroupRename {
        /// Position in the group.
        index: u64,
    },
    /// A group is renamed; before the folder flush that makes it durable.
    BeforeFolderBarrier,
    /// A verified copy is about to replace an existing item: before the old
    /// one goes to the Trash.
    BeforeReplaceTrash,
    /// A moved file is in place: before its original goes to the Trash.
    BeforeSourceTrash,
    /// A replacing folder is complete: before the old folder goes to the
    /// Trash and the new one takes its name.
    BeforeFolderSwap,
    /// Half of a journal file's temporary copy is written.
    MidJournalWrite,
    /// A journal file's temporary copy is flushed; before it replaces a slot.
    BeforeJournalRename,
}

impl FaultPoint {
    /// The point's name in `ST_FAULT` specs ("mid_chunk", "before_rename"…).
    pub fn name(&self) -> &'static str {
        match self {
            FaultPoint::BeforeCopy => "before_copy",
            FaultPoint::MidChunk { .. } => "mid_chunk",
            FaultPoint::AfterWrite => "after_write",
            FaultPoint::BeforeFlush { .. } => "before_flush",
            FaultPoint::AfterFlush { .. } => "after_flush",
            FaultPoint::MidVerify { .. } => "mid_verify",
            FaultPoint::BeforeReread => "before_reread",
            FaultPoint::BeforeRename => "before_rename",
            FaultPoint::AfterRename => "after_rename",
            FaultPoint::BeforeGroupBarrier => "before_group_barrier",
            FaultPoint::AfterGroupBarrier => "after_group_barrier",
            FaultPoint::MidGroupRename { .. } => "mid_group_rename",
            FaultPoint::BeforeFolderBarrier => "before_folder_barrier",
            FaultPoint::BeforeReplaceTrash => "before_replace_trash",
            FaultPoint::BeforeSourceTrash => "before_source_trash",
            FaultPoint::BeforeFolderSwap => "before_folder_swap",
            FaultPoint::MidJournalWrite => "mid_journal_write",
            FaultPoint::BeforeJournalRename => "before_journal_rename",
        }
    }

    /// The byte offset for mid-file points, else 0.
    pub fn offset(&self) -> u64 {
        match self {
            FaultPoint::MidChunk { offset } | FaultPoint::MidVerify { offset } => *offset,
            FaultPoint::MidGroupRename { index } => *index,
            _ => 0,
        }
    }
}

/// Decides what happens at a fault point.
pub trait FaultHook: Send + Sync + std::fmt::Debug {
    /// Called at `point` while copying `source`. Return an error to fail the
    /// file there; block to hold the copy; abort to simulate a crash.
    fn at(&self, point: FaultPoint, source: &Path) -> io::Result<()>;
}

/// What a [`SpecHook`] does when its point is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultAction {
    /// Fail the file with an I/O error.
    Fail,
    /// Print `REACHED <point> <offset>` on stderr and stop forever (until killed).
    Pause,
    /// `std::process::abort()`: an immediate crash without cleanup.
    Abort,
}

/// A hook built from a spec such as `pause@mid_verify:10485760`,
/// `fail@before_rename` or `pause@mid_journal_write#10`: the action fires
/// the first time (or the `#N`th time) the named point is reached at or
/// past the offset.
#[derive(Debug)]
pub struct SpecHook {
    action: FaultAction,
    point: String,
    offset: u64,
    nth: u32,
    hits: std::sync::atomic::AtomicU32,
}

impl SpecHook {
    /// Parses `<fail|pause|abort>@<point>[:<offset>][#<n>]`.
    pub fn parse(spec: &str) -> Option<SpecHook> {
        let (spec, nth) = match spec.split_once('#') {
            Some((s, n)) => (s, n.parse().ok().filter(|n| *n >= 1)?),
            None => (spec, 1),
        };
        let (act, rest) = spec.split_once('@')?;
        let action = match act {
            "fail" => FaultAction::Fail,
            "pause" => FaultAction::Pause,
            "abort" => FaultAction::Abort,
            _ => return None,
        };
        let (point, offset) = match rest.split_once(':') {
            Some((p, o)) => (p, o.parse().ok()?),
            None => (rest, 0),
        };
        Some(SpecHook {
            action,
            point: point.to_string(),
            offset,
            nth,
            hits: Default::default(),
        })
    }

    /// The hook from the `ST_FAULT` environment variable, if set and valid.
    pub fn from_env() -> Option<SpecHook> {
        SpecHook::parse(&std::env::var("ST_FAULT").ok()?)
    }
}

impl FaultHook for SpecHook {
    fn at(&self, point: FaultPoint, _source: &Path) -> io::Result<()> {
        use std::sync::atomic::Ordering;
        if point.name() != self.point || point.offset() < self.offset {
            return Ok(());
        }
        if self.hits.fetch_add(1, Ordering::AcqRel) + 1 != self.nth {
            return Ok(());
        }
        match self.action {
            FaultAction::Fail => Err(io::Error::other(format!(
                "fault injected at {}",
                self.point
            ))),
            FaultAction::Pause => {
                eprintln!("REACHED {} {}", point.name(), point.offset());
                loop {
                    std::thread::park();
                }
            }
            FaultAction::Abort => std::process::abort(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_parse_and_fire_once_at_or_past_the_offset() {
        let h = SpecHook::parse("fail@mid_chunk:100").unwrap();
        let p = Path::new("/x");
        assert!(h.at(FaultPoint::MidChunk { offset: 50 }, p).is_ok());
        assert!(h.at(FaultPoint::BeforeRename, p).is_ok());
        assert!(h.at(FaultPoint::MidChunk { offset: 128 }, p).is_err());
        assert!(h.at(FaultPoint::MidChunk { offset: 256 }, p).is_ok());
        assert!(SpecHook::parse("explode@mid_chunk").is_none());
        let third = SpecHook::parse("fail@mid_journal_write#3").unwrap();
        assert!(third.at(FaultPoint::MidJournalWrite, p).is_ok());
        assert!(third.at(FaultPoint::MidJournalWrite, p).is_ok());
        assert!(third.at(FaultPoint::MidJournalWrite, p).is_err());
        assert!(third.at(FaultPoint::MidJournalWrite, p).is_ok());
        assert!(SpecHook::parse("fail@mid_journal_write#0").is_none());
        assert!(SpecHook::parse("fail@mid_chunk:x").is_none());
        assert_eq!(
            SpecHook::parse("pause@before_rename").unwrap().action,
            FaultAction::Pause
        );
    }
}

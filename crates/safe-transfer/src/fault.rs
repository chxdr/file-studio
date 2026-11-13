//! Named stop points for crash and yank tests (proposal §11.2). Only built
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
}

impl FaultPoint {
    /// The point's name in `ST_FAULT` specs ("mid_chunk", "before_rename"…).
    pub fn name(&self) -> &'static str {
        match self {
            FaultPoint::BeforeCopy => "before_copy",
            FaultPoint::MidChunk { .. } => "mid_chunk",
            FaultPoint::BeforeFlush { .. } => "before_flush",
            FaultPoint::AfterFlush { .. } => "after_flush",
            FaultPoint::MidVerify { .. } => "mid_verify",
            FaultPoint::BeforeReread => "before_reread",
            FaultPoint::BeforeRename => "before_rename",
            FaultPoint::AfterRename => "after_rename",
        }
    }

    /// The byte offset for mid-file points, else 0.
    pub fn offset(&self) -> u64 {
        match self {
            FaultPoint::MidChunk { offset } | FaultPoint::MidVerify { offset } => *offset,
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

/// A hook built from a spec such as `pause@mid_verify:10485760` or
/// `fail@before_rename`: the action fires the first time the named point
/// is reached at or past the offset.
#[derive(Debug)]
pub struct SpecHook {
    action: FaultAction,
    point: String,
    offset: u64,
    fired: std::sync::atomic::AtomicBool,
}

impl SpecHook {
    /// Parses `<fail|pause|abort>@<point>[:<offset>]`.
    pub fn parse(spec: &str) -> Option<SpecHook> {
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
            fired: Default::default(),
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
        if self.fired.swap(true, Ordering::AcqRel) {
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
        assert!(SpecHook::parse("fail@mid_chunk:x").is_none());
        assert_eq!(
            SpecHook::parse("pause@before_rename").unwrap().action,
            FaultAction::Pause
        );
    }
}

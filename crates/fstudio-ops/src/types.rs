//! Public request, plan, status and event types for the operation queue.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fstudio_model::EntryKind;
use serde::{Deserialize, Serialize};

/// Identifies one queued operation. Stable across restarts (it is the journal row id).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OpId(pub i64);

impl fmt::Display for OpId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "op#{}", self.0)
    }
}

/// Identifies one conflict within an op. It is the id of the item that hit it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConflictId(pub i64);

/// What the user asked for. Paths are absolute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpRequest {
    /// Copy each source into `dest_dir`, keeping its name.
    Copy {
        /// The items to copy.
        sources: Vec<PathBuf>,
        /// The folder to copy them into.
        dest_dir: PathBuf,
    },
    /// Move each source into `dest_dir`. A same-volume move is a rename; a
    /// cross-volume move copies, verifies (if on), syncs, then removes the source.
    Move {
        /// The items to move.
        sources: Vec<PathBuf>,
        /// The folder to move them into.
        dest_dir: PathBuf,
    },
    /// Move each path to the Trash (Finder's Put Back keeps working).
    Trash {
        /// The items to trash.
        paths: Vec<PathBuf>,
    },
    /// Permanently delete each path. Refused unless
    /// [`OpsConfig::allow_delete_immediately`] is set.
    DeleteImmediately {
        /// The items to delete.
        paths: Vec<PathBuf>,
    },
    /// Create an empty folder `parent/name`.
    NewFolder {
        /// The folder to create it in.
        parent: PathBuf,
        /// Its name.
        name: String,
    },
    /// Create an empty file `parent/name`.
    NewFile {
        /// The folder to create it in.
        parent: PathBuf,
        /// Its name.
        name: String,
    },
    /// Rename `path` to `new_name` in the same folder.
    Rename {
        /// The item to rename.
        path: PathBuf,
        /// Its new name (no `/`).
        new_name: String,
    },
    /// Copy each path next to itself as "name copy", "name copy 2"… (as Finder does).
    Duplicate {
        /// The items to duplicate.
        paths: Vec<PathBuf>,
    },
    /// Move each `(from, to)` to the exact destination path `to`. The folder
    /// `to` goes into must exist: a missing one is never created silently
    /// (undo asks, then creates it as a journaled step, S14). Undo uses this
    /// to move items back, put them back from the Trash and rename them back.
    MoveTo {
        /// `(from, to)`, where `to` is the full destination path.
        pairs: Vec<(PathBuf, PathBuf)>,
    },
    /// Several requests run one after another as **one** journaled op with
    /// one undo entry (New Folder with Selection, a chain of new folders,
    /// putting an item back into a recreated folder). Each step runs as its
    /// own op once the one before it has completed; if a step doesn't
    /// complete, the rest are not run. Undo undoes the steps in reverse.
    Compound {
        /// The Edit menu's label ("New Folder with Items").
        label: String,
        /// The steps, in order.
        steps: Vec<OpRequest>,
    },
    /// Inside a [`OpRequest::Compound`] only: undo the op `op` (its inverse
    /// is worked out when this step starts, after the steps before it ran).
    UndoOf {
        /// The op to undo.
        op: OpId,
    },
}

impl OpRequest {
    /// The kind of operation.
    pub fn kind(&self) -> OpKind {
        match self {
            OpRequest::Copy { .. } => OpKind::Copy,
            OpRequest::Move { .. } => OpKind::Move,
            OpRequest::Trash { .. } => OpKind::Trash,
            OpRequest::DeleteImmediately { .. } => OpKind::DeleteImmediately,
            OpRequest::NewFolder { .. } => OpKind::NewFolder,
            OpRequest::NewFile { .. } => OpKind::NewFile,
            OpRequest::Rename { .. } => OpKind::Rename,
            OpRequest::Duplicate { .. } => OpKind::Duplicate,
            OpRequest::MoveTo { .. } => OpKind::MoveTo,
            OpRequest::Compound { .. } | OpRequest::UndoOf { .. } => OpKind::Compound,
        }
    }
}

/// The kind of an operation, without its arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OpKind {
    /// An [`OpRequest::Copy`].
    Copy,
    /// An [`OpRequest::Move`].
    Move,
    /// An [`OpRequest::Trash`].
    Trash,
    /// An [`OpRequest::DeleteImmediately`].
    DeleteImmediately,
    /// An [`OpRequest::NewFolder`].
    NewFolder,
    /// An [`OpRequest::NewFile`].
    NewFile,
    /// An [`OpRequest::Rename`].
    Rename,
    /// An [`OpRequest::Duplicate`].
    Duplicate,
    /// An [`OpRequest::MoveTo`].
    MoveTo,
    /// An [`OpRequest::Compound`].
    Compound,
}

impl OpKind {
    /// The verb used in progress titles and undo labels ("Copy", "Move to Trash").
    pub fn verb(self) -> &'static str {
        match self {
            OpKind::Copy => "Copy",
            OpKind::Move | OpKind::MoveTo => "Move",
            OpKind::Trash => "Move to Trash",
            OpKind::DeleteImmediately => "Delete Immediately",
            OpKind::NewFolder => "New Folder",
            OpKind::NewFile => "New File",
            OpKind::Rename => "Rename",
            OpKind::Duplicate => "Duplicate",
            OpKind::Compound => "Change",
        }
    }

    /// Whether the op writes file content (so it has a copy engine and parallel workers).
    pub(crate) fn copies(self) -> bool {
        matches!(
            self,
            OpKind::Copy | OpKind::Move | OpKind::Duplicate | OpKind::MoveTo
        )
    }
}

/// What to do when the destination name is taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConflictDecision {
    /// Keep both: the new item gets the next free "name 2", "name 3"… (as Finder does).
    KeepBoth,
    /// Send the existing item to the Trash first, then place the new one.
    /// The old item is never unlinked.
    Replace,
    /// Leave this item alone.
    Skip,
    /// Folders only: put the source folder's contents into the existing
    /// folder; conflicts inside it are asked about one by one.
    Merge,
}

/// Standing conflict decisions for an op, set up front or by "apply to all".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictRules {
    /// For a folder landing on an existing folder.
    pub folders: Option<ConflictDecision>,
    /// For every other conflict (file onto file, file onto folder, symlinks…).
    pub others: Option<ConflictDecision>,
}

impl ConflictRules {
    /// The same decision for every conflict. `Merge` applies only to folders;
    /// other conflicts still ask.
    pub fn all(d: ConflictDecision) -> Self {
        Self {
            folders: Some(d),
            others: (d != ConflictDecision::Merge).then_some(d),
        }
    }
    pub(crate) fn for_class(&self, folder_onto_folder: bool) -> Option<ConflictDecision> {
        if folder_onto_folder {
            self.folders
        } else {
            self.others
        }
    }
}

/// Per-request options.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnqueueOptions {
    /// Conflict decisions to apply without asking.
    pub rules: ConflictRules,
    /// Read the content of iCloud/File Provider placeholders (`SF_DATALESS`),
    /// which downloads them (F-016). Off by default: such items are skipped with
    /// a per-file error when their content would have to be read.
    pub allow_dataless: bool,
}

/// A failure point for fault-injection tests. See [`FaultHook`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaultStep {
    /// Before any data is written for a file.
    BeforeCopy,
    /// After the temporary file is complete, before it is synced and verified.
    AfterCopy,
    /// Make the verify pass report a mismatch.
    Verify,
    /// Before the temporary file (or a same-volume move) is renamed into place.
    BeforePlace,
    /// After the item is in place, before the source of a move is removed.
    BeforeSourceRemove,
}

/// Test hook: return an error to make the engine fail at `step` for `path`
/// (the source). Never set in production.
pub trait FaultHook: Send + Sync {
    /// Returns an error to fail the item at `step`.
    fn check(&self, step: FaultStep, path: &Path) -> std::io::Result<()>;
}

/// Queue-wide settings.
#[derive(Clone)]
pub struct OpsConfig {
    /// Enables [`OpRequest::DeleteImmediately`]. Off by default (F-007).
    pub allow_delete_immediately: bool,
    /// Verify every copied file with xxh3 over an `F_NOCACHE` re-read. Clones skip it.
    pub verify: bool,
    /// Ops running at once per volume pair. Default 1.
    pub ops_per_volume: usize,
    /// Parallel file workers within one copy or move. Default 4.
    pub file_workers: usize,
    /// `F_FULLFSYNC` every copied file for plain copies too (moves always
    /// sync before removing a source). Default off: a copy's source is intact,
    /// so a power cut can't lose data, only the copy.
    pub fsync_copies: bool,
    /// Minimum time between progress events for one op. Default 16 ms.
    pub progress_interval: Duration,
    /// How many undo steps are kept. Default 100.
    pub undo_limit: usize,
    /// Fault injection for tests. `None` in production.
    pub faults: Option<Arc<dyn FaultHook>>,
}

impl Default for OpsConfig {
    fn default() -> Self {
        Self {
            allow_delete_immediately: false,
            verify: false,
            ops_per_volume: 1,
            file_workers: 4,
            fsync_copies: false,
            progress_interval: Duration::from_millis(16),
            undo_limit: 100,
            faults: None,
        }
    }
}

impl fmt::Debug for OpsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpsConfig")
            .field("allow_delete_immediately", &self.allow_delete_immediately)
            .field("verify", &self.verify)
            .field("ops_per_volume", &self.ops_per_volume)
            .field("file_workers", &self.file_workers)
            .field("fsync_copies", &self.fsync_copies)
            .field("progress_interval", &self.progress_interval)
            .field("undo_limit", &self.undo_limit)
            .field("faults", &self.faults.is_some())
            .finish()
    }
}

/// One top-level item of a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanRoot {
    /// The item as requested.
    pub source: PathBuf,
    /// Where it would land (`None` for Trash and Delete).
    pub dest: Option<PathBuf>,
    /// What the item is (`Other` if it couldn't be read).
    pub kind: EntryKind,
    /// Files (and symlinks) inside, counting the item itself if it's a file.
    pub files: u64,
    /// Folders inside, counting the item itself if it's a folder.
    pub dirs: u64,
    /// Bytes of file content inside (logical sizes).
    pub bytes: u64,
    /// Source and destination are on different volumes.
    pub cross_volume: bool,
}

/// A name that is already taken at the destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanConflict {
    /// The incoming item.
    pub source: PathBuf,
    /// The item already there.
    pub dest: PathBuf,
    /// What the incoming item is.
    pub source_kind: EntryKind,
    /// What the existing item is.
    pub dest_kind: EntryKind,
}

/// Names that are different on the source but the same on a case-insensitive
/// destination (or the same after Unicode normalisation), so they would collide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseCollision {
    /// The folder they would land in.
    pub dest_dir: PathBuf,
    /// The colliding names, exactly as stored.
    pub names: Vec<String>,
}

/// A dry run of a request: what it would touch, found up front.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpPlan {
    /// The top-level items.
    pub roots: Vec<PlanRoot>,
    /// Regular files and symlinks.
    pub files: u64,
    /// Folders, counting top-level folders.
    pub dirs: u64,
    /// Bytes of file content (logical sizes).
    pub bytes: u64,
    /// Top-level names already taken at the destination.
    pub conflicts: Vec<PlanConflict>,
    /// Names that would collide on the destination volume (F-017).
    pub case_collisions: Vec<CaseCollision>,
    /// iCloud/File Provider placeholders whose content is not on this Mac.
    /// They are not read unless the request opts in.
    pub dataless: Vec<PathBuf>,
    /// Any source is on a different volume from the destination.
    pub cross_volume: bool,
    /// The destination volume's case sensitivity (`None` when there is no destination).
    pub dest_case_sensitive: Option<bool>,
    /// Problems found while planning (missing sources, unreadable folders…).
    pub errors: Vec<ItemError>,
    /// Why the whole request can't run, if it can't (e.g. Delete Immediately is off).
    pub refused: Option<String>,
}

/// The lifecycle of an op.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OpState {
    /// Waiting for a free slot on its volume.
    Queued,
    /// Working.
    Running,
    /// Paused by the user.
    Paused,
    /// Everything else is done; waiting for the user to resolve conflicts.
    WaitingForDecision,
    /// Finished without errors.
    Completed,
    /// Finished, but some items failed.
    CompletedWithErrors,
    /// Cancelled by the user; finished items stay.
    Cancelled,
    /// Could not run at all (see the errors).
    Failed,
    /// Found unfinished in the journal after a crash or quit. `resume` it.
    Interrupted,
}

impl OpState {
    /// The op will not change any more.
    pub fn is_finished(self) -> bool {
        matches!(
            self,
            OpState::Completed
                | OpState::CompletedWithErrors
                | OpState::Cancelled
                | OpState::Failed
        )
    }
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            OpState::Queued => "queued",
            OpState::Running => "running",
            OpState::Paused => "paused",
            OpState::WaitingForDecision => "waiting",
            OpState::Completed => "completed",
            OpState::CompletedWithErrors => "completed_with_errors",
            OpState::Cancelled => "cancelled",
            OpState::Failed => "failed",
            OpState::Interrupted => "interrupted",
        }
    }
    pub(crate) fn parse(s: &str) -> OpState {
        match s {
            "queued" => OpState::Queued,
            "running" => OpState::Running,
            "paused" => OpState::Paused,
            "waiting" => OpState::WaitingForDecision,
            "completed" => OpState::Completed,
            "completed_with_errors" => OpState::CompletedWithErrors,
            "cancelled" => OpState::Cancelled,
            "failed" => OpState::Failed,
            _ => OpState::Interrupted,
        }
    }
}

/// A per-file error. Errors never end a batch silently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemError {
    /// The source (or the item itself, for Trash and Delete).
    pub path: PathBuf,
    /// Where it was going, if anywhere.
    pub dest: Option<PathBuf>,
    /// What went wrong, in plain words.
    pub message: String,
    /// The OS error number, when there is one.
    pub errno: Option<i32>,
}

/// A conflict waiting for a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictInfo {
    /// The op it belongs to.
    pub op: OpId,
    /// Pass this to [`crate::OpQueue::resolve_conflict`].
    pub id: ConflictId,
    /// The incoming item.
    pub source: PathBuf,
    /// The item already there.
    pub dest: PathBuf,
    /// What the incoming item is.
    pub source_kind: EntryKind,
    /// What the existing item is.
    pub dest_kind: EntryKind,
    /// The incoming item's size in bytes.
    pub source_size: u64,
    /// The existing item's size in bytes.
    pub dest_size: u64,
    /// The incoming item's modification time (ns since the epoch).
    pub source_mtime_ns: i64,
    /// The existing item's modification time (ns since the epoch).
    pub dest_mtime_ns: i64,
    /// Both are folders, so `Merge` is allowed.
    pub can_merge: bool,
}

/// A live snapshot of one op, for the progress UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpStatus {
    /// The op.
    pub id: OpId,
    /// What it does.
    pub kind: OpKind,
    /// "Copying 12 items to “Backup”".
    pub title: String,
    /// Where it is in its life.
    pub state: OpState,
    /// Items finished (done, failed or skipped).
    pub files_done: u64,
    /// Items in all, from the plan.
    pub files_total: u64,
    /// Bytes copied so far.
    pub bytes_done: u64,
    /// Bytes to copy, from the plan.
    pub bytes_total: u64,
    /// The file being worked on.
    pub current: Option<PathBuf>,
    /// Smoothed throughput, bytes per second.
    pub speed_bps: f64,
    /// Estimated seconds left, once the speed is known.
    pub eta_secs: Option<f64>,
    /// Per-file errors so far.
    pub errors: Vec<ItemError>,
    /// Conflicts waiting for a decision.
    pub conflicts: Vec<ConflictInfo>,
}

/// The summary sent when an op finishes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpReport {
    /// The op.
    pub id: OpId,
    /// What it did.
    pub kind: OpKind,
    /// How it ended.
    pub state: OpState,
    /// Items done.
    pub files_done: u64,
    /// Items that failed.
    pub files_failed: u64,
    /// Items skipped (by a decision, or not downloaded).
    pub files_skipped: u64,
    /// Bytes copied.
    pub bytes_done: u64,
    /// Every per-file error.
    pub errors: Vec<ItemError>,
    /// Wall-clock run time.
    pub elapsed_ms: u64,
}

/// Edit-menu labels.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndoState {
    /// "Undo Move of 12 Items", or `None` when there is nothing to undo.
    pub undo: Option<String>,
    /// "Redo Move of 12 Items", or `None`.
    pub redo: Option<String>,
}

/// Events from the queue. Progress is coalesced per op to at most one event
/// per [`OpsConfig::progress_interval`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OpEvent {
    /// Coalesced progress for one op.
    OpProgress(OpStatus),
    /// An item is waiting for a conflict decision.
    OpConflict {
        /// The op.
        op: OpId,
        /// The conflict to show.
        conflict: ConflictInfo,
    },
    /// An item failed (the op goes on).
    OpItemFailed {
        /// The op.
        op: OpId,
        /// What failed.
        error: ItemError,
    },
    /// An op finished.
    OpFinished {
        /// The op.
        op: OpId,
        /// Its summary.
        report: OpReport,
    },
    /// The undo or redo labels changed.
    UndoChanged(UndoState),
}

/// Receives queue events. Called from worker threads; keep it quick.
pub trait EventSink: Send + Sync {
    /// Handles one event.
    fn on_event(&self, e: OpEvent);
}

impl EventSink for crossbeam_channel::Sender<OpEvent> {
    fn on_event(&self, e: OpEvent) {
        let _ = self.send(e);
    }
}

/// Drops every event.
pub struct NullSink;

impl EventSink for NullSink {
    fn on_event(&self, _e: OpEvent) {}
}

/// Errors from queue calls (per-file errors are [`ItemError`]s instead).
#[derive(Debug, thiserror::Error)]
pub enum OpsError {
    #[error("journal: {0}")]
    /// The database failed.
    Db(#[from] fstudio_db::DbError),
    #[error("journal: {0}")]
    /// An SQL statement failed.
    Sql(#[from] fstudio_db::rusqlite::Error),
    #[error("{0}")]
    /// An I/O error outside any one item.
    Io(#[from] std::io::Error),
    #[error("no such operation: {0}")]
    /// No op with that id.
    NoSuchOp(OpId),
    #[error("no such conflict")]
    /// No pending conflict with that id.
    NoSuchConflict,
    #[error("invalid request: {0}")]
    /// The request is malformed (relative path, bad name, nothing to do).
    InvalidRequest(String),
    #[error("Delete Immediately is turned off in Settings")]
    /// Delete Immediately was requested while it is turned off.
    DeleteImmediatelyDisabled,
    #[error("{0:?} is not allowed for this conflict")]
    /// The decision doesn't fit the conflict (Merge for a file).
    InvalidDecision(ConflictDecision),
    #[error("can't undo: {0}")]
    /// Undo would overwrite or lose something, so it was refused.
    UndoRefused(String),
    /// Undo would put items back into folders that no longer exist (listed,
    /// outermost first). Ask, then call `OpQueue::undo_recreating`.
    #[error("can't undo: a folder it goes back into is gone")]
    NeedsFolders(Vec<PathBuf>),
    /// Redo would act on something that changed since the undo put it back
    /// (the reason is a sentence).
    #[error("can't redo: {0}")]
    RedoRefused(String),
    #[error("journal data: {0}")]
    /// A journal row couldn't be decoded.
    Json(#[from] serde_json::Error),
}

/// Result alias for queue calls.
pub type Result<T> = std::result::Result<T, OpsError>;

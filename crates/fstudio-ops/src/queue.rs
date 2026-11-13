//! The operation queue: journaled ops, per-volume scheduling, pause/resume/
//! cancel, conflicts, crash recovery and undo/redo.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use fstudio_db::Db;
use fstudio_model::EntryKind;
use parking_lot::{Condvar, Mutex};

use crate::engine::{OpCtl, file_name};
use crate::journal::{ItemRow, ItemState, Journal, NewItem, OpRow, Origin, Stack};
use crate::plan;
use crate::recover;
use crate::trash::Trasher;
use crate::types::{
    ConflictDecision, ConflictId, EnqueueOptions, EventSink, OpEvent, OpId, OpKind, OpPlan,
    OpReport, OpRequest, OpState, OpStatus, OpsConfig, OpsError, Result, UndoState,
};
use crate::undo;

/// A message to a running op's scheduler.
pub enum Ctl {
    /// Resolve one conflict.
    Decide(ConflictId, ConflictDecision),
    /// Something changed (pause, resume, cancel, rules); look again.
    Wake,
}

/// Where an op is in the queue's life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Queued,
    Running,
    Finished,
}

/// One op known to this queue.
pub struct OpShared {
    pub id: OpId,
    pub kind: OpKind,
    /// `(source device, destination device)`: ops with the same key share a slot.
    pub key: (u64, u64),
    pub origin: Origin,
    /// Number of top-level items, for labels.
    pub roots: usize,
    /// The first top-level name, for labels.
    pub first_name: String,
    pub ctl: OpCtl,
    pub status: Mutex<OpStatus>,
    pub options: Mutex<EnqueueOptions>,
    pub phase: Mutex<Phase>,
    pub ctl_tx: Sender<Ctl>,
    pub ctl_rx: Receiver<Ctl>,
}

/// Shared state behind an [`OpQueue`].
pub struct Inner {
    pub journal: Journal,
    pub cfg: OpsConfig,
    pub trasher: Arc<dyn Trasher>,
    pub sink: Arc<dyn EventSink>,
    pub no_clone: Mutex<HashSet<u64>>,
    ops: Mutex<Vec<Arc<OpShared>>>,
    changed: Condvar,
}

/// The file operation queue. Cheap to clone; all clones share one queue.
///
/// Every call returns quickly: ops run on their own threads, one at a time
/// per volume pair (configurable), with parallel file workers inside an op.
/// The exceptions are [`OpQueue::plan`], which reads metadata, and
/// [`OpQueue::enqueue`], which `lstat`s the first source and the destination
/// to pick the volume. Both are documented as blocking.
#[derive(Clone)]
pub struct OpQueue {
    inner: Arc<Inner>,
}

/// A volume-pair key of its own for each compound op: a compound op only
/// waits for its steps, which take the real volume slots, so it must never
/// hold one itself.
fn compound_key() -> (u64, u64) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    (u64::MAX, NEXT.fetch_add(1, Ordering::Relaxed))
}

fn dev_of(p: &Path) -> u64 {
    std::fs::symlink_metadata(p).map(|m| m.dev()).unwrap_or(0)
}

fn quoted(name: &str) -> String {
    format!("“{name}”")
}

/// "3 items" or "“a.txt”".
fn what(n: usize, first: &str) -> String {
    if n == 1 {
        quoted(first)
    } else {
        format!("{n} items")
    }
}

fn title_for(req: &OpRequest, origin: Origin) -> String {
    let ts = plan::targets(req);
    let n = ts.len();
    let first = ts.first().map(|t| file_name(&t.0)).unwrap_or_default();
    let w = what(n, &first);
    let into = |d: &Path| quoted(&file_name(d));
    let t = match req {
        OpRequest::Copy { dest_dir, .. } => format!("Copying {w} to {}", into(dest_dir)),
        OpRequest::Move { dest_dir, .. } => format!("Moving {w} to {}", into(dest_dir)),
        OpRequest::Trash { .. } => format!("Moving {w} to the Trash"),
        OpRequest::DeleteImmediately { .. } => format!("Deleting {w}"),
        OpRequest::NewFolder { name, .. } | OpRequest::NewFile { name, .. } => {
            format!("Creating {}", quoted(name))
        }
        OpRequest::Rename { new_name, .. } => format!("Renaming {w} to {}", quoted(new_name)),
        OpRequest::Duplicate { .. } => format!("Duplicating {w}"),
        OpRequest::MoveTo { .. } => format!("Moving {w} back"),
        OpRequest::Compound { label, .. } => label.clone(),
        OpRequest::UndoOf { .. } => "Undo".to_string(),
    };
    match origin {
        Origin::Undo => format!("Undo: {t}"),
        _ => t,
    }
}

/// The undo label for an op: "Copy of 3 Items", "Rename of “a.txt”", "New Folder".
pub fn undo_label(kind: OpKind, roots: usize, first: &str) -> String {
    match kind {
        // A compound op carries its own label ("New Folder with Items").
        OpKind::Compound => first.to_string(),
        OpKind::NewFolder | OpKind::NewFile => kind.verb().to_string(),
        _ if roots == 1 => format!("{} of {}", kind.verb(), quoted(first)),
        _ => format!("{} of {roots} Items", kind.verb()),
    }
}

impl Inner {
    fn find(&self, id: OpId) -> Option<Arc<OpShared>> {
        self.ops.lock().iter().find(|o| o.id == id).cloned()
    }

    /// Starts every queued op that has a free slot on its volume pair, in queue order.
    fn pump(self: &Arc<Self>) {
        let ops = self.ops.lock();
        let mut running: HashMap<(u64, u64), usize> = HashMap::new();
        for o in ops.iter() {
            if *o.phase.lock() == Phase::Running {
                *running.entry(o.key).or_default() += 1;
            }
        }
        for o in ops.iter() {
            let mut ph = o.phase.lock();
            if *ph != Phase::Queued || o.ctl.paused.load(Ordering::Acquire) {
                continue;
            }
            let n = running.entry(o.key).or_default();
            if *n >= self.cfg.ops_per_volume.max(1) {
                continue;
            }
            *n += 1;
            *ph = Phase::Running;
            let (me, op) = (Arc::clone(self), Arc::clone(o));
            std::thread::Builder::new()
                .name(format!("fstudio-op-{}", op.id.0))
                .spawn(move || {
                    if op.kind == OpKind::Compound {
                        run_compound(&me, &op);
                    } else {
                        crate::runner::run(&me, &op);
                    }
                    {
                        let _g = me.ops.lock();
                        *op.phase.lock() = Phase::Finished;
                        me.changed.notify_all();
                    }
                    me.pump();
                })
                .expect("spawn op thread");
        }
    }

    /// Bookkeeping when an op finishes: the undo stack.
    pub fn op_finished(&self, op: &OpShared, report: &OpReport) {
        let undoable = !matches!(op.kind, OpKind::DeleteImmediately);
        // Undo ops and the steps of a compound op are never undone by themselves.
        let on_stack = matches!(op.origin, Origin::User | Origin::Redo);
        if on_stack && undoable && report.files_done > 0 {
            let label = undo_label(op.kind, op.roots, &op.first_name);
            let _ = self
                .journal
                .push_stack(Stack::Undo, op.id, &label, self.cfg.undo_limit);
            self.sink.on_event(OpEvent::UndoChanged(self.undo_state()));
        }
    }

    pub fn undo_state(&self) -> UndoState {
        let peek = |s| self.journal.peek_stack(s).ok().flatten().map(|(_, l)| l);
        UndoState {
            undo: peek(Stack::Undo).map(|l| format!("Undo {l}")),
            redo: peek(Stack::Redo).map(|l| format!("Redo {l}")),
        }
    }
}

impl OpQueue {
    /// Opens the queue on `db` (running the `ops` migrations). Ops left
    /// unfinished by an earlier process are marked interrupted; see
    /// [`OpQueue::interrupted`].
    pub fn open(
        db: Db,
        cfg: OpsConfig,
        trasher: Arc<dyn Trasher>,
        sink: Arc<dyn EventSink>,
    ) -> Result<Self> {
        let journal = Journal::new(db)?;
        for op in journal.unfinished_ops()? {
            if op.state != OpState::Interrupted {
                journal.set_op_state(op.id, OpState::Interrupted)?;
            }
        }
        // Redo never crosses a launch (S3): the world may have changed in
        // between, and a forgotten "Redo Move of “Downloads”" is one key
        // away from repeating an accident.
        journal.clear_stack(Stack::Redo)?;
        Ok(Self {
            inner: Arc::new(Inner {
                journal,
                cfg,
                trasher,
                sink,
                no_clone: Mutex::new(HashSet::new()),
                ops: Mutex::new(Vec::new()),
                changed: Condvar::new(),
            }),
        })
    }

    /// Removes temporary files that failed copies left behind because their
    /// volume was gone at the time, now that it is back (call at launch and
    /// when volumes change, off the main thread: it touches the disk). Only
    /// files the journal records for an item of a finished op, named exactly
    /// as that item's temporary file, are removed. Returns what was removed.
    pub fn sweep_leftover_partials(&self) -> Result<Vec<crate::recover::SweptPartial>> {
        crate::recover::sweep_leftovers(&self.inner.journal)
    }

    /// The queue's settings.
    pub fn config(&self) -> &OpsConfig {
        &self.inner.cfg
    }

    /// A dry run: counts, bytes, conflicts, cross-volume flags, case
    /// collisions and iCloud placeholders. **Blocks on disk** (metadata only).
    pub fn plan(&self, req: &OpRequest) -> OpPlan {
        plan::plan(req, &self.inner.cfg)
    }

    /// Queues a request with default options. See [`OpQueue::enqueue_with`].
    pub fn enqueue(&self, req: OpRequest) -> Result<OpId> {
        self.enqueue_with(req, EnqueueOptions::default())
    }

    /// Queues a request. It is journaled before this returns, so it survives
    /// a crash. `lstat`s the first source and the destination folder to pick
    /// the volume worker.
    pub fn enqueue_with(&self, req: OpRequest, opts: EnqueueOptions) -> Result<OpId> {
        self.enqueue_origin(req, opts, Origin::User, None)
    }

    fn enqueue_origin(
        &self,
        req: OpRequest,
        opts: EnqueueOptions,
        origin: Origin,
        undo_of: Option<OpId>,
    ) -> Result<OpId> {
        self.enqueue_full(req, opts, origin, undo_of, None)
    }

    fn enqueue_full(
        &self,
        req: OpRequest,
        opts: EnqueueOptions,
        origin: Origin,
        undo_of: Option<OpId>,
        part_of: Option<(OpId, usize)>,
    ) -> Result<OpId> {
        plan::validate(&req).map_err(OpsError::InvalidRequest)?;
        if matches!(req, OpRequest::DeleteImmediately { .. })
            && !self.inner.cfg.allow_delete_immediately
        {
            return Err(OpsError::DeleteImmediatelyDisabled);
        }
        let kind = req.kind();
        let ts = plan::targets(&req);
        let roots: Vec<NewItem> = ts
            .iter()
            .map(|(src, dir, name)| NewItem {
                parent: None,
                kind: EntryKind::Other,
                source: src.clone(),
                dest_dir: dir.clone(),
                dest_name: name.clone(),
                src_ident: None,
            })
            .collect();
        let title = title_for(&req, origin);
        let src_dev = ts.first().map(|t| match kind {
            OpKind::NewFolder | OpKind::NewFile => dev_of(t.1.as_deref().unwrap_or(&t.0)),
            _ => dev_of(&t.0),
        });
        let dst_dev = ts.first().and_then(|t| t.1.as_deref()).map(dev_of);
        let key = if kind == OpKind::Compound {
            compound_key()
        } else {
            (src_dev.unwrap_or(0), dst_dev.or(src_dev).unwrap_or(0))
        };
        let id = self
            .inner
            .journal
            .insert_op(&req, &opts, origin, undo_of, &title, &roots, part_of)?;
        if origin == Origin::User {
            self.inner.journal.clear_stack(Stack::Redo)?;
            self.inner
                .sink
                .on_event(OpEvent::UndoChanged(self.inner.undo_state()));
        }
        let first = match &req {
            OpRequest::Compound { label, .. } => Some(label.clone()),
            _ => ts.first().map(|t| match &t.2 {
                Some(n) if kind != OpKind::Rename => n.clone(),
                _ => file_name(&t.0),
            }),
        };
        let shared = self.make_shared(
            id,
            kind,
            key,
            origin,
            roots.len(),
            first.unwrap_or_default(),
            title,
            opts,
        );
        self.inner.ops.lock().push(Arc::new(shared));
        self.inner.pump();
        Ok(id)
    }

    #[allow(clippy::too_many_arguments)]
    fn make_shared(
        &self,
        id: OpId,
        kind: OpKind,
        key: (u64, u64),
        origin: Origin,
        roots: usize,
        first_name: String,
        title: String,
        opts: EnqueueOptions,
    ) -> OpShared {
        let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded();
        OpShared {
            id,
            kind,
            key,
            origin,
            roots,
            first_name,
            ctl: OpCtl::new(opts.rules),
            status: Mutex::new(OpStatus {
                id,
                kind,
                title,
                state: OpState::Queued,
                files_done: 0,
                files_total: 0,
                bytes_done: 0,
                bytes_total: 0,
                current: None,
                speed_bps: 0.0,
                eta_secs: None,
                errors: Vec::new(),
                conflicts: Vec::new(),
            }),
            options: Mutex::new(opts),
            phase: Mutex::new(Phase::Queued),
            ctl_tx,
            ctl_rx,
        }
    }

    /// Every op started in this session (queued, running and finished), in
    /// queue order.
    pub fn list(&self) -> Vec<OpStatus> {
        self.inner
            .ops
            .lock()
            .iter()
            .map(|o| o.status.lock().clone())
            .collect()
    }

    /// One op's status (this session's ops only).
    pub fn status(&self, id: OpId) -> Option<OpStatus> {
        self.inner.find(id).map(|o| o.status.lock().clone())
    }

    /// Pauses an op. A running copy stops inside the current file.
    pub fn pause(&self, id: OpId) -> Result<()> {
        let o = self.inner.find(id).ok_or(OpsError::NoSuchOp(id))?;
        o.ctl.paused.store(true, Ordering::Release);
        {
            let mut st = o.status.lock();
            if !st.state.is_finished() {
                st.state = OpState::Paused;
            }
        }
        let _ = self.inner.journal.set_op_state(id, OpState::Paused);
        let _ = o.ctl_tx.send(Ctl::Wake);
        self.inner
            .sink
            .on_event(OpEvent::OpProgress(o.status.lock().clone()));
        Ok(())
    }

    /// Resumes a paused op, or an op interrupted by a crash or quit.
    ///
    /// For an interrupted op this first removes our own leftover temporary
    /// files (only names ending in `.fstudio-partial` that the journal
    /// recorded), works out how far each item got, then queues the rest.
    /// A move whose copy finished but whose source wasn't removed yet only
    /// removes the source; nothing is copied twice.
    pub fn resume(&self, id: OpId) -> Result<()> {
        if let Some(o) = self.inner.find(id) {
            o.ctl.paused.store(false, Ordering::Release);
            {
                let mut st = o.status.lock();
                if st.state == OpState::Paused {
                    st.state = if *o.phase.lock() == Phase::Running {
                        OpState::Running
                    } else {
                        OpState::Queued
                    };
                }
            }
            let _ = o.ctl_tx.send(Ctl::Wake);
            self.inner.pump();
            return Ok(());
        }
        let row = self
            .inner
            .journal
            .load_op(id)?
            .ok_or(OpsError::NoSuchOp(id))?;
        if row.state != OpState::Interrupted {
            return Err(OpsError::InvalidRequest(format!("{id} is not interrupted")));
        }
        recover::recover(&self.inner.journal, &row)?;
        self.inner.journal.set_op_state(id, OpState::Queued)?;
        let ts = plan::targets(&row.request);
        let kind = row.request.kind();
        let src_dev = ts.first().map(|t| dev_of(&t.0)).unwrap_or(0);
        let dst_dev = ts
            .first()
            .and_then(|t| t.1.as_deref())
            .map(dev_of)
            .unwrap_or(src_dev);
        let first = match &row.request {
            OpRequest::Compound { label, .. } => label.clone(),
            _ => ts.first().map(|t| file_name(&t.0)).unwrap_or_default(),
        };
        let key = if kind == OpKind::Compound {
            compound_key()
        } else {
            (src_dev, dst_dev)
        };
        let shared = self.make_shared(
            id,
            kind,
            key,
            row.origin,
            ts.len(),
            first,
            row.title.clone(),
            row.options.clone(),
        );
        self.inner.ops.lock().push(Arc::new(shared));
        self.inner.pump();
        Ok(())
    }

    /// Cancels an op. Items already finished stay; the item in flight is
    /// stopped and its temporary file removed.
    ///
    /// An op interrupted by an earlier run (see [`OpQueue::interrupted`]) can
    /// be cancelled without resuming it: our leftover temporary files are
    /// removed, nothing else is touched (a move's sources stay where they
    /// are), and the op is marked cancelled.
    pub fn cancel(&self, id: OpId) -> Result<()> {
        let Some(o) = self.inner.find(id) else {
            return self.cancel_interrupted(id);
        };
        o.ctl.cancel.store(true, Ordering::Release);
        let _ = o.ctl_tx.send(Ctl::Wake);
        let guard = self.inner.ops.lock();
        let mut ph = o.phase.lock();
        if *ph == Phase::Queued {
            *ph = Phase::Finished;
            drop(ph);
            self.inner.changed.notify_all();
            drop(guard);
            o.status.lock().state = OpState::Cancelled;
            self.inner.journal.set_op_state(id, OpState::Cancelled)?;
            let report = OpReport {
                id,
                kind: o.kind,
                state: OpState::Cancelled,
                files_done: 0,
                files_failed: 0,
                files_skipped: 0,
                bytes_done: 0,
                errors: Vec::new(),
                elapsed_ms: 0,
            };
            self.inner
                .sink
                .on_event(OpEvent::OpFinished { op: id, report });
        }
        Ok(())
    }

    /// Cancels an op that is only interrupted in the journal (not loaded in
    /// this session). Runs the same recovery as resume, which removes only
    /// our own recorded partial files, then marks the op cancelled.
    fn cancel_interrupted(&self, id: OpId) -> Result<()> {
        let row = self
            .inner
            .journal
            .load_op(id)?
            .ok_or(OpsError::NoSuchOp(id))?;
        if row.state != OpState::Interrupted {
            return Err(OpsError::NoSuchOp(id));
        }
        recover::recover(&self.inner.journal, &row)?;
        self.inner.journal.set_op_state(id, OpState::Cancelled)?;
        let items = self.inner.journal.load_items(id)?;
        let st = status_from_journal(&row, &items);
        let report = OpReport {
            id,
            kind: row.request.kind(),
            state: OpState::Cancelled,
            files_done: st.files_done,
            files_failed: st.errors.len() as u64,
            files_skipped: 0,
            bytes_done: st.bytes_done,
            errors: st.errors,
            elapsed_ms: 0,
        };
        self.inner
            .sink
            .on_event(OpEvent::OpFinished { op: id, report });
        Ok(())
    }

    /// Moves a queued op to position `new_index` among this session's ops.
    /// Running ops keep running; this changes which queued op starts next.
    pub fn reorder(&self, id: OpId, new_index: usize) -> Result<()> {
        {
            let mut ops = self.inner.ops.lock();
            let from = ops
                .iter()
                .position(|o| o.id == id)
                .ok_or(OpsError::NoSuchOp(id))?;
            let o = ops.remove(from);
            let to = new_index.min(ops.len());
            ops.insert(to, o);
            for (i, o) in ops.iter().enumerate() {
                let _ = self.inner.journal.set_op_seq(o.id, i as f64 + 1.0);
            }
        }
        self.inner.pump();
        Ok(())
    }

    /// Answers a conflict. With `apply_to_all`, the decision also answers every
    /// other conflict of the same class in this op (folder onto folder, or
    /// anything else), now and later. `Merge` is only valid for folders.
    pub fn resolve_conflict(
        &self,
        id: OpId,
        conflict: ConflictId,
        decision: ConflictDecision,
        apply_to_all: bool,
    ) -> Result<()> {
        let o = self.inner.find(id).ok_or(OpsError::NoSuchOp(id))?;
        let mut st = o.status.lock();
        let info = st
            .conflicts
            .iter()
            .find(|c| c.id == conflict)
            .cloned()
            .ok_or(OpsError::NoSuchConflict)?;
        if decision == ConflictDecision::Merge && !info.can_merge {
            return Err(OpsError::InvalidDecision(decision));
        }
        // Answered: it leaves the status now, not when the scheduler gets to it.
        st.conflicts.retain(|c| c.id != conflict);
        drop(st);
        if apply_to_all {
            let mut opts = o.options.lock();
            if info.can_merge {
                opts.rules.folders = Some(decision);
            } else {
                opts.rules.others = Some(decision);
            }
            *o.ctl.rules.write() = opts.rules;
            self.inner.journal.set_op_options(id, &opts)?;
        }
        let _ = o.ctl_tx.send(Ctl::Decide(conflict, decision));
        Ok(())
    }

    /// Ops found unfinished in the journal (after a crash or quit) and not
    /// resumed yet.
    pub fn interrupted(&self) -> Result<Vec<OpStatus>> {
        let mut out = Vec::new();
        for row in self.inner.journal.unfinished_ops()? {
            if row.state != OpState::Interrupted || self.inner.find(row.id).is_some() {
                continue;
            }
            let items = self.inner.journal.load_items(row.id)?;
            out.push(status_from_journal(&row, &items));
        }
        Ok(out)
    }

    /// Blocks until the op finishes (or `timeout` passes) and returns its status.
    pub fn wait(&self, id: OpId, timeout: Duration) -> Option<OpStatus> {
        let o = self.inner.find(id)?;
        let deadline = std::time::Instant::now() + timeout;
        let mut guard = self.inner.ops.lock();
        while *o.phase.lock() != Phase::Finished {
            if self
                .inner
                .changed
                .wait_until(&mut guard, deadline)
                .timed_out()
            {
                break;
            }
        }
        drop(guard);
        Some(o.status.lock().clone())
    }

    /// Blocks until no op is queued or running (or `timeout` passes).
    /// Returns `true` if the queue is idle.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        let mut guard = self.inner.ops.lock();
        loop {
            let busy = guard.iter().any(|o| {
                *o.phase.lock() != Phase::Finished && !o.ctl.paused.load(Ordering::Acquire)
            });
            if !busy {
                return true;
            }
            if self
                .inner
                .changed
                .wait_until(&mut guard, deadline)
                .timed_out()
            {
                return false;
            }
        }
    }

    /// Forgets finished ops from [`OpQueue::list`].
    pub fn clear_finished(&self) {
        self.inner
            .ops
            .lock()
            .retain(|o| *o.phase.lock() != Phase::Finished);
    }

    /// Edit-menu labels for undo and redo.
    pub fn undo_state(&self) -> UndoState {
        self.inner.undo_state()
    }

    /// Undoes the most recent undoable op by queuing its inverse:
    /// copy → trash the copies; move → move back; trash → put back;
    /// rename → rename back; new folder → trash it if still empty;
    /// duplicate → trash the duplicates. Items that were replaced are put
    /// back from the Trash.
    ///
    /// Refuses with [`OpsError::UndoRefused`] if anything changed since (a copy
    /// was modified, an original location is taken…); it never overwrites.
    /// Returns the queued inverse ops (empty if nothing to undo). Blocks on
    /// `lstat` of the affected items.
    ///
    /// When an item would go back into a folder that no longer exists, undo
    /// stops with [`OpsError::NeedsFolders`] (S14): nothing is recreated
    /// unasked. [`OpQueue::undo_recreating`] then recreates those folders as
    /// journaled steps of the same undo.
    pub fn undo(&self) -> Result<Vec<OpId>> {
        self.undo_with(None)
    }

    /// Undo after the user agreed to recreate the missing folders that
    /// [`OpQueue::undo`] reported, if `expect` is still the op on top of the
    /// undo stack. The folders are created by the undo op itself (a
    /// compound op), so they are in the journal.
    pub fn undo_recreating(&self, expect: OpId) -> Result<Vec<OpId>> {
        self.undo_with(Some(expect))
    }

    fn undo_with(&self, recreate_for: Option<OpId>) -> Result<Vec<OpId>> {
        let Some((op, label)) = self.inner.journal.peek_stack(Stack::Undo)? else {
            return Ok(Vec::new());
        };
        if recreate_for.is_some_and(|e| e != op) {
            return Err(OpsError::UndoRefused(
                "something else was undone or done meanwhile".into(),
            ));
        }
        if let Some(o) = self.inner.find(op)
            && *o.phase.lock() != Phase::Finished
        {
            return Err(OpsError::UndoRefused("it is still running".into()));
        }
        let row = self
            .inner
            .journal
            .load_op(op)?
            .ok_or(OpsError::NoSuchOp(op))?;
        let mut inverse = self.inverse_of(&row)?;
        let missing = missing_parents(&inverse);
        if !missing.is_empty() {
            // A folder the op itself emptied and removed (the source of a
            // merging move) comes back without asking; one that went away
            // some other way needs the user's yes.
            let own: Vec<PathBuf> = self
                .items_deep(op)?
                .into_iter()
                .filter(|i| i.kind == EntryKind::Dir && i.state == crate::journal::ItemState::Done)
                .map(|i| i.source)
                .collect();
            let asked = missing.iter().any(|d| !own.contains(d));
            if asked && recreate_for.is_none() {
                return Err(OpsError::NeedsFolders(missing));
            }
            let mut steps: Vec<OpRequest> = missing
                .iter()
                .map(|d| OpRequest::NewFolder {
                    parent: d.parent().map(Path::to_path_buf).unwrap_or_default(),
                    name: file_name(d),
                })
                .collect();
            steps.extend(inverse);
            inverse = vec![OpRequest::Compound {
                label: label.clone(),
                steps,
            }];
        }
        let mut ids = Vec::new();
        for req in inverse {
            ids.push(self.enqueue_origin(
                req,
                EnqueueOptions::default(),
                Origin::Undo,
                Some(op),
            )?);
        }
        let label = self
            .inner
            .journal
            .peek_stack(Stack::Undo)?
            .map(|(_, l)| l)
            .unwrap_or_default();
        self.inner.journal.pop_stack(Stack::Undo)?;
        self.inner
            .journal
            .push_stack(Stack::Redo, op, &label, self.inner.cfg.undo_limit)?;
        self.inner
            .sink
            .on_event(OpEvent::UndoChanged(self.inner.undo_state()));
        Ok(ids)
    }

    /// The requests that undo `row` (see [`undo::inverse`]). A compound op
    /// is undone by one compound op that undoes its steps in reverse order,
    /// each worked out when it starts; the last step is checked now, so an
    /// undo that can't work is refused up front.
    fn inverse_of(&self, row: &crate::journal::OpRow) -> Result<Vec<OpRequest>> {
        if let OpRequest::Compound { label, .. } = &row.request {
            let parts: Vec<_> = self
                .inner
                .journal
                .parts_of(row.id)?
                .into_iter()
                .filter(|p| p.state.is_finished())
                // Only steps that changed something (a failed step may not have).
                .filter(|p| {
                    self.items_deep(p.id).is_ok_and(|items| {
                        items
                            .iter()
                            .any(|i| i.state == crate::journal::ItemState::Done)
                    })
                })
                .collect();
            let Some(last) = parts.last() else {
                return Err(OpsError::UndoRefused("nothing was changed".into()));
            };
            let items = self.inner.journal.load_items(last.id)?;
            undo::inverse(last, &items).map_err(OpsError::UndoRefused)?;
            return Ok(vec![OpRequest::Compound {
                label: label.clone(),
                steps: parts
                    .iter()
                    .rev()
                    .map(|p| OpRequest::UndoOf { op: p.id })
                    .collect(),
            }]);
        }
        let items = self.inner.journal.load_items(row.id)?;
        undo::inverse(row, &items).map_err(OpsError::UndoRefused)
    }

    /// The journal items of `op` and, for a compound op, of its steps.
    fn items_deep(&self, op: OpId) -> Result<Vec<ItemRow>> {
        let mut out = self.inner.journal.load_items(op)?;
        for p in self.inner.journal.parts_of(op)? {
            out.extend(self.items_deep(p.id)?);
        }
        Ok(out)
    }

    /// The op Redo would redo and its request, if any.
    pub fn redo_peek(&self) -> Result<Option<(OpId, OpRequest)>> {
        Ok(self.peek(Stack::Redo)?.map(|(op, _, req)| (op, req)))
    }

    /// The op on top of the undo stack: its id, label ("Move of “a”") and request.
    pub fn undo_top(&self) -> Result<Option<(OpId, String, OpRequest)>> {
        self.peek(Stack::Undo)
    }

    /// The op on top of the redo stack: its id, label and request.
    pub fn redo_top(&self) -> Result<Option<(OpId, String, OpRequest)>> {
        self.peek(Stack::Redo)
    }

    fn peek(&self, stack: Stack) -> Result<Option<(OpId, String, OpRequest)>> {
        let Some((op, label)) = self.inner.journal.peek_stack(stack)? else {
            return Ok(None);
        };
        let row = self
            .inner
            .journal
            .load_op(op)?
            .ok_or(OpsError::NoSuchOp(op))?;
        Ok(Some((op, label, row.request)))
    }

    /// Redoes the most recently undone op by queuing its original request
    /// again, but only if every item it acts on is still the object the undo
    /// put back (S3); otherwise [`OpsError::RedoRefused`] says what changed.
    /// The redo stack is emptied at every launch. Blocks on `lstat`.
    pub fn redo(&self) -> Result<Option<OpId>> {
        self.redo_op(None)
    }

    /// [`OpQueue::redo`], but only if the op on top of the redo stack is
    /// still `expect` (for a redo the user confirmed).
    pub fn redo_op(&self, expect: Option<OpId>) -> Result<Option<OpId>> {
        let Some((op, _)) = self.inner.journal.peek_stack(Stack::Redo)? else {
            return Ok(None);
        };
        if expect.is_some_and(|e| e != op) {
            return Err(OpsError::RedoRefused(
                "something else was undone or redone meanwhile".into(),
            ));
        }
        let row = self
            .inner
            .journal
            .load_op(op)?
            .ok_or(OpsError::NoSuchOp(op))?;
        let items = self.items_deep(op)?;
        let mut undo_items = Vec::new();
        for u in self.inner.journal.undo_ops_of(op)? {
            let finished = match self.inner.find(u.id) {
                Some(o) => *o.phase.lock() == Phase::Finished,
                None => u.state.is_finished(),
            };
            if !finished {
                return Err(OpsError::RedoRefused("the undo is still running".into()));
            }
            undo_items.extend(self.items_deep(u.id)?);
        }
        undo::redo_check(&row, &items, &undo_items).map_err(OpsError::RedoRefused)?;
        self.inner.journal.pop_stack(Stack::Redo)?;
        let id =
            self.enqueue_origin(row.request.clone(), row.options.clone(), Origin::Redo, None)?;
        self.inner
            .sink
            .on_event(OpEvent::UndoChanged(self.inner.undo_state()));
        Ok(Some(id))
    }
}

/// The folders that items of these (undo) requests would go back into but
/// that no longer exist, outermost first (S14).
fn missing_parents(reqs: &[OpRequest]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for r in reqs {
        let OpRequest::MoveTo { pairs } = r else {
            continue;
        };
        for (_, to) in pairs {
            let mut d = to.parent();
            while let Some(dir) = d {
                if dir.as_os_str().is_empty() || crate::meta::exists(dir) {
                    break;
                }
                if !out.iter().any(|o| o == dir) {
                    out.push(dir.to_path_buf());
                }
                d = dir.parent();
            }
        }
    }
    out.sort_by_key(|p| p.components().count());
    out
}

/// Runs a compound op (see [`OpRequest::Compound`]): each step as an op of
/// its own (origin `Part`), one after another, stopping at the first step
/// that doesn't complete. A resumed compound op picks up its steps where they
/// were. Never blocks a volume slot itself.
fn run_compound(inner: &Arc<Inner>, op: &OpShared) {
    let q = OpQueue {
        inner: Arc::clone(inner),
    };
    let started = std::time::Instant::now();
    let journal = &inner.journal;
    let (steps, options) = match journal.load_op(op.id) {
        Ok(Some(row)) => match row.request {
            OpRequest::Compound { steps, .. } => (steps, row.options),
            _ => (Vec::new(), row.options),
        },
        _ => (Vec::new(), EnqueueOptions::default()),
    };
    {
        let mut st = op.status.lock();
        st.state = OpState::Running;
        st.files_total = steps.len() as u64;
    }
    let _ = journal.set_op_state(op.id, OpState::Running);
    inner
        .sink
        .on_event(OpEvent::OpProgress(op.status.lock().clone()));
    let existing = journal.parts_of(op.id).unwrap_or_default();
    let mut done = 0u64;
    let mut errors: Vec<crate::types::ItemError> = Vec::new();
    let fail = |errors: &mut Vec<crate::types::ItemError>, msg: String| {
        errors.push(crate::types::ItemError {
            path: PathBuf::new(),
            dest: None,
            message: msg,
            errno: None,
        });
    };
    'steps: for (i, step) in steps.iter().enumerate() {
        if op.ctl.cancelled() {
            break;
        }
        let mine: Vec<OpId> = existing
            .iter()
            .filter(|p| p.part_of.map(|x| x.1) == Some(i))
            .map(|p| p.id)
            .collect();
        let ids = if !mine.is_empty() {
            // Resuming: the step's ops exist already.
            for id in &mine {
                let _ = q.resume(*id);
            }
            mine
        } else {
            let reqs = match step {
                OpRequest::UndoOf { op: target } => {
                    let row = match journal.load_op(*target) {
                        Ok(Some(r)) => r,
                        _ => {
                            fail(&mut errors, format!("{target} is missing from the journal"));
                            break;
                        }
                    };
                    match q.inverse_of(&row) {
                        Ok(r) => r,
                        Err(OpsError::UndoRefused(why)) => {
                            fail(&mut errors, why);
                            break;
                        }
                        Err(e) => {
                            fail(&mut errors, e.to_string());
                            break;
                        }
                    }
                }
                other => vec![other.clone()],
            };
            let mut ids = Vec::new();
            for r in reqs {
                match q.enqueue_full(r, options.clone(), Origin::Part, None, Some((op.id, i))) {
                    Ok(id) => ids.push(id),
                    Err(e) => {
                        fail(&mut errors, e.to_string());
                        break 'steps;
                    }
                }
            }
            ids
        };
        for id in ids {
            // A step may wait for a conflict decision for as long as it takes.
            let st = loop {
                if op.ctl.cancelled() {
                    let _ = q.cancel(id);
                }
                match q.wait(id, Duration::from_millis(250)) {
                    Some(st) if st.state.is_finished() => break Some(st),
                    Some(_) => {}
                    None => break None,
                }
            };
            match st {
                Some(st) if st.state == OpState::Completed => {}
                Some(st) => {
                    if st.errors.is_empty() {
                        fail(&mut errors, format!("“{}” didn't finish", st.title));
                    }
                    errors.extend(st.errors);
                    break 'steps;
                }
                None => {
                    fail(&mut errors, "a step was lost".into());
                    break 'steps;
                }
            }
        }
        done += 1;
        {
            let mut st = op.status.lock();
            st.files_done = done;
        }
        inner
            .sink
            .on_event(OpEvent::OpProgress(op.status.lock().clone()));
    }
    let state = if op.ctl.cancelled() {
        OpState::Cancelled
    } else if !errors.is_empty() {
        OpState::CompletedWithErrors
    } else {
        OpState::Completed
    };
    let _ = journal.set_op_state(op.id, state);
    let report = {
        let mut st = op.status.lock();
        st.state = state;
        st.files_done = done;
        st.errors = errors.clone();
        st.current = None;
        OpReport {
            id: op.id,
            kind: op.kind,
            state,
            files_done: done,
            files_failed: u64::from(!errors.is_empty()),
            files_skipped: 0,
            bytes_done: 0,
            errors,
            elapsed_ms: started.elapsed().as_millis() as u64,
        }
    };
    inner
        .sink
        .on_event(OpEvent::OpProgress(op.status.lock().clone()));
    inner.op_finished(op, &report);
    inner
        .sink
        .on_event(OpEvent::OpFinished { op: op.id, report });
}

/// A status for an op that isn't loaded, from its journal rows.
fn status_from_journal(row: &OpRow, items: &[ItemRow]) -> OpStatus {
    let done = items.iter().filter(|i| i.state.is_terminal()).count() as u64;
    let bytes_done = items
        .iter()
        .filter(|i| i.state == ItemState::Done)
        .map(|i| i.bytes)
        .sum();
    let (files_total, bytes_total) = row
        .plan
        .as_ref()
        .map(|p| (p.files + p.dirs, p.bytes))
        .unwrap_or((items.len() as u64, 0));
    let errors = items
        .iter()
        .filter(|i| i.state == ItemState::Failed)
        .map(|i| crate::types::ItemError {
            path: i.source.clone(),
            dest: i.dest.clone(),
            message: i.error.clone().unwrap_or_default(),
            errno: None,
        })
        .collect();
    OpStatus {
        id: row.id,
        kind: row.request.kind(),
        title: row.title.clone(),
        state: row.state,
        files_done: done,
        files_total: files_total.max(done),
        bytes_done,
        bytes_total,
        current: None,
        speed_bps: 0.0,
        eta_secs: None,
        errors,
        conflicts: Vec::new(),
    }
}

#[allow(dead_code)]
fn _assert_send_sync() {
    fn f<T: Send + Sync>() {}
    f::<OpQueue>();
    let _ = PathBuf::new();
}

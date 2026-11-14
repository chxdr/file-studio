//! Runs one op: schedules its items over a small worker pool, respecting
//! folder order (a folder is placed before its contents and finalized after
//! them), routes conflicts to the user without stopping the other items, and
//! emits coalesced progress.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, select};
use fstudio_model::EntryKind;

use crate::engine::{Env, Outcome, Task};
use crate::journal::{ItemRow, ItemState};
use crate::plan;
use crate::queue::{Ctl, Inner, OpShared};
use crate::types::{ConflictDecision, ItemError, OpEvent, OpKind, OpPlan, OpReport, OpState};

struct Node {
    row: ItemRow,
    /// Children not yet finished (folders only).
    left: usize,
    /// All finished children are done (none failed or skipped).
    ok: bool,
}

/// What the scheduler knows about an op's items.
struct Graph {
    nodes: HashMap<i64, Node>,
    ready: BTreeSet<i64>,
    finalize: BTreeSet<i64>,
    /// Waiting conflicts → whether each is folder-onto-folder (its rule class).
    conflicts: BTreeMap<i64, bool>,
    files_done: u64,
    failed: u64,
    skipped: u64,
}

impl Graph {
    fn new(items: Vec<ItemRow>) -> Self {
        let mut g = Graph {
            nodes: HashMap::new(),
            ready: BTreeSet::new(),
            finalize: BTreeSet::new(),
            conflicts: BTreeMap::new(),
            files_done: 0,
            failed: 0,
            skipped: 0,
        };
        for row in items {
            g.nodes.insert(
                row.id,
                Node {
                    row,
                    left: 0,
                    ok: true,
                },
            );
        }
        let ids: Vec<i64> = g.nodes.keys().copied().collect();
        for id in &ids {
            let (parent, state) = {
                let n = &g.nodes[id];
                (n.row.parent, n.row.state)
            };
            match state {
                ItemState::Done => g.files_done += 1,
                ItemState::Failed => {
                    g.files_done += 1;
                    g.failed += 1;
                }
                ItemState::Skipped => {
                    g.files_done += 1;
                    g.skipped += 1;
                }
                _ => {}
            }
            if let Some(p) = parent
                && let Some(pn) = g.nodes.get_mut(&p)
            {
                if !state.is_terminal() {
                    pn.left += 1;
                } else if state != ItemState::Done {
                    pn.ok = false;
                }
            }
        }
        for id in ids {
            if g.runnable(id) {
                g.ready.insert(id);
            }
            let n = &g.nodes[&id];
            if n.row.kind == EntryKind::Dir
                && n.row.state == ItemState::Placed
                && n.row.expanded
                && n.left == 0
            {
                g.finalize.insert(id);
            }
        }
        g
    }

    /// Whether an item can be handed to a worker now.
    fn runnable(&self, id: i64) -> bool {
        let n = &self.nodes[&id];
        let state_ok = match n.row.state {
            ItemState::Pending | ItemState::Conflict => true,
            ItemState::Placed => !(n.row.kind == EntryKind::Dir && n.row.expanded),
            _ => false,
        };
        let parent_ok = match n.row.parent {
            None => true,
            Some(p) => self
                .nodes
                .get(&p)
                .is_some_and(|pn| pn.row.state == ItemState::Placed && pn.row.expanded),
        };
        state_ok && parent_ok
    }

    fn parent_dest(&self, id: i64) -> Option<PathBuf> {
        let p = self.nodes[&id].row.parent?;
        self.nodes.get(&p).and_then(|n| n.row.dest.clone())
    }

    /// Records a finished item and wakes its parent's finalize when due.
    fn finished(&mut self, row: ItemRow) {
        let (id, parent, state) = (row.id, row.parent, row.state);
        match state {
            ItemState::Failed => self.failed += 1,
            ItemState::Skipped => self.skipped += 1,
            _ => {}
        }
        self.files_done += 1;
        if let Some(n) = self.nodes.get_mut(&id) {
            n.row = row;
        }
        if let Some(p) = parent
            && let Some(pn) = self.nodes.get_mut(&p)
        {
            pn.left = pn.left.saturating_sub(1);
            if state != ItemState::Done {
                pn.ok = false;
            }
            if pn.left == 0 && pn.row.state == ItemState::Placed && pn.row.expanded {
                self.finalize.insert(p);
            }
        }
    }
}

/// Item totals for progress: a same-volume move of a folder is one item.
fn totals(kind: OpKind, plan: &OpPlan) -> (u64, u64) {
    let moves = matches!(kind, OpKind::Move | OpKind::MoveTo | OpKind::Rename);
    let files = plan
        .roots
        .iter()
        .map(|r| {
            if !kind.copies() || (moves && !r.cross_volume) {
                1
            } else {
                r.files + r.dirs
            }
        })
        .sum();
    (files, plan.bytes)
}

/// Runs an op to the end (finished, failed or cancelled).
pub fn run(inner: &Inner, op: &OpShared) {
    let started = Instant::now();
    let journal = &inner.journal;
    let fail_all = |msg: String| {
        let report = OpReport {
            id: op.id,
            kind: op.kind,
            state: OpState::Failed,
            files_done: 0,
            files_failed: 0,
            files_skipped: 0,
            bytes_done: 0,
            errors: vec![ItemError {
                path: PathBuf::new(),
                dest: None,
                message: msg,
                errno: None,
            }],
            elapsed_ms: started.elapsed().as_millis() as u64,
        };
        let _ = journal.set_op_state(op.id, OpState::Failed);
        op.status.lock().state = OpState::Failed;
        inner
            .sink
            .on_event(OpEvent::OpFinished { op: op.id, report });
    };
    let row = match journal.load_op(op.id) {
        Ok(Some(r)) => r,
        Ok(None) => return fail_all("the op is missing from the journal".into()),
        Err(e) => return fail_all(format!("journal: {e}")),
    };
    // A batch rename runs its own ordering (swaps and cycles) and needs no
    // plan (which would walk every renamed folder's contents).
    if matches!(row.request, crate::types::OpRequest::RenameMany { .. }) {
        return match journal.load_items(op.id) {
            Ok(items) => crate::batch_rename::run(inner, op, items),
            Err(e) => fail_all(format!("journal: {e}")),
        };
    }
    let plan = match row.plan.clone() {
        Some(p) => p,
        None => {
            let p = plan::plan(&row.request, &inner.cfg);
            let _ = journal.set_op_plan(op.id, &p);
            p
        }
    };
    if let Some(why) = &plan.refused {
        return fail_all(why.clone());
    }
    let items = match journal.load_items(op.id) {
        Ok(i) => i,
        Err(e) => return fail_all(format!("journal: {e}")),
    };
    // Bytes already done by an earlier (interrupted) run.
    let prior: u64 = items
        .iter()
        .filter(|i| i.state == ItemState::Done && i.kind != EntryKind::Dir)
        .map(|i| i.bytes)
        .sum();
    op.ctl.bytes_done.store(prior, Ordering::Relaxed);
    {
        let (files, bytes) = totals(op.kind, &plan);
        let mut st = op.status.lock();
        st.files_total = files;
        st.bytes_total = bytes;
        // A pause() that landed before this thread started must stick.
        st.state = if op.ctl.paused.load(Ordering::Acquire) {
            OpState::Paused
        } else {
            OpState::Running
        };
    }
    let _ = journal.set_op_state(op.id, op.status.lock().state);

    // Safe Transfer (F-026): an op that already has a job resumes on it; a
    // new copy goes to it when the backend says so. An op ops' own engine
    // started stays on ops.
    let started_on_ops = items.iter().any(|i| i.state != ItemState::Pending);
    let route = if row.st_job.is_some() {
        crate::st_bridge::route(inner, &row.request, true)
    } else if started_on_ops {
        None
    } else {
        crate::st_bridge::route(inner, &row.request, false)
    };
    if let Some(r) = route {
        return crate::st_bridge::run(inner, op, &row, &plan, r);
    }
    if row.st_job.is_some() {
        return fail_all("this copy was started by Safe Transfer, which isn't open".into());
    }

    let mut g = Graph::new(items);
    let workers = if op.kind.copies() {
        inner.cfg.file_workers.max(1)
    } else {
        1
    };
    let env = Env {
        journal,
        cfg: &inner.cfg,
        trasher: inner.trasher.as_ref(),
        ctl: &op.ctl,
        op: op.id,
        kind: op.kind,
        allow_dataless: op.options.lock().allow_dataless,
        no_clone: &inner.no_clone,
    };

    let (work_tx, work_rx) = crossbeam_channel::unbounded::<Task>();
    let (done_tx, done_rx) = crossbeam_channel::unbounded::<Outcome>();
    std::thread::scope(|s| {
        for _ in 0..workers {
            let (rx, tx, env) = (work_rx.clone(), done_tx.clone(), &env);
            s.spawn(move || {
                for task in rx.iter() {
                    if tx.send(env.run(task)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(done_tx);
        schedule(inner, op, &mut g, workers, &work_tx, &done_rx);
        drop(work_tx);
    });

    let cancelled = op.ctl.cancelled();
    let state = if cancelled {
        OpState::Cancelled
    } else if g.failed > 0 {
        OpState::CompletedWithErrors
    } else {
        OpState::Completed
    };
    let _ = journal.set_op_state(op.id, state);
    let report = {
        let mut st = op.status.lock();
        st.state = state;
        st.files_done = g.files_done;
        st.bytes_done = op.ctl.bytes_done.load(Ordering::Relaxed);
        st.current = None;
        st.conflicts.clear();
        st.eta_secs = None;
        OpReport {
            id: op.id,
            kind: op.kind,
            state,
            files_done: g.files_done - g.failed - g.skipped,
            files_failed: g.failed,
            files_skipped: g.skipped,
            bytes_done: st.bytes_done,
            errors: st.errors.clone(),
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

fn schedule(
    inner: &Inner,
    op: &OpShared,
    g: &mut Graph,
    workers: usize,
    work_tx: &Sender<Task>,
    done_rx: &Receiver<Outcome>,
) {
    let mut in_flight = 0usize;
    let mut decisions: HashMap<i64, ConflictDecision> = HashMap::new();
    let mut last_emit = Instant::now() - inner.cfg.progress_interval;
    let mut last_bytes = op.ctl.bytes_done.load(Ordering::Relaxed);
    let mut last_t = Instant::now();
    let mut dirty = true;
    loop {
        let cancelled = op.ctl.cancelled();
        let paused = op.ctl.paused.load(Ordering::Acquire);
        if !cancelled && !paused {
            // Conflicts whose class gained a rule ("apply to all") run again.
            let rules = *op.ctl.rules.read();
            let covered: Vec<i64> = g
                .conflicts
                .iter()
                .filter(|(_, folder)| rules.for_class(**folder).is_some())
                .map(|(id, _)| *id)
                .collect();
            for id in covered {
                g.conflicts.remove(&id);
                op.status.lock().conflicts.retain(|c| c.id.0 != id);
                g.ready.insert(id);
                dirty = true;
            }
            while in_flight < workers {
                let task = if let Some(id) = g.finalize.pop_first() {
                    let n = &g.nodes[&id];
                    Task::Finalize {
                        item: n.row.clone(),
                        children_ok: n.ok,
                    }
                } else if let Some(id) = g.ready.pop_first() {
                    Task::Run {
                        item: g.nodes[&id].row.clone(),
                        parent_dest: g.parent_dest(id),
                        decision: decisions.remove(&id),
                    }
                } else {
                    break;
                };
                if work_tx.send(task).is_err() {
                    return;
                }
                in_flight += 1;
            }
        }
        if in_flight == 0
            && (cancelled
                || (g.ready.is_empty() && g.finalize.is_empty() && g.conflicts.is_empty()))
        {
            return;
        }
        {
            let mut st = op.status.lock();
            let want = if paused {
                OpState::Paused
            } else if in_flight == 0 && g.ready.is_empty() && !g.conflicts.is_empty() {
                OpState::WaitingForDecision
            } else {
                OpState::Running
            };
            if st.state != want {
                st.state = want;
                dirty = true;
            }
        }
        select! {
            recv(done_rx) -> out => {
                let Ok(out) = out else { return };
                in_flight -= 1;
                dirty = true;
                handle(inner, op, g, out);
            }
            recv(op.ctl_rx) -> msg => {
                if let Ok(Ctl::Decide(id, d)) = msg
                    && g.conflicts.remove(&id.0).is_some()
                {
                    op.status.lock().conflicts.retain(|c| c.id != id);
                    if d == ConflictDecision::Skip {
                        // Skip needs no worker.
                        if let Some(n) = g.nodes.get(&id.0) {
                            let mut row = n.row.clone();
                            row.state = ItemState::Skipped;
                            let _ = inner.journal.save_item(&row);
                            g.finished(row);
                        }
                    } else {
                        decisions.insert(id.0, d);
                        g.ready.insert(id.0);
                    }
                }
                dirty = true;
            }
            default(inner.cfg.progress_interval) => {}
        }
        let bytes = op.ctl.bytes_done.load(Ordering::Relaxed);
        if bytes != last_bytes {
            dirty = true;
        }
        if dirty && last_emit.elapsed() >= inner.cfg.progress_interval {
            let now = Instant::now();
            let dt = now.duration_since(last_t).as_secs_f64().max(1e-3);
            let rate = bytes.saturating_sub(last_bytes) as f64 / dt;
            let snapshot = {
                let mut st = op.status.lock();
                st.files_done = g.files_done;
                st.bytes_done = bytes;
                st.current = op.ctl.current.lock().clone();
                // Exponential smoothing with a ~1 s time constant.
                let a = (dt / 1.0).min(1.0);
                st.speed_bps = if st.speed_bps == 0.0 {
                    rate
                } else {
                    st.speed_bps * (1.0 - a) + rate * a
                };
                st.eta_secs = (st.speed_bps > 1.0)
                    .then(|| st.bytes_total.saturating_sub(bytes) as f64 / st.speed_bps);
                st.clone()
            };
            inner.sink.on_event(OpEvent::OpProgress(snapshot));
            last_emit = now;
            last_t = now;
            last_bytes = bytes;
            dirty = false;
        }
    }
}

fn handle(inner: &Inner, op: &OpShared, g: &mut Graph, out: Outcome) {
    match out {
        Outcome::Finished(row) => {
            if let Some(msg) = &row.error
                && matches!(row.state, ItemState::Failed | ItemState::Skipped)
            {
                let err = ItemError {
                    path: row.source.clone(),
                    dest: row.dest.clone(),
                    message: msg.clone(),
                    errno: None,
                };
                op.status.lock().errors.push(err.clone());
                inner.sink.on_event(OpEvent::OpItemFailed {
                    op: op.id,
                    error: err,
                });
            }
            g.finished(row);
        }
        Outcome::Expanded(row, kids) => {
            let id = row.id;
            let mut fresh = Vec::new();
            for k in kids {
                fresh.push(k.id);
                g.nodes.insert(
                    k.id,
                    Node {
                        row: k,
                        left: 0,
                        ok: true,
                    },
                );
            }
            if let Some(n) = g.nodes.get_mut(&id) {
                n.row = row;
                n.left += fresh.len();
            }
            for k in fresh {
                g.ready.insert(k);
            }
            let n = &g.nodes[&id];
            if n.left == 0 {
                g.finalize.insert(id);
            }
        }
        Outcome::Conflict(row, info) => {
            let id = row.id;
            if let Some(n) = g.nodes.get_mut(&id) {
                n.row = row;
            }
            g.conflicts.insert(id, info.can_merge);
            op.status.lock().conflicts.push(info.clone());
            inner.sink.on_event(OpEvent::OpConflict {
                op: op.id,
                conflict: info,
            });
        }
        Outcome::Cancelled(row) => {
            if let Some(n) = g.nodes.get_mut(&row.id) {
                n.row = row;
            }
        }
    }
}

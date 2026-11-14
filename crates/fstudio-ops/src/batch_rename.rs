//! Batch rename ([`OpRequest::RenameMany`]): many items renamed in their own
//! folders as one op with one undo entry.
//!
//! - Every rename is an exclusive rename (`RENAME_EXCL`): nothing is ever
//!   replaced (F-007). A taken name fails that item only.
//! - Order is worked out as the op runs: an item waits while another item of
//!   the batch still holds its new name, so chains (a→b, b→c) need no
//!   temporary names at all.
//! - A cycle (a→b, b→a; a→b→c→a) is broken by parking one item under a
//!   hidden temporary name (`.<name>.fstudio-renaming`), journaled first.
//!   If the cycle can't finish (another member fails), the members that
//!   moved are moved back and the parked item returns to its own name, so a
//!   temporary name is never left behind. Crash recovery does the same.
//! - A case-only change (`File` → `file`) goes through a temporary name, so
//!   it works on case-insensitive volumes.
//! - Undo is a batch rename back, which handles swaps the same way.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::engine::{describe, file_name};
use crate::journal::{ItemRow, ItemState, Journal, OpRow};
use crate::meta;
use crate::names::{keep_both_names, name_key, renaming_path};
use crate::queue::{Inner, OpShared};
use crate::sys;
use crate::types::{
    FaultHook, FaultStep, ItemError, OpEvent, OpReport, OpRequest, OpState, Result,
};

/// Where a node is right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum At {
    /// At its original name, still to do.
    Source,
    /// Parked under its temporary name.
    Parked,
    /// Renamed.
    Done,
    /// Left at its original name (failed, or not reached).
    Stopped,
}

struct Node {
    row: ItemRow,
    /// The folder as given (paths are built from it).
    dir: PathBuf,
    /// The folder resolved (`/tmp` and `/private/tmp` are one folder), for
    /// comparing names across items.
    slot_dir: PathBuf,
    src_key: String,
    dst_key: String,
    target: PathBuf,
    tmp: PathBuf,
    at: At,
}

type Slot = (PathBuf, String);

impl Node {
    fn src_slot(&self) -> Slot {
        (self.slot_dir.clone(), self.src_key.clone())
    }
    fn dst_slot(&self) -> Slot {
        (self.slot_dir.clone(), self.dst_key.clone())
    }
    /// Whether the item itself (by identity) is under its temporary name.
    fn at_tmp(&self) -> bool {
        match (meta::lstat(&self.tmp), self.row.src_ident) {
            (Ok(md), Some(was)) => meta::ident(&md).same_object(&was),
            _ => false,
        }
    }
}

/// Case sensitivity per folder, asked once each.
#[derive(Default)]
struct CaseCache(HashMap<PathBuf, bool>);

impl CaseCache {
    fn get(&mut self, dir: &Path) -> bool {
        *self
            .0
            .entry(dir.to_path_buf())
            .or_insert_with(|| sys::case_sensitive(dir).unwrap_or(false))
    }
}

fn node_of(row: ItemRow, cases: &mut CaseCache) -> Node {
    let dir = row
        .dest_dir
        .clone()
        .or_else(|| row.source.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    let cs = cases.get(&dir);
    let new_name = row
        .dest_name
        .clone()
        .unwrap_or_else(|| file_name(&row.source));
    let target = dir.join(&new_name);
    let slot_dir = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
    let at = match row.state {
        ItemState::Done => At::Done,
        ItemState::Failed | ItemState::Skipped => At::Stopped,
        _ => At::Source,
    };
    Node {
        src_key: name_key(&file_name(&row.source), cs),
        dst_key: name_key(&new_name, cs),
        tmp: renaming_path(&row.source),
        target,
        dir,
        slot_dir,
        row,
        at,
    }
}

/// The run's state: the nodes and who holds which name.
struct Batch<'a> {
    journal: &'a Journal,
    nodes: Vec<Node>,
    /// Names held by items still at their original name.
    holders: HashMap<Slot, usize>,
    /// Names taken by items renamed in this batch.
    moved_into: HashMap<Slot, usize>,
    errors: Vec<ItemError>,
    /// Fault injection for tests (`BeforePlace`, with the item's source).
    faults: Option<std::sync::Arc<dyn FaultHook>>,
}

fn taken_text(p: &Path) -> String {
    format!("“{}” is already taken", file_name(p))
}

impl<'a> Batch<'a> {
    fn new(journal: &'a Journal, rows: Vec<ItemRow>) -> Self {
        let mut cases = CaseCache::default();
        let nodes: Vec<Node> = rows.into_iter().map(|r| node_of(r, &mut cases)).collect();
        let mut b = Batch {
            journal,
            nodes,
            holders: HashMap::new(),
            moved_into: HashMap::new(),
            errors: Vec::new(),
            faults: None,
        };
        for (i, n) in b.nodes.iter().enumerate() {
            match n.at {
                At::Source | At::Stopped => {
                    b.holders.insert(n.src_slot(), i);
                }
                At::Done => {
                    b.moved_into.insert(n.dst_slot(), i);
                }
                At::Parked => {}
            }
        }
        b
    }

    /// Writes the item's journal row. A failure is reported with the item
    /// (never silent); the caller decides whether it may still touch disk.
    fn save(&mut self, i: usize) -> bool {
        let r = match self
            .faults
            .as_ref()
            .map(|f| f.check(FaultStep::JournalWrite, &self.nodes[i].row.source))
        {
            Some(Err(e)) => Err(e.to_string()),
            _ => self
                .journal
                .save_item(&self.nodes[i].row)
                .map_err(|e| e.to_string()),
        };
        match r {
            Ok(()) => true,
            Err(e) => {
                let n = &self.nodes[i];
                self.errors.push(ItemError {
                    path: n.row.source.clone(),
                    dest: Some(n.target.clone()),
                    message: format!("journal write failed: {e}"),
                    errno: None,
                });
                false
            }
        }
    }

    fn fail(&mut self, i: usize, message: String, errno: Option<i32>) {
        let n = &mut self.nodes[i];
        n.at = At::Stopped;
        n.row.state = ItemState::Failed;
        n.row.error = Some(message.clone());
        // The journal keeps the temporary name for as long as the item is
        // really under it, so recovery and the user can find it.
        if !n.at_tmp() {
            n.row.partial = None;
        }
        self.errors.push(ItemError {
            path: n.row.source.clone(),
            dest: Some(n.target.clone()),
            message,
            errno,
        });
        self.holders.insert(self.nodes[i].src_slot(), i);
        let _ = self.save(i);
    }

    fn io_fail(&mut self, i: usize, e: &io::Error) {
        let msg = if e.kind() == io::ErrorKind::AlreadyExists {
            taken_text(&self.nodes[i].target)
        } else if e.kind() == io::ErrorKind::NotFound {
            format!(
                "“{}” is no longer there",
                file_name(&self.nodes[i].row.source)
            )
        } else {
            describe(e)
        };
        self.fail(i, msg, e.raw_os_error());
    }

    /// The item still at its original name that holds `i`'s new name.
    fn holder_of_target(&self, i: usize) -> Option<usize> {
        let n = &self.nodes[i];
        self.holders.get(&n.dst_slot()).copied().filter(|&j| j != i)
    }

    fn mark_done(&mut self, i: usize) {
        let n = &mut self.nodes[i];
        n.at = At::Done;
        n.row.state = ItemState::Done;
        n.row.partial = None;
        n.row.created = true;
        n.row.error = None;
        n.row.dest_ident = meta::lstat(&n.target).ok().map(|m| meta::ident(&m));
        let (src, dst) = (n.src_slot(), n.dst_slot());
        if self.holders.get(&src) == Some(&i) {
            self.holders.remove(&src);
        }
        self.moved_into.insert(dst, i);
        let _ = self.save(i);
    }

    /// Journals that `i` is about to move (to its temporary name when
    /// `tmp`), before anything on disk changes. Returns false, with the item
    /// failed and nothing touched, if it is no longer the item the batch was
    /// started on or the journal can't be written.
    fn begin(&mut self, i: usize, tmp: bool) -> bool {
        let n = &mut self.nodes[i];
        let md = match meta::lstat(&n.row.source) {
            Ok(md) => md,
            Err(e) => {
                self.io_fail(i, &e);
                return false;
            }
        };
        match n.row.src_ident {
            Some(was) if !meta::ident(&md).same_object(&was) => {
                let msg = format!(
                    "“{}” was replaced by another item since the rename started",
                    file_name(&n.row.source)
                );
                self.fail(i, msg, None);
                return false;
            }
            Some(_) => {}
            None => {
                n.row.src_ident = Some(meta::ident(&md));
                n.row.kind = meta::kind_of(&md);
            }
        }
        let before = n.row.clone();
        n.row.dest = Some(n.target.clone());
        n.row.state = ItemState::InProgress;
        if tmp {
            n.row.partial = Some(n.tmp.clone());
        }
        if self.save(i) {
            return true;
        }
        // Journal first: without the record, nothing is renamed.
        let n = &mut self.nodes[i];
        n.row = before;
        n.at = At::Stopped;
        n.row.state = ItemState::Failed;
        n.row.error = Some("the journal couldn't be written, so it wasn't renamed".into());
        let s = n.src_slot();
        self.holders.insert(s, i);
        false
    }

    /// Fault hook for tests.
    fn fault(&self, step: FaultStep, p: &Path) -> io::Result<()> {
        match &self.faults {
            Some(f) => f.check(step, p),
            None => Ok(()),
        }
    }

    /// Gets an item out from under its temporary name after a failure: back
    /// to its original name, else to a free visible name next to it
    /// ("name 2"). Returns where it is now, or `None` if it is still under
    /// the temporary name (the journal then keeps that name). Reports a
    /// changed name as an error.
    fn rescue(&mut self, p: usize) -> Option<PathBuf> {
        let (tmp, source) = (self.nodes[p].tmp.clone(), self.nodes[p].row.source.clone());
        if sys::rename_excl(&tmp, &source).is_ok() {
            return Some(source);
        }
        let who = file_name(&source);
        let is_dir = self.nodes[p].row.kind == fstudio_model::EntryKind::Dir;
        let dir = self.nodes[p].dir.clone();
        let free = keep_both_names(&who, is_dir)
            .map(|n| dir.join(n))
            .take(1000)
            .find(|c| sys::rename_excl(&tmp, c).is_ok());
        match free {
            Some(f) => {
                self.errors.push(ItemError {
                    path: source,
                    dest: Some(f.clone()),
                    message: format!(
                        "its name was taken meanwhile, so it is now “{}”",
                        file_name(&f)
                    ),
                    errno: None,
                });
                Some(f)
            }
            None => {
                self.errors.push(ItemError {
                    path: source,
                    dest: Some(tmp.clone()),
                    message: format!("it is still under the temporary name “{}”", file_name(&tmp)),
                    errno: None,
                });
                None
            }
        }
    }

    /// Moves `i` to its new name: from its temporary name if parked, through
    /// one for a case-only change, else directly.
    fn place(&mut self, i: usize) {
        let (source, target, tmp, at) = {
            let n = &self.nodes[i];
            (n.row.source.clone(), n.target.clone(), n.tmp.clone(), n.at)
        };
        if let Some(f) = &self.faults
            && let Err(e) = f.check(FaultStep::BeforePlace, &source)
        {
            if at == At::Parked {
                self.unwind(i, false);
            }
            self.io_fail(i, &e);
            return;
        }
        if at == At::Parked {
            match sys::rename_excl(&tmp, &target) {
                Ok(_) => self.mark_done(i),
                Err(e) => {
                    // Can't finish the cycle: undo it, then report.
                    self.unwind(i, false);
                    self.io_fail(i, &e);
                }
            }
            return;
        }
        if !meta::exists(&source) {
            self.fail(
                i,
                format!("“{}” is no longer there", file_name(&source)),
                None,
            );
            return;
        }
        let case_only = self.nodes[i].src_key == self.nodes[i].dst_key;
        if case_only {
            if !self.begin(i, true) {
                return;
            }
            if let Err(e) = sys::rename_excl(&source, &tmp) {
                self.io_fail(i, &e);
                return;
            }
            let second = self
                .fault(FaultStep::AfterPark, &source)
                .and_then(|()| sys::rename_excl(&tmp, &target).map(|_| ()));
            if let Err(e) = second {
                // Out from under the temporary name before reporting.
                if let Some(at) = self.rescue(i)
                    && at != source
                {
                    // It has a new visible name; that is where it is now.
                    self.nodes[i].row.source = at;
                }
                self.io_fail(i, &e);
                return;
            }
            self.mark_done(i);
            return;
        }
        if !self.begin(i, false) {
            return;
        }
        match sys::rename_excl(&source, &target) {
            Ok(_) => self.mark_done(i),
            Err(e) => self.io_fail(i, &e),
        }
    }

    /// Parks `i` under its temporary name, freeing its original name.
    fn park(&mut self, i: usize) -> bool {
        if !self.begin(i, true) {
            return false;
        }
        let (source, tmp) = (self.nodes[i].row.source.clone(), self.nodes[i].tmp.clone());
        match sys::rename_excl(&source, &tmp) {
            Ok(_) => {
                let _ = self.fault(FaultStep::AfterPark, &source);
                self.nodes[i].at = At::Parked;
                let s = self.nodes[i].src_slot();
                if self.holders.get(&s) == Some(&i) {
                    self.holders.remove(&s);
                }
                true
            }
            Err(e) => {
                self.io_fail(i, &e);
                false
            }
        }
    }

    /// Puts a parked item back at its original name. Members of its cycle
    /// that already moved into that name are moved back first (last one
    /// first). With `cancelled`, they become pending again; otherwise they
    /// fail with a reason. If the original name can't be freed, the item
    /// gets a free visible name next to it, never a hidden temporary one.
    fn unwind(&mut self, p: usize, cancelled: bool) {
        let mut chain = Vec::new();
        let mut slot = self.nodes[p].src_slot();
        while let Some(&x) = self.moved_into.get(&slot) {
            if chain.contains(&x) || x == p {
                break;
            }
            chain.push(x);
            slot = self.nodes[x].src_slot();
        }
        let who = file_name(&self.nodes[p].row.source);
        for &x in chain.iter().rev() {
            let (from, to) = (
                self.nodes[x].target.clone(),
                self.nodes[x].row.source.clone(),
            );
            match sys::rename_excl(&from, &to) {
                Ok(_) => {
                    let d = self.nodes[x].dst_slot();
                    self.moved_into.remove(&d);
                    let n = &mut self.nodes[x];
                    n.row.created = false;
                    n.row.dest_ident = None;
                    if cancelled {
                        n.at = At::Source;
                        n.row.state = ItemState::Pending;
                        n.row.dest = None;
                        let s = n.src_slot();
                        self.holders.insert(s, x);
                        let _ = self.save(x);
                    } else {
                        self.fail(
                            x,
                            format!("left as it was: “{who}” in the same swap couldn’t be renamed"),
                            None,
                        );
                    }
                }
                Err(e) => {
                    self.errors.push(ItemError {
                        path: from,
                        dest: Some(to),
                        message: format!("couldn’t be moved back: {}", describe(&e)),
                        errno: e.raw_os_error(),
                    });
                    break;
                }
            }
        }
        let source = self.nodes[p].row.source.clone();
        let Some(back) = self.rescue(p) else {
            // Still under the temporary name: the journal keeps it.
            return;
        };
        let n = &mut self.nodes[p];
        n.row.partial = None;
        if back == source {
            n.at = At::Source;
            n.row.state = ItemState::Pending;
            n.row.dest = None;
            let s = n.src_slot();
            self.holders.insert(s, p);
        } else {
            // It has a new visible name: record where it is.
            n.at = At::Stopped;
            n.row.state = ItemState::Failed;
            n.row.source = back;
        }
        let _ = self.save(p);
    }

    fn pending(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.nodes.len()).filter(|&i| matches!(self.nodes[i].at, At::Source | At::Parked))
    }

    /// One pass over the items that can go now. Returns whether anything changed.
    fn pass(&mut self, op: &OpShared, tick: &mut dyn FnMut(&Batch<'_>, &Path)) -> bool {
        let mut progress = false;
        let todo: Vec<usize> = self.pending().collect();
        for i in todo {
            if !op.ctl.wait_while_paused() {
                return progress;
            }
            if !matches!(self.nodes[i].at, At::Source | At::Parked) {
                continue;
            }
            match self.holder_of_target(i) {
                None => {
                    let src = self.nodes[i].row.source.clone();
                    self.place(i);
                    tick(self, &src);
                    progress = true;
                }
                Some(j) if self.nodes[j].at == At::Stopped => {
                    if self.nodes[i].at == At::Parked {
                        self.unwind(i, false);
                    }
                    let t = self.nodes[i].target.clone();
                    self.fail(i, taken_text(&t), None);
                    progress = true;
                }
                Some(_) => {}
            }
        }
        progress
    }

    /// Breaks one cycle among the waiting items by parking one of them.
    fn break_cycle(&mut self) -> bool {
        let waiting: Vec<usize> = self
            .pending()
            .filter(|&i| self.nodes[i].at == At::Source)
            .collect();
        for start in waiting {
            let mut seen = vec![start];
            let mut cur = start;
            while let Some(next) = self.holder_of_target(cur) {
                if let Some(pos) = seen.iter().position(|&s| s == next) {
                    // `seen[pos..]` is a cycle; park its first member.
                    let c = seen[pos];
                    if self.nodes[c].at == At::Source {
                        self.park(c);
                        return true;
                    }
                    break;
                }
                seen.push(next);
                cur = next;
            }
        }
        false
    }
}

/// Runs a [`OpRequest::RenameMany`] op to the end.
pub(crate) fn run(inner: &Inner, op: &OpShared, items: Vec<ItemRow>) {
    let started = Instant::now();
    let total = items.len() as u64;
    let already = items.iter().filter(|i| i.state.is_terminal()).count() as u64;
    {
        let mut st = op.status.lock();
        st.files_total = total;
        st.files_done = already;
        st.state = if op.ctl.paused.load(std::sync::atomic::Ordering::Acquire) {
            OpState::Paused
        } else {
            OpState::Running
        };
    }
    let _ = inner.journal.set_op_state(op.id, op.status.lock().state);
    let mut b = Batch::new(&inner.journal, items);
    b.faults = inner.cfg.faults.clone();
    let mut last = Instant::now() - inner.cfg.progress_interval;
    let mut done = already;
    let mut tick = |b: &Batch<'_>, path: &Path| {
        done += 1;
        if last.elapsed() >= inner.cfg.progress_interval {
            last = Instant::now();
            let mut st = op.status.lock();
            st.files_done = done;
            st.current = Some(path.to_path_buf());
            st.errors.clone_from(&b.errors);
            let snap = st.clone();
            drop(st);
            inner.sink.on_event(OpEvent::OpProgress(snap));
        }
    };
    loop {
        if op.ctl.cancelled() {
            break;
        }
        let moved = b.pass(op, &mut tick);
        if b.pending().next().is_none() || op.ctl.cancelled() {
            break;
        }
        if !moved && !b.break_cycle() {
            // Nothing can go and no cycle to break: everything left waits
            // on parked items whose own targets are stuck. Undo and stop.
            let left: Vec<usize> = b.pending().collect();
            for i in left {
                if b.nodes[i].at == At::Parked {
                    b.unwind(i, false);
                }
                if matches!(b.nodes[i].at, At::Source) {
                    let t = b.nodes[i].target.clone();
                    b.fail(i, taken_text(&t), None);
                }
            }
            break;
        }
    }
    let cancelled = op.ctl.cancelled();
    if cancelled {
        let parked: Vec<usize> = b
            .pending()
            .filter(|&i| b.nodes[i].at == At::Parked)
            .collect();
        for p in parked {
            b.unwind(p, true);
        }
    }
    let failed = b
        .nodes
        .iter()
        .filter(|n| n.row.state == ItemState::Failed)
        .count() as u64;
    let renamed = b.nodes.iter().filter(|n| n.at == At::Done).count() as u64;
    let state = if cancelled {
        OpState::Cancelled
    } else if !b.errors.is_empty() {
        OpState::CompletedWithErrors
    } else {
        OpState::Completed
    };
    let _ = inner.journal.set_op_state(op.id, state);
    let report = {
        let mut st = op.status.lock();
        st.state = state;
        st.files_done = renamed + failed;
        st.current = None;
        st.errors.clone_from(&b.errors);
        OpReport {
            id: op.id,
            kind: op.kind,
            state,
            files_done: renamed,
            files_failed: failed,
            files_skipped: 0,
            bytes_done: 0,
            errors: b.errors.clone(),
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

/// Crash recovery: every parked item goes to its new name if that is free,
/// else back to its original name (moving members of its cycle back first),
/// so no temporary name survives a crash. Other items are settled by what
/// is on disk.
pub(crate) fn recover(journal: &Journal, op: &OpRow) -> Result<()> {
    let rows = journal.load_items(op.id)?;
    let mut b = Batch::new(journal, rows);
    let unsettled: Vec<usize> = (0..b.nodes.len())
        .filter(|&i| b.nodes[i].row.state == ItemState::InProgress)
        .collect();
    // Parked means: the journal recorded the temporary name and the very
    // item (by identity) is under it. Anything else there isn't ours.
    let is_parked = |n: &Node| n.row.partial.is_some() && n.at_tmp();
    // Items that weren't parked: did their one rename land? The item at the
    // new name must be the very object that was at the old one.
    for &i in &unsettled {
        if is_parked(&b.nodes[i]) {
            continue;
        }
        let n = &b.nodes[i];
        let landed = if n.src_key == n.dst_key {
            // A case-only change: the stored name tells.
            stored_name(
                &n.dir,
                &n.dst_key,
                sys::case_sensitive(&n.dir).unwrap_or(false),
            )
            .is_some_and(|s| s == file_name(&n.target))
        } else {
            match (meta::lstat(&n.target), n.row.src_ident) {
                (Ok(md), Some(was)) => {
                    meta::ident(&md).same_object(&was) && !same_at(&n.row.source, &was)
                }
                _ => false,
            }
        };
        if landed {
            b.mark_done(i);
        } else {
            let n = &mut b.nodes[i];
            n.row.partial = None;
            n.row.state = ItemState::Pending;
            n.row.dest = None;
            let _ = b.save(i);
        }
    }
    // Parked items: on to the new name if it is free, else back home.
    for &i in &unsettled {
        if !is_parked(&b.nodes[i]) {
            continue;
        }
        let (target, tmp) = (b.nodes[i].target.clone(), b.nodes[i].tmp.clone());
        if !meta::exists(&target) {
            match sys::rename_excl(&tmp, &target) {
                Ok(_) => b.mark_done(i),
                Err(e) => {
                    // Report it and go on with the other items; the
                    // journal still names the temporary name.
                    b.errors.push(ItemError {
                        path: b.nodes[i].row.source.clone(),
                        dest: Some(target.clone()),
                        message: format!("couldn’t be put back: {}", describe(&e)),
                        errno: e.raw_os_error(),
                    });
                }
            }
        } else {
            b.nodes[i].at = At::Parked;
            let s = b.nodes[i].src_slot();
            if b.holders.get(&s) == Some(&i) {
                b.holders.remove(&s);
            }
            b.unwind(i, true);
        }
    }
    if b.errors.is_empty() {
        Ok(())
    } else {
        let msg = b
            .errors
            .iter()
            .map(|e| e.message.clone())
            .collect::<Vec<_>>()
            .join("; ");
        Err(crate::types::OpsError::InvalidRequest(msg))
    }
}

/// Whether a finished op's item is still under its batch-rename temporary
/// name: the journal recorded it, and the very item (by identity) is there.
pub(crate) fn stranded(it: &ItemRow) -> bool {
    let Some(p) = it.partial.as_deref() else {
        return false;
    };
    let named = p
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(crate::names::RENAMING_SUFFIX));
    named
        && *p == renaming_path(&it.source)
        && matches!((meta::lstat(p), it.src_ident), (Ok(md), Some(was)) if meta::ident(&md).same_object(&was))
}

/// Moves a stranded item (see [`stranded`]) back to its original name, else
/// to a free "name 2" next to it. Returns where it is now.
pub(crate) fn rescue_stranded(it: &ItemRow) -> Option<PathBuf> {
    let tmp = it.partial.clone()?;
    if sys::rename_excl(&tmp, &it.source).is_ok() {
        return Some(it.source.clone());
    }
    let dir = it.source.parent()?.to_path_buf();
    let is_dir = it.kind == fstudio_model::EntryKind::Dir;
    keep_both_names(&file_name(&it.source), is_dir)
        .map(|n| dir.join(n))
        .take(1000)
        .find(|c| sys::rename_excl(&tmp, c).is_ok())
}

/// The name stored in `dir` whose key is `key`.
fn stored_name(dir: &Path, key: &str, case_sensitive: bool) -> Option<String> {
    std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let n = e.file_name().to_string_lossy().into_owned();
        (name_key(&n, case_sensitive) == key).then_some(n)
    })
}

/// Whether the object `was` is (still) at `p`.
fn same_at(p: &Path, was: &meta::Ident) -> bool {
    meta::lstat(p).is_ok_and(|m| meta::ident(&m).same_object(was))
}

/// The request that undoes a finished batch rename: a batch rename back,
/// for every item it renamed that is still as it left it.
pub(crate) fn inverse(items: &[ItemRow]) -> std::result::Result<Vec<OpRequest>, String> {
    let mut pairs = Vec::new();
    for it in items
        .iter()
        .filter(|i| i.state == ItemState::Done && i.created)
    {
        let dest = it.dest.clone().ok_or("the journal has no destination")?;
        let md = meta::lstat(&dest).map_err(|_| {
            format!(
                "“{}” is no longer where it was renamed",
                fstudio_fs::display_name(&dest)
            )
        })?;
        if let Some(was) = it.dest_ident
            && !meta::ident(&md).same_object(&was)
        {
            return Err(format!(
                "“{}” was replaced after it was renamed",
                fstudio_fs::display_name(&dest)
            ));
        }
        pairs.push((dest, file_name(&it.source)));
    }
    if pairs.is_empty() {
        return Err("nothing was changed".into());
    }
    Ok(vec![OpRequest::RenameMany { pairs }])
}

#[cfg(test)]
mod tests {
    // The behaviour is covered end to end in `tests/batch_rename.rs`.
}

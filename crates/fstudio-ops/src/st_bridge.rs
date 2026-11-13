//! Runs a copy or cross-volume move as one Safe Transfer job (F-026,
//! proposal §17). Ops keeps everything around the bytes: the plan's checks,
//! the conflict dialog, undo, Trash and Put Back. Safe Transfer does the
//! bytes: partial, chunk hashes, drive flush, uncached read-back, exclusive
//! rename, then (for a move) the original to the Trash.
//!
//! The job's events become the op's events while it runs. When it ends, every
//! file and folder it placed is written to the op's items, the way ops' own
//! engine would have written them, so undo works unchanged.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crossbeam_channel::select;
use fstudio_model::EntryKind;
use safe_transfer as st;

use crate::engine::file_name;
use crate::journal::{ItemRow, ItemState, OpRow};
use crate::meta;
use crate::names;
use crate::queue::{Ctl, Inner, OpShared};
use crate::transfer::Transfer;
use crate::types::{
    ConflictDecision, ConflictId, ConflictInfo, CopyBackend, ItemError, OpEvent, OpKind, OpPlan,
    OpReport, OpRequest, OpState, ResumeState,
};

/// How often the bridge looks at the job when no event arrives.
const POLL: Duration = Duration::from_secs(1);

/// Where a job's items go, and how.
pub(crate) struct Route {
    dest: PathBuf,
    mode: st::Mode,
    /// Exact names under `dest` (Duplicate's "name copy", MoveTo's names),
    /// one per root, or `None` to keep each root's own name.
    exact: Option<Vec<String>>,
}

/// Whether the op goes to Safe Transfer, and where. `None` keeps it on ops'
/// own path: other kinds of op, same-volume moves (a rename either way;
/// ops' case-only renames stay), and ops whose items go to more than one
/// folder (a job has one destination folder).
///
/// `resuming` (the op already has a job) routes it whatever the backend.
pub(crate) fn route(inner: &Inner, req: &OpRequest, resuming: bool) -> Option<Route> {
    if inner.transfer.is_none()
        || (!resuming && inner.cfg.copy_backend != CopyBackend::SafeTransfer)
    {
        return None;
    }
    let crosses = |src: &Path, dir: &Path| {
        let dev = |p: &Path| meta::lstat(p).map(|m| std::os::unix::fs::MetadataExt::dev(&m));
        match (dev(src), dev(dir)) {
            (Ok(a), Ok(b)) => a != b,
            _ => true,
        }
    };
    match req {
        OpRequest::Copy { dest_dir, .. } => Some(Route {
            dest: dest_dir.clone(),
            mode: st::Mode::Copy,
            exact: None,
        }),
        OpRequest::Move { sources, dest_dir } => {
            sources.iter().any(|s| crosses(s, dest_dir)).then(|| Route {
                dest: dest_dir.clone(),
                mode: st::Mode::Move,
                exact: None,
            })
        }
        OpRequest::Duplicate { paths } => {
            let parent = one_parent(paths.iter().map(|p| p.as_path()))?;
            Some(Route {
                dest: parent,
                mode: st::Mode::Copy,
                exact: Some(Vec::new()),
            })
        }
        OpRequest::MoveTo { pairs } => {
            let parent = one_parent(pairs.iter().map(|(_, to)| to.as_path()))?;
            if !pairs.iter().any(|(from, _)| crosses(from, &parent)) {
                return None;
            }
            Some(Route {
                dest: parent,
                mode: st::Mode::Move,
                exact: Some(pairs.iter().map(|(_, to)| file_name(to)).collect()),
            })
        }
        _ => None,
    }
}

/// Safe Transfer reports canonical paths (`/private/var/…`); the op shows
/// paths as they were given (`/var/…`), as ops' own engine does.
pub(crate) struct Paths(Vec<(PathBuf, PathBuf)>);

impl Paths {
    /// For a job into `dest` from the op's top-level items.
    fn new(dest: &Path, roots: &[ItemRow]) -> Paths {
        let mut pairs = Vec::new();
        let mut add = |given: &Path| {
            if let Ok(c) = std::fs::canonicalize(given)
                && c != given
                && !pairs.iter().any(|(a, _)| *a == c)
            {
                pairs.push((c, given.to_path_buf()));
            }
        };
        add(dest);
        for r in roots {
            if let Some(dir) = r.source.parent() {
                add(dir);
            }
        }
        // Longest prefix first.
        pairs.sort_by_key(|(c, _)| std::cmp::Reverse(c.as_os_str().len()));
        Paths(pairs)
    }

    /// `p` as the op names it.
    fn given(&self, p: &Path) -> PathBuf {
        for (canon, given) in &self.0 {
            if let Ok(rest) = p.strip_prefix(canon) {
                return given.join(rest);
            }
        }
        p.to_path_buf()
    }
}

/// The one folder every path is in, if there is one.
fn one_parent<'a>(mut paths: impl Iterator<Item = &'a Path>) -> Option<PathBuf> {
    let first = paths.next()?.parent()?.to_path_buf();
    paths
        .all(|p| p.parent() == Some(first.as_path()))
        .then_some(first)
}

/// Ops' standing rules as Safe Transfer's policy: no rule means ask.
fn policy(rules: crate::types::ConflictRules) -> st::ConflictPolicy {
    let d = |r: Option<ConflictDecision>| r.map(decision).unwrap_or(st::Decision::Ask);
    st::ConflictPolicy {
        files: d(rules.others),
        folders: d(rules.folders),
    }
}

fn decision(d: ConflictDecision) -> st::Decision {
    match d {
        ConflictDecision::KeepBoth => st::Decision::KeepBoth,
        ConflictDecision::Replace => st::Decision::Replace,
        ConflictDecision::Skip => st::Decision::Skip,
        ConflictDecision::Merge => st::Decision::Merge,
    }
}

fn kind(k: st::journal::EntryKind) -> EntryKind {
    match k {
        st::journal::EntryKind::File => EntryKind::File,
        st::journal::EntryKind::Dir => EntryKind::Dir,
        st::journal::EntryKind::Link => EntryKind::Symlink,
        st::journal::EntryKind::Other => EntryKind::Other,
    }
}

/// How the bridge's watch ended.
enum End {
    /// Every item is done, skipped or failed.
    Finished,
    /// Cancelled: finished items stay, partials are gone.
    Cancelled,
    /// Stopped and resumable (a disk is missing, or the disk filled up).
    Suspended,
    /// The job couldn't be made or started.
    Failed(String),
}

/// Runs `op` as a Safe Transfer job (new, or the one it already has).
pub(crate) fn run(inner: &Inner, op: &OpShared, row: &OpRow, plan: &OpPlan, r: Route) {
    let started = Instant::now();
    let tr = inner
        .transfer
        .clone()
        .expect("routed without a transfer engine");
    let mut roots: Vec<ItemRow> = match inner.journal.load_items(op.id) {
        Ok(items) => items.into_iter().filter(|i| i.parent.is_none()).collect(),
        Err(e) => return finish_failed(inner, op, started, format!("journal: {e}")),
    };
    let job = match &row.st_job {
        Some(id) => match st::JobId::parse(id).filter(|j| tr.engine.job(*j).is_some()) {
            Some(j) => j,
            None => {
                return finish_failed(
                    inner,
                    op,
                    started,
                    "Safe Transfer's record of this copy is gone, so it can't be resumed".into(),
                );
            }
        },
        None => match create(inner, op, &tr, &r, &mut roots) {
            Ok(Some(j)) => j,
            Ok(None) => {
                // Every item failed on its own (gone, into itself, a disk
                // that refuses them all): per-file errors, as on ops' path.
                let paths = Paths::new(&r.dest, &roots);
                return finish(
                    inner,
                    op,
                    Vec::new(),
                    &roots,
                    &paths,
                    r.mode,
                    false,
                    started,
                );
            }
            Err(msg) => return finish_failed(inner, op, started, msg),
        },
    };
    *op.st_job.lock() = Some(job);
    let paths = Paths::new(&r.dest, &roots);
    let rx = tr.hub.subscribe(&job);
    let end = watch(
        inner,
        op,
        &tr,
        &job,
        plan,
        &rx,
        row.st_job.is_some(),
        &paths,
    );
    tr.hub.unsubscribe(&job);
    match end {
        End::Failed(msg) => finish_failed(inner, op, started, msg),
        End::Suspended => suspend(inner, op, &tr, &job),
        End::Finished | End::Cancelled => {
            let cancelled = matches!(end, End::Cancelled);
            let entries = tr.engine.entries(job).unwrap_or_default();
            finish(
                inner, op, entries, &roots, &paths, r.mode, cancelled, started,
            );
        }
    }
}

/// Fails one top-level item on its own, journaled at once.
fn fail_root(inner: &Inner, it: &mut ItemRow, msg: String) -> Result<(), String> {
    it.state = ItemState::Failed;
    it.error = Some(msg);
    inner
        .journal
        .save_item(it)
        .map_err(|e| format!("journal: {e}"))
}

/// Makes the job and records it on the op. Items that can't go (gone, a
/// folder into itself) fail on their own first; `Ok(None)` means none is
/// left, or the destination refuses them all (each failed with the reason).
fn create(
    inner: &Inner,
    op: &OpShared,
    tr: &Transfer,
    r: &Route,
    roots: &mut [ItemRow],
) -> Result<Option<st::JobId>, String> {
    if op.kind == OpKind::MoveTo && !meta::exists(&r.dest) {
        // Never recreated silently (S14), as on ops' own path.
        for it in roots.iter_mut() {
            fail_root(inner, it, "the folder it goes back into is gone".into())?;
        }
        return Ok(None);
    }
    let mut live = Vec::with_capacity(roots.len());
    for (i, it) in roots.iter_mut().enumerate() {
        match meta::lstat(&it.source) {
            Err(e) => fail_root(inner, it, crate::engine::describe(&e))?,
            Ok(md) if md.is_dir() && names::is_within(&r.dest, &it.source) => fail_root(
                inner,
                it,
                "a folder can't be copied or moved into itself".into(),
            )?,
            Ok(_) => live.push(i),
        }
    }
    if live.is_empty() {
        return Ok(None);
    }
    let mut items = Vec::with_capacity(live.len());
    match &r.exact {
        None => items.extend(
            live.iter()
                .map(|&i| st::Item::Path(roots[i].source.clone())),
        ),
        Some(names) => {
            let mut taken: Vec<String> = Vec::new();
            for &i in &live {
                let it = &mut roots[i];
                let name = match names.get(i) {
                    Some(n) => n.clone(),
                    None => {
                        // Duplicate: Finder's next free "name copy".
                        let is_dir = meta::lstat(&it.source).is_ok_and(|m| m.is_dir());
                        names::duplicate_names(&file_name(&it.source), is_dir)
                            .find(|n| !taken.contains(n) && !meta::exists(&r.dest.join(n)))
                            .expect("infinite candidates")
                    }
                };
                taken.push(name.clone());
                it.dest_name = Some(name.clone());
                inner
                    .journal
                    .save_item(it)
                    .map_err(|e| format!("journal: {e}"))?;
                items.push(st::Item::Exact {
                    source: it.source.clone(),
                    dest_rel: PathBuf::from(name),
                });
            }
        }
    }
    let opts = op.options.lock().clone();
    let spec = st::JobSpec {
        items,
        dest: r.dest.clone(),
        options: st::Options {
            mode: r.mode,
            on_conflict: policy(*op.ctl.rules.read()),
            disposal: st::Disposal::Trash,
            reread_source: inner.cfg.reread_originals,
            allow_dataless: opts.allow_dataless,
            ..st::Options::default()
        },
        label: op.status.lock().title.clone(),
        host_ref: Some(op.id.0.to_string()),
    };
    let job = match tr.engine.create(spec) {
        Ok(j) => j,
        Err(e) => {
            // The destination refuses the whole job (full, read-only, gone):
            // every item fails with the reason, as each would on ops' path.
            let msg = match e {
                st::EngineError::NoSpace { need, margin, free } => format!(
                    "there isn't enough space: it needs {}, and {} is free",
                    st::human(need + margin),
                    st::human(free)
                ),
                st::EngineError::IntoItself => {
                    "a folder can't be copied or moved into itself".into()
                }
                st::EngineError::Io { source, .. } => crate::engine::describe(&source),
                e => e.to_string(),
            };
            for &i in &live {
                fail_root(inner, &mut roots[i], msg.clone())?;
            }
            return Ok(None);
        }
    };
    inner
        .journal
        .set_op_st_job(op.id, job.as_str())
        .map_err(|e| format!("journal: {e}"))?;
    Ok(Some(job))
}

/// A Safe Transfer reason in ops' words: "permission denied", not
/// "Copying failed: permission denied".
fn plain(reason: &str) -> String {
    match reason.split_once(" failed: ") {
        Some((head, rest)) if !head.contains(' ') => rest.to_string(),
        _ => reason.to_string(),
    }
}

/// Starts the job and turns its events into the op's, until it stops.
#[allow(clippy::too_many_arguments)]
fn watch(
    inner: &Inner,
    op: &OpShared,
    tr: &Transfer,
    job: &st::JobId,
    plan: &OpPlan,
    rx: &crossbeam_channel::Receiver<st::Event>,
    resuming: bool,
    paths: &Paths,
) -> End {
    let go = if resuming {
        tr.engine.resume(*job)
    } else {
        tr.engine.start(*job)
    };
    if let Err(e) = go {
        return End::Failed(e.to_string());
    }
    let mut paused = false;
    let mut last_emit = Instant::now() - inner.cfg.progress_interval;
    let mut last_poll = Instant::now();
    let dest_root = paths.given(&tr.engine.job(*job).map(|i| i.dest).unwrap_or_default());
    // Sources by path under the destination, for per-file errors.
    let sources: HashMap<PathBuf, PathBuf> = tr
        .engine
        .entries(*job)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| Some((e.rel, paths.given(&e.source?))))
        .collect();
    loop {
        if op.ctl.cancelled() {
            // Stops the job, removes its partials and closes it; finished
            // files stay (ops' meaning of Cancel).
            let _ = tr.engine.cancel(*job, st::Cancel::Close);
            return End::Cancelled;
        }
        let want_pause = op.ctl.paused.load(Ordering::Acquire);
        if want_pause != paused {
            paused = want_pause;
            let _ = if paused {
                tr.engine.pause(*job)
            } else {
                tr.engine.resume(*job)
            };
            set_state(
                inner,
                op,
                if paused {
                    OpState::Paused
                } else {
                    OpState::Running
                },
            );
        }
        let mut stop: Option<End> = None;
        select! {
            recv(rx) -> e => match e {
                Ok(e) => stop = on_event(inner, op, plan, &dest_root, &sources, paths, e),
                Err(_) => return End::Failed("lost touch with the copy engine".into()),
            },
            recv(op.ctl_rx) -> msg => {
                if let Ok(Ctl::Decide(id, d)) = msg {
                    answer(inner, op, tr, job, id, d);
                }
            }
            default(POLL) => {}
        }
        if let Some(end) = stop {
            return end;
        }
        // A stop whose event was missed (or never sent) still ends the watch.
        if last_poll.elapsed() >= POLL {
            last_poll = Instant::now();
            if let Some(info) = tr.engine.job(*job) {
                match info.state {
                    st::JobState::Finished => return End::Finished,
                    st::JobState::Closed => return End::Cancelled,
                    // Stopped and not waiting for an answer: a disk is
                    // missing, it filled up, or something else stopped it.
                    s if s.needs_resume() && s != st::JobState::WaitingForAnswer => {
                        return End::Suspended;
                    }
                    _ => {}
                }
            }
        }
        if last_emit.elapsed() >= inner.cfg.progress_interval {
            last_emit = Instant::now();
            let snap = op.status.lock().clone();
            inner.sink.on_event(OpEvent::OpProgress(snap));
        }
    }
}

fn set_state(inner: &Inner, op: &OpShared, s: OpState) {
    let changed = {
        let mut st = op.status.lock();
        let changed = st.state != s;
        st.state = s;
        changed
    };
    if changed {
        let _ = inner.journal.set_op_state(op.id, s);
    }
}

/// One job event. Returns how the watch ends, when it does.
fn on_event(
    inner: &Inner,
    op: &OpShared,
    plan: &OpPlan,
    dest_root: &Path,
    sources: &HashMap<PathBuf, PathBuf>,
    paths: &Paths,
    e: st::Event,
) -> Option<End> {
    match e {
        st::Event::Progress(p) => {
            let mut st = op.status.lock();
            // Safe Transfer counts work bytes (written, read back, re-read);
            // the op shows content bytes, as ops' own engine does.
            let scale = |n: u64| {
                if p.total == 0 {
                    plan.bytes
                } else {
                    (n as u128 * plan.bytes as u128 / p.total as u128) as u64
                }
            };
            st.bytes_done = scale(p.done);
            st.speed_bps = scale(p.bytes_per_sec) as f64;
            st.eta_secs = p.eta_secs.map(|s| s as f64);
            st.current = p.file.map(|f| dest_root.join(f));
            st.files_done = u64::from(p.files_done);
            op.ctl.bytes_done.store(st.bytes_done, Ordering::Relaxed);
        }
        st::Event::FileNotCopied {
            path,
            failed: true,
            reason,
            ..
        } => {
            let err = ItemError {
                path: sources
                    .get(&path)
                    .cloned()
                    .unwrap_or_else(|| dest_root.join(&path)),
                dest: Some(dest_root.join(&path)),
                message: plain(&reason),
                errno: None,
            };
            op.status.lock().errors.push(err.clone());
            inner.sink.on_event(OpEvent::OpItemFailed {
                op: op.id,
                error: err,
            });
        }
        st::Event::Conflict { info, .. } => {
            let c = conflict_info(op, &info, paths);
            op.status.lock().conflicts.push(c.clone());
            inner.sink.on_event(OpEvent::OpConflict {
                op: op.id,
                conflict: c,
            });
        }
        st::Event::WaitingForDestination { volume, .. } => {
            op.status.lock().waiting_for = vec![volume.clone()];
            inner.sink.on_event(OpEvent::Waiting {
                op: op.id,
                volumes: vec![volume],
                sources: false,
            });
        }
        st::Event::WaitingForSource { missing, .. } => {
            let names: Vec<String> = missing.into_iter().map(|m| m.volume).collect();
            op.status.lock().waiting_for = names.clone();
            inner.sink.on_event(OpEvent::Waiting {
                op: op.id,
                volumes: names,
                sources: true,
            });
        }
        st::Event::JobState { state, .. } => match state {
            st::JobState::Running => set_state(inner, op, OpState::Running),
            st::JobState::Paused => set_state(inner, op, OpState::Paused),
            st::JobState::WaitingForAnswer => set_state(inner, op, OpState::WaitingForDecision),
            st::JobState::WaitingForDestination
            | st::JobState::WaitingForSource
            | st::JobState::DestinationFull
            | st::JobState::Interrupted
            | st::JobState::RunningElsewhere => return Some(End::Suspended),
            st::JobState::Finished => return Some(End::Finished),
            st::JobState::Closed => return Some(End::Cancelled),
            st::JobState::Planned | st::JobState::Cancelled => {}
        },
        st::Event::JobFinished { .. } => return Some(End::Finished),
        st::Event::FileDone { .. }
        | st::Event::FileNotCopied { .. }
        | st::Event::ResumeAvailable { .. } => {}
    }
    None
}

/// A Safe Transfer question as ops' conflict.
fn conflict_info(op: &OpShared, i: &st::ConflictInfo, paths: &Paths) -> ConflictInfo {
    let dest = paths.given(&i.dest);
    let source = i
        .source
        .as_deref()
        .map(|s| paths.given(s))
        .unwrap_or_else(|| dest.clone());
    let kind_at = |p: &Path, folder: bool| {
        meta::lstat(p)
            .map(|m| meta::kind_of(&m))
            .unwrap_or(if folder {
                EntryKind::Dir
            } else {
                EntryKind::File
            })
    };
    ConflictInfo {
        op: op.id,
        id: ConflictId(i64::from(i.id)),
        source_kind: kind_at(&source, i.folder),
        dest_kind: kind_at(&dest, i.existing_folder),
        source: source.clone(),
        dest,
        source_size: i.source_size,
        dest_size: i.existing_size,
        source_mtime_ns: i.source_mtime_ns,
        dest_mtime_ns: i.existing_mtime_ns,
        can_merge: i.folder && i.existing_folder,
    }
}

/// Answers a question. "Apply to all" has already set the op's rule for the
/// class (see `OpQueue::resolve_conflict`); the job gets it the same way.
fn answer(
    inner: &Inner,
    op: &OpShared,
    tr: &Transfer,
    job: &st::JobId,
    id: ConflictId,
    d: ConflictDecision,
) {
    let folder = op
        .status
        .lock()
        .conflicts
        .iter()
        .find(|c| c.id == id)
        .is_some_and(|c| c.can_merge);
    let rules = *op.ctl.rules.read();
    let all = rules.for_class(folder) == Some(d);
    op.status
        .lock()
        .conflicts
        .retain(|c| c.id != id && !(all && c.can_merge == folder));
    if let Err(e) = tr
        .engine
        .resolve_conflict(*job, id.0 as u32, decision(d), all)
    {
        let err = ItemError {
            path: PathBuf::new(),
            dest: None,
            message: format!("the answer couldn't be given: {e}"),
            errno: None,
        };
        op.status.lock().errors.push(err.clone());
        inner.sink.on_event(OpEvent::OpItemFailed {
            op: op.id,
            error: err,
        });
    }
}

/// The resumability of a stopped job, as the op shows it.
pub(crate) fn resume_state(r: &st::Resumability) -> Option<ResumeState> {
    match r {
        st::Resumability::NotNeeded => None,
        st::Resumability::Ready { still_missing } => Some(ResumeState::Ready {
            still_missing: still_missing.clone(),
        }),
        st::Resumability::NeedsVolumes(v) => Some(ResumeState::NeedsVolumes(v.clone())),
        st::Resumability::InUseElsewhere => Some(ResumeState::InUseElsewhere),
    }
}

/// The job stopped and can be resumed: the op is interrupted, not finished.
fn suspend(inner: &Inner, op: &OpShared, tr: &Transfer, job: &st::JobId) {
    let info = tr.engine.job(*job);
    {
        let mut st = op.status.lock();
        st.state = OpState::Interrupted;
        st.current = None;
        st.speed_bps = 0.0;
        st.eta_secs = None;
        if let Some(i) = &info {
            st.resume = resume_state(&i.resume);
            st.files_done = u64::from(i.files_done);
            if st.waiting_for.is_empty() {
                st.waiting_for = match &i.resume {
                    st::Resumability::NeedsVolumes(v) => v.clone(),
                    _ => i.missing.iter().map(|m| m.volume.clone()).collect(),
                };
            }
        }
    }
    let _ = inner.journal.set_op_state(op.id, OpState::Interrupted);
    inner
        .sink
        .on_event(OpEvent::OpProgress(op.status.lock().clone()));
}

/// The op couldn't run at all.
fn finish_failed(inner: &Inner, op: &OpShared, started: Instant, msg: String) {
    let err = ItemError {
        path: PathBuf::new(),
        dest: None,
        message: msg,
        errno: None,
    };
    let report = OpReport {
        id: op.id,
        kind: op.kind,
        state: OpState::Failed,
        files_done: 0,
        files_failed: 0,
        files_skipped: 0,
        bytes_done: 0,
        errors: vec![err.clone()],
        elapsed_ms: started.elapsed().as_millis() as u64,
    };
    let _ = inner.journal.set_op_state(op.id, OpState::Failed);
    {
        let mut st = op.status.lock();
        st.state = OpState::Failed;
        st.errors = vec![err];
    }
    inner
        .sink
        .on_event(OpEvent::OpFinished { op: op.id, report });
}

/// The items the job leaves, as ops' items: what ops' own engine would have
/// journaled (a folder moved by one rename is one item; what is inside a
/// skipped or failed folder isn't listed).
struct Items {
    updates: Vec<ItemRow>,
    inserts: Vec<(usize, ItemRow)>,
}

/// Builds the op's items from the job's entries. `roots` are the op's
/// top-level rows, in the job's item order.
fn items_of(op: &OpShared, roots: &[ItemRow], entries: &[st::EntryReport], moving: bool) -> Items {
    // Each root's name under the destination.
    let root_name = |it: &ItemRow| {
        it.dest_name
            .clone()
            .unwrap_or_else(|| file_name(&it.source))
    };
    let by_name: HashMap<String, usize> = roots
        .iter()
        .enumerate()
        .map(|(i, it)| (root_name(it), i))
        .collect();
    let mut out = Items {
        updates: roots.to_vec(),
        inserts: Vec::new(),
    };
    // rel → (index into updates++inserts, entry state, moved).
    let mut placed: HashMap<PathBuf, (usize, st::journal::FileSt, bool)> = HashMap::new();
    for e in entries {
        if e.source.is_none() {
            continue; // a folder made only to hold other items
        }
        let parent_rel = e.rel.parent().filter(|p| !p.as_os_str().is_empty());
        let (slot, fresh) = match parent_rel {
            None => {
                let Some(&i) = by_name.get(&e.rel.to_string_lossy().into_owned()) else {
                    continue;
                };
                (i, false)
            }
            Some(p) => {
                let Some(&(pi, pst, pmoved)) = placed.get(p) else {
                    continue;
                };
                // Inside a folder that wasn't placed, or that was moved
                // whole by one rename: not an item of its own.
                if pst != st::journal::FileSt::Done || pmoved {
                    continue;
                }
                (pi, true)
            }
        };
        let mut row = if fresh {
            ItemRow {
                id: 0,
                op: op.id,
                parent: None,
                kind: kind(e.kind),
                source: e.source.clone().unwrap_or_default(),
                dest_dir: None,
                dest_name: None,
                dest: None,
                partial: None,
                state: ItemState::Pending,
                created: false,
                merged: false,
                expanded: false,
                bytes: 0,
                hash: None,
                error: None,
                src_ident: None,
                dest_ident: None,
                partial_ino: None,
                partial_dev: None,
                replaced_trash: None,
                trash_loc: None,
            }
        } else {
            out.updates[slot].clone()
        };
        fill(&mut row, e, moving);
        let index = if fresh {
            out.inserts.push((slot, row));
            out.updates.len() + out.inserts.len() - 1
        } else {
            out.updates[slot] = row;
            slot
        };
        placed.insert(e.rel.clone(), (index, e.state, e.moved));
    }
    out
}

/// One entry's outcome on its row.
fn fill(row: &mut ItemRow, e: &st::EntryReport, moving: bool) {
    use st::journal::FileSt;
    row.kind = kind(e.kind);
    row.dest = Some(e.dest.clone());
    let src_name = e.source.as_deref().map(file_name).unwrap_or_default();
    let dest_name = file_name(&e.dest);
    if dest_name != src_name {
        row.dest_name = Some(dest_name);
    }
    row.bytes = if e.kind == st::journal::EntryKind::Dir {
        0
    } else {
        e.size
    };
    row.hash = e.hash.clone();
    // What redo checks the original against (a moved one is in the Trash).
    if let Some(m) = e.source.as_deref().and_then(|s| meta::lstat(s).ok()) {
        row.src_ident = Some(meta::ident(&m));
    }
    row.replaced_trash = e.replaced_trash.clone();
    row.expanded = e.kind == st::journal::EntryKind::Dir;
    row.partial = None;
    match e.state {
        FileSt::Done => {
            row.merged = e.merged;
            row.created = !e.merged;
            row.dest_ident = meta::lstat(&e.dest).ok().map(|m| meta::ident(&m));
            // The original is still where it was (a move that couldn't send
            // it to the Trash). One that is gone went to the Trash before
            // an interruption the job's journal didn't catch.
            let kept = moving
                && !e.moved
                && !e.merged
                && e.kind != st::journal::EntryKind::Dir
                && e.trashed_to.is_none()
                && e.source.as_deref().is_some_and(meta::exists);
            if kept {
                // Copied, but the original stayed: a per-file error, as
                // when ops couldn't remove a moved original.
                row.state = ItemState::Failed;
                row.error = Some(format!(
                    "copied, but the original couldn't be removed: {}",
                    e.reason
                        .as_deref()
                        .unwrap_or("it couldn't be moved to the Trash")
                ));
            } else {
                row.state = ItemState::Done;
                row.error = None;
            }
        }
        FileSt::Failed => {
            row.state = ItemState::Failed;
            row.error = Some(
                e.reason
                    .as_deref()
                    .map(plain)
                    .unwrap_or_else(|| "it couldn't be copied".into()),
            );
        }
        FileSt::Skipped => {
            row.state = ItemState::Skipped;
            // A skip the user chose isn't an error; one for a reason
            // (an iCloud placeholder, a pipe) is reported, as on ops' path.
            let chosen = matches!(
                e.decision,
                Some(st::Decision::Skip | st::Decision::SkipIfIdentical)
            );
            row.error = if chosen { None } else { e.reason.clone() };
        }
        _ => row.state = ItemState::Pending,
    }
}

/// The job finished (or was cancelled): its entries become the op's items,
/// then the op finishes exactly as on ops' own path.
#[allow(clippy::too_many_arguments)]
fn finish(
    inner: &Inner,
    op: &OpShared,
    entries: Vec<st::EntryReport>,
    roots: &[ItemRow],
    paths: &Paths,
    mode: st::Mode,
    cancelled: bool,
    started: Instant,
) {
    let entries: Vec<st::EntryReport> = entries
        .into_iter()
        .map(|mut e| {
            e.dest = paths.given(&e.dest);
            e.source = e.source.map(|s| paths.given(&s));
            e
        })
        .collect();
    let items = items_of(op, roots, &entries, mode == st::Mode::Move);
    let mut errors = Vec::new();
    if let Err(e) = inner.journal.write_items(&items.updates, &items.inserts) {
        errors.push(ItemError {
            path: PathBuf::new(),
            dest: None,
            message: format!("journal write failed: {e}"),
            errno: None,
        });
    }
    let all: Vec<&ItemRow> = items
        .updates
        .iter()
        .chain(items.inserts.iter().map(|(_, r)| r))
        .collect();
    let count = |s: ItemState| all.iter().filter(|r| r.state == s).count() as u64;
    let (done, failed, skipped) = (
        count(ItemState::Done),
        count(ItemState::Failed),
        count(ItemState::Skipped),
    );
    errors.extend(all.iter().filter_map(|r| {
        let reported = matches!(r.state, ItemState::Failed | ItemState::Skipped);
        r.error.as_ref().filter(|_| reported).map(|m| ItemError {
            path: r.source.clone(),
            dest: r.dest.clone(),
            message: m.clone(),
            errno: None,
        })
    }));
    // Bytes that arrived, including copies whose original couldn't go.
    let bytes: u64 = all
        .iter()
        .filter(|r| r.state == ItemState::Done || (r.created && r.dest.is_some()))
        .map(|r| r.bytes)
        .sum();
    let state = if cancelled {
        OpState::Cancelled
    } else if failed > 0 {
        OpState::CompletedWithErrors
    } else {
        OpState::Completed
    };
    let _ = inner.journal.set_op_state(op.id, state);
    let report = {
        let mut st = op.status.lock();
        st.state = state;
        st.files_done = done + failed + skipped;
        st.files_total = st.files_total.max(st.files_done);
        st.bytes_done = bytes;
        st.current = None;
        st.conflicts.clear();
        st.eta_secs = None;
        st.waiting_for.clear();
        st.resume = None;
        st.errors = errors.clone();
        OpReport {
            id: op.id,
            kind: op.kind,
            state,
            files_done: done,
            files_failed: failed,
            files_skipped: skipped,
            bytes_done: bytes,
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

/// Cancels an interrupted op's job without resuming it: its partials go
/// (the destination must be connected), finished files stay. Then the op
/// finishes as cancelled, with its finished items recorded for undo.
pub(crate) fn cancel_stopped(
    inner: &Inner,
    op: &OpShared,
    row: &OpRow,
    job: &st::JobId,
) -> Result<(), String> {
    let tr = inner.transfer.clone().ok_or("Safe Transfer isn't open")?;
    match tr.engine.cancel(*job, st::Cancel::Close) {
        Ok(()) | Err(st::EngineError::Final(_)) => {}
        Err(e) => return Err(e.to_string()),
    }
    let roots: Vec<ItemRow> = inner
        .journal
        .load_items(op.id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|i| i.parent.is_none())
        .collect();
    let mode = match row.request.kind() {
        OpKind::Move | OpKind::MoveTo => st::Mode::Move,
        _ => st::Mode::Copy,
    };
    let dest = route(inner, &row.request, true)
        .map(|r| r.dest)
        .or_else(|| tr.engine.job(*job).map(|i| i.dest))
        .unwrap_or_default();
    let paths = Paths::new(&dest, &roots);
    let entries = tr.engine.entries(*job).unwrap_or_default();
    finish(
        inner,
        op,
        entries,
        &roots,
        &paths,
        mode,
        true,
        Instant::now(),
    );
    Ok(())
}

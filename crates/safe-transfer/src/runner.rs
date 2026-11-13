//! The job runner (proposal §4.7, §6, §7, §8): one thread per running job,
//! with a few workers for the files themselves.
//!
//! A run is a series of passes over the manifest, in order (folders before
//! their contents):
//! 1. each pass creates folders, copies or moves files (small ones in groups
//!    behind one drive flush, big ones a few at a time), and decides taken
//!    names by the answer, the "apply to all" rule or the policy;
//! 2. after a pass, folders whose contents are all done are finished, deepest
//!    first: their dates and attributes are applied, a replacing folder takes
//!    its name, and a moved folder's emptied original is removed;
//! 3. if a question is open, the job waits for an answer (the rest is done),
//!    then passes again.

use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::chunk::FileHashes;
use crate::copy::{
    Checked, ChunkLog, CopyError, CopyRequest, Failure, FileWork, Prep, Resume, describe, ident,
    resume_from,
};
use crate::engine::{
    GROUP_BYTES, GROUP_FILES, GROUP_TIME, Inner, Job, SMALL_FILE, find, hex128, lock, remap,
    set_state, weight,
};
use crate::events::{FileProgress, FileReport, MetaLoss, Outcome, Phase, ProgressSink};
use crate::job::{self, ConflictInfo, Event, JobProgress};
use crate::journal::{Entry, EntryKind, FileRec, FileSt, Found, JobState, Manifest};
use crate::names::{self, JobId};
use crate::options::{Control, Decision, Disposal, Options, Symlinks};
use crate::platform::{FlushLevel, Platform};
use crate::remove::{self, PartialOwner};

/// Why a run stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stop {
    Done,
    Cancelled,
    DestGone,
    DestFull,
}

/// The folder structure of a manifest: each entry's parent folder and the
/// end of each folder's contents (entries are listed folders first).
pub(crate) struct Layout {
    parent: Vec<Option<usize>>,
    end: Vec<usize>,
}

impl Layout {
    pub(crate) fn new(m: &Manifest) -> Layout {
        let mut by_rel: HashMap<&Path, usize> = HashMap::new();
        let mut parent = Vec::with_capacity(m.files.len());
        for (i, e) in m.files.iter().enumerate() {
            parent.push(e.rel.parent().and_then(|p| by_rel.get(p).copied()));
            if e.kind == EntryKind::Dir {
                by_rel.insert(&e.rel, i);
            }
        }
        let mut end: Vec<usize> = (0..m.files.len()).map(|i| i + 1).collect();
        for i in (0..m.files.len()).rev() {
            if let Some(p) = parent[i] {
                end[p] = end[p].max(end[i]);
            }
        }
        Layout { parent, end }
    }

    /// Where entry `idx` goes: under its parent's place (a "keep both" name,
    /// or a replacing folder's hidden partial), with its own new name if any.
    pub(crate) fn dest_of(
        &self,
        m: &Manifest,
        recs: &[FileRec],
        root: &Path,
        job: JobId,
        idx: usize,
    ) -> PathBuf {
        let e = &m.files[idx];
        let own = recs[idx]
            .dest_name
            .clone()
            .map(PathBuf::from)
            .or_else(|| e.rel.file_name().map(PathBuf::from))
            .unwrap_or_default();
        match self.parent[idx] {
            Some(p) => self.children_of(m, recs, root, job, p).join(own),
            None => root.join(e.rel.parent().unwrap_or(Path::new(""))).join(own),
        }
    }

    /// Where folder `p`'s contents go.
    pub(crate) fn children_of(
        &self,
        m: &Manifest,
        recs: &[FileRec],
        root: &Path,
        job: JobId,
        p: usize,
    ) -> PathBuf {
        let d = self.dest_of(m, recs, root, job, p);
        if recs[p].replacing && !recs[p].finished {
            names::partial_path(&d, job, m.files[p].n)
        } else {
            d
        }
    }

    /// Entries inside folder `idx` (all depths).
    pub(crate) fn inside(&self, idx: usize) -> std::ops::Range<usize> {
        idx + 1..self.end[idx]
    }

    pub(crate) fn parent(&self, idx: usize) -> Option<usize> {
        self.parent[idx]
    }
}

/// The journal side of one file: chunk logs, and the records a replace needs.
struct Log<'a> {
    job: &'a Job,
    idx: usize,
}

impl ChunkLog for Log<'_> {
    fn chunk_flushed(&self, index: usize, hash: u64) {
        self.job.update(self.idx, |r| {
            r.chunks.truncate(index);
            r.chunks.push(format!("{hash:016x}"));
        });
    }
    fn before_replace(&self, hashes: &FileHashes) {
        let hash = hashes.file_hex();
        let chunks: Vec<String> = hashes.chunks.iter().map(|c| format!("{c:016x}")).collect();
        self.job.update_now(self.idx, |r| {
            r.st = FileSt::Flushed;
            r.hash = Some(hash);
            r.chunks = chunks;
        });
    }
    fn replaced(&self, trash: &Path) {
        let t = trash.to_path_buf();
        self.job
            .update_now(self.idx, |r| r.replaced_trash = Some(t));
    }
}

/// Turns one file's progress into job progress.
struct FileSink<'a> {
    inner: &'a Inner,
    job: &'a Job,
    idx: usize,
}

impl ProgressSink for FileSink<'_> {
    fn progress(&self, fp: &FileProgress) {
        let e = &self.job.m.files[self.idx];
        let size = e.size;
        let w = weight(&self.job.m, e);
        let off = match fp.phase {
            Phase::Preparing => 0,
            Phase::Copying => fp.done,
            Phase::Flushing => size,
            Phase::Verifying => size + fp.done,
            Phase::Rereading => 2 * size + fp.done,
            Phase::Renaming | Phase::Metadata => w,
        }
        .min(w);
        let (done, files_done, rate) = {
            let st = lock(&self.job.st);
            let done = st.work_done + off;
            let el = st.run_started.map_or(0.0, |t| t.elapsed().as_secs_f64());
            let rate = if el > 0.2 {
                (done.saturating_sub(st.run_base) as f64 / el) as u64
            } else {
                0
            };
            (done, st.files_done, rate)
        };
        let total = self.job.work_total;
        self.inner.sink.on_event(Event::Progress(JobProgress {
            job: self.job.id,
            phase: fp.phase,
            file: Some(e.rel.clone()),
            done,
            total,
            files_done,
            files_total: self.job.files_total,
            bytes_per_sec: rate,
            eta_secs: (rate > 0).then(|| total.saturating_sub(done) / rate),
        }));
    }
}

/// Points of the job (not of one file) for fault injection.
#[derive(Clone, Copy)]
#[cfg_attr(not(feature = "fault-injection"), allow(dead_code))]
enum JobPoint {
    BeforeBarrier,
    AfterBarrier,
    Rename(usize),
    BeforeFolderBarrier,
    BeforeSourceTrash,
    BeforeFolderSwap,
}

/// What to do with a file after looking at the disk.
enum Next {
    /// Nothing more now (the record says why).
    Settled,
    /// Copy it, continuing `Resume` if given; `replace` an existing item.
    Copy {
        resume: Option<Box<Resume>>,
        replace: bool,
    },
}

/// A big file, link or clone waiting for a worker.
struct Big {
    idx: usize,
    req: CopyRequest,
    resume: Option<Box<Resume>>,
    replace: bool,
}

pub(crate) struct Runner<'a> {
    inner: &'a Inner,
    job: &'a Job,
    ctl: &'a Control,
    opts: Options,
    layout: Layout,
    dest_root: PathBuf,
    src_mounts: Vec<Option<PathBuf>>,
    small_workers: usize,
    big_workers: usize,
    /// A decision changed something this pass that needs another pass.
    again: AtomicBool,
    /// Folders a same-volume move renamed into or out of (flushed at the end).
    touched: Mutex<BTreeSet<PathBuf>>,
}

impl<'a> Runner<'a> {
    pub(crate) fn new(inner: &'a Inner, job: &'a Job, ctl: &'a Control) -> Runner<'a> {
        let o = &job.m.options;
        let opts = Options {
            chunk_size: o.chunk_size,
            flush_each_chunk: o.flush_each_chunk,
            reread_source: o.reread_source,
            symlinks: if o.follow_symlinks {
                Symlinks::Follow
            } else {
                Symlinks::CopyLink
            },
            allow_dataless: o.allow_dataless,
            clone_same_volume: o.clone_same_volume,
            on_conflict: o.on_conflict,
            disposal: o.disposal,
            workers: o.workers,
            #[cfg(feature = "fault-injection")]
            faults: inner.cfg.faults.clone(),
            ..Options::default()
        };
        let (small_workers, big_workers) = match o.workers {
            Some(n) => (n.max(1), n.max(1)),
            None => (4, 2),
        };
        Runner {
            inner,
            job,
            ctl,
            opts,
            layout: Layout::new(&job.m),
            dest_root: PathBuf::new(),
            src_mounts: Vec::new(),
            small_workers,
            big_workers,
            again: AtomicBool::new(false),
            touched: Mutex::new(BTreeSet::new()),
        }
    }

    fn p(&self) -> &Platform {
        &self.inner.p
    }

    fn moving(&self) -> bool {
        self.job.m.mode == "move"
    }

    fn fault(&self, point: JobPoint) -> io::Result<()> {
        self.fault_at(point, &self.dest_root)
    }

    /// [`Self::fault`] for one item: the hook sees `path` (a moved file's
    /// original), so a test can pick the file it fails.
    #[cfg_attr(not(feature = "fault-injection"), allow(unused_variables))]
    fn fault_at(&self, point: JobPoint, path: &Path) -> io::Result<()> {
        #[cfg(feature = "fault-injection")]
        if let Some(h) = &self.opts.faults {
            use crate::fault::FaultPoint as F;
            let p = match point {
                JobPoint::BeforeBarrier => F::BeforeGroupBarrier,
                JobPoint::AfterBarrier => F::AfterGroupBarrier,
                JobPoint::Rename(i) => F::MidGroupRename { index: i as u64 },
                JobPoint::BeforeFolderBarrier => F::BeforeFolderBarrier,
                JobPoint::BeforeSourceTrash => F::BeforeSourceTrash,
                JobPoint::BeforeFolderSwap => F::BeforeFolderSwap,
            };
            h.at(p, path)?;
        }
        Ok(())
    }

    pub(crate) fn go(&mut self) -> Stop {
        let vols = self.p().volumes.mounted().unwrap_or_default();
        let m = &self.job.m;
        let Some(dv) = find(&vols, &m.dest.volume) else {
            return Stop::DestGone;
        };
        self.dest_root = remap(&m.dest.path, &m.dest.volume.mount, &dv.mount);
        if std::fs::create_dir_all(&self.dest_root).is_err() {
            return Stop::DestGone;
        }
        self.src_mounts = m
            .sources
            .iter()
            .map(|s| find(&vols, s).map(|v| v.mount.clone()))
            .collect();
        // Items left pending after the last pass, to notice an answer that
        // arrived during it (it is pending again, and needs another pass).
        let mut last_pending = usize::MAX;
        loop {
            if let Some(s) = self.pass() {
                return s;
            }
            if let Some(s) = self.finish_folders() {
                return s;
            }
            self.flush_touched();
            if self.again.swap(false, Ordering::AcqRel) {
                continue;
            }
            if !self.open_questions() {
                let pending = self.pending();
                if pending > 0 && pending != last_pending {
                    last_pending = pending;
                    continue;
                }
                return Stop::Done;
            }
            set_state(self.inner, self.job, JobState::WaitingForAnswer);
            if !self.wait_for_answers() {
                return Stop::Cancelled;
            }
            set_state(self.inner, self.job, JobState::Running);
        }
    }

    /// Items still to do (not settled, not waiting for a card or an answer).
    fn pending(&self) -> usize {
        lock(&self.job.st)
            .recs
            .iter()
            .filter(|r| r.st == FileSt::Pending)
            .count()
    }

    fn open_questions(&self) -> bool {
        lock(&self.job.st)
            .recs
            .iter()
            .any(|r| r.st == FileSt::Conflict)
    }

    /// Waits until no question is open (or the job is cancelled).
    fn wait_for_answers(&self) -> bool {
        let mut st = lock(&self.job.st);
        while st.recs.iter().any(|r| r.st == FileSt::Conflict) {
            if self.ctl.is_cancelled() {
                return false;
            }
            st = self
                .job
                .answers
                .wait_timeout(st, Duration::from_millis(200))
                .map(|(g, _)| g)
                .unwrap_or_else(|e| e.into_inner().0);
        }
        !self.ctl.is_cancelled()
    }

    fn recs(&self) -> Vec<FileRec> {
        lock(&self.job.st).recs.clone()
    }

    fn dest_of(&self, idx: usize) -> PathBuf {
        let st = lock(&self.job.st);
        self.layout
            .dest_of(&self.job.m, &st.recs, &self.dest_root, self.job.id, idx)
    }

    /// The answer for a taken name: given, from "apply to all", or the policy.
    fn decision(&self, idx: usize, folder: bool) -> Decision {
        let st = lock(&self.job.st);
        let rule = if folder {
            st.rules.folders
        } else {
            st.rules.files
        };
        let policy = if folder {
            self.opts.on_conflict.folders
        } else {
            self.opts.on_conflict.files
        };
        st.recs[idx].decision.or(rule).unwrap_or(policy)
    }

    /// One pass over the manifest.
    fn pass(&self) -> Option<Stop> {
        let m = &self.job.m;
        let mut group: Vec<(usize, CopyRequest)> = Vec::new();
        let mut group_bytes = 0u64;
        let mut bigs: Vec<Big> = Vec::new();
        for idx in 0..m.files.len() {
            if self.ctl.is_cancelled() {
                return Some(Stop::Cancelled);
            }
            let rec = self.job.rec(idx);
            let e = &m.files[idx];
            if rec.st.is_terminal() {
                if rec.st == FileSt::Done && self.needs_source_trash(e, &rec) {
                    // A move whose copy finished before an interruption.
                    if let Some(src) = self.source_path(e) {
                        self.trash_source(idx, &src);
                    }
                }
                continue;
            }
            if rec.st == FileSt::Conflict {
                continue;
            }
            match self.ancestry(idx) {
                Ancestry::Ready => {}
                Ancestry::Wait => continue,
                Ancestry::Skip(why) => {
                    self.job.update(idx, |r| {
                        r.st = FileSt::Skipped;
                        r.reason = Some(why);
                    });
                    continue;
                }
            }
            match e.kind {
                EntryKind::Dir => {
                    // Its contents come after it, so pending files are elsewhere.
                    if let Some(s) = self.dir(idx, e) {
                        return Some(self.drain(s, &mut group, &mut bigs));
                    }
                }
                EntryKind::Other => self.job.update(idx, |r| {
                    r.st = FileSt::Skipped;
                    r.reason = Some("pipes, sockets and devices can't be copied".into());
                }),
                EntryKind::File | EntryKind::Link => {
                    let Some(src) = self.source_path(e) else {
                        self.job.update(idx, |r| r.st = FileSt::WaitingSource);
                        continue;
                    };
                    let dest = self.dest_of(idx);
                    if self.moving() && self.same_volume(e) {
                        if let Some(s) = self.rename_move(idx, e, &src, &dest) {
                            return Some(self.drain(s, &mut group, &mut bigs));
                        }
                        continue;
                    }
                    let req = CopyRequest {
                        source: src,
                        dest,
                        job: self.job.id,
                        index: e.n,
                    };
                    let small = e.kind == EntryKind::File && e.size < SMALL_FILE && !self.clones(e);
                    match self.decide(idx, &req, e, small) {
                        Next::Settled => {}
                        Next::Copy {
                            resume: None,
                            replace: false,
                        } if small => {
                            group_bytes += e.size;
                            group.push((idx, req));
                            if group.len() >= GROUP_FILES || group_bytes >= GROUP_BYTES {
                                group_bytes = 0;
                                if let Some(s) = self.commit_group(std::mem::take(&mut group)) {
                                    return Some(self.drain(s, &mut group, &mut bigs));
                                }
                            }
                        }
                        Next::Copy { resume, replace } => {
                            bigs.push(Big {
                                idx,
                                req,
                                resume,
                                replace,
                            });
                            if bigs.len() >= self.big_workers
                                && let Some(s) = self.run_bigs(std::mem::take(&mut bigs))
                            {
                                return Some(self.drain(s, &mut group, &mut bigs));
                            }
                        }
                    }
                }
            }
        }
        if let Some(s) = self.commit_group(group) {
            return Some(s);
        }
        self.run_bigs(bigs)
    }

    /// Stops: pending work that was never started is simply dropped.
    fn drain(&self, s: Stop, group: &mut Vec<(usize, CopyRequest)>, bigs: &mut Vec<Big>) -> Stop {
        group.clear();
        bigs.clear();
        s
    }

    fn ancestry(&self, idx: usize) -> Ancestry {
        // An item that takes the same name as an earlier one waits for it.
        if let Some(t) = self.job.m.files[idx].after {
            let twin = self.job.rec(t as usize).st;
            if !twin.is_terminal() {
                // Look again once the twin is settled: after this pass, or
                // (a question) after its answer.
                if twin != FileSt::Conflict {
                    self.again.store(true, Ordering::Release);
                }
                return Ancestry::Wait;
            }
        }
        let Some(p) = self.layout.parent(idx) else {
            return Ancestry::Ready;
        };
        let r = self.job.rec(p);
        match r.st {
            FileSt::Done if r.moved => {
                // Inside a folder moved by one rename: it moved too.
                self.job.update(idx, |c| {
                    c.st = FileSt::Done;
                    c.moved = true;
                    c.outcome = Some("moved_with_folder".into());
                    c.finished = true;
                });
                Ancestry::Wait
            }
            FileSt::Done => Ancestry::Ready,
            FileSt::Skipped => Ancestry::Skip("its folder was skipped".into()),
            FileSt::Failed => Ancestry::Skip("its folder couldn't be made".into()),
            _ => Ancestry::Wait,
        }
    }

    /// Same APFS volume and cloning on: an instant clone, never grouped.
    fn clones(&self, e: &Entry) -> bool {
        self.opts.clone_same_volume && self.same_volume(e) && self.job.m.dest.volume.fs == "apfs"
    }

    /// Source and destination are the same volume (by UUID).
    fn same_volume(&self, e: &Entry) -> bool {
        let s = &self.job.m.sources[e.src_vol as usize];
        let d = &self.job.m.dest.volume;
        s.uuid.is_some() && s.uuid == d.uuid
    }

    fn source_path(&self, e: &Entry) -> Option<PathBuf> {
        let src = e.src.as_ref()?;
        let v = &self.job.m.sources[e.src_vol as usize];
        let mount = self.src_mounts.get(e.src_vol as usize)?.as_ref()?;
        Some(remap(src, &v.mount, mount))
    }

    fn dest_gone(&self) -> bool {
        match self.p().volumes.volume_of(&self.dest_root) {
            Ok(v) => {
                v.uuid != self.job.m.dest.volume.uuid || v.fs_type != self.job.m.dest.volume.fs
            }
            Err(_) => true,
        }
    }

    fn source_gone(&self, src_vol: u32) -> bool {
        let vols = self.p().volumes.mounted().unwrap_or_default();
        find(&vols, &self.job.m.sources[src_vol as usize]).is_none()
    }

    /// A free "keep both" name next to `dest` ("photo 2.ARW").
    fn free_name(&self, dest: &Path, folder: bool) -> String {
        let name = dest
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let dir = dest.parent().unwrap_or(Path::new("/"));
        names::keep_both_names(&name, folder)
            .find(|n| std::fs::symlink_metadata(dir.join(n)).is_err())
            .expect("keep_both_names is endless")
    }

    /// Records a taken name as a question, unless one is already open.
    fn ask(&self, idx: usize, dest: &Path) {
        let e = &self.job.m.files[idx];
        let found = std::fs::symlink_metadata(dest).ok().map(|m| {
            let i = ident(&m);
            Found {
                dir: m.is_dir(),
                size: i.size,
                mtime_ns: i.mtime_ns,
            }
        });
        let was = self.job.rec(idx).st;
        self.job.update(idx, |r| {
            r.st = FileSt::Conflict;
            r.found = found;
        });
        if was != FileSt::Conflict {
            let f = found.unwrap_or(Found {
                dir: false,
                size: 0,
                mtime_ns: 0,
            });
            self.inner.sink.on_event(Event::Conflict {
                job: self.job.id,
                info: Box::new(ConflictInfo {
                    id: idx as u32,
                    path: e.rel.clone(),
                    source: e.src.clone(),
                    dest: dest.to_path_buf(),
                    folder: e.kind == EntryKind::Dir,
                    source_size: e.size,
                    source_mtime_ns: e.mtime_ns,
                    existing_folder: f.dir,
                    existing_size: f.size,
                    existing_mtime_ns: f.mtime_ns,
                }),
            });
        }
    }

    fn skip(&self, idx: usize, why: &str) {
        let e = &self.job.m.files[idx];
        self.job.update(idx, |r| {
            r.st = FileSt::Skipped;
            r.reason = Some(why.to_string());
            r.chunks.clear();
        });
        self.inner.sink.on_event(Event::FileNotCopied {
            job: self.job.id,
            path: e.rel.clone(),
            failed: false,
            reason: why.to_string(),
        });
    }

    fn fail(&self, idx: usize, why: String) {
        let e = &self.job.m.files[idx];
        self.job.update(idx, |r| {
            r.st = FileSt::Failed;
            r.reason = Some(why.clone());
            r.chunks.clear();
        });
        self.inner.sink.on_event(Event::FileNotCopied {
            job: self.job.id,
            path: e.rel.clone(),
            failed: true,
            reason: why,
        });
    }

    // ───────────────────────── folders ─────────────────────────

    /// A folder: create it, merge into it, or decide its taken name. A move
    /// on one volume renames the whole folder when its name is free.
    fn dir(&self, idx: usize, e: &Entry) -> Option<Stop> {
        let dest = self.dest_of(idx);
        let src = self.source_path(e);
        let same_move = self.moving() && self.same_volume(e) && e.src.is_some();
        if same_move && src.is_none() {
            self.job.update(idx, |r| r.st = FileSt::WaitingSource);
            return None;
        }
        let there = std::fs::symlink_metadata(&dest).ok();
        let Some(md) = there else {
            if same_move {
                return self.rename_dir(idx, src.as_deref().unwrap_or(Path::new("")), &dest);
            }
            return self.make_dir(idx, &dest, "folder");
        };
        if md.is_dir() && same_move && src.as_ref().is_some_and(|s| same_object(s, &dest)) {
            self.skip(idx, "it is already there");
            return None;
        }
        let rec = self.job.rec(idx);
        if md.is_dir() && !rec.existed && rec.decision.is_none() && !rec.replacing {
            // Nothing was there when the job was planned: this is the folder
            // the job made before an interruption the journal didn't catch
            // up with. It is ours; its contents are checked one by one.
            self.job.update(idx, |r| {
                r.st = FileSt::Done;
                r.outcome = Some("folder".into());
                r.resumed = true;
            });
            return None;
        }
        let d = self.decision(idx, true);
        match d {
            Decision::Merge if md.is_dir() => {
                self.job.update(idx, |r| {
                    r.st = FileSt::Done;
                    r.outcome = Some("merged".into());
                });
                None
            }
            Decision::Skip | Decision::SkipIfIdentical => {
                self.skip(
                    idx,
                    "a folder with this name already exists; it was left as it is",
                );
                None
            }
            Decision::KeepBoth => {
                let name = self.free_name(&dest, true);
                self.job.update(idx, |r| r.dest_name = Some(name));
                self.again.store(true, Ordering::Release);
                None
            }
            Decision::Replace
                if src
                    .as_deref()
                    .is_some_and(|s| same_object(s, &dest) || s.starts_with(&dest)) =>
            {
                // The folder itself, or one inside it: replacing it would
                // send the original to the Trash.
                self.fail(
                    idx,
                    "a folder can't replace itself or a folder that contains it".into(),
                );
                None
            }
            Decision::Replace if same_move => {
                // One rename after the old item is in the Trash.
                self.job
                    .update_now(idx, |r| r.decision = Some(Decision::Replace));
                match self.p().trash.trash(&dest) {
                    Ok(loc) => {
                        self.job.update_now(idx, |r| r.replaced_trash = Some(loc));
                        self.rename_dir(idx, src.as_deref().unwrap_or(Path::new("")), &dest)
                    }
                    Err(err) => {
                        self.fail(idx, format!("the existing item couldn't be moved to the Trash, so it was kept: {}", describe(&err)));
                        None
                    }
                }
            }
            Decision::Replace if md.is_dir() => {
                // Built under a hidden partial name; swapped in when complete.
                let partial = names::partial_path(&dest, self.job.id, e.n);
                self.job.update_now(idx, |r| {
                    r.replacing = true;
                    r.decision = Some(Decision::Replace);
                });
                self.make_dir(idx, &partial, "folder")
            }
            Decision::Replace => {
                // A file is where the folder goes: it goes to the Trash first
                // (nothing of it is lost; the folder can't be verified as one).
                match self.p().trash.trash(&dest) {
                    Ok(loc) => {
                        self.job.update_now(idx, |r| r.replaced_trash = Some(loc));
                        self.make_dir(idx, &dest, "folder")
                    }
                    Err(err) => {
                        self.fail(idx, format!("the existing item couldn't be moved to the Trash, so it was kept: {}", describe(&err)));
                        None
                    }
                }
            }
            Decision::Ask | Decision::Merge => {
                self.ask(idx, &dest);
                None
            }
        }
    }

    fn make_dir(&self, idx: usize, path: &Path, outcome: &str) -> Option<Stop> {
        let made = match std::fs::create_dir(path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists && path.is_dir() => Ok(()),
            Err(err) => Err(err),
        };
        match made {
            Ok(()) => {
                if let Some(parent) = path.parent() {
                    let _ = self.p().flush.flush_dir(parent);
                }
                self.job.update(idx, |r| {
                    r.st = FileSt::Done;
                    r.outcome = Some(outcome.into());
                });
                None
            }
            Err(_) if self.dest_gone() => Some(Stop::DestGone),
            Err(err) if err.raw_os_error() == Some(libc::ENOSPC) => Some(Stop::DestFull),
            Err(err) => {
                self.fail(
                    idx,
                    format!("the folder couldn't be made: {}", describe(&err)),
                );
                None
            }
        }
    }

    /// A same-volume move of a whole folder: one exclusive rename.
    fn rename_dir(&self, idx: usize, src: &Path, dest: &Path) -> Option<Stop> {
        match self.p().rename.rename_exclusive(src, dest) {
            Ok(_) => {
                self.note_touched(src, dest);
                let range = self.layout.inside(idx);
                self.job.update(idx, |r| {
                    r.st = FileSt::Done;
                    r.moved = true;
                    r.outcome = Some("moved".into());
                    r.finished = true;
                });
                for i in range {
                    if !self.job.rec(i).st.is_terminal() {
                        self.job.update(i, |r| {
                            r.st = FileSt::Done;
                            r.moved = true;
                            r.outcome = Some("moved_with_folder".into());
                            r.finished = true;
                        });
                    }
                }
                None
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                self.ask(idx, dest);
                None
            }
            Err(err) if err.raw_os_error() == Some(libc::EXDEV) => {
                // Not really one volume (a firmlink boundary): copy its
                // contents instead, then the emptied original is removed.
                self.make_dir(idx, dest, "folder")
            }
            Err(err) => {
                self.fail(idx, format!("it couldn't be moved: {}", describe(&err)));
                None
            }
        }
    }

    /// Folders whose contents are all done, deepest first: dates and
    /// attributes, a replacing folder taking its name, and a moved folder's
    /// emptied original. Their metadata goes on last because adding the
    /// contents changes a folder's dates.
    fn finish_folders(&self) -> Option<Stop> {
        let m = &self.job.m;
        for idx in (0..m.files.len()).rev() {
            let e = &m.files[idx];
            if e.kind != EntryKind::Dir {
                continue;
            }
            let rec = self.job.rec(idx);
            if rec.st != FileSt::Done || rec.finished {
                continue;
            }
            let recs = self.recs();
            let range = self.layout.inside(idx);
            if !range.clone().all(|i| recs[i].st.is_terminal()) {
                continue;
            }
            let all_done = range.clone().all(|i| recs[i].st == FileSt::Done);
            let dest = self
                .layout
                .dest_of(m, &recs, &self.dest_root, self.job.id, idx);
            let src = self.source_path(e);
            let mut lost: Vec<String> = Vec::new();
            if rec.replacing {
                let partial = names::partial_path(&dest, self.job.id, e.n);
                if !all_done {
                    // Never half-replace a folder: the old one stays, and the
                    // new, incomplete one goes to the Trash (nothing is lost).
                    let _ = self.p().trash.trash(&partial);
                    for i in range.clone() {
                        if recs[i].st == FileSt::Done {
                            self.job.update(i, |r| {
                                r.st = FileSt::Skipped;
                                r.reason = Some("its folder wasn't replaced, because some items in it couldn't be copied".into());
                            });
                        }
                    }
                    self.job.update(idx, |r| {
                        r.st = FileSt::Failed;
                        r.replacing = false;
                        r.reason = Some("the folder wasn't replaced, because some items in it couldn't be copied; the old one was kept".into());
                    });
                    continue;
                }
                if let Err(err) = self.fault(JobPoint::BeforeFolderSwap) {
                    return self.job_error(idx, err);
                }
                if std::fs::symlink_metadata(&dest).is_ok() {
                    match self.p().trash.trash(&dest) {
                        Ok(loc) => self.job.update_now(idx, |r| r.replaced_trash = Some(loc)),
                        Err(err) => {
                            if self.dest_gone() {
                                return Some(Stop::DestGone);
                            }
                            self.fail(idx, format!("the existing folder couldn't be moved to the Trash, so it was kept: {}", describe(&err)));
                            continue;
                        }
                    }
                }
                match self.p().rename.rename_exclusive(&partial, &dest) {
                    Ok(_) => {
                        if let Some(parent) = dest.parent() {
                            let _ = self.p().flush.flush_dir(parent);
                        }
                    }
                    Err(err) => {
                        if self.dest_gone() {
                            return Some(Stop::DestGone);
                        }
                        self.fail(
                            idx,
                            format!("the new folder couldn't take its name: {}", describe(&err)),
                        );
                        continue;
                    }
                }
            }
            // Dates and attributes for folders this job made (a merged folder
            // keeps its own).
            if matches!(rec.outcome.as_deref(), Some("folder"))
                && let Some(src) = &src
            {
                if let Err(err) = self.p().meta.copy_metadata(src, &dest) {
                    lost.push(format!(
                        "its dates and attributes couldn't be copied: {}",
                        describe(&err)
                    ));
                }
                if let Err(err) = self.p().meta.apply_security(src, &dest) {
                    lost.push(format!(
                        "its permissions or flags couldn't be applied: {}",
                        describe(&err)
                    ));
                }
                if let (Ok(a), Ok(b)) = (self.p().meta.snapshot(src), self.p().meta.snapshot(&dest))
                {
                    lost.extend(
                        crate::copy::metadata_lost(&a, &b)
                            .iter()
                            .map(MetaLoss::describe),
                    );
                }
            }
            // A move's emptied original folder (never a folder with anything left).
            let mut kept = None;
            if self.moving()
                && !rec.moved
                && self.opts.disposal == Disposal::Trash
                && let Some(src) = &src
                && src.exists()
            {
                if all_done {
                    if let Err(err) = remove::remove_empty_source_dir(src) {
                        kept = Some(if err.raw_os_error() == Some(libc::ENOTEMPTY) {
                            "the original folder still has items in it, so it was kept".to_string()
                        } else {
                            format!("the original folder was kept: {}", describe(&err))
                        });
                    }
                } else {
                    kept = Some(
                        "the original folder was kept, because some items in it weren't moved"
                            .to_string(),
                    );
                }
            }
            self.job.update(idx, |r| {
                r.finished = true;
                r.replacing = false;
                r.meta_lost.extend(lost);
                if kept.is_some() {
                    r.source_kept = kept;
                }
            });
        }
        None
    }

    fn job_error(&self, idx: usize, err: io::Error) -> Option<Stop> {
        if self.dest_gone() {
            return Some(Stop::DestGone);
        }
        self.fail(idx, describe(&err));
        None
    }

    // ───────────────────────── moves ─────────────────────────

    fn needs_source_trash(&self, e: &Entry, rec: &FileRec) -> bool {
        self.moving()
            && e.kind != EntryKind::Dir
            && !rec.moved
            && rec.trashed_to.is_none()
            && rec.source_kept.is_none()
            && self.opts.disposal == Disposal::Trash
            && !self.same_volume(e)
    }

    /// The original of a cross-volume move, once its copy is in place and
    /// verified: to the Trash, unless it changed since it was copied.
    fn trash_source(&self, idx: usize, src: &Path) {
        let rec = self.job.rec(idx);
        let Ok(md) = std::fs::symlink_metadata(src) else {
            return; // already gone (in the Trash from before an interruption)
        };
        let now = ident(&md);
        if rec
            .src_ident
            .is_some_and(|(s, t, _)| s != now.size || t != now.mtime_ns)
        {
            self.job.update(idx, |r| {
                r.source_kept =
                    Some("the original changed after it was copied, so it was kept".into());
            });
            return;
        }
        if self.fault_at(JobPoint::BeforeSourceTrash, src).is_err() {
            self.job.update(idx, |r| {
                r.source_kept = Some("the original was kept (fault injected)".into())
            });
            return;
        }
        match self.p().trash.trash(src) {
            Ok(loc) => self.job.update(idx, |r| r.trashed_to = Some(loc)),
            Err(err) => self.job.update(idx, |r| {
                r.source_kept = Some(format!(
                    "copied and verified; the original was kept because it couldn't be moved to the Trash: {}",
                    describe(&err)
                ));
            }),
        }
    }

    fn note_touched(&self, src: &Path, dest: &Path) {
        let mut t = lock(&self.touched);
        if let Some(p) = src.parent() {
            t.insert(p.to_path_buf());
        }
        if let Some(p) = dest.parent() {
            t.insert(p.to_path_buf());
        }
    }

    /// One drive flush for all folders renamed into or out of.
    fn flush_touched(&self) {
        let dirs = std::mem::take(&mut *lock(&self.touched));
        let last = dirs.iter().next_back().cloned();
        for d in &dirs {
            if let Ok(f) = File::open(d) {
                if Some(d) == last.as_ref() {
                    let _ = self.p().flush.flush_file(&f);
                } else {
                    let _ = self.p().flush.os_flush(&f);
                }
            }
        }
    }

    /// A same-volume move of a file or link: one exclusive rename.
    fn rename_move(&self, idx: usize, e: &Entry, src: &Path, dest: &Path) -> Option<Stop> {
        let rec = self.job.rec(idx);
        if std::fs::symlink_metadata(src).is_err() {
            if std::fs::symlink_metadata(dest).is_ok() && rec.st != FileSt::Pending {
                // Renamed before an interruption.
                self.job.update(idx, |r| {
                    r.st = FileSt::Done;
                    r.moved = true;
                    r.outcome = Some("moved".into());
                });
            } else if self.source_gone(e.src_vol) {
                self.job.update(idx, |r| r.st = FileSt::WaitingSource);
            } else {
                self.fail(idx, "the item no longer exists".into());
            }
            return None;
        }
        if std::fs::symlink_metadata(dest).is_ok() {
            if same_object(src, dest) {
                self.skip(idx, "it is already there");
                return None;
            }
            match self.decision(idx, false) {
                Decision::Skip => {
                    self.skip(idx, "already exists; nothing was replaced");
                    return None;
                }
                Decision::SkipIfIdentical if self.identical(src, dest, e) => {
                    self.skip(
                        idx,
                        "already there, identical; the original was left where it is",
                    );
                    return None;
                }
                Decision::KeepBoth => {
                    let name = self.free_name(dest, false);
                    self.job.update(idx, |r| r.dest_name = Some(name));
                    let dest = self.dest_of(idx);
                    return self.rename_move(idx, e, src, &dest);
                }
                Decision::Replace => {
                    self.job.update_now(idx, |r| {
                        r.st = FileSt::Copying;
                        r.decision = Some(Decision::Replace);
                    });
                    match self.p().trash.trash(dest) {
                        Ok(loc) => self.job.update_now(idx, |r| r.replaced_trash = Some(loc)),
                        Err(err) => {
                            self.fail(idx, format!("the existing item couldn't be moved to the Trash, so it was kept: {}", describe(&err)));
                            return None;
                        }
                    }
                }
                _ => {
                    self.ask(idx, dest);
                    return None;
                }
            }
        }
        self.job.update(idx, |r| r.st = FileSt::Copying);
        match self.p().rename.rename_exclusive(src, dest) {
            Ok(_) => {
                self.note_touched(src, dest);
                self.job.update(idx, |r| {
                    r.st = FileSt::Done;
                    r.moved = true;
                    r.outcome = Some("moved".into());
                });
                None
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                self.ask(idx, dest);
                None
            }
            Err(err) if err.raw_os_error() == Some(libc::EXDEV) => {
                // Copy, verify, then the original to the Trash.
                let req = CopyRequest {
                    source: src.to_path_buf(),
                    dest: dest.to_path_buf(),
                    job: self.job.id,
                    index: e.n,
                };
                self.alone(idx, &req, None, false, true)
            }
            Err(err) => {
                self.fail(idx, format!("it couldn't be moved: {}", describe(&err)));
                None
            }
        }
    }

    // ───────────────────────── copies ─────────────────────────

    /// Looks at the disk before copying a file that isn't finished (§6, §8).
    fn decide(&self, idx: usize, req: &CopyRequest, e: &Entry, small: bool) -> Next {
        let rec = self.job.rec(idx);
        let owner = PartialOwner {
            final_path: req.dest.clone(),
            job: self.job.id,
            n: e.n,
        };
        let partial = owner.path();
        let mut partial_there = std::fs::symlink_metadata(&partial).is_ok();
        let p = self.p();
        let mut replace = false;
        if std::fs::symlink_metadata(&req.dest).is_ok() {
            if rec.st == FileSt::Placed {
                // Renamed before the interruption; only ACLs and flags were left.
                let _ = p.meta.apply_security(&req.source, &req.dest);
                self.job.update(idx, |r| {
                    r.st = FileSt::Done;
                    r.resumed = true;
                    r.outcome.get_or_insert_with(|| "verified".into());
                });
                self.after_done(idx, req);
                return Next::Settled;
            }
            // Nothing was there when planned, or the old one is in the Trash
            // already: most likely our own copy, renamed just before the
            // journal caught up. It counts only if it is identical.
            if (!rec.existed || rec.replaced_trash.is_some()) && self.same_content(req, e, &rec) {
                if partial_there {
                    let _ = remove::remove_partial(p, &partial, &owner);
                }
                self.job.update(idx, |r| {
                    r.st = FileSt::Done;
                    r.resumed = true;
                    r.outcome = Some(
                        if e.kind == EntryKind::Link {
                            "link"
                        } else {
                            "verified"
                        }
                        .into(),
                    );
                });
                self.after_done(idx, req);
                return Next::Settled;
            }
            match self.decision(idx, false) {
                Decision::Skip => {
                    if partial_there {
                        let _ = remove::remove_partial(p, &partial, &owner);
                    }
                    self.skip(idx, "already exists; nothing was replaced");
                    return Next::Settled;
                }
                Decision::SkipIfIdentical if self.identical(&req.source, &req.dest, e) => {
                    if partial_there {
                        let _ = remove::remove_partial(p, &partial, &owner);
                    }
                    self.skip(idx, "already there, identical");
                    return Next::Settled;
                }
                Decision::KeepBoth => {
                    if partial_there {
                        let _ = remove::remove_partial(p, &partial, &owner);
                    }
                    let name = self.free_name(&req.dest, false);
                    self.job.update(idx, |r| r.dest_name = Some(name));
                    let req2 = CopyRequest {
                        dest: self.dest_of(idx),
                        ..req.clone()
                    };
                    return match self.decide(idx, &req2, e, small) {
                        Next::Settled => Next::Settled,
                        // The new name changes the request: run it next pass.
                        Next::Copy { .. } => {
                            self.again.store(true, Ordering::Release);
                            Next::Settled
                        }
                    };
                }
                Decision::Replace if same_object(&req.source, &req.dest) => {
                    // Copying an item onto itself: "replace" would send the
                    // original to the Trash. Never.
                    if partial_there {
                        let _ = remove::remove_partial(p, &partial, &owner);
                    }
                    self.fail(idx, "an item can't replace itself".into());
                    return Next::Settled;
                }
                Decision::Replace => replace = true,
                _ => {
                    if partial_there {
                        let _ = remove::remove_partial(p, &partial, &owner);
                    }
                    self.ask(idx, &req.dest);
                    return Next::Settled;
                }
            }
        }
        let Ok(smd) = std::fs::symlink_metadata(&req.source) else {
            return Next::Copy {
                resume: None,
                replace,
            }; // prepare() reports it
        };
        if let Some((size, mtime, ino)) = rec.src_ident {
            let src_fs = &self.job.m.sources[e.src_vol as usize].fs;
            let ino_counts = matches!(src_fs.as_str(), "apfs" | "hfs");
            let now = ident(&smd);
            if now.size != size || now.mtime_ns != mtime || (ino_counts && now.ino != ino) {
                if partial_there {
                    let _ = remove::remove_partial(p, &partial, &owner);
                }
                let why = if now.size != size {
                    format!(
                        "the original changed ({} → {})",
                        job::human(size),
                        job::human(now.size)
                    )
                } else {
                    "the original was modified since its copy started".into()
                };
                self.job.update(idx, |r| {
                    r.restarted = Some(why);
                    r.st = FileSt::Pending;
                    r.chunks.clear();
                    r.hash = None;
                    r.src_ident = None;
                });
                return Next::Copy {
                    resume: None,
                    replace,
                };
            }
        }
        if partial_there && (small || e.kind != EntryKind::File) {
            let _ = remove::remove_partial(p, &partial, &owner);
            partial_there = false;
        }
        if !partial_there {
            return Next::Copy {
                resume: None,
                replace,
            };
        }
        match resume_from(
            p,
            &partial,
            &rec.chunk_hashes(),
            self.opts.chunk_size,
            smd.len(),
        ) {
            Ok(r) => Next::Copy {
                resume: Some(Box::new(r)),
                replace,
            },
            Err(_) => {
                let _ = remove::remove_partial(p, &partial, &owner);
                Next::Copy {
                    resume: None,
                    replace,
                }
            }
        }
    }

    /// The destination already holds the same content as the original.
    fn identical(&self, src: &Path, dest: &Path, e: &Entry) -> bool {
        if e.kind == EntryKind::Link {
            return std::fs::read_link(dest).ok() == std::fs::read_link(src).ok();
        }
        let p = self.p();
        let (Ok(a), Ok(b)) = (
            std::fs::symlink_metadata(src),
            std::fs::symlink_metadata(dest),
        ) else {
            return false;
        };
        if a.len() != b.len() || !b.is_file() {
            return false;
        }
        matches!(
            (crate::compare::hash_file(p, src), crate::compare::hash_file(p, dest)),
            (Ok(x), Ok(y)) if x == y
        )
    }

    /// Whether the file at the destination is this job's copy: the same link
    /// target, or the same content hash (the journal's, else the original's).
    fn same_content(&self, req: &CopyRequest, e: &Entry, rec: &FileRec) -> bool {
        if e.kind == EntryKind::Link {
            return std::fs::read_link(&req.dest).ok() == std::fs::read_link(&req.source).ok();
        }
        let p = self.p();
        if rec.hash.is_none() && self.moving() && std::fs::symlink_metadata(&req.source).is_err() {
            // A move that finished just before an interruption the journal
            // didn't catch: the original is already in the Trash, so there
            // is nothing to hash. Only a complete, verified copy ever gets
            // the real name, and this one has the original's planned size
            // and modification time (copied with it): it is ours.
            return std::fs::symlink_metadata(&req.dest).is_ok_and(|m| {
                let i = ident(&m);
                i.size == e.size && i.mtime_ns == e.mtime_ns
            });
        }
        let Ok(got) = crate::compare::hash_file(p, &req.dest) else {
            return false;
        };
        let want = match &rec.hash {
            Some(h) => Some(h.clone()),
            None => crate::compare::hash_file(p, &req.source).ok().map(hex128),
        };
        want == Some(hex128(got))
    }

    fn start_rec(&self, idx: usize, req: &CopyRequest, keep_chunks: bool) {
        let si = std::fs::symlink_metadata(&req.source)
            .ok()
            .map(|m| ident(&m));
        self.job.update(idx, |r| {
            r.st = FileSt::Copying;
            r.src_ident = si.map(|i| (i.size, i.mtime_ns, i.ino));
            r.hash = None;
            if !keep_chunks {
                r.chunks.clear();
            }
        });
    }

    fn done(&self, idx: usize, req: &CopyRequest, report: &FileReport) {
        let rel = self.job.m.files[idx].rel.clone();
        self.job.update(idx, |r| {
            r.st = FileSt::Done;
            r.outcome = Some(report.outcome.as_str().into());
            if !report.hashes.chunks.is_empty() || report.outcome == Outcome::Verified {
                r.hash = Some(report.hashes.file_hex());
                r.chunks = report
                    .hashes
                    .chunks
                    .iter()
                    .map(|h| format!("{h:016x}"))
                    .collect();
            }
            r.reread = report.source_reread;
            r.meta_lost = report.metadata_lost.iter().map(|m| m.describe()).collect();
            r.os_flush_only = report.flush_level == FlushLevel::OsOnly;
            r.chunks_reused = report.chunks_reused;
            r.resumed |= report.chunks_reused > 0;
            if let Some(t) = &report.replaced_trash {
                r.replaced_trash = Some(t.clone());
            }
        });
        self.inner.sink.on_event(Event::FileDone {
            job: self.job.id,
            path: rel,
            report: Box::new(report.clone()),
        });
        self.after_done(idx, req);
    }

    /// After a file is in place: a cross-volume move's original to the Trash.
    fn after_done(&self, idx: usize, req: &CopyRequest) {
        let e = &self.job.m.files[idx];
        let rec = self.job.rec(idx);
        if self.needs_source_trash(e, &rec) {
            self.trash_source(idx, &req.source);
        }
    }

    /// Handles a file's error. `Some` stops the whole run.
    fn error(&self, idx: usize, err: &CopyError) -> Option<Stop> {
        let e = &self.job.m.files[idx];
        if matches!(err, CopyError::Cancelled) {
            self.job.update(idx, |r| {
                r.st = FileSt::Pending;
                r.chunks.clear();
            });
            return Some(Stop::Cancelled);
        }
        if self.dest_gone() {
            // Leave the record and any partial: resume continues from them.
            return Some(Stop::DestGone);
        }
        if let CopyError::Io { source, .. } = err
            && source.raw_os_error() == Some(libc::ENOSPC)
        {
            self.job.update(idx, |r| {
                r.st = FileSt::Pending;
                r.chunks.clear();
            });
            return Some(Stop::DestFull);
        }
        if self.source_gone(e.src_vol) {
            self.job.update(idx, |r| {
                r.st = FileSt::WaitingSource;
                r.chunks.clear();
            });
            return None;
        }
        match err {
            // The name was taken while it was being copied: decide again.
            CopyError::Conflict(_) => {
                self.job.update(idx, |r| {
                    r.st = FileSt::Pending;
                    r.chunks.clear();
                    r.existed = true;
                });
                self.again.store(true, Ordering::Release);
            }
            CopyError::Skipped(r) => self.skip(idx, r),
            other => self.fail(idx, other.to_string()),
        }
        None
    }

    fn failure(&self, idx: usize, f: Failure) -> Option<Stop> {
        self.error(idx, &f.error)
    }

    fn prep(&self, replace: bool) -> Prep<'_> {
        Prep {
            resuming: true,
            dest_fs: Some(&self.job.m.dest.volume.fs),
            replace,
        }
    }

    /// A big file, link or clone on its own (with chunk-level resume).
    fn alone(
        &self,
        idx: usize,
        req: &CopyRequest,
        resume: Option<Box<Resume>>,
        replace: bool,
        _exdev: bool,
    ) -> Option<Stop> {
        let resume = resume.map(|r| *r);
        let keep = resume.as_ref().is_some_and(|r| r.len > 0);
        self.start_rec(idx, req, keep);
        if keep {
            self.job.update(idx, |r| r.resumed = true);
        }
        let sink = FileSink {
            inner: self.inner,
            job: self.job,
            idx,
        };
        let log = Log { job: self.job, idx };
        let w = match FileWork::prepare(
            self.p(),
            req,
            &self.opts,
            self.ctl,
            &sink,
            &log,
            self.prep(replace),
        ) {
            Ok(w) => w,
            Err(e) => return self.error(idx, &e),
        };
        match w.run_alone(resume) {
            Ok(r) => {
                self.done(idx, req, &r);
                None
            }
            Err(f) => self.failure(idx, f),
        }
    }

    /// Big files a few at a time: with two, one is copied while the other is
    /// checked (proposal §14, decision 6h).
    fn run_bigs(&self, bigs: Vec<Big>) -> Option<Stop> {
        if bigs.is_empty() {
            return None;
        }
        if bigs.len() == 1 || self.big_workers == 1 {
            for b in bigs {
                if let Some(s) = self.alone(b.idx, &b.req, b.resume, b.replace, false) {
                    return Some(s);
                }
            }
            return None;
        }
        let stop: Mutex<Option<Stop>> = Mutex::new(None);
        let queue = Mutex::new(bigs.into_iter());
        std::thread::scope(|scope| {
            for _ in 0..self.big_workers {
                scope.spawn(|| {
                    loop {
                        if lock(&stop).is_some() {
                            return;
                        }
                        let Some(b) = lock(&queue).next() else { return };
                        if let Some(s) = self.alone(b.idx, &b.req, b.resume, b.replace, false) {
                            lock(&stop).get_or_insert(s);
                        }
                    }
                });
            }
        });
        stop.into_inner().unwrap_or_else(|e| e.into_inner())
    }

    /// Small files: write and `fsync` each (several at once), one drive
    /// flush for the group, then check and rename each, then one folder
    /// flush (§4.7). Nothing gets its real name before the drive flush that
    /// covers its data.
    fn commit_group(&self, reqs: Vec<(usize, CopyRequest)>) -> Option<Stop> {
        if reqs.is_empty() {
            return None;
        }
        let sinks: Vec<FileSink> = reqs
            .iter()
            .map(|(idx, _)| FileSink {
                inner: self.inner,
                job: self.job,
                idx: *idx,
            })
            .collect();
        let logs: Vec<Log> = reqs
            .iter()
            .map(|(idx, _)| Log {
                job: self.job,
                idx: *idx,
            })
            .collect();
        let mut next = 0;
        while next < reqs.len() {
            // Write a batch (until the group's time is up), in parallel.
            let t0 = Instant::now();
            let cursor = AtomicUsize::new(next);
            let written: Mutex<Vec<(usize, FileWork)>> = Mutex::new(Vec::new());
            let errors: Mutex<Vec<(usize, CopyError, Option<FileWork>)>> = Mutex::new(Vec::new());
            let stop_writing = AtomicBool::new(false);
            std::thread::scope(|scope| {
                for _ in 0..self.small_workers.min(reqs.len() - next) {
                    scope.spawn(|| {
                        loop {
                            if stop_writing.load(Ordering::Acquire)
                                || (t0.elapsed() >= GROUP_TIME && !lock(&written).is_empty())
                            {
                                return;
                            }
                            let k = cursor.fetch_add(1, Ordering::AcqRel);
                            if k >= reqs.len() {
                                return;
                            }
                            let (idx, req) = &reqs[k];
                            self.start_rec(*idx, req, false);
                            let mut w = match FileWork::prepare(
                                self.p(),
                                req,
                                &self.opts,
                                self.ctl,
                                &sinks[k],
                                &logs[k],
                                self.prep(false),
                            ) {
                                Ok(w) => w,
                                Err(e) => {
                                    lock(&errors).push((k, e, None));
                                    continue;
                                }
                            };
                            match w.write(None, false).and_then(|()| w.seal(false)) {
                                Ok(()) => {
                                    let hash = w.hashes().map(|h| h.file_hex());
                                    self.job.update(*idx, |r| {
                                        r.st = FileSt::Written;
                                        r.hash = hash;
                                    });
                                    lock(&written).push((k, w));
                                }
                                Err(e) => {
                                    if matches!(e, CopyError::Cancelled) {
                                        stop_writing.store(true, Ordering::Release);
                                    }
                                    lock(&errors).push((k, e, Some(w)));
                                }
                            }
                        }
                    });
                }
            });
            next = cursor.load(Ordering::Acquire).min(reqs.len());
            let mut works = written.into_inner().unwrap_or_else(|e| e.into_inner());
            works.sort_by_key(|(k, _)| *k);
            let errors = errors.into_inner().unwrap_or_else(|e| e.into_inner());
            let gone = !errors.is_empty() && self.dest_gone();
            let mut stop = None;
            for (k, e, w) in errors {
                if let Some(mut w) = w {
                    if gone {
                        w.keep_partial();
                    } else {
                        w.discard();
                    }
                }
                if let Some(s) = self.error(reqs[k].0, &e) {
                    stop.get_or_insert(s);
                }
            }
            if let Some(s) = stop {
                return Some(self.abandon(works, s));
            }
            if works.is_empty() {
                continue;
            }
            // One drive flush for the whole group.
            let barrier = self.fault(JobPoint::BeforeBarrier).and_then(|()| {
                let last = works
                    .last()
                    .map(|(_, w)| w.partial.clone())
                    .unwrap_or_default();
                let f = File::open(last)?;
                self.p().flush.flush_file(&f)
            });
            let level = match barrier {
                Ok(l) => l,
                Err(e) => return Some(self.barrier_failed(works, &reqs, e)),
            };
            if let Err(e) = self.fault(JobPoint::AfterBarrier) {
                return Some(self.barrier_failed(works, &reqs, e));
            }
            for (k, w) in works.iter_mut() {
                w.flushed(level);
                let chunks: Vec<String> = w
                    .hashes()
                    .map(|h| h.chunks.iter().map(|c| format!("{c:016x}")).collect())
                    .unwrap_or_default();
                self.job.update(reqs[*k].0, |r| {
                    r.st = FileSt::Flushed;
                    r.chunks = chunks;
                });
            }
            // Check and rename, in parallel; the order of renames doesn't matter.
            let placed: Mutex<Vec<(usize, FileWork)>> = Mutex::new(Vec::new());
            let redo: Mutex<Vec<(usize, Result<FileReport, Failure>)>> = Mutex::new(Vec::new());
            let failed: Mutex<Vec<(usize, Failure)>> = Mutex::new(Vec::new());
            let left: Mutex<Vec<(usize, FileWork)>> = Mutex::new(Vec::new());
            let halt = AtomicBool::new(false);
            let jobs = Mutex::new(works.into_iter().enumerate());
            std::thread::scope(|scope| {
                for _ in 0..self.small_workers {
                    scope.spawn(|| {
                        loop {
                            let Some((j, (k, mut w))) = lock(&jobs).next() else {
                                return;
                            };
                            if halt.load(Ordering::Acquire) {
                                lock(&left).push((k, w));
                                continue;
                            }
                            let step = self
                                .fault(JobPoint::Rename(j))
                                .map_err(|e| CopyError::Io {
                                    phase: Phase::Renaming,
                                    message: describe(&e),
                                    source: e,
                                })
                                .and_then(|()| w.verify());
                            match step {
                                Ok(Checked::Good) => match w.place(false) {
                                    Ok(()) => {
                                        self.job.update(reqs[k].0, |r| r.st = FileSt::Placed);
                                        lock(&placed).push((k, w));
                                    }
                                    Err(e) => {
                                        halt.store(self.dest_gone(), Ordering::Release);
                                        let f = self.fail_or_keep(w, e);
                                        lock(&failed).push((k, f));
                                    }
                                },
                                Ok(Checked::Redo) => {
                                    let r = w.redo();
                                    lock(&redo).push((k, r));
                                }
                                Err(e) => {
                                    halt.store(self.dest_gone(), Ordering::Release);
                                    let f = self.fail_or_keep(w, e);
                                    lock(&failed).push((k, f));
                                }
                            }
                        }
                    });
                }
            });
            let mut stop = None;
            for (k, r) in redo.into_inner().unwrap_or_else(|e| e.into_inner()) {
                match r {
                    Ok(rep) => self.done(reqs[k].0, &reqs[k].1, &rep),
                    Err(f) => {
                        if let Some(s) = self.failure(reqs[k].0, f) {
                            stop.get_or_insert(s);
                        }
                    }
                }
            }
            for (k, f) in failed.into_inner().unwrap_or_else(|e| e.into_inner()) {
                if let Some(s) = self.failure(reqs[k].0, f) {
                    stop.get_or_insert(s);
                }
            }
            // Files not reached because the run stopped keep their partials
            // on a pulled disk, and lose them otherwise.
            for (k, mut w) in left.into_inner().unwrap_or_else(|e| e.into_inner()) {
                if stop == Some(Stop::DestGone) || self.dest_gone() {
                    w.keep_partial();
                } else {
                    w.discard();
                    self.job.update(reqs[k].0, |r| {
                        r.st = FileSt::Pending;
                        r.chunks.clear();
                    });
                }
            }
            let placed = placed.into_inner().unwrap_or_else(|e| e.into_inner());
            if let Some(s) = self.folder_barrier(placed, &reqs) {
                return Some(s);
            }
            if let Some(s) = stop {
                return Some(s);
            }
        }
        None
    }

    fn fail_or_keep(&self, mut w: FileWork, e: CopyError) -> Failure {
        if self.dest_gone() {
            w.keep_partial();
            Failure {
                error: e,
                partial_left: None,
            }
        } else {
            w.fail(e)
        }
    }

    /// The renamed files of a group: one folder flush, then their metadata
    /// (and a move's originals to the Trash).
    fn folder_barrier(
        &self,
        placed: Vec<(usize, FileWork)>,
        reqs: &[(usize, CopyRequest)],
    ) -> Option<Stop> {
        if placed.is_empty() {
            return None;
        }
        let dirs: BTreeSet<PathBuf> = placed
            .iter()
            .filter_map(|(k, _)| reqs[*k].1.dest.parent().map(Path::to_path_buf))
            .collect();
        let r = self.fault(JobPoint::BeforeFolderBarrier).and_then(|()| {
            let mut level = FlushLevel::Device;
            let last = dirs.iter().next_back();
            for d in &dirs {
                if Some(d) == last {
                    level = self.p().flush.flush_dir(d)?;
                } else {
                    self.p().flush.os_flush(&File::open(d)?)?;
                }
            }
            Ok(level)
        });
        let level = match r {
            Ok(l) => l,
            Err(_) if self.dest_gone() => return Some(Stop::DestGone),
            // The files are verified and renamed; only the folder flush is in
            // doubt. Say so instead of claiming a full flush.
            Err(_) => FlushLevel::OsOnly,
        };
        for (k, mut w) in placed {
            w.flushed(level);
            let report = w.finish();
            self.done(reqs[k].0, &reqs[k].1, &report);
        }
        None
    }

    /// The group's drive flush failed: nothing of it is renamed.
    fn barrier_failed(
        &self,
        works: Vec<(usize, FileWork)>,
        reqs: &[(usize, CopyRequest)],
        e: io::Error,
    ) -> Stop {
        if self.dest_gone() {
            return self.abandon(works, Stop::DestGone);
        }
        let why = format!("the drive flush failed: {}", describe(&e));
        for (k, mut w) in works {
            w.discard();
            self.fail(reqs[k].0, why.clone());
        }
        if e.raw_os_error() == Some(libc::ENOSPC) {
            Stop::DestFull
        } else {
            Stop::Done
        }
    }

    /// Stops mid-group: partials stay on a pulled disk (resume uses them),
    /// and are removed otherwise.
    fn abandon(&self, works: Vec<(usize, FileWork)>, s: Stop) -> Stop {
        for (_, mut w) in works {
            if s == Stop::DestGone {
                w.keep_partial();
            } else {
                w.discard();
            }
        }
        s
    }
}

enum Ancestry {
    Ready,
    Wait,
    Skip(String),
}

/// Whether two paths are the same filesystem object.
fn same_object(a: &Path, b: &Path) -> bool {
    match (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) {
        (Ok(x), Ok(y)) => {
            let (x, y) = (ident(&x), ident(&y));
            x.dev == y.dev && x.ino == y.ino
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{Dest, JobOptions, VolRef};

    fn manifest(rels: &[(&str, bool)]) -> Manifest {
        let v = VolRef {
            uuid: None,
            fs: "apfs".into(),
            name: "x".into(),
            capacity: 1,
            mount: "/".into(),
        };
        Manifest {
            format: "safe-transfer/manifest".into(),
            version: 1,
            job: "k3f9q2xm".into(),
            created: 0,
            label: String::new(),
            host_ref: None,
            mode: "copy".into(),
            options: JobOptions {
                chunk_size: 1 << 20,
                flush_each_chunk: true,
                reread_source: false,
                follow_symlinks: false,
                allow_dataless: false,
                clone_same_volume: false,
                on_conflict: Default::default(),
                disposal: Default::default(),
                workers: None,
            },
            dest: Dest {
                path: "/d".into(),
                volume: v.clone(),
            },
            sources: vec![v],
            files: rels
                .iter()
                .enumerate()
                .map(|(i, (r, dir))| Entry {
                    n: i as u32,
                    kind: if *dir {
                        EntryKind::Dir
                    } else {
                        EntryKind::File
                    },
                    src: Some(PathBuf::from("/s").join(r)),
                    src_vol: 0,
                    rel: r.into(),
                    size: 1,
                    mtime_ns: 0,
                    ino: 0,
                    dataless: false,
                    after: None,
                })
                .collect(),
        }
    }

    #[test]
    fn layout_knows_parents_and_contents() {
        let m = manifest(&[
            ("a", true),
            ("a/b", true),
            ("a/b/c.txt", false),
            ("a/d.txt", false),
            ("e.txt", false),
        ]);
        let l = Layout::new(&m);
        assert_eq!(l.parent(2), Some(1));
        assert_eq!(l.parent(3), Some(0));
        assert_eq!(l.parent(4), None);
        assert_eq!(l.inside(0), 1..4);
        assert_eq!(l.inside(1), 2..3);
        assert!(l.inside(4).is_empty());
    }

    #[test]
    fn destinations_follow_new_names_and_replacing_folders() {
        let m = manifest(&[("a", true), ("a/b", true), ("a/b/c.txt", false)]);
        let l = Layout::new(&m);
        let job = JobId::parse("k3f9q2xm").unwrap();
        let mut recs = vec![FileRec::default(); 3];
        let root = Path::new("/d");
        assert_eq!(
            l.dest_of(&m, &recs, root, job, 2),
            Path::new("/d/a/b/c.txt")
        );
        recs[0].dest_name = Some("a 2".into());
        assert_eq!(
            l.dest_of(&m, &recs, root, job, 2),
            Path::new("/d/a 2/b/c.txt")
        );
        recs[1].replacing = true;
        assert_eq!(
            l.dest_of(&m, &recs, root, job, 2),
            Path::new("/d/a 2/.b.k3f9q2xm-1.st-partial/c.txt")
        );
        recs[1].finished = true;
        assert_eq!(
            l.dest_of(&m, &recs, root, job, 2),
            Path::new("/d/a 2/b/c.txt")
        );
        recs[2].dest_name = Some("c 2.txt".into());
        assert_eq!(
            l.dest_of(&m, &recs, root, job, 2),
            Path::new("/d/a 2/b/c 2.txt")
        );
    }
}

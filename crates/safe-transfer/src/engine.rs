//! The engine: multi-file jobs on their own threads, journaled on the Mac,
//! resumable after a crash, a pulled card or a cancel (proposal §2, §3, §6),
//! with small files committed in groups behind one drive flush (§4.7).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::job::{
    self, ConflictInfo, EngineError, EntryReport, Event, JobInfo, JobPlan, JobReport, JobSpec,
    MissingSource, Note, Resumability, SharedSink, io_ctx,
};
use crate::journal::{
    Dest, Entry, EntryKind, FileRec, FileSt, Found, Head, JobFiles, JobOptions, JobState, Manifest,
    Rules, SEGMENT_FILES, Segment, VERSION, VolRef, now_secs, segment_name,
};
use crate::names::JobId;
use crate::options::{Control, Decision, Mode, Symlinks};
use crate::platform::{Platform, VolumeInfo};
use crate::remove::{self, PartialOwner};

/// Files smaller than this are committed in groups (proposal §4.7).
pub const SMALL_FILE: u64 = 8 << 20;
/// Most files in one group.
pub const GROUP_FILES: usize = 256;
/// Most bytes in one group.
pub const GROUP_BYTES: u64 = 64 << 20;
/// Longest a group keeps writing before its drive flush.
pub const GROUP_TIME: Duration = Duration::from_secs(1);
/// Default for [`EngineConfig::journal_interval`].
pub const JOURNAL_INTERVAL: Duration = Duration::from_millis(500);

/// How an engine is set up.
#[derive(Clone)]
pub struct EngineConfig {
    /// Where job journals live (the host's App Support folder). Each job
    /// gets `<journal_dir>/jobs/<id>/`.
    pub journal_dir: PathBuf,
    /// Resume a job by itself when its disks are back. Default off.
    pub auto_resume: bool,
    /// Finished jobs' journals are kept this many days. Default 30.
    pub keep_finished_days: u32,
    /// Longest the journal goes without a write while something changed.
    /// Default 0.5 s. A crash repeats at most this much work (the journal is
    /// re-checked against the disk either way). Tests use zero.
    pub journal_interval: Duration,
    /// Test hooks for every job. Only with the `fault-injection` feature.
    #[cfg(feature = "fault-injection")]
    pub faults: Option<Arc<dyn crate::fault::FaultHook>>,
}

impl EngineConfig {
    /// Defaults, with journals in `journal_dir`.
    pub fn new(journal_dir: impl Into<PathBuf>) -> EngineConfig {
        EngineConfig {
            journal_dir: journal_dir.into(),
            auto_resume: false,
            keep_finished_days: 30,
            journal_interval: JOURNAL_INTERVAL,
            #[cfg(feature = "fault-injection")]
            faults: None,
        }
    }
}

/// How to cancel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cancel {
    /// Stop, remove this job's partial files, keep the journal: the job can
    /// be resumed later. Finished files stay; nothing else changes.
    KeepResumable,
    /// The same, and close the job for good (its report is kept).
    Close,
}

/// The copy engine. Cheap to clone; every clone is the same engine.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

pub(crate) struct Inner {
    pub(crate) cfg: EngineConfig,
    pub(crate) p: Platform,
    pub(crate) sink: SharedSink,
    jobs: Mutex<BTreeMap<String, Arc<Job>>>,
    damaged: Mutex<Vec<PathBuf>>,
}

pub(crate) struct Job {
    pub(crate) id: JobId,
    interval: Duration,
    /// Work bytes of the whole job, and its file count (not folders).
    pub(crate) work_total: u64,
    pub(crate) files_total: u32,
    pub(crate) files: JobFiles,
    pub(crate) m: Manifest,
    ctl: Mutex<Arc<Control>>,
    close: AtomicBool,
    pub(crate) st: Mutex<St>,
    cv: Condvar,
    /// Woken when a conflict is answered (or the job is cancelled).
    pub(crate) answers: Condvar,
}

pub(crate) struct St {
    pub(crate) state: JobState,
    generation: u64,
    seg_gen: Vec<u64>,
    pub(crate) recs: Vec<FileRec>,
    pub(crate) dirty: BTreeSet<usize>,
    head_dirty: bool,
    /// "Apply to all" answers.
    pub(crate) rules: Rules,
    last_save: Instant,
    running: bool,
    lock: Option<File>,
    seconds: f64,
    offered: bool,
    pub(crate) work_done: u64,
    /// Files (not folders) that are done, skipped or failed.
    pub(crate) files_done: u32,
    pub(crate) run_started: Option<Instant>,
    pub(crate) run_base: u64,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Work bytes for one entry: written, read back, and re-read in re-read mode.
pub(crate) fn weight(m: &Manifest, e: &Entry) -> u64 {
    if e.kind != EntryKind::File {
        return 0;
    }
    e.size * if m.options.reread_source { 3 } else { 2 }
}

/// A job's totals, counted once (progress events must not walk every file).
struct Totals {
    work_total: u64,
    work_done: u64,
    files_total: u32,
    files_done: u32,
}

impl Totals {
    fn of(m: &Manifest, recs: &[FileRec]) -> Totals {
        let mut t = Totals {
            work_total: 0,
            work_done: 0,
            files_total: 0,
            files_done: 0,
        };
        for (e, r) in m.files.iter().zip(recs) {
            t.work_total += weight(m, e);
            if r.st.is_terminal() {
                t.work_done += weight(m, e);
            }
            if e.kind != EntryKind::Dir {
                t.files_total += 1;
                t.files_done += u32::from(r.st.is_terminal());
            }
        }
        t
    }
}

impl Job {
    pub(crate) fn rec(&self, idx: usize) -> FileRec {
        lock(&self.st).recs[idx].clone()
    }

    /// Changes one file's record and saves the journal when due.
    pub(crate) fn update(&self, idx: usize, f: impl FnOnce(&mut FileRec)) {
        let mut st = lock(&self.st);
        let was = st.recs[idx].st.is_terminal();
        f(&mut st.recs[idx]);
        if !was && st.recs[idx].st.is_terminal() {
            st.work_done += weight(&self.m, &self.m.files[idx]);
            if self.m.files[idx].kind != EntryKind::Dir {
                st.files_done += 1;
            }
        }
        st.dirty.insert(idx / SEGMENT_FILES);
        self.save(&mut st, false);
    }

    /// Writes dirty segments, then the head. Not forced: at most every 0.5 s.
    /// A failing journal write never stops a copy (the journal is advisory:
    /// everything it says is checked again on resume).
    pub(crate) fn save(&self, st: &mut St, force: bool) {
        if !force && st.last_save.elapsed() < self.interval {
            return;
        }
        let job = self.id.to_string();
        let mut out: Vec<(String, Vec<u8>)> = Vec::new();
        for seg in std::mem::take(&mut st.dirty) {
            let lo = seg * SEGMENT_FILES;
            let hi = (lo + SEGMENT_FILES).min(st.recs.len());
            st.seg_gen[seg] += 1;
            let s = Segment {
                format: "safe-transfer/segment".into(),
                version: VERSION,
                job: job.clone(),
                seg: seg as u32,
                generation: st.seg_gen[seg],
                files: (lo..hi)
                    .filter(|i| st.recs[*i] != FileRec::default())
                    .map(|i| (i as u32, st.recs[i].clone()))
                    .collect(),
            };
            out.push((
                JobFiles::slot_name(&segment_name(seg), &s),
                crate::journal::encode(&s),
            ));
        }
        st.generation += 1;
        let h = Head {
            format: "safe-transfer/head".into(),
            version: VERSION,
            job,
            generation: st.generation,
            state: st.state,
            updated: now_secs(),
            rules: st.rules,
        };
        out.push((JobFiles::slot_name("head", &h), crate::journal::encode(&h)));
        // One drive flush for all of them; a failure never stops a copy.
        let _ = self.files.write_files(&out);
        st.head_dirty = false;
        st.last_save = Instant::now();
    }

    /// Whether it can be resumed by hand now, given the mounted volumes.
    fn resumability(&self, st: &St, vols: &[VolumeInfo]) -> Resumability {
        if st.state == JobState::RunningElsewhere {
            return Resumability::InUseElsewhere;
        }
        if st.running || !st.state.needs_resume() {
            return Resumability::NotNeeded;
        }
        let dest = &self.m.dest.volume;
        if find(vols, dest).is_none() {
            return Resumability::NeedsVolumes(vec![dest.name.clone()]);
        }
        let mut have = false;
        let mut missing: BTreeSet<u32> = BTreeSet::new();
        for (e, r) in self.m.files.iter().zip(&st.recs) {
            if r.st.is_terminal() || e.kind == EntryKind::Dir || e.src.is_none() {
                continue;
            }
            if find(vols, &self.m.sources[e.src_vol as usize]).is_some() {
                have = true;
            } else {
                missing.insert(e.src_vol);
            }
        }
        let names: Vec<String> = missing
            .iter()
            .map(|i| self.m.sources[*i as usize].name.clone())
            .collect();
        if have || names.is_empty() {
            Resumability::Ready {
                still_missing: names,
            }
        } else {
            Resumability::NeedsVolumes(names)
        }
    }

    fn info(&self) -> JobInfo {
        self.info_with(&[], false)
    }

    fn info_with(&self, vols: &[VolumeInfo], with_volumes: bool) -> JobInfo {
        let st = lock(&self.st);
        let resume = if with_volumes {
            self.resumability(&st, vols)
        } else {
            Resumability::NotNeeded
        };
        let mut info = JobInfo {
            id: self.id,
            label: self.m.label.clone(),
            state: st.state,
            dest: self.m.dest.path.clone(),
            dest_volume: self.m.dest.volume.name.clone(),
            files_total: 0,
            files_done: 0,
            files_skipped: 0,
            files_failed: 0,
            bytes_total: 0,
            bytes_done: 0,
            missing: missing(&self.m, &st.recs),
            dest_uuid: self.m.dest.volume.uuid.clone(),
            resume,
            conflicts: st.recs.iter().filter(|r| r.st == FileSt::Conflict).count() as u32,
        };
        for (e, r) in self.m.files.iter().zip(&st.recs) {
            if e.kind == EntryKind::Dir {
                continue;
            }
            info.files_total += 1;
            info.bytes_total += e.size;
            match r.st {
                FileSt::Done => {
                    info.files_done += 1;
                    info.bytes_done += e.size;
                }
                FileSt::Skipped => info.files_skipped += 1,
                FileSt::Failed => info.files_failed += 1,
                _ => {}
            }
        }
        info
    }

    fn report(&self) -> JobReport {
        let st = lock(&self.st);
        let mut r = JobReport {
            job: self.id.to_string(),
            label: self.m.label.clone(),
            seconds: st.seconds,
            ..Default::default()
        };
        for (e, rec) in self.m.files.iter().zip(&st.recs) {
            let note = |reason: &str| Note {
                path: e.rel.clone(),
                reason: reason.to_string(),
            };
            if let Some(why) = &rec.restarted {
                r.restarted.push(note(why));
            }
            match rec.st {
                FileSt::Done => {
                    match rec.outcome.as_deref() {
                        Some("verified") => r.copied_verified += 1,
                        Some("verified_from_memory") => r.verified_from_memory += 1,
                        Some("cloned") => r.cloned += 1,
                        Some("link") => r.links += 1,
                        Some("folder") | Some("merged") => r.folders += 1,
                        Some("moved") => {}
                        _ => r.copied_verified += 1,
                    }
                    // Items inside a folder moved by one rename count with it.
                    if rec.moved && rec.outcome.as_deref() == Some("moved") {
                        r.moved += 1;
                    }
                    if let Some(t) = &rec.replaced_trash {
                        r.replaced.push(note(&format!(
                            "the old one is in the Trash: {}",
                            t.display()
                        )));
                    }
                    if let Some(n) = &rec.dest_name {
                        r.kept_both.push(note(&format!("copied as “{n}”")));
                    }
                    if let Some(t) = &rec.trashed_to {
                        r.sources_trashed.push(note(&t.display().to_string()));
                    }
                    if let Some(why) = &rec.source_kept {
                        r.sources_kept.push(note(why));
                    }
                    if e.kind != EntryKind::Dir {
                        r.bytes += e.size;
                    }
                    r.resumed += u32::from(rec.resumed);
                    r.chunks_reused += rec.chunks_reused;
                    r.source_reread += u32::from(rec.reread);
                    r.os_flush_only += u32::from(rec.os_flush_only);
                    if !rec.meta_lost.is_empty() {
                        r.metadata_lost.push(note(&rec.meta_lost.join("; ")));
                    }
                }
                FileSt::Skipped => r
                    .skipped
                    .push(note(rec.reason.as_deref().unwrap_or("skipped"))),
                FileSt::Failed => r
                    .failed
                    .push(note(rec.reason.as_deref().unwrap_or("failed"))),
                FileSt::Conflict => r.waiting_for_answer.push(note("the name is taken")),
                _ => {}
            }
        }
        r
    }

    /// The questions waiting for an answer.
    fn conflicts(&self) -> Vec<ConflictInfo> {
        let st = lock(&self.st);
        self.m
            .files
            .iter()
            .zip(&st.recs)
            .enumerate()
            .filter(|(_, (_, r))| r.st == FileSt::Conflict)
            .map(|(i, (e, r))| {
                let f = r.found.unwrap_or(Found {
                    dir: false,
                    size: 0,
                    mtime_ns: 0,
                });
                ConflictInfo {
                    id: i as u32,
                    path: e.rel.clone(),
                    source: e.src.clone(),
                    dest: self.m.dest.path.join(&e.rel),
                    folder: e.kind == EntryKind::Dir,
                    source_size: e.size,
                    source_mtime_ns: e.mtime_ns,
                    existing_folder: f.dir,
                    existing_size: f.size,
                    existing_mtime_ns: f.mtime_ns,
                }
            })
            .collect()
    }

    /// Changes one record and saves the journal at once (before an action
    /// that can't be taken back, such as sending something to the Trash).
    pub(crate) fn update_now(&self, idx: usize, f: impl FnOnce(&mut FileRec)) {
        self.update(idx, f);
        let mut st = lock(&self.st);
        self.save(&mut st, true);
    }
}

pub(crate) fn missing(m: &Manifest, recs: &[FileRec]) -> Vec<MissingSource> {
    let mut by: BTreeMap<u32, MissingSource> = BTreeMap::new();
    for (e, r) in m.files.iter().zip(recs) {
        if r.st == FileSt::WaitingSource {
            let v = &m.sources[e.src_vol as usize];
            by.entry(e.src_vol)
                .or_insert_with(|| MissingSource {
                    volume: v.name.clone(),
                    uuid: v.uuid.clone(),
                    files: Vec::new(),
                })
                .files
                .push(e.rel.clone());
        }
    }
    by.into_values().collect()
}

/// The mounted volume the journal means, by UUID (cross-checked with the
/// filesystem type and size), never by name or mount path.
pub(crate) fn find<'v>(vols: &'v [VolumeInfo], r: &VolRef) -> Option<&'v VolumeInfo> {
    vols.iter().find(|v| match &r.uuid {
        Some(u) => v.uuid.as_ref() == Some(u) && v.fs_type == r.fs && v.capacity == r.capacity,
        None => v.mount == r.mount && v.fs_type == r.fs && v.capacity == r.capacity,
    })
}

/// The mounted volumes, read again a few times if the list can't be read.
/// `None`: still unreadable. Unknown is never "not connected": callers that
/// would conclude a disk is gone ask its path instead.
pub(crate) fn list_volumes(p: &Platform) -> Option<Vec<VolumeInfo>> {
    for i in 0..3u64 {
        if let Ok(v) = p.volumes.mounted() {
            return Some(v);
        }
        std::thread::sleep(Duration::from_millis(20 * (i + 1)));
    }
    None
}

/// `path` under `old` moved to under `new` (a card remounted as "Untitled 1").
pub(crate) fn remap(path: &Path, old: &Path, new: &Path) -> PathBuf {
    match path.strip_prefix(old) {
        Ok(rest) => new.join(rest),
        Err(_) => path.to_path_buf(),
    }
}

pub(crate) fn hex128(h: u128) -> String {
    format!("{h:032x}")
}

impl Engine {
    /// Opens the engine on `cfg.journal_dir` and loads every unfinished job.
    /// Nothing is started by itself; jobs a crash interrupted are
    /// [`JobState::Interrupted`]. Call [`Engine::volumes_changed`] to have
    /// [`Event::ResumeAvailable`] sent for jobs whose disks are connected.
    pub fn open(
        cfg: EngineConfig,
        p: Platform,
        sink: Arc<dyn job::EventSink>,
    ) -> Result<Engine, EngineError> {
        let root = cfg.journal_dir.join("jobs");
        std::fs::create_dir_all(&root).map_err(io_ctx("creating the journal folder"))?;
        let e = Engine {
            inner: Arc::new(Inner {
                cfg,
                p,
                sink,
                jobs: Mutex::new(BTreeMap::new()),
                damaged: Mutex::new(Vec::new()),
            }),
        };
        let keep = u64::from(e.inner.cfg.keep_finished_days) * 86_400;
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
            .map_err(io_ctx("reading the journal folder"))?
            .filter_map(|d| d.ok().map(|d| d.path()))
            .collect();
        dirs.sort();
        for dir in dirs {
            let Some(id) = dir
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(JobId::parse)
            else {
                continue;
            };
            let files = e.job_files(dir.clone());
            let Some(m) = files.read_manifest().filter(|m| m.job == id.as_str()) else {
                lock(&e.inner.damaged).push(dir);
                continue;
            };
            let head: Option<Head> = files.read_slotted("head", id.as_str());
            let mut state = head.as_ref().map_or(JobState::Planned, |h| h.state);
            if state.is_final()
                && head
                    .as_ref()
                    .is_some_and(|h| now_secs().saturating_sub(h.updated) > keep)
            {
                let _ = remove::remove_job_dir(&root, &dir);
                continue;
            }
            if matches!(
                state,
                JobState::Running | JobState::Paused | JobState::RunningElsewhere
            ) {
                state = match files.lock() {
                    Ok(Some(_released_on_drop)) => JobState::Interrupted,
                    _ => JobState::RunningElsewhere,
                };
            }
            let n = m.files.len();
            let nseg = n.div_ceil(SEGMENT_FILES).max(1);
            let mut recs = vec![FileRec::default(); n];
            let mut seg_gen = vec![0; nseg];
            for (seg, sg) in seg_gen.iter_mut().enumerate() {
                // Unreadable: its files stay pending and are rebuilt from the
                // disk (a file already there counts only if identical).
                if let Some(s) = files.read_slotted::<Segment>(&segment_name(seg), id.as_str()) {
                    *sg = s.generation;
                    for (i, r) in s.files {
                        if let Some(slot) = recs.get_mut(i as usize) {
                            *slot = r;
                        }
                    }
                }
            }
            let t = Totals::of(&m, &recs);
            let job = Job {
                id,
                interval: e.inner.cfg.journal_interval,
                work_total: t.work_total,
                files_total: t.files_total,
                files,
                m,
                ctl: Mutex::new(Arc::new(Control::new())),
                close: AtomicBool::new(false),
                st: Mutex::new(St {
                    state,
                    generation: head.as_ref().map_or(0, |h| h.generation),
                    rules: head.as_ref().map(|h| h.rules).unwrap_or_default(),
                    seg_gen,
                    recs,
                    dirty: BTreeSet::new(),
                    head_dirty: false,
                    last_save: Instant::now(),
                    running: false,
                    lock: None,
                    seconds: 0.0,
                    offered: false,
                    work_done: t.work_done,
                    files_done: t.files_done,
                    run_started: None,
                    run_base: 0,
                }),
                cv: Condvar::new(),
                answers: Condvar::new(),
            };
            lock(&e.inner.jobs).insert(id.to_string(), Arc::new(job));
        }
        Ok(e)
    }

    fn job_files(&self, dir: PathBuf) -> JobFiles {
        #[allow(unused_mut)]
        let mut f = JobFiles::new(dir, self.inner.p.flush.clone());
        #[cfg(feature = "fault-injection")]
        {
            f.faults = self.inner.cfg.faults.clone();
        }
        f
    }

    fn get(&self, id: &JobId) -> Result<Arc<Job>, EngineError> {
        lock(&self.inner.jobs)
            .get(id.as_str())
            .cloned()
            .ok_or_else(|| EngineError::UnknownJob(id.to_string()))
    }

    /// Journal folders that couldn't be read (both manifests damaged). Their
    /// partial files are left alone: nothing can be matched to them safely.
    pub fn damaged(&self) -> Vec<PathBuf> {
        lock(&self.inner.damaged).clone()
    }

    /// What `spec` would do. Stat only; writes nothing.
    pub fn plan(&self, spec: &JobSpec) -> Result<JobPlan, EngineError> {
        job::plan(&self.inner.p, spec)
    }

    /// Plans `spec`, refuses it up front if it won't fit, and writes its
    /// journal. Nothing is written at the destination yet.
    pub fn create(&self, spec: JobSpec) -> Result<JobId, EngineError> {
        let plan = self.plan(&spec)?;
        if !plan.fits {
            return Err(EngineError::NoSpace {
                need: plan.need,
                margin: plan.margin,
                free: plan.free,
            });
        }
        let id = JobId::new().map_err(io_ctx("making a job id"))?;
        let o = &spec.options;
        let m = Manifest {
            format: "safe-transfer/manifest".into(),
            version: VERSION,
            job: id.to_string(),
            created: now_secs(),
            label: spec.label.clone(),
            host_ref: spec.host_ref.clone(),
            mode: match o.mode {
                Mode::Copy => "copy".into(),
                Mode::Move => "move".into(),
            },
            options: JobOptions {
                chunk_size: o.chunk_size,
                flush_each_chunk: o.flush_each_chunk,
                reread_source: o.reread_source,
                follow_symlinks: o.symlinks == Symlinks::Follow,
                allow_dataless: o.allow_dataless,
                clone_same_volume: o.clone_same_volume,
                on_conflict: o.on_conflict,
                disposal: o.disposal,
                workers: o.workers,
            },
            dest: Dest {
                path: plan.dest.clone(),
                volume: job::volref(&plan.dest_volume),
            },
            sources: plan.sources,
            files: plan.entries,
        };
        let dir = self.inner.cfg.journal_dir.join("jobs").join(id.as_str());
        let files = self.job_files(dir);
        files
            .write_manifest(&m)
            .map_err(io_ctx("writing the job's journal"))?;
        let nseg = m.files.len().div_ceil(SEGMENT_FILES).max(1);
        let t = Totals::of(&m, &plan.recs);
        let job = Job {
            id,
            interval: self.inner.cfg.journal_interval,
            work_total: t.work_total,
            files_total: t.files_total,
            files,
            ctl: Mutex::new(Arc::new(Control::new())),
            close: AtomicBool::new(false),
            st: Mutex::new(St {
                state: JobState::Planned,
                generation: 0,
                rules: Rules::default(),
                seg_gen: vec![0; nseg],
                work_done: t.work_done,
                files_done: t.files_done,
                recs: plan.recs,
                dirty: (0..nseg).collect(),
                head_dirty: true,
                last_save: Instant::now(),
                running: false,
                lock: None,
                seconds: 0.0,
                offered: false,
                run_started: None,
                run_base: 0,
            }),
            cv: Condvar::new(),
            answers: Condvar::new(),
            m,
        };
        {
            let mut st = lock(&job.st);
            job.save(&mut st, true);
        }
        lock(&self.inner.jobs).insert(id.to_string(), Arc::new(job));
        Ok(id)
    }

    /// Starts (or continues) a job on its own thread. Returns at once.
    pub fn start(&self, id: JobId) -> Result<(), EngineError> {
        let job = self.get(&id)?;
        {
            let mut st = lock(&job.st);
            if st.state.is_final() {
                return Err(EngineError::Final(st.state.as_str()));
            }
            if st.running {
                return Ok(());
            }
            let l = job
                .files
                .lock()
                .map_err(io_ctx("locking the job's journal"))?
                .ok_or(EngineError::InUse)?;
            st.lock = Some(l);
            st.running = true;
            st.offered = false;
        }
        *lock(&job.ctl) = Arc::new(Control::new());
        job.close.store(false, Ordering::Release);
        // Running before the thread exists, so a pause() right after start()
        // can't be overwritten by the thread saying "running".
        set_state(&self.inner, &job, JobState::Running);
        let inner = self.inner.clone();
        std::thread::Builder::new()
            .name(format!("safe-transfer {id}"))
            .spawn(move || run(inner, job))
            .map_err(io_ctx("starting the job's thread"))?;
        Ok(())
    }

    /// Pauses a running job at its next 4 MiB step.
    pub fn pause(&self, id: JobId) -> Result<(), EngineError> {
        let job = self.get(&id)?;
        let running = lock(&job.st).running;
        if running {
            lock(&job.ctl).pause();
            set_state(&self.inner, &job, JobState::Paused);
        }
        Ok(())
    }

    /// Continues a paused job, or restarts an interrupted, waiting or
    /// cancelled one (its finished files and flushed chunks are kept).
    pub fn resume(&self, id: JobId) -> Result<(), EngineError> {
        let job = self.get(&id)?;
        let running = lock(&job.st).running;
        let ctl = lock(&job.ctl).clone();
        if running && ctl.is_paused() {
            ctl.resume();
            set_state(&self.inner, &job, JobState::Running);
            return Ok(());
        }
        self.start(id)
    }

    /// Cancels: always safe. Removes this job's partial files and leaves
    /// every finished file and every original as they are.
    pub fn cancel(&self, id: JobId, how: Cancel) -> Result<(), EngineError> {
        let job = self.get(&id)?;
        job.close.store(how == Cancel::Close, Ordering::Release);
        let running = lock(&job.st).running;
        if running {
            lock(&job.ctl).cancel();
            job.answers.notify_all();
            self.wait(id, Duration::from_secs(3600));
            return Ok(());
        }
        let state = lock(&job.st).state;
        if state.is_final() {
            return Err(EngineError::Final(state.as_str()));
        }
        if how == Cancel::Close {
            // Discarding removes this job's partials on its destination; with
            // the disk away they can't be removed, so ask for it first.
            let vols = list_volumes(&self.inner.p).unwrap_or_default();
            if find(&vols, &job.m.dest.volume).is_none() {
                return Err(EngineError::NeedsVolume(job.m.dest.volume.name.clone()));
            }
        }
        let l = job
            .files
            .lock()
            .map_err(io_ctx("locking the job's journal"))?
            .ok_or(EngineError::InUse)?;
        remove_partials(&self.inner, &job);
        finish_cancel(&self.inner, &job, how == Cancel::Close);
        drop(l);
        Ok(())
    }

    /// The job, at a glance, including whether it can be resumed now.
    pub fn job(&self, id: JobId) -> Option<JobInfo> {
        let vols = list_volumes(&self.inner.p).unwrap_or_default();
        self.get(&id).ok().map(|j| j.info_with(&vols, true))
    }

    /// Every job the engine knows (unfinished ones, and finished ones kept
    /// for `keep_finished_days`), each with whether it can be resumed now.
    pub fn jobs(&self) -> Vec<JobInfo> {
        let vols = list_volumes(&self.inner.p).unwrap_or_default();
        let jobs: Vec<Arc<Job>> = lock(&self.inner.jobs).values().cloned().collect();
        jobs.iter().map(|j| j.info_with(&vols, true)).collect()
    }

    /// Jobs waiting for a resume (§6.1): the operations list's "Interrupted"
    /// rows and the toolbar badge's count.
    pub fn resumable(&self) -> Vec<JobInfo> {
        self.jobs()
            .into_iter()
            .filter(|j| j.resume != Resumability::NotNeeded)
            .collect()
    }

    /// Unfinished jobs that write to, or read from, the volume `uuid` (the
    /// sidebar's "Resume Transfer" for that card or drive).
    pub fn jobs_on_volume(&self, uuid: &str) -> Vec<JobInfo> {
        let ids: Vec<JobId> = lock(&self.inner.jobs)
            .values()
            .filter(|j| {
                j.m.dest.volume.uuid.as_deref() == Some(uuid)
                    || j.m.sources.iter().any(|s| s.uuid.as_deref() == Some(uuid))
            })
            .map(|j| j.id)
            .collect();
        self.resumable()
            .into_iter()
            .filter(|j| ids.contains(&j.id))
            .collect()
    }

    /// Resumes every job whose disks are connected (`transfer.resume_all`,
    /// `safe-transfer resume --all`). Returns the jobs started.
    pub fn resume_all(&self) -> Vec<JobId> {
        self.resumable()
            .into_iter()
            .filter(|j| matches!(j.resume, Resumability::Ready { .. }))
            .filter(|j| self.resume(j.id).is_ok())
            .map(|j| j.id)
            .collect()
    }

    /// The questions a job is waiting for ("“DSC01234.ARW” already exists").
    pub fn conflicts(&self, id: JobId) -> Vec<ConflictInfo> {
        self.get(&id).map(|j| j.conflicts()).unwrap_or_default()
    }

    /// Answers a question: `decision` for item `conflict` (an id from
    /// [`Event::Conflict`] or [`Engine::conflicts`]). With `apply_to_all`,
    /// the same answer goes to every waiting item of the same kind (files,
    /// or folders) and to the ones still to come in this job. A job that
    /// isn't running keeps the answer for when it resumes.
    pub fn resolve_conflict(
        &self,
        id: JobId,
        conflict: u32,
        decision: Decision,
        apply_to_all: bool,
    ) -> Result<(), EngineError> {
        let job = self.get(&id)?;
        let idx = conflict as usize;
        let mut st = lock(&job.st);
        if st.recs.get(idx).map(|r| r.st) != Some(FileSt::Conflict) {
            return Err(EngineError::NoSuchConflict);
        }
        let folder = job.m.files[idx].kind == EntryKind::Dir;
        match decision {
            Decision::Ask => return Err(EngineError::BadAnswer("ask")),
            Decision::Merge if !folder => return Err(EngineError::BadAnswer("merge")),
            _ => {}
        }
        let class = |i: usize| (job.m.files[i].kind == EntryKind::Dir) == folder;
        for i in 0..st.recs.len() {
            let pick = i == idx || (apply_to_all && st.recs[i].st == FileSt::Conflict && class(i));
            if pick {
                st.recs[i].decision = Some(decision);
                st.recs[i].st = FileSt::Pending;
                st.dirty.insert(i / SEGMENT_FILES);
            }
        }
        if apply_to_all {
            if folder {
                st.rules.folders = Some(decision);
            } else {
                st.rules.files = Some(decision);
            }
        }
        job.save(&mut st, true);
        drop(st);
        job.answers.notify_all();
        Ok(())
    }

    /// The job's report as it stands (final once the job is finished).
    pub fn report(&self, id: JobId) -> Option<JobReport> {
        self.get(&id).ok().map(|j| j.report())
    }

    /// Every entry of the job and what happened to it, in job order
    /// (folders before what is inside them). Paths are at the destination's
    /// current mount point when it is connected, else where it was last seen.
    pub fn entries(&self, id: JobId) -> Option<Vec<EntryReport>> {
        let job = self.get(&id).ok()?;
        let vols = list_volumes(&self.inner.p).unwrap_or_default();
        let root = match find(&vols, &job.m.dest.volume) {
            Some(v) => remap(&job.m.dest.path, &job.m.dest.volume.mount, &v.mount),
            None => job.m.dest.path.clone(),
        };
        let layout = crate::runner::Layout::new(&job.m);
        let recs = lock(&job.st).recs.clone();
        Some(
            job.m
                .files
                .iter()
                .zip(&recs)
                .enumerate()
                .map(|(i, (e, r))| EntryReport {
                    index: e.n,
                    kind: e.kind,
                    rel: e.rel.clone(),
                    source: e.src.clone(),
                    dest: layout.dest_of(&job.m, &recs, &root, job.id, i),
                    state: r.st,
                    merged: r.outcome.as_deref() == Some("merged"),
                    moved: r.moved,
                    decision: r.decision,
                    replaced_trash: r.replaced_trash.clone(),
                    trashed_to: r.trashed_to.clone(),
                    reason: r.reason.clone().or_else(|| r.source_kept.clone()),
                    size: e.size,
                    hash: r.hash.clone(),
                })
                .collect(),
        )
    }

    /// Waits until the job's thread stops (finished, paused-and-stopped,
    /// waiting, cancelled) or `timeout` passes, and returns its info.
    pub fn wait(&self, id: JobId, timeout: Duration) -> Option<JobInfo> {
        let job = self.get(&id).ok()?;
        let deadline = Instant::now() + timeout;
        let mut st = lock(&job.st);
        while st.running {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            st = job
                .cv
                .wait_timeout(st, left)
                .map(|(g, _)| g)
                .unwrap_or_else(|e| e.into_inner().0);
        }
        drop(st);
        Some(job.info())
    }

    /// The host calls this when volumes mount or unmount. Unfinished jobs
    /// whose destination (and, for waiting files, source) is connected are
    /// offered with [`Event::ResumeAvailable`], or resumed when
    /// `auto_resume` is on.
    pub fn volumes_changed(&self) {
        let vols = list_volumes(&self.inner.p).unwrap_or_default();
        let jobs: Vec<Arc<Job>> = lock(&self.inner.jobs).values().cloned().collect();
        for job in jobs {
            let (state, running, waiting_srcs) = {
                let st = lock(&job.st);
                let srcs: BTreeSet<u32> = job
                    .m
                    .files
                    .iter()
                    .zip(&st.recs)
                    .filter(|(_, r)| r.st == FileSt::WaitingSource)
                    .map(|(e, _)| e.src_vol)
                    .collect();
                (st.state, st.running, srcs)
            };
            if running
                || !matches!(
                    state,
                    JobState::Interrupted
                        | JobState::WaitingForDestination
                        | JobState::WaitingForSource
                        | JobState::RunningElsewhere
                )
            {
                continue;
            }
            let dest_ok = find(&vols, &job.m.dest.volume).is_some();
            let srcs_ok = state != JobState::WaitingForSource
                || waiting_srcs
                    .iter()
                    .any(|i| find(&vols, &job.m.sources[*i as usize]).is_some());
            if !(dest_ok && srcs_ok) {
                lock(&job.st).offered = false;
                continue;
            }
            if self.inner.cfg.auto_resume {
                let _ = self.start(job.id);
                continue;
            }
            let offer = {
                let mut st = lock(&job.st);
                !std::mem::replace(&mut st.offered, true)
            };
            if offer {
                let info = job.info();
                self.inner.sink.on_event(Event::ResumeAvailable {
                    job: job.id,
                    label: job.m.label.clone(),
                    volume: job.m.dest.volume.name.clone(),
                    files_left: info.files_total
                        - info.files_done
                        - info.files_skipped
                        - info.files_failed,
                    bytes_left: info.bytes_total - info.bytes_done,
                });
            }
        }
    }
}

pub(crate) fn set_state(inner: &Inner, job: &Job, state: JobState) {
    {
        let mut st = lock(&job.st);
        if st.state == state {
            return;
        }
        st.state = state;
        st.head_dirty = true;
        job.save(&mut st, true);
    }
    inner.sink.on_event(Event::JobState { job: job.id, state });
}

/// Removes the partial of every unfinished file (journal-matched names only,
/// on the job's own destination volume).
fn remove_partials(inner: &Inner, job: &Job) {
    let vols = list_volumes(&inner.p).unwrap_or_default();
    let Some(v) = find(&vols, &job.m.dest.volume) else {
        return;
    };
    let root = remap(&job.m.dest.path, &job.m.dest.volume.mount, &v.mount);
    let layout = crate::runner::Layout::new(&job.m);
    let recs = lock(&job.st).recs.clone();
    // A replacing folder's hidden partial holds this job's new copies: it
    // goes to the Trash whole (nothing of yours is in it), and its items
    // start again on resume.
    let mut redo: BTreeSet<usize> = BTreeSet::new();
    for (i, (e, r)) in job.m.files.iter().zip(&recs).enumerate() {
        if e.kind == EntryKind::Dir && r.replacing && !r.finished {
            let partial = layout.children_of(&job.m, &recs, &root, job.id, i);
            if partial.exists() {
                let _ = inner.p.trash.trash(&partial);
            }
            redo.insert(i);
            redo.extend(layout.inside(i));
        }
    }
    for (i, (e, r)) in job.m.files.iter().zip(&recs).enumerate() {
        if r.st.is_terminal()
            || r.st == FileSt::Placed
            || e.kind == EntryKind::Dir
            || redo.contains(&i)
        {
            continue;
        }
        let owner = PartialOwner {
            final_path: layout.dest_of(&job.m, &recs, &root, job.id, i),
            job: job.id,
            n: e.n,
        };
        let _ = remove::remove_partial(&inner.p, &owner.path(), &owner);
    }
    let mut st = lock(&job.st);
    for (i, r) in st.recs.iter_mut().enumerate() {
        if matches!(r.st, FileSt::Copying | FileSt::Written | FileSt::Flushed) || redo.contains(&i)
        {
            // Answers and names chosen stay; the copy itself starts again.
            r.st = FileSt::Pending;
            r.chunks.clear();
            r.hash = None;
            r.src_ident = None;
            r.replacing = false;
            r.outcome = None;
        }
    }
    let nseg = st.seg_gen.len();
    st.dirty.extend(0..nseg);
}

fn finish_cancel(inner: &Inner, job: &Job, close: bool) {
    if close {
        let report = job.report();
        let _ = job
            .files
            .write_file("report.json", &crate::journal::encode(&report));
        set_state(inner, job, JobState::Closed);
        inner.sink.on_event(Event::JobFinished {
            job: job.id,
            report: Box::new(report),
        });
    } else {
        set_state(inner, job, JobState::Cancelled);
    }
}

pub(crate) use crate::runner::Stop;

fn run(inner: Arc<Inner>, job: Arc<Job>) {
    let ctl = lock(&job.ctl).clone();
    {
        let mut st = lock(&job.st);
        st.run_started = Some(Instant::now());
        st.run_base = st.work_done;
    }
    let t = Instant::now();
    let stop = crate::runner::Runner::new(&inner, &job, &ctl).go();
    lock(&job.st).seconds += t.elapsed().as_secs_f64();
    match stop {
        Stop::Done => {
            let waiting = missing(&job.m, &lock(&job.st).recs);
            if waiting.is_empty() {
                let report = job.report();
                let _ = job
                    .files
                    .write_file("report.json", &crate::journal::encode(&report));
                set_state(&inner, &job, JobState::Finished);
                inner.sink.on_event(Event::JobFinished {
                    job: job.id,
                    report: Box::new(report),
                });
            } else {
                set_state(&inner, &job, JobState::WaitingForSource);
                inner.sink.on_event(Event::WaitingForSource {
                    job: job.id,
                    missing: waiting,
                });
            }
        }
        Stop::Cancelled => {
            remove_partials(&inner, &job);
            finish_cancel(&inner, &job, job.close.load(Ordering::Acquire));
        }
        Stop::DestGone => {
            set_state(&inner, &job, JobState::WaitingForDestination);
            inner.sink.on_event(Event::WaitingForDestination {
                job: job.id,
                volume: job.m.dest.volume.name.clone(),
            });
        }
        Stop::DestFull => set_state(&inner, &job, JobState::DestinationFull),
    }
    let mut st = lock(&job.st);
    job.save(&mut st, true);
    st.lock = None;
    st.running = false;
    drop(st);
    job.cv.notify_all();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remap_moves_paths_to_the_new_mount() {
        assert_eq!(
            remap(
                Path::new("/Volumes/Untitled/DCIM/a.ARW"),
                Path::new("/Volumes/Untitled"),
                Path::new("/Volumes/Untitled 1")
            ),
            Path::new("/Volumes/Untitled 1/DCIM/a.ARW")
        );
        assert_eq!(
            remap(Path::new("/x/y"), Path::new("/z"), Path::new("/w")),
            Path::new("/x/y")
        );
    }

    #[test]
    fn volumes_are_found_by_uuid_never_by_name() {
        let v = |uuid: &str, name: &str, mount: &str| VolumeInfo {
            uuid: Some(uuid.into()),
            fs_type: "exfat".into(),
            name: name.into(),
            mount: mount.into(),
            device: String::new(),
            capacity: 64,
            free: 1,
            block_size: 1,
            read_only: false,
        };
        let vols = vec![
            v("AAAA", "Untitled", "/Volumes/Untitled"),
            v("BBBB", "Untitled", "/Volumes/Untitled 1"),
        ];
        let r = VolRef {
            uuid: Some("BBBB".into()),
            fs: "exfat".into(),
            name: "Untitled".into(),
            capacity: 64,
            mount: "/Volumes/Untitled".into(),
        };
        assert_eq!(
            find(&vols, &r).unwrap().mount,
            Path::new("/Volumes/Untitled 1")
        );
        let other_size = VolRef {
            capacity: 128,
            ..r.clone()
        };
        assert!(find(&vols, &other_size).is_none());
    }
}

//! The engine: multi-file jobs on their own threads, journaled on the Mac,
//! resumable after a crash, a pulled card or a cancel (proposal §2, §3, §6),
//! with small files committed in groups behind one drive flush (§4.7).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::copy::{
    Checked, ChunkLog, CopyError, CopyRequest, Failure, FileWork, Resume, ident, resume_from,
};
use crate::events::{FileProgress, FileReport, Outcome, Phase, ProgressSink};
use crate::job::{
    self, EngineError, Event, JobInfo, JobPlan, JobProgress, JobReport, JobSpec, MissingSource,
    Note, Resumability, SharedSink, io_ctx,
};
use crate::journal::{
    Dest, Entry, EntryKind, FileRec, FileSt, Head, JobFiles, JobOptions, JobState, Manifest,
    SEGMENT_FILES, Segment, VERSION, VolRef, now_secs, segment_name,
};
use crate::names::JobId;
use crate::options::{Control, Options, Symlinks};
use crate::platform::{FlushLevel, Platform, VolumeInfo};
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

struct Inner {
    cfg: EngineConfig,
    p: Platform,
    sink: SharedSink,
    jobs: Mutex<BTreeMap<String, Arc<Job>>>,
    damaged: Mutex<Vec<PathBuf>>,
}

struct Job {
    id: JobId,
    interval: Duration,
    /// Work bytes of the whole job, and its file count (not folders).
    work_total: u64,
    files_total: u32,
    files: JobFiles,
    m: Manifest,
    ctl: Mutex<Arc<Control>>,
    close: AtomicBool,
    st: Mutex<St>,
    cv: Condvar,
}

struct St {
    state: JobState,
    generation: u64,
    seg_gen: Vec<u64>,
    recs: Vec<FileRec>,
    dirty: BTreeSet<usize>,
    head_dirty: bool,
    last_save: Instant,
    running: bool,
    lock: Option<File>,
    seconds: f64,
    offered: bool,
    work_done: u64,
    /// Files (not folders) that are done, skipped or failed.
    files_done: u32,
    run_started: Option<Instant>,
    run_base: u64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Work bytes for one entry: written, read back, and re-read in re-read mode.
fn weight(m: &Manifest, e: &Entry) -> u64 {
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
    fn rec(&self, idx: usize) -> FileRec {
        lock(&self.st).recs[idx].clone()
    }

    /// Changes one file's record and saves the journal when due.
    fn update(&self, idx: usize, f: impl FnOnce(&mut FileRec)) {
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
    fn save(&self, st: &mut St, force: bool) {
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
                        Some("folder") => r.folders += 1,
                        _ => r.copied_verified += 1,
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
                _ => {}
            }
        }
        r
    }
}

fn missing(m: &Manifest, recs: &[FileRec]) -> Vec<MissingSource> {
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
fn find<'v>(vols: &'v [VolumeInfo], r: &VolRef) -> Option<&'v VolumeInfo> {
    vols.iter().find(|v| match &r.uuid {
        Some(u) => v.uuid.as_ref() == Some(u) && v.fs_type == r.fs && v.capacity == r.capacity,
        None => v.mount == r.mount && v.fs_type == r.fs && v.capacity == r.capacity,
    })
}

/// `path` under `old` moved to under `new` (a card remounted as "Untitled 1").
fn remap(path: &Path, old: &Path, new: &Path) -> PathBuf {
    match path.strip_prefix(old) {
        Ok(rest) => new.join(rest),
        Err(_) => path.to_path_buf(),
    }
}

fn hex128(h: u128) -> String {
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
                    generation: head.map_or(0, |h| h.generation),
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
            mode: "copy".into(),
            options: JobOptions {
                chunk_size: o.chunk_size,
                flush_each_chunk: o.flush_each_chunk,
                reread_source: o.reread_source,
                follow_symlinks: o.symlinks == Symlinks::Follow,
                allow_dataless: o.allow_dataless,
                clone_same_volume: o.clone_same_volume,
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
            let vols = self.inner.p.volumes.mounted().unwrap_or_default();
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
        let vols = self.inner.p.volumes.mounted().unwrap_or_default();
        self.get(&id).ok().map(|j| j.info_with(&vols, true))
    }

    /// Every job the engine knows (unfinished ones, and finished ones kept
    /// for `keep_finished_days`), each with whether it can be resumed now.
    pub fn jobs(&self) -> Vec<JobInfo> {
        let vols = self.inner.p.volumes.mounted().unwrap_or_default();
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

    /// The job's report as it stands (final once the job is finished).
    pub fn report(&self, id: JobId) -> Option<JobReport> {
        self.get(&id).ok().map(|j| j.report())
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
        let vols = self.inner.p.volumes.mounted().unwrap_or_default();
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

fn set_state(inner: &Inner, job: &Job, state: JobState) {
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
    let vols = inner.p.volumes.mounted().unwrap_or_default();
    let Some(v) = find(&vols, &job.m.dest.volume) else {
        return;
    };
    let root = remap(&job.m.dest.path, &job.m.dest.volume.mount, &v.mount);
    let recs = lock(&job.st).recs.clone();
    for (e, r) in job.m.files.iter().zip(&recs) {
        if r.st.is_terminal() || r.st == FileSt::Placed || e.kind == EntryKind::Dir {
            continue;
        }
        let owner = PartialOwner {
            final_path: root.join(&e.rel),
            job: job.id,
            n: e.n,
        };
        let _ = remove::remove_partial(&inner.p, &owner.path(), &owner);
    }
    let mut st = lock(&job.st);
    for (i, r) in st.recs.iter_mut().enumerate() {
        if matches!(r.st, FileSt::Copying | FileSt::Written | FileSt::Flushed) {
            *r = FileRec {
                restarted: r.restarted.take(),
                ..Default::default()
            };
            let _ = i;
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
            report,
        });
    } else {
        set_state(inner, job, JobState::Cancelled);
    }
}

/// Why a run stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    Done,
    Cancelled,
    DestGone,
    DestFull,
}

fn run(inner: Arc<Inner>, job: Arc<Job>) {
    let ctl = lock(&job.ctl).clone();
    {
        let mut st = lock(&job.st);
        st.run_started = Some(Instant::now());
        st.run_base = st.work_done;
    }
    let t = Instant::now();
    let stop = Runner::new(&inner, &job, &ctl).go();
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
                    report,
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

/// Logs a big file's flushed chunks into its journal record.
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
        let (done, total, files_done, files_total, rate) = {
            let st = lock(&self.job.st);
            let done = st.work_done + off;
            let el = st.run_started.map_or(0.0, |t| t.elapsed().as_secs_f64());
            let rate = if el > 0.2 {
                (done.saturating_sub(st.run_base) as f64 / el) as u64
            } else {
                0
            };
            (
                done,
                self.job.work_total,
                st.files_done,
                self.job.files_total,
                rate,
            )
        };
        self.inner.sink.on_event(Event::Progress(JobProgress {
            job: self.job.id,
            phase: fp.phase,
            file: Some(e.rel.clone()),
            done,
            total,
            files_done,
            files_total,
            bytes_per_sec: rate,
            eta_secs: (rate > 0).then(|| total.saturating_sub(done) / rate),
        }));
    }
}

struct Runner<'a> {
    inner: &'a Inner,
    job: &'a Job,
    ctl: &'a Control,
    opts: Options,
    dest_root: PathBuf,
    src_mounts: Vec<Option<PathBuf>>,
}

/// What to do with a file after looking at the disk.
enum Next {
    /// Nothing more (the record is already updated).
    Settled,
    /// Copy it, continuing `Resume` if given.
    Copy(Option<Box<Resume>>),
}

impl<'a> Runner<'a> {
    fn new(inner: &'a Inner, job: &'a Job, ctl: &'a Control) -> Runner<'a> {
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
            #[cfg(feature = "fault-injection")]
            faults: inner.cfg.faults.clone(),
            ..Options::default()
        };
        Runner {
            inner,
            job,
            ctl,
            opts,
            dest_root: PathBuf::new(),
            src_mounts: Vec::new(),
        }
    }

    fn p(&self) -> &Platform {
        &self.inner.p
    }

    #[cfg_attr(not(feature = "fault-injection"), allow(unused_variables))]
    fn fault(&self, point: GroupPoint) -> io::Result<()> {
        #[cfg(feature = "fault-injection")]
        if let Some(h) = &self.opts.faults {
            use crate::fault::FaultPoint as F;
            let p = match point {
                GroupPoint::BeforeBarrier => F::BeforeGroupBarrier,
                GroupPoint::AfterBarrier => F::AfterGroupBarrier,
                GroupPoint::Rename(i) => F::MidGroupRename { index: i as u64 },
                GroupPoint::BeforeFolderBarrier => F::BeforeFolderBarrier,
            };
            h.at(p, &self.dest_root)?;
        }
        Ok(())
    }

    fn go(&mut self) -> Stop {
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
        let mut group: Vec<(usize, CopyRequest)> = Vec::new();
        let mut group_bytes = 0u64;
        for idx in 0..m.files.len() {
            if self.ctl.is_cancelled() {
                return Stop::Cancelled;
            }
            let rec = self.job.rec(idx);
            if rec.st.is_terminal() {
                continue;
            }
            let e = &m.files[idx];
            match e.kind {
                EntryKind::Dir => {
                    if let Some(s) = self.make_dir(idx, e) {
                        return self.stop_with_group(s, &mut group);
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
                    let req = CopyRequest {
                        source: src,
                        dest: self.dest_root.join(&e.rel),
                        job: self.job.id,
                        index: e.n,
                    };
                    let small = e.kind == EntryKind::File && e.size < SMALL_FILE && !self.clones(e);
                    match self.decide(idx, &req, e, small) {
                        Next::Settled => {}
                        Next::Copy(resume) if small && resume.is_none() => {
                            group_bytes += e.size;
                            group.push((idx, req));
                            if group.len() >= GROUP_FILES || group_bytes >= GROUP_BYTES {
                                group_bytes = 0;
                                if let Some(s) = self.commit_group(&mut group) {
                                    return s;
                                }
                            }
                        }
                        Next::Copy(resume) => {
                            group_bytes = 0;
                            if let Some(s) = self.commit_group(&mut group) {
                                return s;
                            }
                            if let Some(s) = self.alone(idx, &req, resume.map(|r| *r)) {
                                return s;
                            }
                        }
                    }
                }
            }
        }
        self.commit_group(&mut group).unwrap_or(Stop::Done)
    }

    /// Stops; a pending group is dropped (nothing of it was written yet).
    fn stop_with_group(&self, s: Stop, group: &mut Vec<(usize, CopyRequest)>) -> Stop {
        group.clear();
        s
    }

    /// Same APFS volume and cloning on: an instant clone, never grouped.
    fn clones(&self, e: &Entry) -> bool {
        let s = &self.job.m.sources[e.src_vol as usize];
        let d = &self.job.m.dest.volume;
        self.opts.clone_same_volume
            && s.fs == "apfs"
            && d.fs == "apfs"
            && s.uuid.is_some()
            && s.uuid == d.uuid
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

    fn make_dir(&self, idx: usize, e: &Entry) -> Option<Stop> {
        let path = self.dest_root.join(&e.rel);
        let made = match std::fs::create_dir(&path) {
            Ok(()) => Ok(true),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                if std::fs::metadata(&path).is_ok_and(|m| m.is_dir()) {
                    Ok(false)
                } else {
                    self.job.update(idx, |r| {
                        r.st = FileSt::Skipped;
                        r.reason = Some("a file with this name is in the way".into());
                    });
                    return None;
                }
            }
            Err(err) => Err(err),
        };
        match made {
            Ok(created) => {
                if created && let Some(parent) = path.parent() {
                    let _ = self.p().flush.flush_dir(parent);
                }
                self.job.update(idx, |r| {
                    r.st = FileSt::Done;
                    r.outcome = Some("folder".into());
                });
                None
            }
            Err(_) if self.dest_gone() => Some(Stop::DestGone),
            Err(err) if err.raw_os_error() == Some(libc::ENOSPC) => Some(Stop::DestFull),
            Err(err) => {
                self.job.update(idx, |r| {
                    r.st = FileSt::Failed;
                    r.reason = Some(format!(
                        "the folder couldn't be made: {}",
                        crate::copy::describe(&err)
                    ));
                });
                None
            }
        }
    }

    /// Looks at the disk before copying a file that isn't finished (§6).
    fn decide(&self, idx: usize, req: &CopyRequest, e: &Entry, small: bool) -> Next {
        let rec = self.job.rec(idx);
        let owner = PartialOwner {
            final_path: req.dest.clone(),
            job: self.job.id,
            n: e.n,
        };
        let partial = owner.path();
        let partial_there = std::fs::symlink_metadata(&partial).is_ok();
        let p = self.p();
        if std::fs::symlink_metadata(&req.dest).is_ok() {
            if partial_there {
                let _ = remove::remove_partial(p, &partial, &owner);
            }
            if rec.st == FileSt::Placed {
                // Renamed before the interruption; only ACLs and flags were left.
                let _ = p.meta.apply_security(&req.source, &req.dest);
                self.job.update(idx, |r| {
                    r.st = FileSt::Done;
                    r.resumed = true;
                    r.outcome.get_or_insert_with(|| "verified".into());
                });
                return Next::Settled;
            }
            // Existing files were marked skipped when the job was planned, so
            // this one appeared since: most likely our own copy, renamed just
            // before the journal caught up. It counts only if it is identical.
            if self.same_content(req, e, &rec) {
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
                return Next::Settled;
            }
            self.job.update(idx, |r| {
                r.st = FileSt::Skipped;
                r.reason = Some("already exists; nothing was replaced".into());
            });
            return Next::Settled;
        }
        let Ok(smd) = std::fs::symlink_metadata(&req.source) else {
            return Next::Copy(None); // prepare() reports it
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
                    *r = FileRec {
                        restarted: Some(why),
                        ..Default::default()
                    };
                });
                return Next::Copy(None);
            }
        }
        if !partial_there {
            return Next::Copy(None);
        }
        if small || e.kind != EntryKind::File {
            let _ = remove::remove_partial(p, &partial, &owner);
            return Next::Copy(None);
        }
        match resume_from(
            p,
            &partial,
            &rec.chunk_hashes(),
            self.opts.chunk_size,
            smd.len(),
        ) {
            Ok(r) => Next::Copy(Some(Box::new(r))),
            Err(_) => {
                let _ = remove::remove_partial(p, &partial, &owner);
                Next::Copy(None)
            }
        }
    }

    /// Whether the file at the destination is this job's copy: the same link
    /// target, or the same content hash (the journal's, else the original's).
    fn same_content(&self, req: &CopyRequest, e: &Entry, rec: &FileRec) -> bool {
        if e.kind == EntryKind::Link {
            return std::fs::read_link(&req.dest).ok() == std::fs::read_link(&req.source).ok();
        }
        let p = self.p();
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

    fn done(&self, idx: usize, report: &FileReport) {
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
        });
        self.inner.sink.on_event(Event::FileDone {
            job: self.job.id,
            path: rel,
            report: Box::new(report.clone()),
        });
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
        let (st, why) = match err {
            CopyError::Conflict(_) => (
                FileSt::Skipped,
                "already exists; nothing was replaced".to_string(),
            ),
            CopyError::Skipped(r) => (FileSt::Skipped, r.clone()),
            other => (FileSt::Failed, other.to_string()),
        };
        self.job.update(idx, |r| {
            r.st = st;
            r.reason = Some(why.clone());
            r.chunks.clear();
        });
        self.inner.sink.on_event(Event::FileNotCopied {
            job: self.job.id,
            path: e.rel.clone(),
            failed: st == FileSt::Failed,
            reason: why,
        });
        None
    }

    fn failure(&self, idx: usize, f: Failure) -> Option<Stop> {
        self.error(idx, &f.error)
    }

    /// A big file, link or clone on its own (with chunk-level resume).
    fn alone(&self, idx: usize, req: &CopyRequest, resume: Option<Resume>) -> Option<Stop> {
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
            true,
            Some(&self.job.m.dest.volume.fs),
        ) {
            Ok(w) => w,
            Err(e) => return self.error(idx, &e),
        };
        match w.run_alone(resume) {
            Ok(r) => {
                self.done(idx, &r);
                None
            }
            Err(f) => self.failure(idx, f),
        }
    }

    /// Small files: write and `fsync` each, one drive flush for the group,
    /// then check and rename each, then one folder flush (§4.7). Nothing gets
    /// its real name before the drive flush that covers its data.
    fn commit_group(&self, group: &mut Vec<(usize, CopyRequest)>) -> Option<Stop> {
        if group.is_empty() {
            return None;
        }
        let reqs = std::mem::take(group);
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
            let t0 = Instant::now();
            let mut works: Vec<(usize, FileWork)> = Vec::new();
            while next < reqs.len() && (works.is_empty() || t0.elapsed() < GROUP_TIME) {
                let k = next;
                next += 1;
                let (idx, req) = &reqs[k];
                self.start_rec(*idx, req, false);
                let mut w = match FileWork::prepare(
                    self.p(),
                    req,
                    &self.opts,
                    self.ctl,
                    &sinks[k],
                    &logs[k],
                    true,
                    Some(&self.job.m.dest.volume.fs),
                ) {
                    Ok(w) => w,
                    Err(e) => match self.error(*idx, &e) {
                        Some(s) => return Some(self.abandon(works, s)),
                        None => continue,
                    },
                };
                match w.write(None, false).and_then(|()| w.seal(false)) {
                    Ok(()) => {
                        let hash = w.hashes().map(|h| h.file_hex());
                        self.job.update(*idx, |r| {
                            r.st = FileSt::Written;
                            r.hash = hash;
                        });
                        works.push((k, w));
                    }
                    Err(e) => {
                        if self.dest_gone() {
                            w.keep_partial();
                        } else {
                            w.discard();
                        }
                        if let Some(s) = self.error(*idx, &e) {
                            return Some(self.abandon(works, s));
                        }
                    }
                }
            }
            if works.is_empty() {
                continue;
            }
            // One drive flush for the whole group.
            let barrier = self.fault(GroupPoint::BeforeBarrier).and_then(|()| {
                let last = &works
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
            if let Err(e) = self.fault(GroupPoint::AfterBarrier) {
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
            let mut placed: Vec<(usize, FileWork)> = Vec::new();
            let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();
            let mut stop = None;
            let mut rest = works.into_iter().enumerate();
            for (j, (k, mut w)) in rest.by_ref() {
                let idx = reqs[k].0;
                let step = self
                    .fault(GroupPoint::Rename(j))
                    .map_err(|e| CopyError::Io {
                        phase: Phase::Renaming,
                        message: crate::copy::describe(&e),
                        source: e,
                    })
                    .and_then(|()| w.verify());
                match step {
                    Ok(Checked::Good) => match w.place(false) {
                        Ok(()) => {
                            self.job.update(idx, |r| r.st = FileSt::Placed);
                            if let Some(d) = reqs[k].1.dest.parent() {
                                dirs.insert(d.to_path_buf());
                            }
                            placed.push((k, w));
                        }
                        Err(e) => {
                            let f = self.fail_or_keep(w, e);
                            if let Some(s) = self.failure(idx, f) {
                                stop = Some(s);
                                break;
                            }
                        }
                    },
                    Ok(Checked::Redo) => match w.redo() {
                        Ok(r) => self.done(idx, &r),
                        Err(f) => {
                            if let Some(s) = self.failure(idx, f) {
                                stop = Some(s);
                                break;
                            }
                        }
                    },
                    Err(e) => {
                        let f = self.fail_or_keep(w, e);
                        if let Some(s) = self.failure(idx, f) {
                            stop = Some(s);
                            break;
                        }
                    }
                }
            }
            // Files not reached because the run stopped keep their partials
            // on a pulled disk, and lose them otherwise.
            for (_, (k, mut w)) in rest {
                if stop == Some(Stop::DestGone) {
                    w.keep_partial();
                } else {
                    w.discard();
                    self.job.update(reqs[k].0, |r| {
                        r.st = FileSt::Pending;
                        r.chunks.clear();
                    });
                }
            }
            if let Some(s) = self.folder_barrier(&dirs, placed, &reqs) {
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

    /// The renamed files of a group: one folder flush, then their metadata.
    fn folder_barrier(
        &self,
        dirs: &BTreeSet<PathBuf>,
        placed: Vec<(usize, FileWork)>,
        reqs: &[(usize, CopyRequest)],
    ) -> Option<Stop> {
        if placed.is_empty() {
            return None;
        }
        let r = self.fault(GroupPoint::BeforeFolderBarrier).and_then(|()| {
            let mut level = FlushLevel::Device;
            let last = dirs.iter().next_back();
            for d in dirs {
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
            self.done(reqs[k].0, &report);
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
        let why = format!("the drive flush failed: {}", crate::copy::describe(&e));
        for (k, mut w) in works {
            w.discard();
            let idx = reqs[k].0;
            self.job.update(idx, |r| {
                r.st = FileSt::Failed;
                r.reason = Some(why.clone());
                r.chunks.clear();
            });
            self.inner.sink.on_event(Event::FileNotCopied {
                job: self.job.id,
                path: self.job.m.files[idx].rel.clone(),
                failed: true,
                reason: why.clone(),
            });
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

#[derive(Clone, Copy)]
#[cfg_attr(not(feature = "fault-injection"), allow(dead_code))]
enum GroupPoint {
    BeforeBarrier,
    AfterBarrier,
    Rename(usize),
    BeforeFolderBarrier,
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

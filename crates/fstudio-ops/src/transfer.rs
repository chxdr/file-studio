//! The Safe Transfer engine as the queue uses it (F-026): one engine per
//! app, shared by every queue opened on it, with its events routed to the op
//! that runs each job.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;
use safe_transfer as st;

use crate::trash::Trasher;
use crate::types::FaultHook;

/// Events for jobs no op is watching right now (the resume offer when a
/// card comes back, for example).
pub(crate) type Fallback = Arc<dyn Fn(st::Event) + Send + Sync>;

/// Routes each job's events to the op running it.
#[derive(Default)]
pub(crate) struct Hub {
    routes: Mutex<HashMap<String, Sender<st::Event>>>,
    fallback: Mutex<Option<Fallback>>,
}

impl Hub {
    /// Sends `job`'s events to the returned channel until [`Hub::unsubscribe`].
    pub(crate) fn subscribe(&self, job: &st::JobId) -> Receiver<st::Event> {
        let (tx, rx) = crossbeam_channel::unbounded();
        self.routes.lock().insert(job.to_string(), tx);
        rx
    }

    pub(crate) fn unsubscribe(&self, job: &st::JobId) {
        self.routes.lock().remove(job.as_str());
    }

    pub(crate) fn set_fallback(&self, f: Fallback) {
        *self.fallback.lock() = Some(f);
    }
}

/// The job an event is about.
fn job_of(e: &st::Event) -> &st::JobId {
    match e {
        st::Event::JobState { job, .. }
        | st::Event::FileDone { job, .. }
        | st::Event::FileNotCopied { job, .. }
        | st::Event::WaitingForSource { job, .. }
        | st::Event::WaitingForDestination { job, .. }
        | st::Event::ResumeAvailable { job, .. }
        | st::Event::Conflict { job, .. }
        | st::Event::JobFinished { job, .. } => job,
        st::Event::Progress(p) => &p.job,
    }
}

impl st::EventSink for Hub {
    fn on_event(&self, e: st::Event) {
        let key = job_of(&e).to_string();
        let route = self.routes.lock().get(&key).cloned();
        if let Some(tx) = route
            && tx.send(e.clone()).is_ok()
            && !matches!(e, st::Event::ResumeAvailable { .. })
        {
            return;
        }
        let f = self.fallback.lock().clone();
        if let Some(f) = f {
            f(e);
        }
    }
}

/// Sends moved originals and replaced items to the queue's own Trash, so
/// Put Back and the tests' folder trash work the same on both backends.
struct TrashVia(Arc<dyn Trasher>);

impl st::Trash for TrashVia {
    fn trash(&self, path: &Path) -> io::Result<PathBuf> {
        self.0.trash(path)
    }
}

/// The Safe Transfer engine for the queue. Cheap to clone.
#[derive(Clone)]
pub struct Transfer {
    pub(crate) engine: st::Engine,
    pub(crate) hub: Arc<Hub>,
}

impl Transfer {
    /// Opens the engine with its job journals in `journal_dir` (File
    /// Studio's `transfers` folder). `trasher` must be the one the queue
    /// uses. `auto_resume` resumes a job by itself when its disks are back
    /// (off by default in File Studio, proposal §6.1). `faults` reaches Safe
    /// Transfer's fault points in tests and needs the `st-faults` feature.
    pub fn open(
        journal_dir: impl Into<PathBuf>,
        auto_resume: bool,
        trasher: Arc<dyn Trasher>,
        faults: Option<Arc<dyn FaultHook>>,
    ) -> io::Result<Transfer> {
        let mut cfg = st::EngineConfig::new(journal_dir);
        cfg.auto_resume = auto_resume;
        set_faults(&mut cfg, faults)?;
        let mut p = st::Platform::native();
        p.trash = Arc::new(TrashVia(trasher));
        let hub = Arc::new(Hub::default());
        let engine = st::Engine::open(cfg, p, hub.clone()).map_err(io::Error::other)?;
        Ok(Transfer { engine, hub })
    }

    /// Tells the engine that disks came or went: jobs whose disks are back
    /// are offered for resuming ([`crate::OpEvent::ResumeAvailable`]), or
    /// resumed when auto-resume is on. Touches the disk (lists volumes).
    pub fn volumes_changed(&self) {
        self.engine.volumes_changed();
    }

    /// The engine itself (for the CLI and for reports).
    pub fn engine(&self) -> &st::Engine {
        &self.engine
    }
}

#[cfg(feature = "st-faults")]
fn set_faults(cfg: &mut st::EngineConfig, faults: Option<Arc<dyn FaultHook>>) -> io::Result<()> {
    cfg.faults = faults.map(|f| Arc::new(OpsFaults(f)) as Arc<dyn st::fault::FaultHook>);
    Ok(())
}

#[cfg(not(feature = "st-faults"))]
fn set_faults(_cfg: &mut st::EngineConfig, faults: Option<Arc<dyn FaultHook>>) -> io::Result<()> {
    match faults {
        None => Ok(()),
        Some(_) => Err(io::Error::other(
            "fault hooks on the safe_transfer backend need fstudio-ops' `st-faults` feature",
        )),
    }
}

/// Ops' fault steps at Safe Transfer's matching points.
#[cfg(feature = "st-faults")]
struct OpsFaults(Arc<dyn FaultHook>);

#[cfg(feature = "st-faults")]
impl std::fmt::Debug for OpsFaults {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OpsFaults")
    }
}

#[cfg(feature = "st-faults")]
impl st::fault::FaultHook for OpsFaults {
    fn at(&self, point: st::fault::FaultPoint, source: &Path) -> io::Result<()> {
        use crate::types::FaultStep as S;
        let step = match point.name() {
            "before_copy" => S::BeforeCopy,
            "after_write" => S::AfterCopy,
            "mid_verify" if point.offset() == 0 => S::Verify,
            "before_rename" => S::BeforePlace,
            "before_source_trash" => S::BeforeSourceRemove,
            _ => return Ok(()),
        };
        self.0.check(step, source).map_err(|e| {
            if step == S::Verify {
                io::Error::other("verification failed: the copy doesn't match the original")
            } else {
                e
            }
        })
    }
}

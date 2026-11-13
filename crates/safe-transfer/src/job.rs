//! Jobs: what to copy, the plan built from it (stat only), and what a job
//! reports (proposal §2).

use std::collections::{HashMap, HashSet};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::events::{FileReport, Phase};
use crate::journal::{Entry, EntryKind, FileRec, FileSt, JobState, VolRef};
use crate::names::{self, JobId};
use crate::options::{Options, Symlinks};
use crate::platform::{Platform, VolumeInfo};
use crate::space;

/// One thing to copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// A file, symlink or folder (copied as a tree), keeping its name.
    Path(PathBuf),
    /// One file to an exact path under the destination (Import Studio's
    /// renamed imports). Missing folders on the way are created.
    Exact {
        /// The original.
        source: PathBuf,
        /// Where it goes, relative to the destination folder.
        dest_rel: PathBuf,
    },
}

/// A job to create.
#[derive(Debug, Clone)]
pub struct JobSpec {
    /// What to copy.
    pub items: Vec<Item>,
    /// The destination folder (must exist).
    pub dest: PathBuf,
    /// Options (the fault hook, if any, comes from the engine instead).
    pub options: Options,
    /// For the resume prompt: "Smith wedding → CLIENT_A".
    pub label: String,
    /// The host's own reference, returned in events, never interpreted.
    pub host_ref: Option<String>,
}

impl JobSpec {
    /// A job copying `items` into `dest` with default options.
    pub fn new(items: Vec<Item>, dest: impl Into<PathBuf>) -> JobSpec {
        let dest = dest.into();
        JobSpec {
            label: dest.display().to_string(),
            items,
            dest,
            options: Options::default(),
            host_ref: None,
        }
    }
}

/// A path and a plain-language reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    /// The item, relative to the destination folder.
    pub path: PathBuf,
    /// Why.
    pub reason: String,
}

/// What a job would do. Built from metadata only.
#[derive(Debug, Clone)]
pub struct JobPlan {
    /// Files to copy (not counting skipped ones).
    pub files: u32,
    /// Folders.
    pub folders: u32,
    /// Symlinks.
    pub links: u32,
    /// Bytes to copy.
    pub bytes: u64,
    /// Bytes needed on the destination (allocation units, AppleDouble files).
    pub need: u64,
    /// The safety margin on top.
    pub margin: u64,
    /// Free bytes on the destination.
    pub free: u64,
    /// `need + margin <= free`.
    pub fits: bool,
    /// Items that won't be copied, and why (already there, iCloud placeholders…).
    pub skipped: Vec<Note>,
    /// The destination volume.
    pub dest_volume: VolumeInfo,
    /// The destination folder, canonical (so it can be moved to another
    /// mount point of the same volume on resume).
    pub(crate) dest: PathBuf,
    pub(crate) entries: Vec<Entry>,
    pub(crate) recs: Vec<FileRec>,
    pub(crate) sources: Vec<VolRef>,
}

/// A source card a job is waiting for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissingSource {
    /// The volume's name.
    pub volume: String,
    /// Its UUID.
    pub uuid: Option<String>,
    /// The files waiting on it (relative to the destination).
    pub files: Vec<PathBuf>,
}

/// Progress of a whole job.
#[derive(Debug, Clone, PartialEq)]
pub struct JobProgress {
    /// The job.
    pub job: JobId,
    /// The phase of the file named in `file`.
    pub phase: Phase,
    /// The current file, relative to the destination.
    pub file: Option<PathBuf>,
    /// Work bytes done: bytes written plus bytes read back (plus re-read).
    pub done: u64,
    /// Work bytes in the whole job.
    pub total: u64,
    /// Files finished (done, skipped or failed).
    pub files_done: u32,
    /// Files in the job.
    pub files_total: u32,
    /// Work bytes per second since the job (re)started.
    pub bytes_per_sec: u64,
    /// Seconds left, once there is a rate to go by.
    pub eta_secs: Option<u64>,
}

/// Something that happened in a job.
#[derive(Debug, Clone)]
pub enum Event {
    /// The job's state changed.
    JobState {
        /// The job.
        job: JobId,
        /// Its new state.
        state: JobState,
    },
    /// Progress (at most every 16 ms per file).
    Progress(JobProgress),
    /// A file is in place under its real name.
    FileDone {
        /// The job.
        job: JobId,
        /// The file, relative to the destination.
        path: PathBuf,
        /// What happened to it.
        report: Box<FileReport>,
    },
    /// A file was not copied.
    FileNotCopied {
        /// The job.
        job: JobId,
        /// The file, relative to the destination.
        path: PathBuf,
        /// Skipped on purpose (`false`) or failed (`true`).
        failed: bool,
        /// Why.
        reason: String,
    },
    /// Files wait for source cards that aren't connected.
    WaitingForSource {
        /// The job.
        job: JobId,
        /// Which files wait on which card.
        missing: Vec<MissingSource>,
    },
    /// The destination isn't connected.
    WaitingForDestination {
        /// The job.
        job: JobId,
        /// The volume's name.
        volume: String,
    },
    /// An unfinished job's destination (and sources) are connected.
    ResumeAvailable {
        /// The job.
        job: JobId,
        /// Its label.
        label: String,
        /// The destination volume's name.
        volume: String,
        /// Files left.
        files_left: u32,
        /// Bytes left.
        bytes_left: u64,
    },
    /// The job finished; here is its report.
    JobFinished {
        /// The job.
        job: JobId,
        /// The report.
        report: JobReport,
    },
}

/// Receives events. Called on the job's thread; keep it quick.
pub trait EventSink: Send + Sync {
    /// One event.
    fn on_event(&self, e: Event);
}

impl<F: Fn(Event) + Send + Sync> EventSink for F {
    fn on_event(&self, e: Event) {
        self(e)
    }
}

/// Drops every event.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoEvents;

impl EventSink for NoEvents {
    fn on_event(&self, _e: Event) {}
}

/// A job at a glance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobInfo {
    /// The job.
    pub id: JobId,
    /// Its label.
    pub label: String,
    /// Its state.
    pub state: JobState,
    /// The destination folder (as it was mounted when the job was made).
    pub dest: PathBuf,
    /// The destination volume's name.
    pub dest_volume: String,
    /// Files in the job.
    pub files_total: u32,
    /// Files done.
    pub files_done: u32,
    /// Files skipped.
    pub files_skipped: u32,
    /// Files failed.
    pub files_failed: u32,
    /// Bytes of files in the job.
    pub bytes_total: u64,
    /// Bytes of files done.
    pub bytes_done: u64,
    /// Files waiting for source cards.
    pub missing: Vec<MissingSource>,
    /// The destination volume's UUID (to match a sidebar disk to its jobs).
    pub dest_uuid: Option<String>,
    /// Whether this job is waiting for someone to resume it, and what for.
    pub resume: Resumability,
}

/// Whether a job can be resumed by hand right now (proposal §6.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resumability {
    /// Running, planned, finished or closed: nothing to resume.
    NotNeeded,
    /// Its disks are connected: [`crate::Engine::resume`] makes progress now.
    /// `still_missing` names source cards some files are still waiting for.
    Ready {
        /// Source disks that are still not connected (their files keep waiting).
        still_missing: Vec<String>,
    },
    /// Waiting for these disks to be connected ("Insert CLIENT_A to resume").
    NeedsVolumes(Vec<String>),
    /// Another app or terminal is running it.
    InUseElsewhere,
}

/// The end-of-job report (also saved as `report.json`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct JobReport {
    /// The job id.
    pub job: String,
    /// Its label.
    pub label: String,
    /// Copied byte for byte and read back from the drive.
    pub copied_verified: u32,
    /// Copied and matched, but part of the read-back may have come from memory.
    pub verified_from_memory: u32,
    /// Instant APFS clones (nothing to read back).
    pub cloned: u32,
    /// Symlinks copied as links.
    pub links: u32,
    /// Folders created or merged into.
    pub folders: u32,
    /// Files continued from an interrupted attempt.
    pub resumed: u32,
    /// Chunks kept from interrupted attempts.
    pub chunks_reused: u32,
    /// Files whose original was read twice and agreed.
    pub source_reread: u32,
    /// Files started again because their original changed.
    pub restarted: Vec<Note>,
    /// Not copied on purpose.
    pub skipped: Vec<Note>,
    /// Not copied because of an error.
    pub failed: Vec<Note>,
    /// Metadata the destination didn't keep.
    pub metadata_lost: Vec<Note>,
    /// Files whose flush only reached OS level.
    pub os_flush_only: u32,
    /// Bytes of files done.
    pub bytes: u64,
    /// Seconds spent running (across resumes).
    pub seconds: f64,
}

/// Why a job couldn't be created or controlled.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// The job won't fit.
    #[error(
        "it needs {} (including a {} safety margin), and the destination has {} free",
        human(need + margin), human(*margin), human(*free)
    )]
    NoSpace {
        /// Bytes needed.
        need: u64,
        /// The margin.
        margin: u64,
        /// Bytes free.
        free: u64,
    },
    /// The destination is inside one of the folders being copied.
    #[error("a folder can't be copied into itself")]
    IntoItself,
    /// The destination isn't a folder.
    #[error("{} is not a folder", .0.display())]
    NotAFolder(PathBuf),
    /// No such job.
    #[error("there is no job {0}")]
    UnknownJob(String),
    /// Another process runs this job.
    #[error("another app or terminal is running this job right now")]
    InUse,
    /// A disk the job needs isn't connected.
    #[error("insert “{0}” first")]
    NeedsVolume(String),
    /// The job is finished or closed.
    #[error("this job is already {0}")]
    Final(&'static str),
    /// An I/O error while planning or journaling.
    #[error("{what}: {}", crate::copy::describe(.source))]
    Io {
        /// What was being done.
        what: String,
        /// The error.
        #[source]
        source: io::Error,
    },
}

pub(crate) fn io_ctx(what: impl Into<String>) -> impl FnOnce(io::Error) -> EngineError {
    let what = what.into();
    move |source| EngineError::Io { what, source }
}

/// Bytes for people: "48.2 GB".
pub fn human(bytes: u64) -> String {
    let b = bytes as f64;
    match bytes {
        0..1_000 => format!("{bytes} B"),
        1_000..1_000_000 => format!("{:.1} KB", b / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1} MB", b / 1e6),
        _ => format!("{:.2} GB", b / 1e9),
    }
}

pub(crate) fn volref(v: &VolumeInfo) -> VolRef {
    VolRef {
        uuid: v.uuid.clone(),
        fs: v.fs_type.clone(),
        name: v.name.clone(),
        capacity: v.capacity,
        mount: v.mount.clone(),
    }
}

/// Filesystems that store extended attributes in AppleDouble `._` files.
pub(crate) fn no_native_xattrs(fs: &str) -> bool {
    matches!(fs, "exfat" | "msdos")
}

struct Planner<'a> {
    p: &'a Platform,
    follow: bool,
    entries: Vec<Entry>,
    sources: Vec<VolRef>,
    by_dev: HashMap<u64, u32>,
    seen_dirs: HashSet<(u64, u64)>,
    dirs_planned: HashSet<PathBuf>,
}

impl Planner<'_> {
    fn src_vol(&mut self, path: &Path, dev: u64) -> Result<u32, EngineError> {
        if let Some(i) = self.by_dev.get(&dev) {
            return Ok(*i);
        }
        let v = self
            .p
            .volumes
            .volume_of(path)
            .map_err(io_ctx(format!("reading the volume of {}", path.display())))?;
        let i = self.sources.len() as u32;
        self.sources.push(volref(&v));
        self.by_dev.insert(dev, i);
        Ok(i)
    }

    fn push(
        &mut self,
        kind: EntryKind,
        src: Option<PathBuf>,
        src_vol: u32,
        rel: PathBuf,
        md: Option<&std::fs::Metadata>,
    ) {
        let n = self.entries.len() as u32;
        self.entries.push(Entry {
            n,
            kind,
            src,
            src_vol,
            rel,
            size: md.filter(|m| m.is_file()).map_or(0, |m| m.len()),
            mtime_ns: md.map_or(0, |m| m.mtime() * 1_000_000_000 + m.mtime_nsec()),
            ino: md.map_or(0, |m| m.ino()),
            dataless: md.is_some_and(|m| crate::sys::file_flags(m) & 0x4000_0000 != 0),
        });
    }

    fn walk(&mut self, path: &Path, rel: PathBuf) -> Result<(), EngineError> {
        let lmd = std::fs::symlink_metadata(path)
            .map_err(io_ctx(format!("reading {}", path.display())))?;
        let md = if self.follow && lmd.file_type().is_symlink() {
            std::fs::metadata(path).map_err(io_ctx(format!("following {}", path.display())))?
        } else {
            lmd
        };
        let vol = self.src_vol(path, md.dev())?;
        let ft = md.file_type();
        if ft.is_dir() {
            if !self.seen_dirs.insert((md.dev(), md.ino())) {
                return Ok(()); // a folder loop through followed links
            }
            self.dirs_planned.insert(rel.clone());
            self.push(
                EntryKind::Dir,
                Some(path.to_path_buf()),
                vol,
                rel.clone(),
                Some(&md),
            );
            let mut kids: Vec<std::ffi::OsString> = std::fs::read_dir(path)
                .map_err(io_ctx(format!("listing {}", path.display())))?
                .filter_map(|e| e.ok().map(|e| e.file_name()))
                .collect();
            kids.sort();
            let fs = self.sources[vol as usize].fs.clone();
            for k in &kids {
                let name = k.to_string_lossy();
                if name.ends_with(names::PARTIAL_SUFFIX) || name.ends_with(".fstudio-partial") {
                    continue;
                }
                // macOS's own AppleDouble companion of a sibling: its xattrs
                // travel with the sibling, not as a file of their own.
                if no_native_xattrs(&fs)
                    && let Some(base) = name.strip_prefix("._")
                    && kids.iter().any(|o| o.to_string_lossy() == base)
                {
                    continue;
                }
                self.walk(&path.join(k), rel.join(k))?;
            }
        } else {
            let kind = if ft.is_symlink() {
                EntryKind::Link
            } else if ft.is_file() {
                EntryKind::File
            } else {
                EntryKind::Other
            };
            self.push(kind, Some(path.to_path_buf()), vol, rel, Some(&md));
        }
        Ok(())
    }

    fn ensure_dirs(&mut self, rel: &Path) {
        let mut acc = PathBuf::new();
        let parts: Vec<_> = rel.components().collect();
        for c in &parts[..parts.len().saturating_sub(1)] {
            acc.push(c);
            if self.dirs_planned.insert(acc.clone()) {
                self.push(EntryKind::Dir, None, 0, acc.clone(), None);
            }
        }
    }
}

/// Plans `spec`: lists folders (stat only, no data, no iCloud downloads),
/// predicts conflicts and checks free space.
pub(crate) fn plan(p: &Platform, spec: &JobSpec) -> Result<JobPlan, EngineError> {
    let dmd = std::fs::metadata(&spec.dest).map_err(io_ctx(format!(
        "opening the destination {}",
        spec.dest.display()
    )))?;
    if !dmd.is_dir() {
        return Err(EngineError::NotAFolder(spec.dest.clone()));
    }
    let dest_vol = p
        .volumes
        .volume_of(&spec.dest)
        .map_err(io_ctx("reading the destination's volume"))?;
    let case_sensitive = p.volumes.case_sensitive(&spec.dest).unwrap_or(false);
    // Canonical paths ("/private/var/…", not "/var/…") so they share a
    // prefix with the mount point and can be moved to a new one on resume.
    let dest_canon = std::fs::canonicalize(&spec.dest).unwrap_or(spec.dest.clone());
    let canon_item = |path: &Path| -> PathBuf {
        match (path.parent(), path.file_name()) {
            (Some(dir), Some(name)) if !dir.as_os_str().is_empty() => std::fs::canonicalize(dir)
                .map(|d| d.join(name))
                .unwrap_or_else(|_| path.to_path_buf()),
            _ => path.to_path_buf(),
        }
    };
    let mut pl = Planner {
        p,
        follow: spec.options.symlinks == Symlinks::Follow,
        entries: Vec::new(),
        sources: Vec::new(),
        by_dev: HashMap::new(),
        seen_dirs: HashSet::new(),
        dirs_planned: HashSet::new(),
    };
    for item in &spec.items {
        match item {
            Item::Path(path) => {
                let path = &canon_item(path);
                if std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
                    && std::fs::canonicalize(path).is_ok_and(|c| dest_canon.starts_with(c))
                {
                    return Err(EngineError::IntoItself);
                }
                let name = path.file_name().ok_or_else(|| EngineError::Io {
                    what: format!("{} has no name", path.display()),
                    source: io::ErrorKind::InvalidInput.into(),
                })?;
                pl.walk(path, PathBuf::from(name))?;
            }
            Item::Exact { source, dest_rel } => {
                let source = &canon_item(source);
                pl.ensure_dirs(dest_rel);
                let md = std::fs::symlink_metadata(source)
                    .map_err(io_ctx(format!("reading {}", source.display())))?;
                let vol = pl.src_vol(source, md.dev())?;
                let kind = if md.file_type().is_symlink() {
                    EntryKind::Link
                } else if md.is_file() {
                    EntryKind::File
                } else {
                    EntryKind::Other
                };
                pl.push(kind, Some(source.clone()), vol, dest_rel.clone(), Some(&md));
            }
        }
    }
    let same_apfs = |e: &Entry| {
        let s = &pl.sources[e.src_vol as usize];
        spec.options.clone_same_volume
            && s.fs == "apfs"
            && dest_vol.fs_type == "apfs"
            && s.uuid.is_some()
            && s.uuid == dest_vol.uuid
    };
    let mut keys: HashSet<String> = HashSet::new();
    let mut recs = Vec::with_capacity(pl.entries.len());
    let mut skipped = Vec::new();
    let (mut files, mut folders, mut links, mut bytes, mut need) = (0u32, 0u32, 0u32, 0u64, 0u64);
    for e in &pl.entries {
        let dest = dest_canon.join(&e.rel);
        let key = names::name_key(&e.rel.to_string_lossy(), case_sensitive);
        let there = std::fs::symlink_metadata(&dest).ok();
        let skip = |r: &str| Some(r.to_string());
        let reason = match e.kind {
            EntryKind::Dir if there.as_ref().is_some_and(|m| !m.is_dir()) => {
                skip("a file with this name is in the way")
            }
            EntryKind::Dir => None,
            EntryKind::Other => skip("pipes, sockets and devices can't be copied"),
            _ if !keys.insert(key) => {
                skip("another item in this job has the same name on this disk")
            }
            _ if there.is_some() => skip("already exists; nothing was replaced"),
            _ if e.dataless && !spec.options.allow_dataless => {
                skip("not downloaded from iCloud; skipped so it isn't downloaded")
            }
            _ => None,
        };
        let mut rec = FileRec::default();
        if let Some(r) = reason {
            rec.st = FileSt::Skipped;
            rec.reason = Some(r.clone());
            skipped.push(Note {
                path: e.rel.clone(),
                reason: r,
            });
        } else {
            match e.kind {
                EntryKind::Dir => folders += 1,
                EntryKind::Link => links += 1,
                EntryKind::File => {
                    files += 1;
                    bytes += e.size;
                    if !same_apfs(e) {
                        let ad = no_native_xattrs(&dest_vol.fs_type)
                            && e.src.as_deref().is_some_and(|s| {
                                !crate::sys::xattr_names(s).unwrap_or_default().is_empty()
                            });
                        need += space::on_disk(e.size, dest_vol.block_size, ad);
                    }
                }
                EntryKind::Other => {}
            }
        }
        recs.push(rec);
    }
    let margin = space::margin(need);
    Ok(JobPlan {
        files,
        folders,
        links,
        bytes,
        need,
        margin,
        free: dest_vol.free,
        fits: need == 0 || space::fits(need, dest_vol.free),
        skipped,
        dest_volume: dest_vol,
        dest: dest_canon,
        entries: pl.entries,
        recs,
        sources: pl.sources,
    })
}

/// The `Arc` an engine shares with its job threads.
pub(crate) type SharedSink = Arc<dyn EventSink>;

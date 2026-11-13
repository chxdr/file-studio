//! The single-file pipeline (proposal §4):
//!
//! partial file (`F_NOCACHE`) → chunks hashed while written, flushed per chunk
//! → metadata → full flush → evict from the cache → uncached read-back and
//! hash compare → (re-read mode) second read of the original → exclusive
//! rename to the real name → folder flush → ACLs and flags → metadata check.
//!
//! The real name only ever appears on a file that is complete, flushed and
//! verified. Any failure before the rename removes the partial (through
//! `remove.rs`) and leaves the destination as it was.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::chunk::{ChunkHasher, FileHashes};
use crate::events::{FileReport, MetaLoss, Outcome, Phase, ProgressSink, Throttle};
use crate::names::JobId;
use crate::options::{Control, Options, Symlinks};
use crate::platform::{FlushLevel, MetaSnapshot, Platform, RenameMode};
use crate::remove::{PartialOwner, remove_partial};

/// Read and write size. A chunk (for hashing and flushing) is many of these;
/// the chunk size never changes how much memory a copy uses.
const IO_BUF: usize = 4 << 20;

/// A buffer for a file of `len` bytes: up to [`IO_BUF`], never bigger than
/// the file needs (plus one byte, so a read can see the end).
fn buf_len(len: u64) -> usize {
    (len as usize).saturating_add(1).clamp(4096, IO_BUF)
}

/// Granularity at which zeros inside a reported hole are left unwritten.
const HOLE_GRAIN: usize = 64 << 10;

/// Whether the original reports any hole in `[off, off + n)`. Only a hint
/// for keeping the destination sparse; the bytes are always read.
fn touches_hole(s: &File, off: u64, n: u64) -> bool {
    let starts_in_hole = match crate::sys::seek_data(s, off) {
        Ok(None) => true,
        Ok(Some(data)) => data > off,
        Err(_) => false,
    };
    starts_in_hole || matches!(crate::sys::seek_hole(s, off), Ok(Some(h)) if h < off + n)
}

/// `SF_DATALESS`: an iCloud / File Provider placeholder (F-016).
const SF_DATALESS: u32 = 0x4000_0000;

/// One file to copy, to an exact destination path.
#[derive(Debug, Clone)]
pub struct CopyRequest {
    /// The original. Only ever read.
    pub source: PathBuf,
    /// The destination path, including the file's name.
    pub dest: PathBuf,
    /// The job this copy belongs to (part of the partial's name).
    pub job: JobId,
    /// The file's index in its job (part of the partial's name).
    pub index: u32,
}

impl CopyRequest {
    /// A copy of `source` to the exact path `dest`, as its own one-file job.
    pub fn new(source: impl Into<PathBuf>, dest: impl Into<PathBuf>) -> io::Result<CopyRequest> {
        Ok(CopyRequest {
            source: source.into(),
            dest: dest.into(),
            job: JobId::new()?,
            index: 0,
        })
    }

    /// A copy of `source` into the folder `dir`, keeping its name.
    pub fn into_dir(source: impl Into<PathBuf>, dir: &Path) -> io::Result<CopyRequest> {
        let source = source.into();
        let name = source.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "the source has no file name")
        })?;
        let dest = dir.join(name);
        CopyRequest::new(source, dest)
    }

    /// Where this copy's partial file is written.
    pub fn partial_path(&self) -> PathBuf {
        crate::names::partial_path(&self.dest, self.job, self.index)
    }
}

/// Why a copy didn't finish.
#[derive(Debug, thiserror::Error)]
pub enum CopyError {
    /// Something already has the destination name. Nothing was replaced.
    #[error("“{}” already exists; nothing was replaced", .0.display())]
    Conflict(PathBuf),
    /// Deliberately not copied.
    #[error("skipped: {0}")]
    Skipped(String),
    /// This kind of item can't be copied by this version.
    #[error("{0}")]
    Unsupported(String),
    /// Cancelled. The destination is as it was.
    #[error("cancelled")]
    Cancelled,
    /// The original changed while it was being copied.
    #[error("the original changed while it was being copied")]
    SourceChanged,
    /// Two reads of the original disagreed.
    #[error("the original read differently twice; the card or reader may be failing")]
    SourceUnstable,
    /// The copy didn't match the original, twice.
    #[error("verification failed: the copy didn't match the original (tried twice)")]
    VerifyFailed,
    /// An I/O error in a phase.
    #[error("{phase} failed: {message}")]
    Io {
        /// The phase it happened in.
        phase: Phase,
        /// A plain-language description.
        message: String,
        /// The error itself.
        #[source]
        source: io::Error,
    },
}

/// A failed copy, and whether a partial file had to be left behind (its
/// volume was gone, for example). A left partial is always hidden and never
/// carries the real name.
#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub struct Failure {
    /// What went wrong.
    pub error: CopyError,
    /// A partial that could not be removed, for a later sweep.
    pub partial_left: Option<PathBuf>,
}

/// A plain-language description of an I/O error.
pub fn describe(e: &io::Error) -> String {
    match e.raw_os_error() {
        Some(libc::ENOSPC) => "there isn't enough free space".into(),
        Some(libc::EACCES) | Some(libc::EPERM) => "permission denied".into(),
        Some(libc::EROFS) => "the volume is read-only".into(),
        Some(libc::ENOENT) => "the item no longer exists".into(),
        Some(libc::ENAMETOOLONG) => "the name is too long for this disk".into(),
        Some(libc::EDQUOT) => "the disk quota is exceeded".into(),
        Some(libc::ENXIO) | Some(libc::ENODEV) => "the disk is no longer connected".into(),
        Some(libc::EIO) => "an input/output error (the disk may have been disconnected)".into(),
        _ => e.to_string(),
    }
}

fn io_err(phase: Phase) -> impl Fn(io::Error) -> CopyError {
    move |e| {
        if e.raw_os_error() == Some(libc::ECANCELED) {
            return CopyError::Cancelled;
        }
        CopyError::Io {
            phase,
            message: describe(&e),
            source: e,
        }
    }
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Phase::Preparing => "Preparing",
            Phase::Copying => "Copying",
            Phase::Flushing => "Flushing",
            Phase::Verifying => "Verifying",
            Phase::Rereading => "Re-reading the original",
            Phase::Renaming => "Renaming",
            Phase::Metadata => "Applying metadata",
        };
        f.write_str(s)
    }
}

/// What identifies the original for "did it change?" checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ident {
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    pub mtime_ns: i64,
}

pub(crate) fn ident(md: &std::fs::Metadata) -> Ident {
    Ident {
        dev: md.dev(),
        ino: md.ino(),
        size: md.len(),
        mtime_ns: md.mtime() * 1_000_000_000 + md.mtime_nsec(),
    }
}

/// Receives each chunk's hash once the chunk is flushed (the job journal).
pub(crate) trait ChunkLog: Sync {
    /// Chunk `index` of the file is on the drive with hash `hash`.
    fn chunk_flushed(&self, index: usize, hash: u64);
    /// The verified copy is about to replace an existing item: the journal
    /// must know the copy's hash before the old item goes to the Trash.
    fn before_replace(&self, _hashes: &FileHashes) {}
    /// The existing item is in the Trash at `trash`.
    fn replaced(&self, _trash: &Path) {}
}

/// How [`FileWork::prepare`] treats what it finds.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Prep<'a> {
    /// A partial of this file from an earlier attempt is expected.
    pub resuming: bool,
    /// The destination's filesystem, when the caller knows it.
    pub dest_fs: Option<&'a str>,
    /// An existing item at the name is to be replaced (after the new copy
    /// is verified, it goes to the Trash).
    pub replace: bool,
}

/// Logs nothing (single-file copies).
pub(crate) struct NoLog;

impl ChunkLog for NoLog {
    fn chunk_flushed(&self, _index: usize, _hash: u64) {}
}

/// Where a resumed copy continues: the first `len` bytes of the partial
/// were re-read from the drive and matched the journal's chunk hashes.
pub(crate) struct Resume {
    /// Bytes kept.
    pub len: u64,
    /// The hasher, already fed with the kept bytes.
    pub hasher: ChunkHasher,
    /// Chunks kept.
    pub chunks_reused: usize,
}

/// Re-reads the partial uncached and keeps the longest run of chunks from
/// the start whose hashes match `journal` (proposal §6 step 4).
pub(crate) fn resume_from(
    p: &Platform,
    partial: &Path,
    journal: &[u64],
    chunk_size: u64,
    len: u64,
) -> io::Result<Resume> {
    let _ = p.uncached.evict(partial);
    let f = File::open(partial)?;
    let _ = p.uncached.set_uncached(&f);
    let plen = f.metadata()?.len();
    let mut h = ChunkHasher::new(chunk_size);
    let cs = h.chunk_size();
    let mut buf = vec![0u8; IO_BUF];
    let mut off = 0u64;
    let mut reused = 0;
    for (i, want) in journal.iter().enumerate() {
        let end = ((i as u64 + 1) * cs).min(len);
        if end > plen || end <= off {
            break;
        }
        let before = h.clone();
        let mut pos = off;
        while pos < end {
            let n = f.read_at(&mut buf[..((end - pos) as usize).min(IO_BUF)], pos)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            pos += n as u64;
        }
        let got = if end - i as u64 * cs == cs {
            h.chunks().get(i).copied()
        } else {
            h.clone().finish().chunks.get(i).copied()
        };
        if pos != end || got != Some(*want) {
            h = before;
            break;
        }
        off = end;
        reused += 1;
    }
    Ok(Resume {
        len: off,
        hasher: h,
        chunks_reused: reused,
    })
}

/// Copies one file, symlink or clone, verified, to `req.dest`. Blocks the
/// calling thread until done; progress goes to `sink`, pause and cancel come
/// from `ctl`. Never replaces anything at `req.dest`.
pub fn copy_file(
    platform: &Platform,
    req: &CopyRequest,
    opts: &Options,
    ctl: &Control,
    sink: &dyn ProgressSink,
) -> Result<FileReport, Failure> {
    match FileWork::prepare(platform, req, opts, ctl, sink, &NoLog, Prep::default()) {
        Ok(w) => w.run_alone(None),
        Err(error) => Err(Failure {
            error,
            partial_left: None,
        }),
    }
}

/// One file on its way through the pipeline. The job runner drives the
/// steps itself for grouped small files; [`FileWork::run_alone`] does them
/// all in order for everything else.
pub(crate) struct FileWork<'a> {
    p: &'a Platform,
    opts: &'a Options,
    ctl: &'a Control,
    thr: Throttle<'a>,
    log: &'a dyn ChunkLog,
    req: &'a CopyRequest,
    source: PathBuf,
    smd: std::fs::Metadata,
    owner: PartialOwner,
    pub(crate) partial: PathBuf,
    /// The partial exists and is ours to remove.
    ours: bool,
    same_volume: bool,
    is_link: bool,
    dest_fs: String,
    flush: FlushLevel,
    copy_time: Duration,
    flush_time: Duration,
    verify_time: Duration,
    reread_time: Duration,
    warnings: Vec<MetaLoss>,
    lost: Vec<MetaLoss>,
    written: Option<Written>,
    retries: u32,
    reread: bool,
    rename: RenameMode,
    from_device: bool,
    /// Chunks kept from an earlier, interrupted attempt.
    pub(crate) chunks_reused: usize,
    /// Chunks already handed to the journal.
    logged: usize,
    /// Replace an existing item at the name once the copy is verified.
    replace: bool,
    /// Where the replaced item went.
    replaced: Option<PathBuf>,
}

type Res<T> = Result<T, CopyError>;

/// What the data path produced.
struct Written {
    hashes: FileHashes,
    holes: bool,
    /// The partial, still open for writing (sealed through it).
    file: Option<File>,
}

/// What the read-back found.
pub(crate) enum Checked {
    /// Matches: ready to be named.
    Good,
    /// The copy (not the original) was wrong; the partial is gone.
    Redo,
}

impl<'a> FileWork<'a> {
    /// The checks before anything is written (see [`Prep`]).
    pub(crate) fn prepare(
        p: &'a Platform,
        req: &'a CopyRequest,
        opts: &'a Options,
        ctl: &'a Control,
        sink: &'a dyn ProgressSink,
        log: &'a dyn ChunkLog,
        how: Prep<'_>,
    ) -> Res<FileWork<'a>> {
        let mut thr = Throttle::new(sink, opts.progress_interval);
        thr.report(Phase::Preparing, 0, 0);
        let mut source = req.source.clone();
        let mut smd = std::fs::symlink_metadata(&source).map_err(io_err(Phase::Preparing))?;
        if smd.file_type().is_symlink() && opts.symlinks == Symlinks::Follow {
            source = std::fs::canonicalize(&source).map_err(io_err(Phase::Preparing))?;
            smd = std::fs::symlink_metadata(&source).map_err(io_err(Phase::Preparing))?;
        }
        let ft = smd.file_type();
        if ft.is_dir() {
            return Err(CopyError::Unsupported(
                "a folder is copied by a job, not as a single file".into(),
            ));
        }
        if !ft.is_file() && !ft.is_symlink() {
            return Err(CopyError::Unsupported(
                "pipes, sockets and devices can't be copied".into(),
            ));
        }
        if ft.is_file() && crate::sys::file_flags(&smd) & SF_DATALESS != 0 && !opts.allow_dataless {
            return Err(CopyError::Skipped(
                "not downloaded from iCloud; skipped so it isn't downloaded".into(),
            ));
        }
        let dir = req.dest.parent().unwrap_or(Path::new("/"));
        let dmd = std::fs::metadata(dir).map_err(io_err(Phase::Preparing))?;
        if !dmd.is_dir() {
            return Err(CopyError::Unsupported(
                "the destination is not a folder".into(),
            ));
        }
        if std::fs::symlink_metadata(&req.dest).is_ok() && !how.replace {
            return Err(CopyError::Conflict(req.dest.clone()));
        }
        let partial = req.partial_path();
        let partial_there = std::fs::symlink_metadata(&partial).is_ok();
        if partial_there && !how.resuming {
            // Ours by name, but not from this run: never removed here.
            return Err(CopyError::Io {
                phase: Phase::Preparing,
                message: format!(
                    "a partial file from an earlier attempt is in the way: {}",
                    partial.display()
                ),
                source: io::ErrorKind::AlreadyExists.into(),
            });
        }
        let dest_fs = match how.dest_fs {
            Some(fs) => fs.to_string(),
            None => p
                .volumes
                .volume_of(dir)
                .map(|v| v.fs_type)
                .unwrap_or_default(),
        };
        let w = FileWork {
            p,
            opts,
            ctl,
            thr,
            log,
            req,
            same_volume: dmd.dev() == smd.dev(),
            is_link: ft.is_symlink(),
            source,
            smd,
            owner: PartialOwner {
                final_path: req.dest.clone(),
                job: req.job,
                n: req.index,
            },
            partial,
            ours: partial_there,
            dest_fs,
            flush: FlushLevel::Device,
            copy_time: Duration::ZERO,
            flush_time: Duration::ZERO,
            verify_time: Duration::ZERO,
            reread_time: Duration::ZERO,
            warnings: Vec::new(),
            lost: Vec::new(),
            written: None,
            retries: 0,
            reread: false,
            rename: RenameMode::Exclusive,
            from_device: false,
            chunks_reused: 0,
            logged: 0,
            replace: how.replace,
            replaced: None,
        };
        w.check()?;
        Ok(w)
    }

    /// The hashes of what was written, once written.
    pub(crate) fn hashes(&self) -> Option<&FileHashes> {
        self.written.as_ref().map(|w| &w.hashes)
    }

    fn fault(&self, _point: FaultPointArg) -> Res<()> {
        #[cfg(feature = "fault-injection")]
        if let Some(h) = &self.opts.faults {
            h.at(_point.into(), &self.req.source)
                .map_err(io_err(Phase::Preparing))?;
        }
        Ok(())
    }

    fn check(&self) -> Res<()> {
        if self.ctl.wait() {
            Ok(())
        } else {
            Err(CopyError::Cancelled)
        }
    }

    /// Removes our partial. Returns it if it couldn't be removed.
    pub(crate) fn discard(&mut self) -> Option<PathBuf> {
        if !self.ours {
            return None;
        }
        match remove_partial(self.p, &self.partial, &self.owner) {
            Ok(()) => {
                self.ours = false;
                None
            }
            Err(_) => std::fs::symlink_metadata(&self.partial)
                .is_ok()
                .then(|| self.partial.clone())
                // Can't tell (the folder is gone with its disk): keep the record.
                .or_else(|| (!self.dest_dir().exists()).then(|| self.partial.clone())),
        }
    }

    /// Leaves the partial where it is (a yank or a cancel that keeps work).
    pub(crate) fn keep_partial(&mut self) {
        self.ours = false;
    }

    /// Turns an error into a failure, removing the partial.
    pub(crate) fn fail(mut self, error: CopyError) -> Failure {
        let partial_left = self.discard();
        Failure {
            error,
            partial_left,
        }
    }

    fn dest_dir(&self) -> &Path {
        self.req.dest.parent().unwrap_or(Path::new("/"))
    }

    /// exFAT and FAT: no ACLs, no immutable flags, no exclusive rename.
    fn plain_fs(&self) -> bool {
        matches!(self.dest_fs.as_str(), "exfat" | "msdos")
    }

    /// Records a flush that happened outside this file (a group barrier).
    pub(crate) fn flushed(&mut self, level: FlushLevel) {
        self.flush = self.flush.min(level);
    }

    fn flush_file(&mut self, f: &File) -> Res<()> {
        let t = Instant::now();
        let level = self
            .p
            .flush
            .flush_file(f)
            .map_err(io_err(Phase::Flushing))?;
        self.flush_time += t.elapsed();
        self.flushed(level);
        Ok(())
    }

    fn flush_dir(&mut self) -> Res<()> {
        let t = Instant::now();
        let level = self
            .p
            .flush
            .flush_dir(self.dest_dir())
            .map_err(io_err(Phase::Renaming))?;
        self.flush_time += t.elapsed();
        self.flushed(level);
        Ok(())
    }

    /// Every step, in order: a symlink, an instant clone, or the verified
    /// byte copy with per-chunk flushes (and one retry if the copy, not the
    /// original, turned out wrong).
    pub(crate) fn run_alone(mut self, resume: Option<Resume>) -> Result<FileReport, Failure> {
        match self.alone(resume) {
            Ok(()) => Ok(self.finish()),
            Err(e) => Err(self.fail(e)),
        }
    }

    /// Starts again from scratch after a read-back showed the copy (not the
    /// original) was wrong; a second mismatch fails the file.
    pub(crate) fn redo(mut self) -> Result<FileReport, Failure> {
        self.retries = 1;
        match self.redo_steps() {
            Ok(()) => Ok(self.finish()),
            Err(e) => Err(self.fail(e)),
        }
    }

    fn redo_steps(&mut self) -> Res<()> {
        self.write(None, true)?;
        self.seal(true)?;
        match self.verify()? {
            Checked::Good => self.place(true),
            Checked::Redo => Err(CopyError::VerifyFailed),
        }
    }

    fn alone(&mut self, mut resume: Option<Resume>) -> Res<()> {
        self.fault(FaultPointArg::BeforeCopy)?;
        if self.is_link {
            return self.copy_link();
        }
        if resume.is_none()
            && self.same_volume
            && self.opts.clone_same_volume
            && self.try_clone()?
        {
            return Ok(());
        }
        loop {
            self.write(resume.take(), true)?;
            self.seal(true)?;
            match self.verify()? {
                Checked::Good => break,
                Checked::Redo => {
                    self.retries += 1;
                    if self.retries > 1 {
                        return Err(CopyError::VerifyFailed);
                    }
                }
            }
        }
        self.place(true)
    }

    /// A symlink is copied as a link: same target text, never followed.
    fn copy_link(&mut self) -> Res<()> {
        let target = std::fs::read_link(&self.source).map_err(io_err(Phase::Copying))?;
        self.thr.report(Phase::Copying, 0, 0);
        if self.ours {
            let _ = self.discard();
        }
        std::os::unix::fs::symlink(&target, &self.partial).map_err(io_err(Phase::Copying))?;
        self.ours = true;
        let back = std::fs::read_link(&self.partial).map_err(io_err(Phase::Verifying))?;
        if back != target {
            return Err(CopyError::VerifyFailed);
        }
        self.flush_dir()?;
        self.place(true)
    }

    /// Same APFS volume: an instant clone. `false` when the volume can't clone.
    fn try_clone(&mut self) -> Res<bool> {
        let len = self.smd.len();
        self.thr.report(Phase::Copying, 0, len);
        match self.p.clone.clone_file(&self.source, &self.partial) {
            Ok(()) => self.ours = true,
            Err(e)
                if matches!(
                    e.raw_os_error(),
                    Some(libc::ENOTSUP | libc::EXDEV | libc::EINVAL | libc::ENOSYS)
                ) || e.kind() == io::ErrorKind::Unsupported =>
            {
                if std::fs::symlink_metadata(&self.partial).is_ok() {
                    self.ours = true;
                    let _ = self.discard();
                }
                return Ok(false);
            }
            Err(e) => return Err(io_err(Phase::Copying)(e)),
        }
        self.thr.report(Phase::Copying, len, len);
        if let Err(e) = self.p.meta.clear_security(&self.partial) {
            self.lost.push(MetaLoss::Failed(format!(
                "flags couldn't be cleared: {}",
                describe(&e)
            )));
        }
        self.thr.report(Phase::Flushing, 0, 0);
        let f = File::open(&self.partial).map_err(io_err(Phase::Flushing))?;
        self.flush_file(&f)?;
        self.source_unchanged()?;
        self.place(true)?;
        self.written = None;
        Ok(true)
    }

    /// Writes the data (continuing `resume` if given), then the metadata.
    /// With `chunk_barriers`, files bigger than one chunk get a drive flush
    /// after each chunk, which is logged as complete only afterwards.
    pub(crate) fn write(&mut self, resume: Option<Resume>, chunk_barriers: bool) -> Res<()> {
        if self.opts.reread_source {
            // Both reads of the original should come from the disk.
            let _ = self.p.uncached.evict(&self.source);
        }
        let written = self.write_partial(resume, chunk_barriers)?;
        if let Err(e) = self.p.meta.copy_metadata(&self.source, &self.partial) {
            self.lost.push(MetaLoss::Failed(format!(
                "extended attributes and dates couldn't be copied: {}",
                describe(&e)
            )));
        }
        if !self.plain_fs()
            && let Err(e) = self.p.meta.clear_security(&self.partial)
        {
            self.lost.push(MetaLoss::Failed(format!(
                "flags couldn't be cleared: {}",
                describe(&e)
            )));
        }
        self.written = Some(written);
        Ok(())
    }

    /// Makes the written partial durable: a drive flush (`barrier`), or only
    /// an OS-level `fsync` when a group barrier will follow (proposal §4.7).
    pub(crate) fn seal(&mut self, barrier: bool) -> Res<()> {
        self.thr.report(Phase::Flushing, 0, 0);
        let f = match self.written.as_mut().and_then(|w| w.file.take()) {
            Some(f) => f,
            None => File::open(&self.partial).map_err(io_err(Phase::Flushing))?,
        };
        if barrier {
            self.fault(FaultPointArg::BeforeFlush(None))?;
            self.flush_file(&f)?;
            self.fault(FaultPointArg::AfterFlush(None))?;
        } else {
            let t = Instant::now();
            self.p.flush.os_flush(&f).map_err(io_err(Phase::Flushing))?;
            self.flush_time += t.elapsed();
        }
        Ok(())
    }

    /// Checks the sealed partial: the original is unchanged, the read-back
    /// from the drive matches, and (re-read mode) a second read of the
    /// original agrees. `Redo` means the copy was wrong and is gone.
    pub(crate) fn verify(&mut self) -> Res<Checked> {
        self.source_unchanged()?;
        let len = self.smd.len();
        let want = self
            .written
            .as_ref()
            .map(|w| w.hashes.clone())
            .unwrap_or_default();
        let (back, from_device) = self.read_back(len)?;
        if back != want {
            // Whose fault? Read the original again.
            let _ = self.p.uncached.evict(&self.source);
            let src = self.source.clone();
            let again = self.hash(&src, len, Phase::Rereading)?;
            if again != want {
                return Err(CopyError::SourceUnstable);
            }
            let _ = self.discard();
            self.written = None;
            return Ok(Checked::Redo);
        }
        self.from_device = from_device;
        if self.opts.reread_source {
            self.fault(FaultPointArg::BeforeReread)?;
            let evicted = self
                .p
                .uncached
                .evict(&self.source)
                .is_ok_and(|r| r.resident_pages == 0);
            if !evicted {
                self.warnings.push(MetaLoss::Failed(
                    "the original couldn't be dropped from memory, so the second read may have come from RAM"
                        .into(),
                ));
            }
            let t = Instant::now();
            let src = self.source.clone();
            let second = self.hash(&src, len, Phase::Rereading)?;
            self.reread_time += t.elapsed();
            if second != want {
                return Err(CopyError::SourceUnstable);
            }
            self.source_unchanged()?;
            self.reread = evicted;
        }
        Ok(Checked::Good)
    }

    fn source_unchanged(&self) -> Res<()> {
        let now = std::fs::symlink_metadata(&self.source).map_err(io_err(Phase::Verifying))?;
        if ident(&now) != ident(&self.smd) {
            return Err(CopyError::SourceChanged);
        }
        Ok(())
    }

    /// Writes the partial with `F_NOCACHE`, hashing every byte read from the
    /// original, and keeps holes. Flushes after each completed chunk of
    /// files bigger than one chunk.
    ///
    /// **Every byte of the original is read**, holes included: `SEEK_HOLE`
    /// is only a hint about where the destination may stay sparse, never a
    /// reason to skip reading. For an APFS-compressed (decmpfs) file the
    /// whole data fork is reported as a hole while the content lives
    /// elsewhere; trusting the hint copied such files as zeros, and the
    /// verify (hashing the same zeros on both sides) could not tell.
    fn write_partial(&mut self, resume: Option<Resume>, chunk_barriers: bool) -> Res<Written> {
        let t = Instant::now();
        let len = self.smd.len();
        let s = File::open(&self.source).map_err(io_err(Phase::Copying))?;
        let _ = self.p.uncached.set_uncached(&s);
        let (d, mut h, mut off) = match resume {
            Some(r) if self.ours => {
                let d = OpenOptions::new()
                    .write(true)
                    .open(&self.partial)
                    .map_err(io_err(Phase::Copying))?;
                d.set_len(r.len).map_err(io_err(Phase::Copying))?;
                self.chunks_reused = r.chunks_reused;
                self.logged = r.chunks_reused;
                (d, r.hasher, r.len)
            }
            _ => {
                if self.ours {
                    let _ = self.discard();
                }
                let d = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&self.partial)
                    .map_err(io_err(Phase::Copying))?;
                self.logged = 0;
                (d, ChunkHasher::new(self.opts.chunk_size), 0)
            }
        };
        self.ours = true;
        // The eviction before verifying is the real guarantee; this only keeps
        // our writes out of the cache in the first place.
        let _ = self.p.uncached.set_uncached(&d);
        let per_chunk = chunk_barriers && self.opts.flush_each_chunk && len > h.chunk_size();
        let mut buf = vec![0u8; buf_len(len)];
        let mut holes = false;
        self.thr.report(Phase::Copying, off, len);
        while off < len {
            self.check()?;
            self.fault(FaultPointArg::MidChunk(off))?;
            let want = ((len - off) as usize).min(IO_BUF);
            let n = match s.read_at(&mut buf[..want], off) {
                Ok(0) => return Err(CopyError::SourceChanged),
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(io_err(Phase::Copying)(e)),
            };
            let block = &buf[..n];
            if touches_hole(&s, off, n as u64) {
                // Write only the pieces that aren't zeros: the bytes were read,
                // so a skipped piece is known to be zeros, not assumed to be.
                for (i, piece) in block.chunks(HOLE_GRAIN).enumerate() {
                    if piece.iter().all(|b| *b == 0) {
                        holes = true;
                    } else {
                        d.write_all_at(piece, off + (i * HOLE_GRAIN) as u64)
                            .map_err(io_err(Phase::Copying))?;
                    }
                }
            } else {
                d.write_all_at(block, off).map_err(io_err(Phase::Copying))?;
            }
            off += n as u64;
            let completed = h.update(block);
            self.thr.report(Phase::Copying, off, len);
            if completed > 0 && per_chunk {
                self.chunk_flush(&d, &h)?;
            }
        }
        if holes || d.metadata().map_or(true, |m| m.len() != len) {
            d.set_len(len).map_err(io_err(Phase::Copying))?;
        }
        self.copy_time += t.elapsed();
        drop(d);
        Ok(Written {
            hashes: h.finish(),
            holes,
            file: None,
        })
    }

    /// A drive flush after one or more chunks completed, then each of them
    /// is logged, in order (one 4 MiB write can complete several small chunks).
    fn chunk_flush(&mut self, d: &File, h: &ChunkHasher) -> Res<()> {
        let last = h.chunks_done() - 1;
        self.fault(FaultPointArg::BeforeFlush(Some(last)))?;
        self.flush_file(d)?;
        self.fault(FaultPointArg::AfterFlush(Some(last)))?;
        for i in self.logged..=last {
            self.log.chunk_flushed(i, h.chunks()[i]);
        }
        self.logged = last + 1;
        Ok(())
    }

    /// Evicts the partial from the cache, checks that nothing of it is left
    /// there, and hashes it from a fresh uncached descriptor. Returns the
    /// hashes and whether the read provably came from the drive.
    fn read_back(&mut self, len: u64) -> Res<(FileHashes, bool)> {
        let t = Instant::now();
        self.thr.report(Phase::Verifying, 0, len);
        let f = File::open(&self.partial).map_err(io_err(Phase::Verifying))?;
        let mut from_device = false;
        for _ in 0..2 {
            match self.p.uncached.evict_open(&f, &self.partial) {
                Ok(r) if r.resident_pages == 0 => {
                    from_device = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        let h = self.hash_open(&f, len, Phase::Verifying)?;
        self.verify_time += t.elapsed();
        Ok((h, from_device))
    }

    /// Hashes a file from a fresh uncached descriptor.
    fn hash(&mut self, path: &Path, len: u64, phase: Phase) -> Res<FileHashes> {
        let f = File::open(path).map_err(io_err(phase))?;
        self.hash_open(&f, len, phase)
    }

    /// Hashes an open file from its start, uncached.
    fn hash_open(&mut self, f: &File, len: u64, phase: Phase) -> Res<FileHashes> {
        let _ = self.p.uncached.set_uncached(f);
        let mut h = ChunkHasher::new(self.opts.chunk_size);
        let mut buf = vec![0u8; buf_len(len)];
        let mut off = 0u64;
        self.thr.report(phase, 0, len);
        loop {
            self.check()?;
            if phase == Phase::Verifying {
                self.fault(FaultPointArg::MidVerify(off))?;
            }
            let n = match f.read_at(&mut buf, off) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(io_err(phase)(e)),
            };
            h.update(&buf[..n]);
            off += n as u64;
            self.thr.report(phase, off.min(len), len);
        }
        Ok(h.finish())
    }

    /// Exclusive rename to the real name; with `dir_barrier`, then a folder
    /// flush (a group does one folder flush for all its files instead).
    pub(crate) fn place(&mut self, dir_barrier: bool) -> Res<()> {
        self.check()?;
        self.thr.report(Phase::Renaming, 0, 0);
        self.fault(FaultPointArg::BeforeRename)?;
        if self.replace && std::fs::symlink_metadata(&self.req.dest).is_ok() {
            // The new copy is complete, flushed and verified: only now does
            // the old item go to the Trash (Put Back works), then the name
            // is ours. The journal knows the copy's hash first.
            if let Some(w) = &self.written {
                self.log.before_replace(&w.hashes);
            }
            self.fault(FaultPointArg::BeforeReplaceTrash)?;
            let loc = self
                .p
                .trash
                .trash(&self.req.dest)
                .map_err(|e| CopyError::Io {
                    phase: Phase::Renaming,
                    message: format!(
                        "the existing item couldn't be moved to the Trash, so it was kept: {}",
                        describe(&e)
                    ),
                    source: e,
                })?;
            self.log.replaced(&loc);
            self.replaced = Some(loc);
        }
        self.rename = match self
            .p
            .rename
            .rename_exclusive(&self.partial, &self.req.dest)
        {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                return Err(CopyError::Conflict(self.req.dest.clone()));
            }
            Err(e) => return Err(io_err(Phase::Renaming)(e)),
        };
        // It has its real name now; nothing of ours is left to remove.
        self.ours = false;
        self.fault(FaultPointArg::AfterRename)?;
        if dir_barrier {
            self.flush_dir()?;
        }
        Ok(())
    }

    /// After the rename (and its folder flush): ACLs and immutable flags,
    /// the metadata check, and the report.
    pub(crate) fn finish(mut self) -> FileReport {
        self.thr.report(Phase::Metadata, 0, 0);
        if let Err(e) = self.p.meta.apply_security(&self.source, &self.req.dest) {
            self.lost.push(MetaLoss::Failed(format!(
                "permissions or flags couldn't be applied: {}",
                describe(&e)
            )));
        }
        match (
            self.p.meta.snapshot(&self.source),
            self.p.meta.snapshot(&self.req.dest),
        ) {
            (Ok(s), Ok(d)) if !self.is_link => self.lost.extend(metadata_lost(&s, &d)),
            (Ok(_), Ok(_)) => {}
            (_, Err(e)) | (Err(e), _) => self.lost.push(MetaLoss::Failed(format!(
                "its metadata couldn't be checked: {}",
                describe(&e)
            ))),
        }
        let holes = self.written.as_ref().is_some_and(|w| w.holes);
        if holes && !matches!(self.dest_fs.as_str(), "apfs" | "hfs") {
            self.lost.push(MetaLoss::Sparseness);
        }
        let (outcome, bytes, hashes) = match self.written.take() {
            _ if self.is_link => (Outcome::Link, 0, FileHashes::default()),
            None => (Outcome::Cloned, self.smd.len(), FileHashes::default()),
            Some(w) if self.from_device => (Outcome::Verified, self.smd.len(), w.hashes),
            Some(w) => (Outcome::VerifiedFromMemory, self.smd.len(), w.hashes),
        };
        let mut lost = std::mem::take(&mut self.lost);
        lost.append(&mut self.warnings);
        FileReport {
            source: self.source.clone(),
            dest: self.req.dest.clone(),
            outcome,
            bytes,
            hashes,
            source_reread: self.reread,
            flush_level: self.flush,
            rename: self.rename,
            metadata_lost: lost,
            retries: self.retries,
            chunks_reused: self.chunks_reused as u32,
            replaced_trash: self.replaced.take(),
            copy_time: self.copy_time,
            flush_time: self.flush_time,
            verify_time: self.verify_time,
            reread_time: self.reread_time,
        }
    }
}

/// Flags worth reporting when lost: no-dump, locked, append-only, hidden.
const KEPT_FLAGS: u32 = 0x1 | 0x2 | 0x4 | 0x8000;
/// Dates closer than this count as kept (FAT stores 2-second steps).
const DATE_SLACK_NS: i64 = 2_000_000_000;

/// What `dest` didn't keep of `src`'s metadata. Pure.
pub fn metadata_lost(src: &MetaSnapshot, dest: &MetaSnapshot) -> Vec<MetaLoss> {
    let mut lost = Vec::new();
    if src.mode != dest.mode {
        lost.push(MetaLoss::Permissions);
    }
    if src.uid != dest.uid {
        lost.push(MetaLoss::Owner);
    }
    if (src.mtime_ns - dest.mtime_ns).abs() > DATE_SLACK_NS {
        lost.push(MetaLoss::ModifiedDate);
    }
    if let Some(b) = src.birth_ns
        && dest.birth_ns.is_none_or(|d| (b - d).abs() > DATE_SLACK_NS)
    {
        lost.push(MetaLoss::CreatedDate);
    }
    let missing: Vec<String> = src
        .xattrs
        .iter()
        .filter(|n| !dest.xattrs.contains(n))
        .cloned()
        .collect();
    if !missing.is_empty() {
        lost.push(MetaLoss::Xattrs(missing));
    }
    if src.acl && !dest.acl {
        lost.push(MetaLoss::Acl);
    }
    if src.flags & KEPT_FLAGS & !dest.flags != 0 {
        lost.push(MetaLoss::Flags);
    }
    lost
}

/// The fault point to check, built without the `fault-injection` feature too
/// (its fields are only read when the feature is on).
#[derive(Clone, Copy)]
#[cfg_attr(not(feature = "fault-injection"), allow(dead_code))]
enum FaultPointArg {
    BeforeCopy,
    MidChunk(u64),
    BeforeFlush(Option<usize>),
    AfterFlush(Option<usize>),
    MidVerify(u64),
    BeforeReread,
    BeforeRename,
    AfterRename,
    BeforeReplaceTrash,
}

#[cfg(feature = "fault-injection")]
impl From<FaultPointArg> for crate::fault::FaultPoint {
    fn from(a: FaultPointArg) -> Self {
        use crate::fault::FaultPoint as P;
        match a {
            FaultPointArg::BeforeCopy => P::BeforeCopy,
            FaultPointArg::MidChunk(offset) => P::MidChunk { offset },
            FaultPointArg::BeforeFlush(chunk) => P::BeforeFlush { chunk },
            FaultPointArg::AfterFlush(chunk) => P::AfterFlush { chunk },
            FaultPointArg::MidVerify(offset) => P::MidVerify { offset },
            FaultPointArg::BeforeReread => P::BeforeReread,
            FaultPointArg::BeforeRename => P::BeforeRename,
            FaultPointArg::AfterRename => P::AfterRename,
            FaultPointArg::BeforeReplaceTrash => P::BeforeReplaceTrash,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> MetaSnapshot {
        MetaSnapshot {
            mode: 0o644,
            uid: 501,
            mtime_ns: 1_700_000_000_000_000_000,
            birth_ns: Some(1_700_000_000_000_000_000),
            xattrs: vec!["com.apple.metadata:_kMDItemUserTags".into()],
            acl: true,
            flags: 0x8000,
        }
    }

    #[test]
    fn nothing_lost_when_identical() {
        assert!(metadata_lost(&snap(), &snap()).is_empty());
    }

    #[test]
    fn an_exfat_like_destination_reports_each_loss() {
        let d = MetaSnapshot {
            mode: 0o777,
            uid: 99,
            mtime_ns: snap().mtime_ns + 1_000_000_000, // rounded: fine
            birth_ns: None,
            xattrs: vec![],
            acl: false,
            flags: 0,
        };
        let lost = metadata_lost(&snap(), &d);
        assert_eq!(
            lost,
            vec![
                MetaLoss::Permissions,
                MetaLoss::Owner,
                MetaLoss::CreatedDate,
                MetaLoss::Xattrs(vec!["com.apple.metadata:_kMDItemUserTags".into()]),
                MetaLoss::Acl,
                MetaLoss::Flags,
            ]
        );
    }

    #[test]
    fn dates_beyond_two_seconds_are_lost() {
        let mut d = snap();
        d.mtime_ns += 3_000_000_000;
        assert_eq!(metadata_lost(&snap(), &d), vec![MetaLoss::ModifiedDate]);
    }

    #[test]
    fn extra_xattrs_on_the_destination_are_fine() {
        let mut d = snap();
        d.xattrs.push("com.apple.provenance".into());
        assert!(metadata_lost(&snap(), &d).is_empty());
    }
}

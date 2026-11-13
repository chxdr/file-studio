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
struct Ident {
    dev: u64,
    ino: u64,
    size: u64,
    mtime_ns: i64,
}

fn ident(md: &std::fs::Metadata) -> Ident {
    Ident {
        dev: md.dev(),
        ino: md.ino(),
        size: md.len(),
        mtime_ns: md.mtime() * 1_000_000_000 + md.mtime_nsec(),
    }
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
    let mut run = Run {
        p: platform,
        req,
        opts,
        ctl,
        thr: Throttle::new(sink, opts.progress_interval),
        owner: PartialOwner {
            final_path: req.dest.clone(),
            job: req.job,
            n: req.index,
        },
        partial: req.partial_path(),
        ours: false,
        flush: FlushLevel::Device,
        copy_time: Duration::ZERO,
        flush_time: Duration::ZERO,
        verify_time: Duration::ZERO,
        reread_time: Duration::ZERO,
        warnings: Vec::new(),
    };
    match run.go() {
        Ok(r) => Ok(r),
        Err(error) => {
            let partial_left = run.discard_partial();
            Err(Failure {
                error,
                partial_left,
            })
        }
    }
}

struct Run<'a> {
    p: &'a Platform,
    req: &'a CopyRequest,
    opts: &'a Options,
    ctl: &'a Control,
    thr: Throttle<'a>,
    owner: PartialOwner,
    partial: PathBuf,
    /// The partial exists and is ours to remove.
    ours: bool,
    flush: FlushLevel,
    copy_time: Duration,
    flush_time: Duration,
    verify_time: Duration,
    reread_time: Duration,
    warnings: Vec<MetaLoss>,
}

type Res<T> = Result<T, CopyError>;

/// What the data path produced.
struct Written {
    hashes: FileHashes,
    holes: bool,
}

impl Run<'_> {
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

    /// Removes our partial after a failure. Returns it if it couldn't be removed.
    fn discard_partial(&mut self) -> Option<PathBuf> {
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

    fn dest_dir(&self) -> &Path {
        self.req.dest.parent().unwrap_or(Path::new("/"))
    }

    fn flushed(&mut self, level: FlushLevel) {
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

    fn go(&mut self) -> Res<FileReport> {
        self.thr.report(Phase::Preparing, 0, 0);
        let mut source = self.req.source.clone();
        let mut smd = std::fs::symlink_metadata(&source).map_err(io_err(Phase::Preparing))?;
        if smd.file_type().is_symlink() && self.opts.symlinks == Symlinks::Follow {
            source = std::fs::canonicalize(&source).map_err(io_err(Phase::Preparing))?;
            smd = std::fs::symlink_metadata(&source).map_err(io_err(Phase::Preparing))?;
        }
        let ft = smd.file_type();
        if ft.is_dir() {
            return Err(CopyError::Unsupported(
                "folders are copied as multi-file jobs (Safe Transfer M2)".into(),
            ));
        }
        if !ft.is_file() && !ft.is_symlink() {
            return Err(CopyError::Unsupported(
                "pipes, sockets and devices can't be copied".into(),
            ));
        }
        if ft.is_file()
            && crate::sys::file_flags(&smd) & SF_DATALESS != 0
            && !self.opts.allow_dataless
        {
            return Err(CopyError::Skipped(
                "not downloaded from iCloud; skipped so it isn't downloaded".into(),
            ));
        }
        let dmd = std::fs::metadata(self.dest_dir()).map_err(io_err(Phase::Preparing))?;
        if !dmd.is_dir() {
            return Err(CopyError::Unsupported(
                "the destination is not a folder".into(),
            ));
        }
        if std::fs::symlink_metadata(&self.req.dest).is_ok() {
            return Err(CopyError::Conflict(self.req.dest.clone()));
        }
        if std::fs::symlink_metadata(&self.partial).is_ok() {
            // Ours by name, but not from this run: never removed here.
            return Err(CopyError::Io {
                phase: Phase::Preparing,
                message: format!(
                    "a partial file from an earlier attempt is in the way: {}",
                    self.partial.display()
                ),
                source: io::ErrorKind::AlreadyExists.into(),
            });
        }
        self.check()?;
        self.fault(FaultPointArg::BeforeCopy)?;

        if ft.is_symlink() {
            return self.copy_link(&source);
        }
        let same_volume = dmd.dev() == smd.dev();
        if same_volume
            && self.opts.clone_same_volume
            && let Some(report) = self.try_clone(&source, &smd)?
        {
            return Ok(report);
        }
        self.copy_data(&source, &smd)
    }

    /// A symlink is copied as a link: same target text, never followed.
    fn copy_link(&mut self, source: &Path) -> Res<FileReport> {
        let target = std::fs::read_link(source).map_err(io_err(Phase::Copying))?;
        self.thr.report(Phase::Copying, 0, 0);
        std::os::unix::fs::symlink(&target, &self.partial).map_err(io_err(Phase::Copying))?;
        self.ours = true;
        let back = std::fs::read_link(&self.partial).map_err(io_err(Phase::Verifying))?;
        if back != target {
            return Err(CopyError::VerifyFailed);
        }
        self.flush_dir()?;
        let rename = self.place()?;
        Ok(self.report(
            source,
            Outcome::Link,
            0,
            FileHashes::default(),
            false,
            rename,
            0,
            Vec::new(),
        ))
    }

    /// Same APFS volume: an instant clone. `None` when the volume can't clone.
    fn try_clone(&mut self, source: &Path, smd: &std::fs::Metadata) -> Res<Option<FileReport>> {
        self.thr.report(Phase::Copying, 0, smd.len());
        match self.p.clone.clone_file(source, &self.partial) {
            Ok(()) => self.ours = true,
            Err(e)
                if matches!(
                    e.raw_os_error(),
                    Some(libc::ENOTSUP | libc::EXDEV | libc::EINVAL | libc::ENOSYS)
                ) || e.kind() == io::ErrorKind::Unsupported =>
            {
                if std::fs::symlink_metadata(&self.partial).is_ok() {
                    self.ours = true;
                    let _ = self.discard_partial();
                }
                return Ok(None);
            }
            Err(e) => return Err(io_err(Phase::Copying)(e)),
        }
        self.thr.report(Phase::Copying, smd.len(), smd.len());
        let mut lost = Vec::new();
        if let Err(e) = self.p.meta.clear_security(&self.partial) {
            lost.push(MetaLoss::Failed(format!(
                "flags couldn't be cleared: {}",
                describe(&e)
            )));
        }
        self.thr.report(Phase::Flushing, 0, 0);
        let f = File::open(&self.partial).map_err(io_err(Phase::Flushing))?;
        self.flush_file(&f)?;
        self.source_unchanged(source, smd)?;
        let rename = self.place()?;
        lost.extend(self.finish_metadata(source));
        Ok(Some(self.report(
            source,
            Outcome::Cloned,
            smd.len(),
            FileHashes::default(),
            false,
            rename,
            0,
            lost,
        )))
    }

    /// The byte-for-byte path, with up to one retry when the read-back shows
    /// the copy (not the original) was wrong.
    fn copy_data(&mut self, source: &Path, smd: &std::fs::Metadata) -> Res<FileReport> {
        let len = smd.len();
        let dest_fs = self
            .p
            .volumes
            .volume_of(self.dest_dir())
            .map(|v| v.fs_type)
            .unwrap_or_default();
        let mut retries = 0;
        loop {
            if self.opts.reread_source {
                // Both reads of the original should come from the disk.
                let _ = self.p.uncached.evict(source);
            }
            let written = self.write_partial(source, len)?;
            let mut lost = Vec::new();
            if let Err(e) = self.p.meta.copy_metadata(source, &self.partial) {
                lost.push(MetaLoss::Failed(format!(
                    "extended attributes and dates couldn't be copied: {}",
                    describe(&e)
                )));
            }
            if let Err(e) = self.p.meta.clear_security(&self.partial) {
                lost.push(MetaLoss::Failed(format!(
                    "flags couldn't be cleared: {}",
                    describe(&e)
                )));
            }
            self.thr.report(Phase::Flushing, 0, 0);
            self.fault(FaultPointArg::BeforeFlush(None))?;
            let f = File::open(&self.partial).map_err(io_err(Phase::Flushing))?;
            self.flush_file(&f)?;
            drop(f);
            self.fault(FaultPointArg::AfterFlush(None))?;
            self.source_unchanged(source, smd)?;

            let (back, from_device) = self.read_back(len)?;
            if back != written.hashes {
                // Whose fault? Read the original again.
                let _ = self.p.uncached.evict(source);
                let again = self.hash(source, len, Phase::Rereading)?;
                if again != written.hashes {
                    return Err(CopyError::SourceUnstable);
                }
                let _ = self.discard_partial();
                retries += 1;
                if retries > 1 {
                    return Err(CopyError::VerifyFailed);
                }
                continue;
            }
            let mut reread = false;
            if self.opts.reread_source {
                self.fault(FaultPointArg::BeforeReread)?;
                let evicted = self
                    .p
                    .uncached
                    .evict(source)
                    .is_ok_and(|r| r.resident_pages == 0);
                if !evicted {
                    self.warnings.push(MetaLoss::Failed(
                        "the original couldn't be dropped from memory, so the second read may have come from RAM"
                            .into(),
                    ));
                }
                let t = Instant::now();
                let second = self.hash(source, len, Phase::Rereading)?;
                self.reread_time += t.elapsed();
                if second != written.hashes {
                    return Err(CopyError::SourceUnstable);
                }
                self.source_unchanged(source, smd)?;
                reread = evicted;
            }

            let rename = self.place()?;
            lost.extend(self.finish_metadata(source));
            if written.holes && !matches!(dest_fs.as_str(), "apfs" | "hfs") {
                lost.push(MetaLoss::Sparseness);
            }
            let outcome = if from_device {
                Outcome::Verified
            } else {
                Outcome::VerifiedFromMemory
            };
            return Ok(self.report(
                source,
                outcome,
                len,
                written.hashes,
                reread,
                rename,
                retries,
                lost,
            ));
        }
    }

    fn source_unchanged(&self, source: &Path, before: &std::fs::Metadata) -> Res<()> {
        let now = std::fs::symlink_metadata(source).map_err(io_err(Phase::Verifying))?;
        if ident(&now) != ident(before) {
            return Err(CopyError::SourceChanged);
        }
        Ok(())
    }

    /// Writes the partial with `F_NOCACHE`, hashing every byte from the
    /// original as it goes and keeping holes. Flushes after each completed
    /// chunk of files bigger than one chunk.
    fn write_partial(&mut self, source: &Path, len: u64) -> Res<Written> {
        let t = Instant::now();
        let s = File::open(source).map_err(io_err(Phase::Copying))?;
        let _ = self.p.uncached.set_uncached(&s);
        let d = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&self.partial)
            .map_err(io_err(Phase::Copying))?;
        self.ours = true;
        // The eviction before verifying is the real guarantee; this only keeps
        // our writes out of the cache in the first place.
        let _ = self.p.uncached.set_uncached(&d);
        let mut h = ChunkHasher::new(self.opts.chunk_size);
        let per_chunk = self.opts.flush_each_chunk && len > h.chunk_size();
        let mut buf = vec![0u8; IO_BUF];
        let mut holes = false;
        let mut off = 0u64;
        self.thr.report(Phase::Copying, 0, len);
        while off < len {
            self.check()?;
            let data = match crate::sys::seek_data(&s, off) {
                Ok(v) => v.unwrap_or(len).min(len),
                Err(e) if e.raw_os_error() == Some(libc::EINVAL) => off,
                Err(e) => return Err(io_err(Phase::Copying)(e)),
            };
            if data > off {
                holes = true;
                if h.update_zeros(data - off) > 0 && per_chunk {
                    self.chunk_flush(&d, h.chunks_done() - 1)?;
                }
                off = data;
            }
            if off >= len {
                break;
            }
            let hole = match crate::sys::seek_hole(&s, off) {
                Ok(v) => v.unwrap_or(len).min(len),
                Err(e) if e.raw_os_error() == Some(libc::EINVAL) => len,
                Err(e) => return Err(io_err(Phase::Copying)(e)),
            };
            while off < hole {
                self.check()?;
                self.fault(FaultPointArg::MidChunk(off))?;
                let want = ((hole - off) as usize).min(IO_BUF);
                let n = match s.read_at(&mut buf[..want], off) {
                    Ok(0) => return Err(CopyError::SourceChanged),
                    Ok(n) => n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(io_err(Phase::Copying)(e)),
                };
                d.write_all_at(&buf[..n], off)
                    .map_err(io_err(Phase::Copying))?;
                off += n as u64;
                let completed = h.update(&buf[..n]);
                self.thr.report(Phase::Copying, off, len);
                if completed > 0 && per_chunk {
                    self.chunk_flush(&d, h.chunks_done() - 1)?;
                }
            }
        }
        d.set_len(len).map_err(io_err(Phase::Copying))?;
        self.copy_time += t.elapsed();
        Ok(Written {
            hashes: h.finish(),
            holes,
        })
    }

    fn chunk_flush(&mut self, d: &File, chunk: usize) -> Res<()> {
        self.fault(FaultPointArg::BeforeFlush(Some(chunk)))?;
        self.flush_file(d)?;
        self.fault(FaultPointArg::AfterFlush(Some(chunk)))
    }

    /// Evicts the partial from the cache, checks that nothing of it is left
    /// there, and hashes it from a fresh uncached descriptor. Returns the
    /// hashes and whether the read provably came from the drive.
    fn read_back(&mut self, len: u64) -> Res<(FileHashes, bool)> {
        let t = Instant::now();
        self.thr.report(Phase::Verifying, 0, len);
        let mut from_device = false;
        for _ in 0..2 {
            match self.p.uncached.evict(&self.partial) {
                Ok(r) if r.resident_pages == 0 => {
                    from_device = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        let h = self.hash(&self.partial.clone(), len, Phase::Verifying)?;
        self.verify_time += t.elapsed();
        Ok((h, from_device))
    }

    /// Hashes a file from a fresh uncached descriptor.
    fn hash(&mut self, path: &Path, len: u64, phase: Phase) -> Res<FileHashes> {
        let f = File::open(path).map_err(io_err(phase))?;
        let _ = self.p.uncached.set_uncached(&f);
        let mut h = ChunkHasher::new(self.opts.chunk_size);
        let mut buf = vec![0u8; IO_BUF];
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

    /// Exclusive rename to the real name, then a folder flush.
    fn place(&mut self) -> Res<RenameMode> {
        self.check()?;
        self.thr.report(Phase::Renaming, 0, 0);
        self.fault(FaultPointArg::BeforeRename)?;
        let mode = match self
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
        self.flush_dir()?;
        Ok(mode)
    }

    /// After the rename: ACLs and immutable flags, then what was kept.
    fn finish_metadata(&mut self, source: &Path) -> Vec<MetaLoss> {
        self.thr.report(Phase::Metadata, 0, 0);
        let mut lost = Vec::new();
        if let Err(e) = self.p.meta.apply_security(source, &self.req.dest) {
            lost.push(MetaLoss::Failed(format!(
                "permissions or flags couldn't be applied: {}",
                describe(&e)
            )));
        }
        match (
            self.p.meta.snapshot(source),
            self.p.meta.snapshot(&self.req.dest),
        ) {
            (Ok(s), Ok(d)) => lost.extend(metadata_lost(&s, &d)),
            (_, Err(e)) | (Err(e), _) => lost.push(MetaLoss::Failed(format!(
                "its metadata couldn't be checked: {}",
                describe(&e)
            ))),
        }
        lost
    }

    #[allow(clippy::too_many_arguments)]
    fn report(
        &mut self,
        source: &Path,
        outcome: Outcome,
        bytes: u64,
        hashes: FileHashes,
        source_reread: bool,
        rename: RenameMode,
        retries: u32,
        mut lost: Vec<MetaLoss>,
    ) -> FileReport {
        lost.append(&mut self.warnings);
        FileReport {
            source: source.to_path_buf(),
            dest: self.req.dest.clone(),
            outcome,
            bytes,
            hashes,
            source_reread,
            flush_level: self.flush,
            rename,
            metadata_lost: lost,
            retries,
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

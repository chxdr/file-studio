//! The per-item engine: one item of an op, from the journal row to the disk
//! and back. Every state change is saved to the journal *before* the disk
//! step it describes.
//!
//! A file goes: `pending` → `in_progress` (writing `.<name>.fstudio-partial`)
//! → `copied` (partial complete, its inode recorded) → `verified` (optional)
//! → rename with `RENAME_EXCL` → `placed` → (move: remove the source) → `done`.
//!
//! A folder goes: `pending` → `in_progress` (an empty partial folder, renamed
//! into place at once) → `placed` + its children journaled → children run →
//! finalize (metadata; for a move, `rmdir` the now-empty source) → `done`.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{DirBuilderExt, FileExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use fstudio_model::EntryKind;
use parking_lot::{Mutex, RwLock};
use xxhash_rust::xxh3::Xxh3;

use crate::journal::{ItemRow, ItemState, Journal, NewItem};
use crate::meta::{self, IMMUTABLE_FLAGS};
use crate::names::{self, is_within, partial_path, renaming_path};
use crate::remove::{self, DeleteGate, Placed};
use crate::sys::{self, CopyProgress};
use crate::trash::Trasher;
use crate::types::{
    ConflictDecision, ConflictId, ConflictInfo, ConflictRules, FaultStep, OpId, OpKind, OpsConfig,
};

/// Read/write chunk for the verifying copy loop.
const CHUNK: usize = 4 << 20;

/// Live controls and counters shared by an op's scheduler and its workers.
pub struct OpCtl {
    pub paused: AtomicBool,
    pub cancel: AtomicBool,
    pub bytes_done: AtomicU64,
    pub rules: RwLock<ConflictRules>,
    /// Source folders already expanded, by `(dev, ino)`, to stop folder loops.
    pub visited: Mutex<HashSet<(u64, u64)>>,
    pub current: Mutex<Option<PathBuf>>,
}

impl OpCtl {
    pub fn new(rules: ConflictRules) -> Self {
        Self {
            paused: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            bytes_done: AtomicU64::new(0),
            rules: RwLock::new(rules),
            visited: Mutex::new(HashSet::new()),
            current: Mutex::new(None),
        }
    }

    /// Blocks while paused. Returns `false` if the op was cancelled.
    pub fn wait_while_paused(&self) -> bool {
        while self.paused.load(Ordering::Acquire) && !self.cancelled() {
            std::thread::sleep(Duration::from_millis(5));
        }
        !self.cancelled()
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
}

/// Everything the engine needs for one op.
pub struct Env<'a> {
    pub journal: &'a Journal,
    pub cfg: &'a OpsConfig,
    pub trasher: &'a dyn Trasher,
    pub ctl: &'a OpCtl,
    pub op: OpId,
    pub kind: OpKind,
    pub allow_dataless: bool,
    /// Devices where cloning failed once, so we don't try again.
    pub no_clone: &'a Mutex<HashSet<u64>>,
}

/// One unit of work for a worker.
pub enum Task {
    /// Run (or continue) an item. `parent_dest` is the parent folder's final
    /// destination for items inside a folder.
    Run {
        item: ItemRow,
        parent_dest: Option<PathBuf>,
        decision: Option<ConflictDecision>,
    },
    /// All children of a placed folder are finished.
    Finalize { item: ItemRow, children_ok: bool },
}

/// What happened to an item.
pub enum Outcome {
    /// Done, failed or skipped; saved.
    Finished(ItemRow),
    /// A folder is placed and its children are journaled.
    Expanded(ItemRow, Vec<ItemRow>),
    /// The destination name is taken; waiting for a decision. Saved as `conflict`.
    Conflict(ItemRow, ConflictInfo),
    /// Cancelled before it finished; saved as `pending`.
    Cancelled(ItemRow),
}

type Res<T> = std::result::Result<T, Stop>;

/// Why an item stopped early.
enum Stop {
    Fail(String),
    Skip(Option<String>),
    Conflict(ConflictInfo),
    Cancelled,
}

impl From<io::Error> for Stop {
    fn from(e: io::Error) -> Self {
        if e.raw_os_error() == Some(libc::ECANCELED) {
            return Stop::Cancelled;
        }
        Stop::Fail(describe(&e))
    }
}

/// A user-facing description of an I/O error.
pub fn describe(e: &io::Error) -> String {
    match e.raw_os_error() {
        Some(libc::ENOSPC) => "there isn't enough free space".into(),
        Some(libc::EACCES) | Some(libc::EPERM) => "permission denied".into(),
        Some(libc::EROFS) => "the volume is read-only".into(),
        Some(libc::ENOENT) => "the item no longer exists".into(),
        Some(libc::ENAMETOOLONG) => "the name is too long".into(),
        Some(libc::EDQUOT) => "the disk quota is exceeded".into(),
        _ => e.to_string(),
    }
}

fn fail(msg: impl Into<String>) -> Stop {
    Stop::Fail(msg.into())
}

impl Env<'_> {
    fn save(&self, it: &ItemRow) -> Res<()> {
        self.journal
            .save_item(it)
            .map_err(|e| fail(format!("journal write failed: {e}")))
    }

    fn fault(&self, step: FaultStep, path: &Path) -> io::Result<()> {
        match &self.cfg.faults {
            Some(f) => f.check(step, path),
            None => Ok(()),
        }
    }

    fn is_move(&self) -> bool {
        matches!(self.kind, OpKind::Move | OpKind::MoveTo | OpKind::Rename)
    }

    /// Runs one task to an outcome. Never panics on I/O errors; they become
    /// per-item failures.
    pub fn run(&self, task: Task) -> Outcome {
        match task {
            Task::Run {
                mut item,
                parent_dest,
                decision,
            } => {
                *self.ctl.current.lock() = Some(item.source.clone());
                let r = self.run_item(&mut item, parent_dest.as_deref(), decision);
                self.settle(item, r)
            }
            Task::Finalize {
                mut item,
                children_ok,
            } => {
                let r = self.finalize_dir(&mut item, children_ok);
                item.state = ItemState::Done;
                self.settle(item, r.map(|()| Vec::new()))
            }
        }
    }

    /// Turns a result into an outcome and journals the final state.
    fn settle(&self, mut item: ItemRow, r: Res<Vec<ItemRow>>) -> Outcome {
        let (state, out) = match r {
            Ok(children) if item.state == ItemState::Placed && item.kind == EntryKind::Dir => {
                return Outcome::Expanded(item, children);
            }
            Ok(_) => (ItemState::Done, None),
            Err(Stop::Fail(msg)) => {
                item.error = Some(msg);
                (ItemState::Failed, None)
            }
            Err(Stop::Skip(msg)) => {
                item.error = msg;
                (ItemState::Skipped, None)
            }
            Err(Stop::Conflict(info)) => (ItemState::Conflict, Some(Ok(info))),
            Err(Stop::Cancelled) => (ItemState::Pending, Some(Err(()))),
        };
        item.state = state;
        if state != ItemState::Done {
            // Anything we wrote for an unfinished item is gone by now.
            item.partial = None;
        }
        if let Err(Stop::Fail(m)) = self.save(&item) {
            item.state = ItemState::Failed;
            item.error = Some(m);
        }
        match out {
            Some(Ok(info)) => Outcome::Conflict(item, info),
            Some(Err(())) => Outcome::Cancelled(item),
            None => Outcome::Finished(item),
        }
    }

    fn run_item(
        &self,
        item: &mut ItemRow,
        parent_dest: Option<&Path>,
        decision: Option<ConflictDecision>,
    ) -> Res<Vec<ItemRow>> {
        if !self.ctl.wait_while_paused() {
            return Err(Stop::Cancelled);
        }
        if item.state == ItemState::Placed {
            return self.continue_placed(item);
        }
        match self.kind {
            OpKind::Copy | OpKind::Move | OpKind::Duplicate | OpKind::MoveTo | OpKind::Rename => {
                self.transfer(item, parent_dest, decision)
            }
            OpKind::Trash => self.trash(item).map(|()| Vec::new()),
            OpKind::DeleteImmediately => self.delete(item).map(|()| Vec::new()),
            OpKind::NewFolder | OpKind::NewFile => self.create(item, decision).map(|()| Vec::new()),
        }
    }

    // ───────────────────────── conflicts ─────────────────────────

    fn conflict_info(&self, item: &ItemRow, src: &std::fs::Metadata, dest: &Path) -> Stop {
        let dmd = match meta::lstat(dest) {
            Ok(m) => m,
            Err(e) => return e.into(),
        };
        let (sk, dk) = (meta::kind_of(src), meta::kind_of(&dmd));
        Stop::Conflict(ConflictInfo {
            op: self.op,
            id: ConflictId(item.id),
            source: item.source.clone(),
            dest: dest.to_path_buf(),
            source_kind: sk,
            dest_kind: dk,
            source_size: src.len(),
            dest_size: dmd.len(),
            source_mtime_ns: meta::mtime_ns(src),
            dest_mtime_ns: meta::mtime_ns(&dmd),
            can_merge: sk == EntryKind::Dir && dk == EntryKind::Dir && self.kind.copies(),
        })
    }

    /// The destination `dest` is taken. Applies the decision (given, or from
    /// the op's rules) and returns where to put the item and whether to merge.
    fn resolve(
        &self,
        item: &mut ItemRow,
        src: &std::fs::Metadata,
        dest: PathBuf,
        decision: Option<ConflictDecision>,
    ) -> Res<(PathBuf, bool)> {
        let dmd = meta::lstat(&dest)?;
        let both_dirs = src.is_dir() && dmd.is_dir();
        let class_folder = both_dirs && self.kind.copies();
        let decision = decision.or_else(|| self.ctl.rules.read().for_class(class_folder));
        let Some(decision) = decision else {
            return Err(self.conflict_info(item, src, &dest));
        };
        match decision {
            ConflictDecision::Skip => Err(Stop::Skip(None)),
            ConflictDecision::KeepBoth => {
                let name = dest
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let dir = dest.parent().unwrap_or(Path::new("/"));
                let free = names::keep_both_names(&name, src.is_dir())
                    .map(|n| dir.join(n))
                    .find(|p| !meta::exists(p))
                    .expect("infinite candidates");
                item.dest_name = Some(file_name(&free));
                Ok((free, false))
            }
            ConflictDecision::Merge => {
                if both_dirs && self.kind.copies() {
                    Ok((dest, true))
                } else {
                    Err(self.conflict_info(item, src, &dest))
                }
            }
            ConflictDecision::Replace => {
                if meta::ident(&dmd).same_object(&meta::ident(src)) {
                    return Err(fail("an item can't replace itself"));
                }
                if is_within(&item.source, &dest) {
                    return Err(fail("it can't replace a folder that contains it"));
                }
                // Journal before the step: we are about to trash `dest`.
                item.dest = Some(dest.clone());
                item.state = ItemState::InProgress;
                self.save(item)?;
                let loc = self.trasher.trash(&dest).map_err(|e| {
                    fail(format!(
                        "couldn't move the existing item to the Trash: {}",
                        describe(&e)
                    ))
                })?;
                item.replaced_trash = Some(loc);
                self.save(item)?;
                Ok((dest, false))
            }
        }
    }

    // ───────────────────────── copy and move ─────────────────────────

    fn transfer(
        &self,
        item: &mut ItemRow,
        parent_dest: Option<&Path>,
        decision: Option<ConflictDecision>,
    ) -> Res<Vec<ItemRow>> {
        let src = meta::lstat(&item.source)?;
        item.kind = meta::kind_of(&src);
        item.src_ident = Some(meta::ident(&src));
        let dest_dir = match (&item.dest_dir, parent_dest) {
            (Some(d), _) => d.clone(),
            (None, Some(p)) => p.to_path_buf(),
            (None, None) => return Err(fail("the item has no destination")),
        };
        if self.kind == OpKind::MoveTo && !meta::exists(&dest_dir) {
            std::fs::create_dir_all(&dest_dir)?;
        }
        let dir_md = meta::lstat(&dest_dir)?;
        if !dir_md.is_dir() {
            return Err(fail("the destination is not a folder"));
        }
        let src_name = file_name(&item.source);
        // Only the duplicated item itself gets "name copy"; what is inside a
        // duplicated folder keeps its name.
        if self.kind == OpKind::Duplicate && item.parent.is_none() && item.dest_name.is_none() {
            let free = names::duplicate_names(&src_name, src.is_dir())
                .find(|n| !meta::exists(&dest_dir.join(n)))
                .expect("infinite candidates");
            item.dest_name = Some(free);
        }
        let mut dest = dest_dir.join(item.dest_name.as_deref().unwrap_or(&src_name));
        if src.is_dir() && is_within(&dest, &item.source) {
            return Err(fail("a folder can't be copied or moved into itself"));
        }
        let same_volume = dir_md.dev() == src.dev();
        let rename_move = self.is_move() && same_volume;
        if !rename_move && meta::is_dataless(&src) && !self.allow_dataless {
            return Err(Stop::Skip(Some(
                "not downloaded from iCloud; skipped so it isn't downloaded".into(),
            )));
        }

        let mut merge = false;
        match meta::lstat(&dest) {
            Ok(dmd) => {
                let same = meta::ident(&dmd).same_object(&meta::ident(&src));
                if same && self.is_move() {
                    if file_name(&dest) == src_name {
                        return Err(Stop::Skip(Some("it is already there".into())));
                    }
                    // The same item under another case or normalisation:
                    // a case-only rename.
                    return self.case_only_rename(item, dest).map(|()| Vec::new());
                }
                let (d, m) = self.resolve(item, &src, dest, decision)?;
                dest = d;
                merge = m;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }

        if rename_move && !merge {
            return self.rename_move(item, &src, dest);
        }
        match item.kind {
            EntryKind::Dir => self.place_dir(item, dest, merge),
            EntryKind::File | EntryKind::Symlink => {
                self.copy_file(item, &src, dest, same_volume)?;
                Ok(Vec::new())
            }
            EntryKind::Other => Err(fail("pipes, sockets and devices can't be copied")),
        }
    }

    /// A same-volume move: one exclusive rename (whole folders included).
    fn rename_move(
        &self,
        item: &mut ItemRow,
        src: &std::fs::Metadata,
        dest: PathBuf,
    ) -> Res<Vec<ItemRow>> {
        item.dest = Some(dest.clone());
        item.state = ItemState::InProgress;
        self.save(item)?;
        self.fault(FaultStep::BeforePlace, &item.source)?;
        match sys::rename_excl(&item.source, &dest) {
            Ok(_) => {
                item.created = true;
                item.dest_ident = meta::lstat(&dest).ok().map(|m| meta::ident(&m));
                item.bytes = src.len();
                if item.kind != EntryKind::Dir {
                    self.ctl.bytes_done.fetch_add(src.len(), Ordering::Relaxed);
                }
                Ok(Vec::new())
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                Err(self.conflict_info(item, src, &dest))
            }
            Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
                // Same device id but a different filesystem (firmlinks): copy instead.
                match item.kind {
                    EntryKind::Dir => self.place_dir(item, dest, false),
                    _ => self.copy_file(item, src, dest, false).map(|()| Vec::new()),
                }
            }
            Err(e) => Err(e.into()),
        }
    }

    /// `File` → `file` on a case-insensitive volume (or NFC ↔ NFD): an
    /// exclusive rename to a hidden intermediate name, then to the new name.
    fn case_only_rename(&self, item: &mut ItemRow, dest: PathBuf) -> Res<()> {
        let tmp = renaming_path(&item.source);
        item.dest = Some(dest.clone());
        item.partial = Some(tmp.clone());
        item.state = ItemState::InProgress;
        self.save(item)?;
        self.fault(FaultStep::BeforePlace, &item.source)?;
        sys::rename_excl(&item.source, &tmp)?;
        if let Err(e) = sys::rename_excl(&tmp, &dest) {
            let _ = sys::rename_excl(&tmp, &item.source);
            return Err(e.into());
        }
        item.partial = None;
        item.created = true;
        item.dest_ident = meta::lstat(&dest).ok().map(|m| meta::ident(&m));
        Ok(())
    }

    /// Copies a file or symlink to `dest` through a partial file; for a move,
    /// then removes the source.
    fn copy_file(
        &self,
        item: &mut ItemRow,
        src: &std::fs::Metadata,
        dest: PathBuf,
        same_volume: bool,
    ) -> Res<()> {
        let partial = partial_path(&dest, item.id);
        if meta::exists(&partial) {
            return Err(fail(format!(
                "a temporary file from an earlier copy is in the way: {}",
                partial.display()
            )));
        }
        item.dest = Some(dest.clone());
        item.partial = Some(partial.clone());
        item.state = ItemState::InProgress;
        self.save(item)?;

        let tap = Tap::new(self.ctl);
        let result = self.write_partial(item, src, &partial, same_volume, &tap);
        let (cloned, hash) = match result {
            Ok(v) => v,
            Err(e) => {
                tap.rollback();
                let _ = remove::remove_partial(&partial);
                return Err(e);
            }
        };
        match self.finish_partial(item, src, &partial, &dest, cloned, hash) {
            Ok(()) => Ok(()),
            Err(e) => {
                // A copy that landed keeps its bytes even if a later step failed.
                if item.state != ItemState::Placed {
                    tap.rollback();
                }
                if item.partial.is_some() {
                    let _ = remove::remove_partial(&partial);
                }
                Err(e)
            }
        }
    }

    /// Writes the partial file. Returns whether it was cloned, and the source
    /// hash when the verifying loop computed one.
    fn write_partial(
        &self,
        item: &ItemRow,
        src: &std::fs::Metadata,
        partial: &Path,
        same_volume: bool,
        tap: &Tap<'_>,
    ) -> Res<(bool, Option<String>)> {
        self.fault(FaultStep::BeforeCopy, &item.source)?;
        let base = libc::COPYFILE_XATTR
            | libc::COPYFILE_STAT
            | libc::COPYFILE_EXCL
            | libc::COPYFILE_NOFOLLOW_SRC;
        if item.kind == EntryKind::Symlink {
            sys::copyfile_with_progress(&item.source, partial, base | libc::COPYFILE_DATA, tap)?;
            return Ok((false, None));
        }
        if same_volume && !self.no_clone.lock().contains(&src.dev()) {
            match sys::copyfile_with_progress(
                &item.source,
                partial,
                libc::COPYFILE_CLONE_FORCE | libc::COPYFILE_NOFOLLOW_SRC,
                tap,
            ) {
                Ok(_) => {
                    tap.add(src.len());
                    return Ok((true, None));
                }
                Err(e) if is_no_clone(&e) => {
                    self.no_clone.lock().insert(src.dev());
                    let _ = remove::remove_partial(partial);
                }
                Err(e) => return Err(e.into()),
            }
        }
        if self.cfg.verify {
            let hash = copy_loop(&item.source, partial, tap)?;
            // Data is ours; now xattrs (incl. resource fork, tags) and stat
            // (mode, times, flags). The ACL comes after the rename.
            sys::copyfile_plain(
                &item.source,
                partial,
                libc::COPYFILE_XATTR | libc::COPYFILE_STAT | libc::COPYFILE_NOFOLLOW_SRC,
            )?;
            Ok((false, Some(hash)))
        } else {
            let out = sys::copyfile_with_progress(
                &item.source,
                partial,
                base | libc::COPYFILE_DATA | libc::COPYFILE_DATA_SPARSE,
                tap,
            )?;
            tap.settle(src.len());
            Ok((out.cloned, None))
        }
    }

    /// Partial → (sync, verify) → exclusive rename → ACL and flags → move:
    /// remove the source.
    fn finish_partial(
        &self,
        item: &mut ItemRow,
        src: &std::fs::Metadata,
        partial: &Path,
        dest: &Path,
        cloned: bool,
        hash: Option<String>,
    ) -> Res<()> {
        let pmd = meta::lstat(partial)?;
        item.partial_ino = Some(meta::ident(&pmd).ino);
        item.hash = hash.clone();
        item.bytes = src.len();
        item.state = ItemState::Copied;
        self.save(item)?;
        self.fault(FaultStep::AfterCopy, &item.source)?;

        // The source must not have changed under us.
        let now = meta::ident(&meta::lstat(&item.source)?);
        if item.src_ident.is_some_and(|s| !s.unchanged(&now)) {
            return Err(fail("the original changed while it was being copied"));
        }

        // Immutable flags and ACLs (e.g. `deny delete`) would block the rename;
        // they are re-applied after it.
        let pflags = meta::flags(&pmd);
        if pflags & IMMUTABLE_FLAGS != 0 {
            sys::set_flags(partial, pflags & !IMMUTABLE_FLAGS)?;
        }
        sys::clear_acl(partial)?;

        let is_file = item.kind == EntryKind::File;
        // Durable before a move removes its source; for copies only on request.
        // Verify doesn't need it: the loop writes with F_NOCACHE, so the re-read
        // hits the device. A clone wrote nothing, so there is nothing to sync.
        let must_sync = self.is_move() || (self.cfg.fsync_copies && !cloned);
        if is_file && must_sync {
            sys::full_sync(&File::open(partial)?)?;
        }
        if self.cfg.verify && is_file && !cloned {
            let want = match hash {
                Some(h) => h,
                None => hash_file(&item.source)?,
            };
            let got = hash_file(partial)?;
            if got != want || self.fault(FaultStep::Verify, &item.source).is_err() {
                return Err(fail(
                    "verification failed: the copy doesn't match the original",
                ));
            }
            item.hash = Some(got);
            item.state = ItemState::Verified;
            self.save(item)?;
        }

        self.fault(FaultStep::BeforePlace, &item.source)?;
        match sys::rename_excl(partial, dest) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let _ = remove::remove_partial(partial);
                item.partial = None;
                return Err(self.conflict_info(item, src, dest));
            }
            Err(e) => return Err(e.into()),
        }
        item.partial = None;
        item.created = true;
        item.state = ItemState::Placed;
        item.dest_ident = meta::lstat(dest).ok().map(|m| meta::ident(&m));
        self.save(item)?;
        if let Err(e) = apply_security(&item.source, dest, meta::flags(src)) {
            item.error = Some(format!(
                "copied, but its permissions couldn't be applied: {}",
                describe(&e)
            ));
        }
        if must_sync && let Some(dir) = dest.parent() {
            let _ = sys::sync_dir(dir);
        }
        if self.is_move() {
            self.remove_source(item)?;
        }
        Ok(())
    }

    /// The last step of a cross-volume move of one file.
    fn remove_source(&self, item: &mut ItemRow) -> Res<()> {
        let keep = |e: &io::Error| {
            fail(format!(
                "copied, but the original couldn't be removed: {}",
                describe(e)
            ))
        };
        if let Err(e) = self.fault(FaultStep::BeforeSourceRemove, &item.source) {
            return Err(keep(&e));
        }
        let (Some(src_ident), Some(dest)) = (item.src_ident, item.dest.clone()) else {
            return Err(fail("the journal lost track of this item"));
        };
        let proof = Placed {
            source: item.source.clone(),
            source_ident: src_ident,
            dest,
        };
        match remove::remove_moved_source(&proof) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound && !meta::exists(&item.source) => Ok(()),
            Err(e) => Err(keep(&e)),
        }
    }

    /// Creates (or merges into) a destination folder, then journals its children.
    fn place_dir(&self, item: &mut ItemRow, dest: PathBuf, merge: bool) -> Res<Vec<ItemRow>> {
        if merge {
            item.dest = Some(dest.clone());
            item.merged = true;
            item.created = false;
        } else {
            let partial = partial_path(&dest, item.id);
            if meta::exists(&partial) {
                return Err(fail(format!(
                    "a temporary folder from an earlier copy is in the way: {}",
                    partial.display()
                )));
            }
            item.dest = Some(dest.clone());
            item.partial = Some(partial.clone());
            item.state = ItemState::InProgress;
            self.save(item)?;
            std::fs::DirBuilder::new().mode(0o700).create(&partial)?;
            item.partial_ino = meta::lstat(&partial).ok().map(|m| meta::ident(&m).ino);
            self.save(item)?;
            match sys::rename_excl(&partial, &dest) {
                Ok(_) => {}
                Err(e) => {
                    let _ = remove::remove_partial(&partial);
                    item.partial = None;
                    if e.kind() == io::ErrorKind::AlreadyExists {
                        let src = meta::lstat(&item.source)?;
                        return Err(self.conflict_info(item, &src, &dest));
                    }
                    return Err(e.into());
                }
            }
            item.partial = None;
            item.created = true;
        }
        item.dest_ident = meta::lstat(&dest).ok().map(|m| meta::ident(&m));
        item.state = ItemState::Placed;
        self.save(item)?;
        self.expand(item)
    }

    /// Lists a source folder and journals its children (one transaction).
    fn expand(&self, item: &mut ItemRow) -> Res<Vec<ItemRow>> {
        let md = meta::lstat(&item.source)?;
        let key = (meta::ident(&md).dev, meta::ident(&md).ino);
        if !self.ctl.visited.lock().insert(key) {
            return Err(fail(
                "this folder is inside itself (a folder loop); skipped",
            ));
        }
        let mut kids = Vec::new();
        for ent in std::fs::read_dir(&item.source)? {
            let ent = ent?;
            let p = ent.path();
            let Ok(cmd) = meta::lstat(&p) else { continue };
            kids.push(NewItem {
                parent: Some(item.id),
                kind: meta::kind_of(&cmd),
                source: p,
                dest_dir: None,
                dest_name: None,
                src_ident: Some(meta::ident(&cmd)),
            });
        }
        kids.sort_by(|a, b| a.source.cmp(&b.source));
        let rows = self
            .journal
            .expand(item, &kids)
            .map_err(|e| fail(format!("journal write failed: {e}")))?;
        item.expanded = true;
        Ok(rows)
    }

    /// Resumes an item that is already in place.
    fn continue_placed(&self, item: &mut ItemRow) -> Res<Vec<ItemRow>> {
        if item.kind == EntryKind::Dir {
            if item.expanded {
                return Ok(Vec::new());
            }
            return self.expand(item);
        }
        let dest = item
            .dest
            .clone()
            .ok_or_else(|| fail("the journal lost track of this item"))?;
        let dmd = meta::lstat(&dest)?;
        if let Some(d) = item.dest_ident
            && !meta::ident(&dmd).same_object(&d)
        {
            return Err(fail(
                "the copy was replaced by something else before it finished",
            ));
        }
        if let Ok(smd) = meta::lstat(&item.source) {
            let _ = apply_security(&item.source, &dest, meta::flags(&smd));
        }
        if self.is_move() && meta::exists(&item.source) {
            self.remove_source(item)?;
        }
        Ok(Vec::new())
    }

    /// After all children: folder metadata, and for a move the empty source folder.
    fn finalize_dir(&self, item: &mut ItemRow, children_ok: bool) -> Res<()> {
        let Some(dest) = item.dest.clone() else {
            return Ok(());
        };
        if item.created && !item.merged {
            let r = sys::copyfile_plain(
                &item.source,
                &dest,
                libc::COPYFILE_STAT | libc::COPYFILE_XATTR | libc::COPYFILE_NOFOLLOW_SRC,
            )
            .and_then(|()| {
                let fl = meta::lstat(&item.source)
                    .map(|m| meta::flags(&m))
                    .unwrap_or(0);
                apply_security(&item.source, &dest, fl)
            });
            if let Err(e) = r {
                item.error = Some(format!(
                    "its folder attributes couldn't be copied: {}",
                    describe(&e)
                ));
            }
        }
        if self.is_move() && meta::exists(&item.source) {
            if !children_ok {
                item.error = Some(
                    "the original folder was kept because some items in it weren't moved".into(),
                );
            } else if let Err(e) = self.fault(FaultStep::BeforeSourceRemove, &item.source) {
                item.error = Some(format!("the original folder was kept: {}", describe(&e)));
            } else if let Err(e) = remove::remove_moved_source_dir(&item.source) {
                item.error = Some(format!(
                    "the original folder was kept: {}",
                    if e.raw_os_error() == Some(libc::ENOTEMPTY) {
                        "it isn't empty".to_string()
                    } else {
                        describe(&e)
                    }
                ));
            }
        }
        item.dest_ident = meta::lstat(&dest).ok().map(|m| meta::ident(&m));
        Ok(())
    }

    // ───────────────────────── trash, delete, create ─────────────────────────

    fn trash(&self, item: &mut ItemRow) -> Res<()> {
        let md = meta::lstat(&item.source)?;
        item.kind = meta::kind_of(&md);
        item.src_ident = Some(meta::ident(&md));
        item.state = ItemState::InProgress;
        self.save(item)?;
        let loc = self.trasher.trash(&item.source)?;
        item.trash_loc = Some(loc);
        if item.kind != EntryKind::Dir {
            item.bytes = md.len();
        }
        Ok(())
    }

    fn delete(&self, item: &mut ItemRow) -> Res<()> {
        let Some(gate) = DeleteGate::new(self.cfg.allow_delete_immediately) else {
            return Err(fail("Delete Immediately is turned off in Settings"));
        };
        let md = meta::lstat(&item.source)?;
        item.kind = meta::kind_of(&md);
        item.state = ItemState::InProgress;
        self.save(item)?;
        remove::delete_immediately(&item.source, &gate)?;
        Ok(())
    }

    fn create(&self, item: &mut ItemRow, decision: Option<ConflictDecision>) -> Res<()> {
        let (Some(dir), Some(name)) = (item.dest_dir.clone(), item.dest_name.clone()) else {
            return Err(fail("the journal lost track of this item"));
        };
        if let Err(e) = names::validate_name(&name) {
            return Err(fail(e));
        }
        let is_dir = self.kind == OpKind::NewFolder;
        let mut dest = dir.join(&name);
        if meta::exists(&dest) {
            // Describe the new item as an empty source of the right kind.
            let fake = meta::lstat(&dir)?;
            let decision = decision.or_else(|| self.ctl.rules.read().for_class(false));
            match decision {
                None => {
                    let mut info = match self.conflict_info(item, &fake, &dest) {
                        Stop::Conflict(i) => i,
                        other => return Err(other),
                    };
                    info.source_kind = if is_dir {
                        EntryKind::Dir
                    } else {
                        EntryKind::File
                    };
                    info.source_size = 0;
                    info.can_merge = false;
                    return Err(Stop::Conflict(info));
                }
                Some(ConflictDecision::Skip) => return Err(Stop::Skip(None)),
                Some(ConflictDecision::KeepBoth) => {
                    dest = names::keep_both_names(&name, is_dir)
                        .map(|n| dir.join(n))
                        .find(|p| !meta::exists(p))
                        .expect("infinite candidates");
                    item.dest_name = Some(file_name(&dest));
                }
                Some(ConflictDecision::Replace) => {
                    item.dest = Some(dest.clone());
                    item.state = ItemState::InProgress;
                    self.save(item)?;
                    let loc = self.trasher.trash(&dest)?;
                    item.replaced_trash = Some(loc);
                }
                Some(ConflictDecision::Merge) => {
                    return Err(fail("Merge is only for folders being copied"));
                }
            }
        }
        let partial = partial_path(&dest, item.id);
        if meta::exists(&partial) {
            return Err(fail(format!(
                "a temporary item is in the way: {}",
                partial.display()
            )));
        }
        item.source = dest.clone();
        item.dest = Some(dest.clone());
        item.partial = Some(partial.clone());
        item.kind = if is_dir {
            EntryKind::Dir
        } else {
            EntryKind::File
        };
        item.state = ItemState::InProgress;
        self.save(item)?;
        let made = if is_dir {
            std::fs::create_dir(&partial)
        } else {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&partial)
                .map(|_| ())
        };
        made?;
        item.partial_ino = meta::lstat(&partial).ok().map(|m| meta::ident(&m).ino);
        self.save(item)?;
        if let Err(e) = sys::rename_excl(&partial, &dest) {
            let _ = remove::remove_partial(&partial);
            item.partial = None;
            return Err(e.into());
        }
        item.partial = None;
        item.created = true;
        item.dest_ident = meta::lstat(&dest).ok().map(|m| meta::ident(&m));
        Ok(())
    }
}

/// Applies the source's ACL and immutable flags to a placed item. Done after
/// the rename, because a `deny delete` ACL or `uchg` would block it.
fn apply_security(src: &Path, dest: &Path, src_flags: u32) -> io::Result<()> {
    if sys::has_acl(src) {
        sys::copyfile_plain(src, dest, libc::COPYFILE_ACL | libc::COPYFILE_NOFOLLOW_SRC)?;
    }
    let want = src_flags & IMMUTABLE_FLAGS;
    if want != 0 {
        let cur = meta::flags(&meta::lstat(dest)?);
        sys::set_flags(dest, cur | want)?;
    }
    Ok(())
}

fn is_no_clone(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::ENOTSUP) | Some(libc::EXDEV) | Some(libc::EINVAL) | Some(libc::ENOSYS)
    )
}

/// The last component of a path, as stored.
pub fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Feeds byte progress into the op's counter and implements pause/cancel.
struct Tap<'a> {
    ctl: &'a OpCtl,
    added: AtomicU64,
}

impl<'a> Tap<'a> {
    fn new(ctl: &'a OpCtl) -> Self {
        Self {
            ctl,
            added: AtomicU64::new(0),
        }
    }
    fn add(&self, n: u64) {
        self.added.fetch_add(n, Ordering::Relaxed);
        self.ctl.bytes_done.fetch_add(n, Ordering::Relaxed);
    }
    /// Tops the counter up to `total` for this file (callbacks stop short of the end).
    fn settle(&self, total: u64) {
        let have = self.added.load(Ordering::Relaxed);
        if total > have {
            self.add(total - have);
        }
    }
    /// Takes this file's bytes back out of the counter (it failed).
    fn rollback(&self) {
        let n = self.added.swap(0, Ordering::Relaxed);
        self.ctl.bytes_done.fetch_sub(n, Ordering::Relaxed);
    }
    /// Pause and cancel point for loops.
    fn check(&self) -> io::Result<()> {
        if self.ctl.wait_while_paused() {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(libc::ECANCELED))
        }
    }
}

impl CopyProgress for Tap<'_> {
    fn progress(&self, copied: u64) -> bool {
        let have = self.added.load(Ordering::Relaxed);
        if copied > have {
            self.add(copied - have);
        }
        self.ctl.wait_while_paused()
    }
}

fn zeros() -> &'static [u8] {
    static Z: [u8; 1 << 16] = [0; 1 << 16];
    &Z
}

/// Our own copy loop, used when verifying: reads the source once with
/// `F_NOCACHE`, hashes it in flight, writes the partial with `F_NOCACHE`
/// (so the verify re-read hits the device), and keeps holes
/// (`SEEK_DATA`/`SEEK_HOLE`). Returns the source's xxh3-128 as hex.
fn copy_loop(src: &Path, partial: &Path, tap: &Tap<'_>) -> io::Result<String> {
    let s = File::open(src)?;
    sys::set_nocache(&s);
    let len = s.metadata()?.len();
    let d = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(partial)?;
    sys::set_nocache(&d);
    let mut h = Xxh3::new();
    let mut buf = vec![0u8; CHUNK];
    let mut off = 0u64;
    while off < len {
        tap.check()?;
        // Where the next data starts; filesystems without hole support say "here".
        let data = match sys::seek_data(&s, off) {
            Ok(v) => v.unwrap_or(len).min(len),
            Err(e) if e.raw_os_error() == Some(libc::EINVAL) => off,
            Err(e) => return Err(e),
        };
        let mut z = data - off;
        tap.add(z);
        while z > 0 {
            let n = z.min(zeros().len() as u64) as usize;
            h.update(&zeros()[..n]);
            z -= n as u64;
        }
        off = data;
        if off >= len {
            break;
        }
        let hole = match sys::seek_hole(&s, off) {
            Ok(v) => v.unwrap_or(len).min(len),
            Err(e) if e.raw_os_error() == Some(libc::EINVAL) => len,
            Err(e) => return Err(e),
        };
        while off < hole {
            tap.check()?;
            let want = ((hole - off) as usize).min(CHUNK);
            let n = s.read_at(&mut buf[..want], off)?;
            if n == 0 {
                return Err(io::Error::other(
                    "the original got shorter while it was being copied",
                ));
            }
            d.write_all_at(&buf[..n], off)?;
            h.update(&buf[..n]);
            off += n as u64;
            tap.add(n as u64);
        }
    }
    d.set_len(len)?;
    Ok(format!("{:032x}", h.digest128()))
}

/// xxh3-128 of a file's content, read with `F_NOCACHE`.
pub fn hash_file(path: &Path) -> io::Result<String> {
    let f = File::open(path)?;
    sys::set_nocache(&f);
    let mut h = Xxh3::new();
    let mut buf = vec![0u8; CHUNK];
    let mut r = &f;
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        h.update(&buf[..n]);
    }
    Ok(format!("{:032x}", h.digest128()))
}

use std::os::unix::fs::MetadataExt as _;

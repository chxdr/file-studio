//! The only place in `fstudio-ops` that talks to the OS below `std`.
//!
//! Each function is a small, documented wrapper. Everything else in the crate
//! stays safe Rust. macOS only for now; a Linux port puts `renameat2`,
//! `copy_file_range` and the freedesktop Trash behind the same functions.
#![allow(unsafe_code)]

pub mod trash;

use std::ffi::{CString, c_char, c_int, c_void};
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// A path as a NUL-terminated C string. Paths with interior NUL bytes are rejected.
fn cstr(p: &Path) -> io::Result<CString> {
    CString::new(p.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))
}

fn check(r: c_int) -> io::Result<()> {
    if r == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// How [`rename_excl`] placed the item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameMode {
    /// `renamex_np(RENAME_EXCL)`: atomic, never replaces.
    Exclusive,
    /// The filesystem can't rename exclusively (exFAT, SMB…): we checked that
    /// the destination was free, then renamed. There is a small race window.
    CheckThenRename,
}

/// Renames `from` to `to`, failing with `AlreadyExists` instead of ever
/// replacing an existing item (F-007).
///
/// Uses `renamex_np(RENAME_EXCL)`. Filesystems that don't support exclusive
/// rename (`ENOTSUP`/`EINVAL`) fall back to check-then-rename.
pub fn rename_excl(from: &Path, to: &Path) -> io::Result<RenameMode> {
    let (a, b) = (cstr(from)?, cstr(to)?);
    // SAFETY: both pointers are valid NUL-terminated strings that outlive the call.
    let r = unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_EXCL) };
    if r == 0 {
        return Ok(RenameMode::Exclusive);
    }
    let e = io::Error::last_os_error();
    match e.raw_os_error() {
        Some(libc::ENOTSUP) | Some(libc::EINVAL) => {}
        _ => return Err(e),
    }
    if std::fs::symlink_metadata(to).is_ok() {
        return Err(io::ErrorKind::AlreadyExists.into());
    }
    // SAFETY: as above. Plain rename(2) never follows symlinks in either path's
    // last component; we just checked that `to` does not exist.
    check(unsafe { libc::rename(a.as_ptr(), b.as_ptr()) })?;
    Ok(RenameMode::CheckThenRename)
}

/// Asks the caller whether a running `copyfile` should go on. Called on the
/// copying thread about once per MiB.
pub trait CopyProgress {
    /// `copied` is the number of bytes of this file written so far. Return
    /// `false` to cancel. Pausing is done by blocking inside this call.
    fn progress(&self, copied: u64) -> bool;
}

struct CbCtx<'a> {
    progress: &'a dyn CopyProgress,
    cancelled: bool,
    /// The errno copyfile reported through a `COPYFILE_ERR` callback.
    error: Option<i32>,
}

extern "C" fn status_cb(
    what: c_int,
    stage: c_int,
    state: libc::copyfile_state_t,
    _src: *const c_char,
    _dst: *const c_char,
    ctx: *mut c_void,
) -> c_int {
    if ctx.is_null() {
        return libc::COPYFILE_CONTINUE;
    }
    if stage == libc::COPYFILE_ERR {
        // Returning CONTINUE here means "retry", which loops forever on a
        // full disk. Stop, and keep the errno for the caller.
        let errno = io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(libc::EIO);
        // SAFETY: see below; same context pointer, same thread.
        let ctx = unsafe { &mut *(ctx as *mut CbCtx<'_>) };
        ctx.error = Some(errno);
        return libc::COPYFILE_QUIT;
    }
    if what != libc::COPYFILE_COPY_DATA || stage != libc::COPYFILE_PROGRESS {
        return libc::COPYFILE_CONTINUE;
    }
    let mut copied: libc::off_t = 0;
    // SAFETY: `state` is the live state copyfile passes to its own callback, and
    // COPYFILE_STATE_COPIED writes one off_t into the pointer we pass.
    unsafe {
        libc::copyfile_state_get(
            state,
            libc::COPYFILE_STATE_COPIED as u32,
            (&mut copied as *mut libc::off_t).cast(),
        );
    }
    // SAFETY: `ctx` is the `&mut CbCtx` set by `copyfile_with_progress`, which
    // outlives the copyfile call that invokes this callback, on the same thread.
    let ctx = unsafe { &mut *(ctx as *mut CbCtx<'_>) };
    let go = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ctx.progress.progress(copied.max(0) as u64)
    }))
    .unwrap_or(false);
    if go {
        libc::COPYFILE_CONTINUE
    } else {
        ctx.cancelled = true;
        libc::COPYFILE_QUIT
    }
}

/// What a finished `copyfile` call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyfileOutcome {
    /// The data was cloned (APFS), not copied byte for byte.
    pub cloned: bool,
}

/// `copyfile(3)` with a status callback for progress, pause and cancel.
///
/// A cancel makes `copyfile` return `ECANCELED` (it removes its own partial
/// output). Blocks on disk for the duration of the copy.
pub fn copyfile_with_progress(
    src: &Path,
    dst: &Path,
    flags: u32,
    progress: &dyn CopyProgress,
) -> io::Result<CopyfileOutcome> {
    let (a, b) = (cstr(src)?, cstr(dst)?);
    let mut ctx = CbCtx {
        progress,
        cancelled: false,
        error: None,
    };
    let cb: libc::copyfile_callback_t = Some(status_cb);
    // SAFETY: the state is allocated, used and freed in this block. The callback
    // pointer is a plain `extern "C" fn`, and the context pointer refers to
    // `ctx`, which lives until after `copyfile_state_free`. Both paths are valid
    // NUL-terminated strings for the duration of the call.
    unsafe {
        let st = libc::copyfile_state_alloc();
        if st.is_null() {
            return Err(io::Error::last_os_error());
        }
        libc::copyfile_state_set(
            st,
            libc::COPYFILE_STATE_STATUS_CB as u32,
            cb.map_or(std::ptr::null(), |f| f as *const c_void),
        );
        libc::copyfile_state_set(
            st,
            libc::COPYFILE_STATE_STATUS_CTX as u32,
            (&mut ctx as *mut CbCtx<'_>).cast::<c_void>().cast_const(),
        );
        let r = libc::copyfile(a.as_ptr(), b.as_ptr(), st, flags);
        let err = io::Error::last_os_error();
        let mut cloned: c_int = 0;
        libc::copyfile_state_get(
            st,
            libc::COPYFILE_STATE_WAS_CLONED as u32,
            (&mut cloned as *mut c_int).cast(),
        );
        libc::copyfile_state_free(st);
        if r == 0 {
            Ok(CopyfileOutcome {
                cloned: cloned != 0,
            })
        } else if let Some(errno) = ctx.error {
            Err(io::Error::from_raw_os_error(errno))
        } else if ctx.cancelled {
            Err(io::Error::from_raw_os_error(libc::ECANCELED))
        } else {
            Err(err)
        }
    }
}

/// `copyfile(3)` without a callback, for metadata-only copies
/// (`COPYFILE_ACL`, `COPYFILE_STAT`, `COPYFILE_XATTR`).
pub fn copyfile_plain(src: &Path, dst: &Path, flags: u32) -> io::Result<()> {
    let (a, b) = (cstr(src)?, cstr(dst)?);
    // SAFETY: valid NUL-terminated paths; a NULL state is allowed by copyfile(3).
    check(unsafe { libc::copyfile(a.as_ptr(), b.as_ptr(), std::ptr::null_mut(), flags) })
}

/// Bypass the unified buffer cache for this descriptor (`F_NOCACHE`), so a
/// verify pass reads the device rather than RAM.
pub fn set_nocache(f: &File) {
    // SAFETY: fcntl with an int argument on a descriptor we own.
    unsafe {
        libc::fcntl(f.as_raw_fd(), libc::F_NOCACHE, 1);
    }
}

/// Durable flush. Plain fsync on macOS doesn't flush the drive's cache;
/// `F_FULLFSYNC` does. Falls back to plain `fsync(2)` where `F_FULLFSYNC`
/// is refused (some SMB servers, devices).
///
/// Not `File::sync_all` for the fallback: on macOS that *is* `F_FULLFSYNC`,
/// so it fails the same way and the fallback would never help.
pub fn full_sync(f: &File) -> io::Result<()> {
    // SAFETY: fcntl on a valid descriptor with no pointer arguments.
    let r = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_FULLFSYNC) };
    if r == 0 {
        return Ok(());
    }
    // SAFETY: fsync on a valid descriptor we own.
    check(unsafe { libc::fsync(f.as_raw_fd()) })
}

/// Drops this file's pages from the unified buffer cache, so the next
/// `F_NOCACHE` read comes from the drive. `F_NOCACHE` alone doesn't evict
/// pages that are already cached (written through the cache, or read by
/// Spotlight or Quick Look in the meantime); `msync(MS_INVALIDATE)` on a
/// read-only mapping does, without admin rights (Safe Transfer proposal §4.2).
/// An empty file has nothing to evict.
pub fn evict_cache(f: &File) -> io::Result<()> {
    let len = f.metadata()?.len();
    if len == 0 {
        return Ok(());
    }
    let len = usize::try_from(len).map_err(|_| io::Error::other("file too large to map"))?;
    // SAFETY: a fresh read-only shared mapping of a descriptor we own, of the
    // file's current length; nothing reads through the pointer, and it is
    // unmapped below before returning.
    let p = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ,
            libc::MAP_SHARED,
            f.as_raw_fd(),
            0,
        )
    };
    if p == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `p` and `len` are exactly the mapping created above.
    let r = unsafe { libc::msync(p, len, libc::MS_INVALIDATE) };
    let err = io::Error::last_os_error();
    // SAFETY: unmaps the mapping created above; it is not used afterwards.
    unsafe {
        libc::munmap(p, len);
    }
    if r == 0 { Ok(()) } else { Err(err) }
}

/// The UUID of the volume holding `path` (`getattrlist(ATTR_VOL_UUID)` on its
/// mount point), as 16 bytes. Unlike `st_dev`, it survives unplugging and
/// re-attaching: APFS and HFS+ volumes have their own, and macOS derives
/// exFAT and FAT ones from the card's volume serial number (Safe Transfer
/// proposal §3.4). `None` for volumes that have none (some network shares).
pub fn volume_uuid(path: &Path) -> Option<[u8; 16]> {
    let p = cstr(path).ok()?;
    // SAFETY: `statfs` is plain old data, so all-zero is a valid value.
    let mut sfs: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: valid NUL-terminated path; `sfs` is writable for its size.
    if unsafe { libc::statfs(p.as_ptr(), &mut sfs) } != 0 {
        return None;
    }
    let mut al = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: 0,
        volattr: libc::ATTR_VOL_INFO | libc::ATTR_VOL_UUID,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    // u32 length, then a 16-byte uuid_t.
    let mut buf = [0u8; 4 + 16 + 12];
    // SAFETY: `f_mntonname` is NUL-terminated by the kernel; `al` is a valid
    // attrlist; `buf` is writable for `buf.len()` bytes, which we pass.
    let r = unsafe {
        libc::getattrlist(
            sfs.f_mntonname.as_ptr(),
            (&mut al as *mut libc::attrlist).cast(),
            buf.as_mut_ptr().cast(),
            buf.len(),
            0,
        )
    };
    if r != 0 {
        return None;
    }
    let len = u32::from_ne_bytes(buf[0..4].try_into().ok()?) as usize;
    if len < 20 {
        return None;
    }
    let uuid: [u8; 16] = buf[4..20].try_into().ok()?;
    (uuid != [0u8; 16]).then_some(uuid)
}

/// For a path on a network volume (`MNT_LOCAL` clear): the filesystem type
/// and what the volume was mounted from (`smbfs`, `//milky@archbox/media`).
/// `None` on local volumes and when `statfs` fails. Never waits on the
/// server beyond the one `statfs`.
pub fn network_source(path: &Path) -> Option<(String, String)> {
    let p = cstr(path).ok()?;
    // SAFETY: `statfs` is plain old data, so all-zero is a valid value.
    let mut sfs: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: valid NUL-terminated path; `sfs` is writable for its size.
    if unsafe { libc::statfs(p.as_ptr(), &mut sfs) } != 0 {
        return None;
    }
    if sfs.f_flags & libc::MNT_LOCAL as u32 != 0 {
        return None;
    }
    let text = |b: &[c_char]| {
        let bytes: Vec<u8> = b
            .iter()
            .take_while(|c| **c != 0)
            .map(|c| *c as u8)
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    Some((text(&sfs.f_fstypename), text(&sfs.f_mntfromname)))
}

/// fsyncs a directory so a rename inside it is durable.
pub fn sync_dir(dir: &Path) -> io::Result<()> {
    full_sync(&File::open(dir)?)
}

/// Whether names on the volume holding `dir` are case-sensitive
/// (`pathconf(_PC_CASE_SENSITIVE)`, F-017). Unknown counts as insensitive,
/// which is the cautious answer for conflict prediction.
pub fn case_sensitive(dir: &Path) -> io::Result<bool> {
    let p = cstr(dir)?;
    // SAFETY: valid NUL-terminated path; pathconf takes no pointers besides it.
    let r = unsafe { libc::pathconf(p.as_ptr(), libc::_PC_CASE_SENSITIVE) };
    if r < 0 {
        let e = io::Error::last_os_error();
        // EINVAL: the filesystem doesn't say.
        if e.raw_os_error() == Some(libc::EINVAL) {
            return Ok(false);
        }
        return Err(e);
    }
    Ok(r == 1)
}

/// Sets BSD file flags without following a symlink (`lchflags`).
pub fn set_flags(path: &Path, flags: u32) -> io::Result<()> {
    let p = cstr(path)?;
    // SAFETY: valid NUL-terminated path; flags is a plain integer.
    check(unsafe { libc::lchflags(p.as_ptr(), flags as _) })
}

type AclT = *mut c_void;
const ACL_TYPE_EXTENDED: c_int = 0x0000_0100;

unsafe extern "C" {
    fn acl_get_link_np(path: *const c_char, ty: c_int) -> AclT;
    fn acl_set_link_np(path: *const c_char, ty: c_int, acl: AclT) -> c_int;
    fn acl_init(count: c_int) -> AclT;
    fn acl_free(obj: *mut c_void) -> c_int;
}

/// Whether the item (not a symlink's target) carries an extended ACL.
pub fn has_acl(path: &Path) -> bool {
    let Ok(p) = cstr(path) else { return false };
    // SAFETY: valid path; a non-null result is owned by us and freed below.
    unsafe {
        let acl = acl_get_link_np(p.as_ptr(), ACL_TYPE_EXTENDED);
        if acl.is_null() {
            return false;
        }
        acl_free(acl);
        true
    }
}

/// Removes every extended ACL entry from the item (like `chmod -N`), so a
/// `deny delete` entry can't block renaming or removing a temporary file
/// (SPIKES S2).
pub fn clear_acl(path: &Path) -> io::Result<()> {
    if !has_acl(path) {
        return Ok(());
    }
    let p = cstr(path)?;
    // SAFETY: acl_init returns an owned, empty ACL that we free after the call.
    unsafe {
        let empty = acl_init(0);
        if empty.is_null() {
            return Err(io::Error::last_os_error());
        }
        let r = acl_set_link_np(p.as_ptr(), ACL_TYPE_EXTENDED, empty);
        let e = io::Error::last_os_error();
        acl_free(empty);
        if r == 0 { Ok(()) } else { Err(e) }
    }
}

/// The next data region at or after `off` (`lseek(SEEK_DATA)`), or `None`
/// when only a hole remains.
pub fn seek_data(f: &File, off: u64) -> io::Result<Option<u64>> {
    lseek(f, off, libc::SEEK_DATA)
}

/// The next hole at or after `off` (`lseek(SEEK_HOLE)`). The end of the file
/// counts as a hole.
pub fn seek_hole(f: &File, off: u64) -> io::Result<Option<u64>> {
    lseek(f, off, libc::SEEK_HOLE)
}

fn lseek(f: &File, off: u64, whence: c_int) -> io::Result<Option<u64>> {
    // SAFETY: lseek on a valid descriptor with integer arguments.
    let r = unsafe { libc::lseek(f.as_raw_fd(), off as libc::off_t, whence) };
    if r >= 0 {
        return Ok(Some(r as u64));
    }
    let e = io::Error::last_os_error();
    match e.raw_os_error() {
        Some(libc::ENXIO) => Ok(None),
        _ => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_never_replaces() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a");
        let b = d.path().join("b");
        std::fs::write(&a, b"new").unwrap();
        std::fs::write(&b, b"old").unwrap();
        let e = rename_excl(&a, &b).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&b).unwrap(), b"old");
        assert_eq!(
            rename_excl(&a, &d.path().join("c")).unwrap(),
            RenameMode::Exclusive
        );
    }

    /// A descriptor that refuses `F_FULLFSYNC` but takes `fsync` (a device)
    /// still syncs: the fallback must not be `File::sync_all`, which is
    /// `F_FULLFSYNC` again on macOS.
    #[test]
    fn full_sync_falls_back_to_plain_fsync() {
        let f = File::options().write(true).open("/dev/null").unwrap();
        // SAFETY: fcntl on a valid descriptor with no pointer arguments.
        let refused = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_FULLFSYNC) } != 0;
        assert!(
            refused,
            "the test needs a descriptor that refuses F_FULLFSYNC"
        );
        assert!(f.sync_all().is_err(), "sync_all is F_FULLFSYNC on macOS");
        full_sync(&f).unwrap();
    }

    /// The fallbacks on a real SMB share: set `FSTUDIO_SMB_DIR` to a
    /// writable, empty folder on a mounted share and run with `--ignored`.
    /// Rename still never replaces (exclusively, or by check-then-rename),
    /// the flush works, and nothing is left behind.
    #[test]
    #[ignore = "needs a mounted SMB share (FSTUDIO_SMB_DIR)"]
    fn smb_share_fallbacks_hold() {
        let base = std::path::PathBuf::from(std::env::var("FSTUDIO_SMB_DIR").unwrap());
        let d = tempfile::tempdir_in(&base).unwrap();
        let a = d.path().join("a");
        let b = d.path().join("b");
        std::fs::write(&a, b"new").unwrap();
        std::fs::write(&b, b"old").unwrap();
        assert_eq!(
            rename_excl(&a, &b).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&b).unwrap(), b"old");
        // Differs only by case: the server decides whether that is the same name.
        let mode = rename_excl(&a, &d.path().join("c")).unwrap();
        eprintln!("rename mode on this share: {mode:?}");
        full_sync(&File::open(d.path().join("c")).unwrap()).unwrap();
        let _ = sync_dir(d.path());
        eprintln!("volume uuid: {:?}", volume_uuid(d.path()));
        eprintln!("network source: {:?}", network_source(d.path()));
        assert!(network_source(d.path()).is_some());
    }

    #[test]
    fn case_sensitivity_of_tempdir_is_known() {
        let d = tempfile::tempdir().unwrap();
        // The internal volume is case-insensitive APFS on a default Mac.
        let _ = case_sensitive(d.path()).unwrap();
    }

    #[test]
    fn acl_set_and_clear() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("f");
        std::fs::write(&f, b"x").unwrap();
        assert!(!has_acl(&f));
        let ok = std::process::Command::new("chmod")
            .args(["+a", "everyone deny delete"])
            .arg(&f)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        assert!(has_acl(&f));
        clear_acl(&f).unwrap();
        assert!(!has_acl(&f));
    }

    #[test]
    fn sparse_seek() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s");
        let f = File::create(&p).unwrap();
        f.set_len(64 << 20).unwrap();
        use std::os::unix::fs::FileExt;
        f.write_at(&[1u8; 4096], 32 << 20).unwrap();
        f.sync_all().unwrap();
        let f = File::open(&p).unwrap();
        let first = seek_data(&f, 0).unwrap().unwrap();
        assert!(first <= 32 << 20);
        let hole = seek_hole(&f, first).unwrap().unwrap();
        assert!(hole > first);
    }
}

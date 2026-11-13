//! macOS system calls. Most of these were extracted from `fstudio-ops`
//! (`sys/mod.rs`), where they are proven; the cache eviction, residency and
//! volume UUID calls are new (measured in `spikes/safe-transfer`).

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::platform::{FlushLevel, RenameMode, Residency, VolumeInfo};

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

/// Renames `from` to `to`, failing with `AlreadyExists` instead of ever
/// replacing an existing item. Uses `renamex_np(RENAME_EXCL)`; filesystems
/// without it (exFAT, FAT, SMB) fall back to check-then-rename.
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
    // SAFETY: as above. rename(2) never follows a symlink in the last
    // component of either path; we just checked that `to` does not exist.
    check(unsafe { libc::rename(a.as_ptr(), b.as_ptr()) })?;
    Ok(RenameMode::CheckThenRename)
}

/// Flushes the file to the drive and asks the drive to empty its write
/// cache (`F_FULLFSYNC`). Falls back to `fsync` where that is refused.
pub fn full_sync(f: &File) -> io::Result<FlushLevel> {
    // SAFETY: fcntl on a valid descriptor with no pointer arguments.
    let r = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_FULLFSYNC) };
    if r == 0 {
        Ok(FlushLevel::Device)
    } else {
        // Not `File::sync_all`: on macOS that is F_FULLFSYNC again.
        os_sync(f)?;
        Ok(FlushLevel::OsOnly)
    }
}

/// Plain `fsync(2)`: sends the file's data and attributes to the drive,
/// without asking the drive to empty its cache. Cheap; a group of these is
/// followed by one [`full_sync`] (proposal §4.7). Note that Rust's
/// `File::sync_all` is **not** this on macOS: it issues `F_FULLFSYNC`.
pub fn os_sync(f: &File) -> io::Result<()> {
    // SAFETY: fsync on a descriptor we own.
    check(unsafe { libc::fsync(f.as_raw_fd()) })
}

/// [`full_sync`] on a folder, so a rename inside it is durable.
pub fn sync_dir(dir: &Path) -> io::Result<FlushLevel> {
    full_sync(&File::open(dir)?)
}

/// Stops this descriptor's reads and writes from filling the unified buffer
/// cache (`F_NOCACHE`). Pages that are already cached stay cached: see [`evict`].
pub fn set_nocache(f: &File) -> io::Result<()> {
    // SAFETY: fcntl with an int argument on a descriptor we own.
    check(unsafe { libc::fcntl(f.as_raw_fd(), libc::F_NOCACHE, 1) }.min(0))
}

/// A read-only shared mapping of a whole file, unmapped on drop.
struct Mapping {
    addr: *mut c_void,
    len: usize,
}

impl Mapping {
    fn new(f: &File, len: usize) -> io::Result<Mapping> {
        // SAFETY: a read-only shared mapping of `len` bytes of a descriptor we
        // own; `len` is non-zero (callers check). The result is checked below.
        let addr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                f.as_raw_fd(),
                0,
            )
        };
        if addr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Mapping { addr, len })
    }

    fn residency(&self) -> io::Result<Residency> {
        // SAFETY: sysconf takes no pointers.
        let page = (unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).max(4096) as usize;
        let pages = self.len.div_ceil(page);
        let mut v = vec![0 as c_char; pages];
        // SAFETY: `addr..addr+len` is our live mapping and `v` has one byte per page.
        check(unsafe { libc::mincore(self.addr, self.len, v.as_mut_ptr()) })?;
        let resident = v.iter().filter(|b| (**b as u8) & 1 != 0).count() as u64;
        Ok(Residency {
            resident_pages: resident,
            total_pages: pages as u64,
        })
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: unmaps exactly the mapping created in `Mapping::new`.
        unsafe {
            libc::munmap(self.addr, self.len);
        }
    }
}

/// How much of the file at `path` is in the unified buffer cache (`mincore`).
pub fn residency(path: &Path) -> io::Result<Residency> {
    let f = File::open(path)?;
    let len = f.metadata()?.len() as usize;
    if len == 0 {
        return Ok(Residency::default());
    }
    Mapping::new(&f, len)?.residency()
}

/// Drops the file's pages from the unified buffer cache and says how many
/// are left. `MS_SYNC | MS_INVALIDATE` writes back any page another program
/// has dirtied before dropping it, so no content can ever be lost; the file
/// is mapped read-only and is never modified. Measured to work without admin
/// rights on APFS, exFAT and FAT (SAFE-TRANSFER-PROPOSAL §4.2).
pub fn evict(path: &Path) -> io::Result<Residency> {
    evict_open(&File::open(path)?)
}

/// [`evict`] through a descriptor that is already open (one `open` fewer;
/// on exFAT every `open` is a round trip to the user-space driver).
pub fn evict_open(f: &File) -> io::Result<Residency> {
    let len = f.metadata()?.len() as usize;
    if len == 0 {
        return Ok(Residency::default());
    }
    let m = Mapping::new(f, len)?;
    // SAFETY: `addr..addr+len` is our live read-only mapping. MS_SYNC writes
    // dirty pages back first; MS_INVALIDATE then drops the clean ones. Never
    // MS_KILLPAGES, which would discard data.
    check(unsafe { libc::msync(m.addr, m.len, libc::MS_SYNC | libc::MS_INVALIDATE) })?;
    m.residency()
}

/// The next data region at or after `off` (`SEEK_DATA`), or `None` when only
/// a hole remains. `Err(EINVAL)` means the filesystem doesn't know holes.
pub fn seek_data(f: &File, off: u64) -> io::Result<Option<u64>> {
    lseek(f, off, libc::SEEK_DATA)
}

/// The next hole at or after `off` (`SEEK_HOLE`). The end of the file counts as a hole.
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

/// `copyfile(3)` without a state or callback.
fn copyfile(src: &Path, dst: &Path, flags: u32) -> io::Result<()> {
    let (a, b) = (cstr(src)?, cstr(dst)?);
    // SAFETY: valid NUL-terminated paths; a NULL state is allowed by copyfile(3),
    // which then reports no progress and needs no callback.
    check(unsafe { libc::copyfile(a.as_ptr(), b.as_ptr(), std::ptr::null_mut(), flags) })
}

/// Clones `src` to the new file `dst` on the same APFS volume
/// (`COPYFILE_CLONE_FORCE`, never a byte copy). Errors with `ENOTSUP`,
/// `EXDEV` or similar when the volume can't clone.
pub fn clone_file(src: &Path, dst: &Path) -> io::Result<()> {
    copyfile(
        src,
        dst,
        libc::COPYFILE_CLONE_FORCE | libc::COPYFILE_NOFOLLOW_SRC | libc::COPYFILE_EXCL,
    )
}

/// Copies extended attributes (Finder tags, comments, resource fork) and
/// stat information (mode, dates, flags) from `src` onto `dst`.
pub fn copy_metadata(src: &Path, dst: &Path) -> io::Result<()> {
    // Without xattrs on the original, copying them only costs (on exFAT,
    // copyfile probes the destination's AppleDouble data for each one).
    let xattr = if xattr_names(src).is_ok_and(|n| n.is_empty()) {
        0
    } else {
        libc::COPYFILE_XATTR
    };
    copyfile(
        src,
        dst,
        xattr | libc::COPYFILE_STAT | libc::COPYFILE_NOFOLLOW_SRC,
    )
}

/// Copies the extended ACL of `src` onto `dst`.
pub fn copy_acl(src: &Path, dst: &Path) -> io::Result<()> {
    copyfile(src, dst, libc::COPYFILE_ACL | libc::COPYFILE_NOFOLLOW_SRC)
}

/// Sets BSD file flags without following a symlink (`lchflags`).
pub fn set_flags(path: &Path, flags: u32) -> io::Result<()> {
    let p = cstr(path)?;
    // SAFETY: valid NUL-terminated path; flags is a plain integer.
    check(unsafe { libc::lchflags(p.as_ptr(), flags as _) })
}

/// The BSD flags (`st_flags`) of an item, from its metadata.
pub fn file_flags(md: &std::fs::Metadata) -> u32 {
    use std::os::macos::fs::MetadataExt;
    md.st_flags()
}

/// Birth (creation) time in ns since the epoch, if the filesystem keeps one.
pub fn birth_ns(md: &std::fs::Metadata) -> Option<i64> {
    use std::os::macos::fs::MetadataExt;
    let s = md.st_birthtime();
    (s > 0).then(|| s * 1_000_000_000 + md.st_birthtime_nsec())
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
/// `deny delete` entry can't block renaming or removing a partial file.
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

/// The names of the item's extended attributes (not following a symlink).
pub fn xattr_names(path: &Path) -> io::Result<Vec<String>> {
    let p = cstr(path)?;
    let opts = libc::XATTR_NOFOLLOW;
    // SAFETY: a NULL buffer with size 0 asks for the needed size.
    let n = unsafe { libc::listxattr(p.as_ptr(), std::ptr::null_mut(), 0, opts) };
    if n < 0 {
        let e = io::Error::last_os_error();
        // Filesystems without xattrs say ENOTSUP: none.
        return match e.raw_os_error() {
            Some(libc::ENOTSUP) => Ok(Vec::new()),
            _ => Err(e),
        };
    }
    if n == 0 {
        return Ok(Vec::new());
    }
    let mut buf = vec![0u8; n as usize];
    // SAFETY: `buf` has `buf.len()` writable bytes.
    let n = unsafe { libc::listxattr(p.as_ptr(), buf.as_mut_ptr().cast(), buf.len(), opts) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    buf.truncate(n as usize);
    let mut names: Vec<String> = buf
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    names.sort();
    Ok(names)
}

/// Whether names on the volume holding `dir` are case-sensitive. Unknown
/// counts as insensitive, the cautious answer for conflict prediction.
pub fn case_sensitive(dir: &Path) -> io::Result<bool> {
    let p = cstr(dir)?;
    // SAFETY: valid NUL-terminated path; pathconf takes no other pointers.
    let r = unsafe { libc::pathconf(p.as_ptr(), libc::_PC_CASE_SENSITIVE) };
    if r < 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::EINVAL) {
            return Ok(false);
        }
        return Err(e);
    }
    Ok(r == 1)
}

/// Takes an exclusive advisory lock on `f` without waiting (`flock`).
/// `Ok(false)` means another process holds it. Released when `f` is closed
/// (also when the process dies).
pub fn try_lock(f: &File) -> io::Result<bool> {
    // SAFETY: flock on a descriptor we own, with integer flags only.
    let r = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if r == 0 {
        return Ok(true);
    }
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(false)
    } else {
        Err(e)
    }
}

/// Fills `buf` with random bytes from the kernel (`getentropy`, ≤ 256 bytes).
pub fn random_bytes(buf: &mut [u8]) -> io::Result<()> {
    // SAFETY: `buf` is writable for `buf.len()` bytes; getentropy allows ≤ 256.
    check(unsafe { libc::getentropy(buf.as_mut_ptr().cast(), buf.len().min(256)) })
}

fn c_chars(a: &[c_char]) -> String {
    // SAFETY: statfs strings are NUL-terminated within their fixed arrays;
    // the slice covers the whole array, so the scan stays inside it.
    let s = unsafe { CStr::from_ptr(a.as_ptr()) };
    s.to_string_lossy().into_owned()
}

#[repr(C, packed(4))]
struct UuidBuf {
    len: u32,
    uuid: [u8; 16],
}

/// The volume's UUID from `getattrlist(ATTR_VOL_UUID)`. On exFAT and FAT
/// macOS derives it from the volume serial number, which survives
/// unplugging and remounting (measured); `st_dev` does not.
fn volume_uuid(mount: &Path) -> Option<String> {
    let p = cstr(mount).ok()?;
    let mut al = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: 0,
        volattr: libc::ATTR_VOL_INFO | libc::ATTR_VOL_UUID,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut out = UuidBuf {
        len: 0,
        uuid: [0; 16],
    };
    // SAFETY: `al` and `out` are valid for the duration of the call, and the
    // buffer size passed is exactly `out`'s size.
    let r = unsafe {
        libc::getattrlist(
            p.as_ptr(),
            (&mut al as *mut libc::attrlist).cast(),
            (&mut out as *mut UuidBuf).cast(),
            std::mem::size_of::<UuidBuf>(),
            0,
        )
    };
    let uuid = out.uuid;
    if r != 0 || uuid == [0; 16] {
        return None;
    }
    let h: String = uuid.iter().map(|b| format!("{b:02X}")).collect();
    Some(format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    ))
}

fn info_from(s: &libc::statfs) -> VolumeInfo {
    let mount = PathBuf::from(c_chars(&s.f_mntonname));
    let name = if mount == Path::new("/") {
        "/".to_string()
    } else {
        mount
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let bsize = s.f_bsize as u64;
    let fs_type = c_chars(&s.f_fstypename);
    let mut free = s.f_bavail * bsize;
    if fs_type == "apfs" {
        // Finder's figure: purgeable space counts as free (it is freed on demand).
        free = free.max(super::important_free(&mount).unwrap_or(0));
    }
    VolumeInfo {
        uuid: volume_uuid(&mount),
        fs_type,
        name,
        capacity: s.f_blocks * bsize,
        free,
        block_size: bsize,
        device: c_chars(&s.f_mntfromname),
        mount,
    }
}

/// The volume that holds `path` (`statfs` + its UUID).
pub fn volume_info(path: &Path) -> io::Result<VolumeInfo> {
    let c = cstr(path)?;
    // SAFETY: a zeroed statfs is a valid out-buffer; `c` is a valid path.
    let s = unsafe {
        let mut s: libc::statfs = std::mem::zeroed();
        check(libc::statfs(c.as_ptr(), &mut s))?;
        s
    };
    Ok(info_from(&s))
}

/// Every mounted volume (`getmntinfo`, no waiting on unresponsive mounts).
pub fn mounted_volumes() -> io::Result<Vec<VolumeInfo>> {
    let mut p: *mut libc::statfs = std::ptr::null_mut();
    // SAFETY: getmntinfo points `p` at an array it owns of `n` entries,
    // valid until the next call on this thread; we copy out of it at once.
    let all: Vec<libc::statfs> = unsafe {
        let n = libc::getmntinfo(&mut p, libc::MNT_NOWAIT);
        if n <= 0 || p.is_null() {
            return Err(io::Error::last_os_error());
        }
        std::slice::from_raw_parts(p, n as usize).to_vec()
    };
    Ok(all.iter().map(info_from).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_never_replaces() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a"), d.path().join("b"));
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

    #[test]
    fn eviction_empties_the_cache_and_keeps_the_content() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f");
        let data: Vec<u8> = (0..8u32 << 20).map(|i| (i % 251) as u8).collect();
        // Written through the cache and NOT synced: the pages are dirty.
        std::fs::write(&p, &data).unwrap();
        assert!(residency(&p).unwrap().resident_pages > 0);
        let after = evict(&p).unwrap();
        assert_eq!(after.resident_pages, 0, "{after:?}");
        assert_eq!(std::fs::read(&p).unwrap(), data);
    }

    #[test]
    fn empty_files_have_no_pages() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("e");
        std::fs::write(&p, b"").unwrap();
        assert_eq!(evict(&p).unwrap(), Residency::default());
    }

    #[test]
    fn the_startup_volume_has_a_uuid() {
        let v = volume_info(Path::new("/")).unwrap();
        assert!(v.uuid.as_ref().is_some_and(|u| u.len() == 36), "{v:?}");
        assert!(v.capacity > 0 && v.block_size > 0);
        assert!(
            mounted_volumes()
                .unwrap()
                .iter()
                .any(|m| m.mount == v.mount)
        );
    }

    #[test]
    fn full_sync_reaches_the_device_on_apfs() {
        let d = tempfile::tempdir().unwrap();
        let f = File::create(d.path().join("s")).unwrap();
        assert_eq!(full_sync(&f).unwrap(), FlushLevel::Device);
        assert_eq!(sync_dir(d.path()).unwrap(), FlushLevel::Device);
    }

    #[test]
    fn random_bytes_differ() {
        let (mut a, mut b) = ([0u8; 8], [0u8; 8]);
        random_bytes(&mut a).unwrap();
        random_bytes(&mut b).unwrap();
        assert_ne!(a, b);
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
}

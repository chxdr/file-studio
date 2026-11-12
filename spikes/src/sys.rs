//! Raw macOS syscall declarations shared by the copy / iCloud spikes. Every call site documents its
//! safety argument. libc's crate does not expose copyfile(3).
use std::ffi::{c_void, CString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

pub type CopyfileState = *mut c_void;
pub type CopyfileCb = extern "C" fn(what: i32, stage: i32, state: CopyfileState, src: *const libc::c_char, dst: *const libc::c_char, ctx: *mut c_void) -> i32;

extern "C" {
    pub fn copyfile(from: *const libc::c_char, to: *const libc::c_char, state: CopyfileState, flags: u32) -> i32;
    pub fn copyfile_state_alloc() -> CopyfileState;
    pub fn copyfile_state_free(s: CopyfileState) -> i32;
    pub fn copyfile_state_set(s: CopyfileState, flag: u32, src: *const c_void) -> i32;
    pub fn copyfile_state_get(s: CopyfileState, flag: u32, dst: *mut c_void) -> i32;
    pub fn clonefile(src: *const libc::c_char, dst: *const libc::c_char, flags: u32) -> i32;
}

pub const COPYFILE_ACL: u32 = 1 << 0;
pub const COPYFILE_STAT: u32 = 1 << 1;
pub const COPYFILE_XATTR: u32 = 1 << 2;
pub const COPYFILE_DATA: u32 = 1 << 3;
pub const COPYFILE_METADATA: u32 = COPYFILE_ACL | COPYFILE_STAT | COPYFILE_XATTR;
pub const COPYFILE_ALL: u32 = COPYFILE_METADATA | COPYFILE_DATA;
pub const COPYFILE_RECURSIVE: u32 = 1 << 15;
pub const COPYFILE_EXCL: u32 = 1 << 17;
pub const COPYFILE_NOFOLLOW: u32 = (1 << 18) | (1 << 19);
pub const COPYFILE_CLONE: u32 = 1 << 24;
pub const COPYFILE_CLONE_FORCE: u32 = 1 << 25;
pub const COPYFILE_DATA_SPARSE: u32 = 1 << 27;
pub const ST_STATUS_CB: u32 = 6;
pub const ST_STATUS_CTX: u32 = 7;
pub const ST_COPIED: u32 = 8;
pub const ST_WAS_CLONED: u32 = 10;
pub const CB_COPY_DATA: i32 = 4;
pub const CB_START: i32 = 1;
pub const CB_FINISH: i32 = 2;
pub const CB_ERR: i32 = 3;
pub const CB_PROGRESS: i32 = 4;
pub const CB_CONTINUE: i32 = 0;
pub const CB_QUIT: i32 = 2;

pub fn cstr(p: &Path) -> CString { CString::new(p.as_os_str().as_bytes()).unwrap() }

pub fn copyfile_path(src: &Path, dst: &Path, flags: u32) -> io::Result<()> {
    let (a, b) = (cstr(src), cstr(dst));
    // SAFETY: both are valid NUL-terminated paths; a NULL state is allowed.
    let r = unsafe { copyfile(a.as_ptr(), b.as_ptr(), std::ptr::null_mut(), flags) };
    if r == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

pub fn clonefile_path(src: &Path, dst: &Path) -> io::Result<()> {
    let (a, b) = (cstr(src), cstr(dst));
    // SAFETY: valid NUL-terminated paths; flags=0 follows symlinks (CLONE_NOFOLLOW would be 1).
    let r = unsafe { clonefile(a.as_ptr(), b.as_ptr(), 0) };
    if r == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// F_NOCACHE: do not keep this fd's I/O in the unified buffer cache.
pub fn set_nocache(fd: i32) {
    // SAFETY: fcntl with an int argument on a descriptor the caller owns.
    unsafe { libc::fcntl(fd, libc::F_NOCACHE, 1); }
}

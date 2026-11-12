//! A minimal direct FSEvents stream (see F-014: the `notify` crate hard-codes a zero latency
//! and a fixed flag set and does not expose `WatchRoot` or the dropped-event flags).

use core_foundation::array::CFArray;
use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use std::ffi::{CStr, c_void};
use std::io;
use std::path::Path;

/// `kFSEventStreamCreateFlagNoDefer`.
pub const CF_NO_DEFER: u32 = 0x2;
/// `kFSEventStreamCreateFlagWatchRoot`.
pub const CF_WATCH_ROOT: u32 = 0x4;
/// `kFSEventStreamCreateFlagFileEvents`.
pub const CF_FILE_EVENTS: u32 = 0x10;
const SINCE_NOW: u64 = u64::MAX; // kFSEventStreamEventIdSinceNow

/// The callback gets one batch of `(path, event flags)` per FSEvents delivery.
pub type Callback = dyn Fn(Vec<(String, u32)>) + Send + Sync + 'static;

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn FSEventStreamCreate(
        alloc: *const c_void,
        cb: extern "C" fn(*const c_void, *mut c_void, usize, *mut c_void, *const u32, *const u64),
        ctx: *const FsCtx,
        paths: *const c_void,
        since: u64,
        latency: f64,
        flags: u32,
    ) -> *mut c_void;
    fn FSEventStreamSetDispatchQueue(s: *mut c_void, q: *mut c_void);
    fn FSEventStreamStart(s: *mut c_void) -> u8;
    fn FSEventStreamStop(s: *mut c_void);
    fn FSEventStreamInvalidate(s: *mut c_void);
    fn FSEventStreamRelease(s: *mut c_void);
}

unsafe extern "C" {
    fn dispatch_queue_create(label: *const libc::c_char, attr: *const c_void) -> *mut c_void;
    fn dispatch_release(o: *mut c_void);
    fn dispatch_sync_f(queue: *mut c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
}

#[repr(C)]
struct FsCtx {
    version: isize,
    info: *mut c_void,
    retain: *const c_void,
    release: *const c_void,
    copy_desc: *const c_void,
}

extern "C" fn noop(_: *mut c_void) {}

extern "C" fn trampoline(
    _s: *const c_void,
    info: *mut c_void,
    n: usize,
    paths: *mut c_void,
    flags: *const u32,
    _ids: *const u64,
) {
    // SAFETY: `info` is the `Box<Box<Callback>>` pointer given at creation, kept alive by the
    // `Stream` until the dispatch queue has drained in `Drop`. Without
    // kFSEventStreamCreateFlagUseCFTypes, `paths` is `char *[n]` and `flags` is `u32[n]`.
    let (cb, pv, fl) = unsafe {
        (
            &*(info as *const Box<Callback>),
            std::slice::from_raw_parts(paths as *const *const libc::c_char, n),
            std::slice::from_raw_parts(flags, n),
        )
    };
    let batch = pv
        .iter()
        .zip(fl)
        // SAFETY: each path is a NUL-terminated C string valid for the duration of the callback.
        .map(|(p, f)| {
            (
                unsafe { CStr::from_ptr(*p) }.to_string_lossy().into_owned(),
                *f,
            )
        })
        .collect();
    cb(batch);
}

/// A running FSEvents stream on one folder. Dropping it stops the stream.
pub struct Stream {
    s: *mut c_void,
    q: *mut c_void,
    cb: *mut Box<Callback>,
}

// SAFETY: the stream is only touched from `new` and `Drop`; the callback runs on its own
// dispatch queue and only uses `Send + Sync` data.
unsafe impl Send for Stream {}

impl Stream {
    /// Starts watching `path` (recursively; FSEvents has no non-recursive mode).
    pub fn start(
        path: &Path,
        latency_secs: f64,
        flags: u32,
        callback: Box<Callback>,
    ) -> io::Result<Stream> {
        let p = CFString::new(&path.to_string_lossy());
        let arr = CFArray::from_CFTypes(&[p]);
        let cb: *mut Box<Callback> = Box::into_raw(Box::new(callback));
        let ctx = FsCtx {
            version: 0,
            info: cb.cast(),
            retain: std::ptr::null(),
            release: std::ptr::null(),
            copy_desc: std::ptr::null(),
        };
        // SAFETY: standard FSEvents setup. `ctx` is copied by FSEventStreamCreate; the callback
        // box stays alive until `Drop` has stopped, invalidated and drained the stream. On every
        // failure path below the partially built objects are released and the box is reclaimed.
        unsafe {
            let s = FSEventStreamCreate(
                std::ptr::null(),
                trampoline,
                &ctx,
                arr.as_concrete_TypeRef() as *const c_void,
                SINCE_NOW,
                latency_secs,
                flags,
            );
            if s.is_null() {
                drop(Box::from_raw(cb));
                return Err(io::Error::other("FSEventStreamCreate failed"));
            }
            let q = dispatch_queue_create(c"fstudio.fsevents".as_ptr(), std::ptr::null());
            FSEventStreamSetDispatchQueue(s, q);
            if FSEventStreamStart(s) == 0 {
                FSEventStreamInvalidate(s);
                FSEventStreamRelease(s);
                dispatch_release(q);
                drop(Box::from_raw(cb));
                return Err(io::Error::other("FSEventStreamStart failed"));
            }
            Ok(Stream { s, q, cb })
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: stop -> invalidate -> release is the documented order. `dispatch_sync_f` with
        // a no-op waits for any callback already queued, after which nothing can use `cb`.
        unsafe {
            FSEventStreamStop(self.s);
            FSEventStreamInvalidate(self.s);
            FSEventStreamRelease(self.s);
            dispatch_sync_f(self.q, std::ptr::null_mut(), noop);
            dispatch_release(self.q);
            drop(Box::from_raw(self.cb));
        }
    }
}

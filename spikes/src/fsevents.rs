//! Minimal raw FSEvents wrapper (dispatch-queue delivery) used only for latency measurement.
use core_foundation::array::CFArray;
use core_foundation::string::CFString;
use core_foundation::base::TCFType;
use std::ffi::{c_void, CStr};
use std::sync::mpsc::Sender;
use std::time::Instant;

pub const CF_USE_CFTYPES: u32 = 0x1;
pub const CF_NO_DEFER: u32 = 0x2;
pub const CF_WATCH_ROOT: u32 = 0x4;
pub const CF_FILE_EVENTS: u32 = 0x10;
pub const SINCE_NOW: u64 = 0xFFFF_FFFF_FFFF_FFFF; // kFSEventStreamEventIdSinceNow

pub const EV_MUST_SCAN_SUBDIRS: u32 = 0x1;
pub const EV_CREATED: u32 = 0x100;
pub const EV_REMOVED: u32 = 0x200;
pub const EV_RENAMED: u32 = 0x800;
pub const EV_MODIFIED: u32 = 0x1000;
pub const EV_IS_FILE: u32 = 0x10000;

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
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
    fn FSEventStreamFlushSync(s: *mut c_void);
}
extern "C" {
    fn dispatch_queue_create(label: *const libc::c_char, attr: *const c_void) -> *mut c_void;
    fn dispatch_release(o: *mut c_void);
}

#[repr(C)]
struct FsCtx {
    version: isize,
    info: *mut c_void,
    retain: *const c_void,
    release: *const c_void,
    copy_desc: *const c_void,
}

pub type Batch = (Instant, Vec<(String, u32)>);

extern "C" fn trampoline(_s: *const c_void, info: *mut c_void, n: usize, paths: *mut c_void, flags: *const u32, _ids: *const u64) {
    let now = Instant::now(); // timestamp first, before any allocation
    // SAFETY: `info` is the Box<Sender<Batch>> pointer we passed at creation and which outlives the
    // stream; without kFSEventStreamCreateFlagUseCFTypes `paths` is a `char *[n]` and `flags` is `u32[n]`.
    let (tx, pv, fl) = unsafe { (&*(info as *const Sender<Batch>), std::slice::from_raw_parts(paths as *const *const libc::c_char, n), std::slice::from_raw_parts(flags, n)) };
    let v = pv.iter().zip(fl).map(|(p, f)| (unsafe { CStr::from_ptr(*p) }.to_string_lossy().into_owned(), *f)).collect();
    let _ = tx.send((now, v));
}

pub struct Stream {
    s: *mut c_void,
    q: *mut c_void,
    _tx: Box<Sender<Batch>>,
}

impl Stream {
    pub fn start(path: &str, latency: f64, flags: u32, tx: Sender<Batch>) -> Stream {
        let tx = Box::new(tx);
        let arr = CFArray::from_CFTypes(&[CFString::new(path)]);
        let ctx = FsCtx { version: 0, info: &*tx as *const Sender<Batch> as *mut c_void, retain: std::ptr::null(), release: std::ptr::null(), copy_desc: std::ptr::null() };
        // SAFETY: standard FSEvents setup; `ctx` is copied by FSEventStreamCreate; the callback context
        // (`tx`) is kept alive by the returned Stream until Drop stops/invalidates/releases the stream.
        unsafe {
            let s = FSEventStreamCreate(std::ptr::null(), trampoline, &ctx, arr.as_concrete_TypeRef() as *const c_void, SINCE_NOW, latency, flags & !CF_USE_CFTYPES);
            assert!(!s.is_null());
            let q = dispatch_queue_create(b"fs.spike\0".as_ptr() as *const _, std::ptr::null());
            FSEventStreamSetDispatchQueue(s, q);
            assert!(FSEventStreamStart(s) != 0);
            Stream { s, q, _tx: tx }
        }
    }
    pub fn flush_sync(&self) {
        // SAFETY: valid started stream.
        unsafe { FSEventStreamFlushSync(self.s) }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: stream was started with this queue; stop -> invalidate -> release is the documented order.
        unsafe {
            FSEventStreamStop(self.s);
            FSEventStreamInvalidate(self.s);
            FSEventStreamRelease(self.s);
            dispatch_release(self.q);
        }
    }
}

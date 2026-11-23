//! The only place in `fstudio-net` that talks to the OS below `std`:
//! NetFS (mounting), dns_sd (Bonjour browsing) and `getfsstat` (the mount
//! table). Each function is a small wrapper; everything else is safe Rust.
#![allow(unsafe_code)]

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation_sys::base::{CFRelease, CFTypeRef, kCFAllocatorDefault};
use core_foundation_sys::string::{
    CFStringCreateWithBytes, CFStringGetCString, CFStringGetLength,
    CFStringGetMaximumSizeForEncoding, CFStringRef, kCFStringEncodingUTF8,
};
use core_foundation_sys::url::{CFURLCreateWithString, CFURLRef};

// ───────────────────────── NetFS ─────────────────────────

/// `AsyncRequestID` (`void *`).
type RequestId = *mut c_void;

/// `NetFSMountURLBlock`: `void (^)(int status, AsyncRequestID, CFArrayRef mountpoints)`.
type MountBlock = block2::Block<dyn Fn(c_int, *mut c_void, *const c_void)>;

#[link(name = "NetFS", kind = "framework")]
unsafe extern "C" {
    fn NetFSMountURLAsync(
        url: CFURLRef,
        mountpath: CFURLRef,
        user: CFStringRef,
        passwd: CFStringRef,
        open_options: *mut c_void,
        mount_options: *mut c_void,
        request_id: *mut RequestId,
        dispatchq: *mut c_void,
        mount_report: &MountBlock,
    ) -> c_int;
    fn NetFSMountURLCancel(request_id: RequestId) -> c_int;
}

unsafe extern "C" {
    // libdispatch, part of libSystem.
    fn dispatch_get_global_queue(identifier: isize, flags: usize) -> *mut c_void;
}

/// `QOS_CLASS_UTILITY`: the mount report is not urgent work.
const QOS_CLASS_UTILITY: isize = 0x11;

/// A started mount, for cancelling it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MountRequest(usize);

/// Starts mounting `url` with `NetFSMountURLAsync`, letting the system show
/// its sign-in window and share picker. `done` runs once, on a system
/// dispatch queue, with NetFS's status (0, an errno, or a negative
/// `OSStatus`) and the mount points. It does not run if the mount is
/// cancelled with [`cancel_mount`]. An error here means the mount never
/// started (and `done` never runs).
pub(crate) fn mount_async(
    url: &str,
    done: Box<dyn FnOnce(c_int, Vec<PathBuf>) + Send>,
) -> Result<MountRequest, c_int> {
    let cfurl = cf_url(url).ok_or(libc::EINVAL)?;
    let cell = Mutex::new(Some(done));
    let block = block2::RcBlock::new(
        move |status: c_int, _req: *mut c_void, mps: *const c_void| {
            let paths = strings_of(mps as CFArrayRef);
            let f = cell.lock().ok().and_then(|mut g| g.take());
            if let Some(f) = f {
                // Never unwind into NetFS.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    f(status, paths.into_iter().map(PathBuf::from).collect())
                }));
            }
        },
    );
    let mut req: RequestId = std::ptr::null_mut();
    // SAFETY: `cfurl` is a live CFURL we own; NULL is allowed for the mount
    // path (default under /Volumes), user, password and both option
    // dictionaries (defaults: the system UI may be shown, soft mount). `req`
    // is writable. The global queue is never released. NetFS copies the
    // block (it is called later on the queue); our `RcBlock` keeps its own
    // reference until it is dropped below.
    let r = unsafe {
        let q = dispatch_get_global_queue(QOS_CLASS_UTILITY, 0);
        let r = NetFSMountURLAsync(
            cfurl,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut req,
            q,
            &block,
        );
        CFRelease(cfurl as CFTypeRef);
        r
    };
    if r == 0 {
        Ok(MountRequest(req as usize))
    } else {
        Err(r)
    }
}

/// Cancels a pending mount; its `done` is not called. Cancelling one that
/// already finished does nothing.
pub(crate) fn cancel_mount(req: MountRequest) {
    // SAFETY: the id came from NetFSMountURLAsync; NetFS ignores ids of
    // requests that already finished.
    unsafe {
        NetFSMountURLCancel(req.0 as RequestId);
    }
}

/// A new CFURL for `s`, or `None` when CF says it isn't a URL.
fn cf_url(s: &str) -> Option<CFURLRef> {
    // SAFETY: the bytes are valid UTF-8 for their length; the CFString and
    // CFURL are created here, and the string is released after the URL holds
    // its own reference. The caller releases the URL.
    unsafe {
        let cs = CFStringCreateWithBytes(
            kCFAllocatorDefault,
            s.as_ptr(),
            s.len() as _,
            kCFStringEncodingUTF8,
            0,
        );
        if cs.is_null() {
            return None;
        }
        let url = CFURLCreateWithString(kCFAllocatorDefault, cs, std::ptr::null());
        CFRelease(cs as CFTypeRef);
        (!url.is_null()).then_some(url)
    }
}

/// The strings in a CFArray of CFStrings (NULL: none). Doesn't release it.
fn strings_of(arr: CFArrayRef) -> Vec<String> {
    if arr.is_null() {
        return Vec::new();
    }
    // SAFETY: `arr` is the live array NetFS passed to its report block; we
    // only read it while the block runs. Items are CFStrings per NetFS.h.
    unsafe {
        let n = CFArrayGetCount(arr);
        (0..n)
            .filter_map(|i| cf_string(CFArrayGetValueAtIndex(arr, i) as CFStringRef))
            .collect()
    }
}

/// A CFString's text.
fn cf_string(s: CFStringRef) -> Option<String> {
    if s.is_null() {
        return None;
    }
    // SAFETY: `s` is a live CFString; the buffer is sized by CF's own maximum
    // for UTF-8 plus the NUL, and CF writes at most that many bytes.
    unsafe {
        let len = CFStringGetLength(s);
        let cap = CFStringGetMaximumSizeForEncoding(len, kCFStringEncodingUTF8) + 1;
        let mut buf = vec![0u8; cap.max(1) as usize];
        if CFStringGetCString(s, buf.as_mut_ptr().cast(), cap, kCFStringEncodingUTF8) == 0 {
            return None;
        }
        Some(
            CStr::from_ptr(buf.as_ptr().cast())
                .to_string_lossy()
                .into_owned(),
        )
    }
}

// ───────────────────────── dns_sd (Bonjour) ─────────────────────────

type DnsServiceRef = *mut c_void;
type DnsServiceBrowseReply = extern "C" fn(
    sd_ref: DnsServiceRef,
    flags: u32,
    interface_index: u32,
    error_code: i32,
    service_name: *const c_char,
    regtype: *const c_char,
    reply_domain: *const c_char,
    context: *mut c_void,
);

unsafe extern "C" {
    // libsystem_dnssd, part of libSystem.
    fn DNSServiceBrowse(
        sd_ref: *mut DnsServiceRef,
        flags: u32,
        interface_index: u32,
        regtype: *const c_char,
        domain: *const c_char,
        callback: DnsServiceBrowseReply,
        context: *mut c_void,
    ) -> i32;
    fn DNSServiceRefSockFD(sd_ref: DnsServiceRef) -> c_int;
    fn DNSServiceProcessResult(sd_ref: DnsServiceRef) -> i32;
    fn DNSServiceRefDeallocate(sd_ref: DnsServiceRef);
}

/// `kDNSServiceFlagsAdd`: the service appeared (else it went away).
const DNS_FLAG_ADD: u32 = 0x2;

/// One answer from a Bonjour browse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowseReply {
    /// Appeared (`true`) or went away.
    pub added: bool,
    /// The service's name ("Arch Box").
    pub name: String,
    /// Its domain ("local.").
    pub domain: String,
    /// The network interface it was seen on.
    pub interface: u32,
}

/// A running browse. Not `Send`: it is made, read and dropped on one thread.
pub(crate) struct DnsBrowse {
    sd: DnsServiceRef,
    /// Boxed on purpose: dns_sd holds a pointer to this `Vec` (the
    /// callback's context), so it must stay put when `DnsBrowse` moves.
    #[allow(clippy::box_collection)]
    replies: Box<Vec<BrowseReply>>,
}

extern "C" fn browse_reply(
    _sd: DnsServiceRef,
    flags: u32,
    interface: u32,
    error: i32,
    name: *const c_char,
    _regtype: *const c_char,
    domain: *const c_char,
    ctx: *mut c_void,
) {
    if error != 0 || ctx.is_null() || name.is_null() {
        return;
    }
    // SAFETY: `ctx` is the `Vec` boxed in `DnsBrowse`, which outlives every
    // `DNSServiceProcessResult` call that invokes this callback on the same
    // thread. `name` and `domain` are NUL-terminated strings valid during
    // the callback.
    unsafe {
        let out = &mut *(ctx as *mut Vec<BrowseReply>);
        out.push(BrowseReply {
            added: flags & DNS_FLAG_ADD != 0,
            name: CStr::from_ptr(name).to_string_lossy().into_owned(),
            domain: if domain.is_null() {
                String::new()
            } else {
                CStr::from_ptr(domain).to_string_lossy().into_owned()
            },
            interface,
        });
    }
}

impl DnsBrowse {
    /// Starts browsing for `service` (`_smb._tcp`) in the default domains.
    /// Errors are dns_sd error codes (`kDNSServiceErr_PolicyDenied` when
    /// local network access was refused).
    pub(crate) fn start(service: &str) -> Result<DnsBrowse, i32> {
        let ty = CString::new(service).map_err(|_| -65540)?; // kDNSServiceErr_BadParam
        let mut replies: Box<Vec<BrowseReply>> = Box::default();
        let mut sd: DnsServiceRef = std::ptr::null_mut();
        // SAFETY: `sd` is writable; `ty` is NUL-terminated and only read
        // during the call; a NULL domain means the default ones. The context
        // points into the boxed Vec, which stays at one address for the life
        // of the `DnsBrowse` that deallocates the ref first in `drop`.
        let r = unsafe {
            DNSServiceBrowse(
                &mut sd,
                0,
                0,
                ty.as_ptr(),
                std::ptr::null(),
                browse_reply,
                (&mut *replies as *mut Vec<BrowseReply>).cast(),
            )
        };
        if r != 0 {
            return Err(r);
        }
        Ok(DnsBrowse { sd, replies })
    }

    /// Waits up to `timeout_ms` for answers and returns them (often none).
    /// An error means the browse broke (the daemon went away).
    pub(crate) fn poll(&mut self, timeout_ms: i32) -> Result<Vec<BrowseReply>, i32> {
        // SAFETY: `sd` is live until drop.
        let fd = unsafe { DNSServiceRefSockFD(self.sd) };
        if fd < 0 {
            return Err(-65537); // kDNSServiceErr_Unknown
        }
        let mut p = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd for the duration of the call.
        let n = unsafe { libc::poll(&mut p, 1, timeout_ms) };
        if n <= 0 {
            return Ok(Vec::new());
        }
        // SAFETY: `sd` is live; the reply callback writes into
        // `self.replies` (see `browse_reply`) on this thread.
        let r = unsafe { DNSServiceProcessResult(self.sd) };
        if r != 0 {
            return Err(r);
        }
        Ok(std::mem::take(&mut *self.replies))
    }
}

impl Drop for DnsBrowse {
    fn drop(&mut self) {
        // SAFETY: `sd` came from DNSServiceBrowse and is deallocated once,
        // before `replies` (the callback's context) is freed.
        unsafe { DNSServiceRefDeallocate(self.sd) }
    }
}

// ───────────────────────── mount table ─────────────────────────

/// One mounted filesystem.
#[derive(Debug, Clone)]
pub(crate) struct Mount {
    pub mount_on: PathBuf,
    pub mount_from: String,
    pub fs_type: String,
    /// `MNT_LOCAL`: not a network volume.
    pub local: bool,
}

fn c_chars(b: &[c_char]) -> String {
    let bytes: Vec<u8> = b
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Every mounted filesystem (`getfsstat(MNT_NOWAIT)` into our own buffer:
/// it never waits on an unresponsive server).
pub(crate) fn mounts() -> io::Result<Vec<Mount>> {
    for _ in 0..8 {
        // SAFETY: a null buffer only asks for the number of mounts.
        let n = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        let cap = n as usize + 8;
        let mut buf: Vec<libc::statfs> = Vec::with_capacity(cap);
        let bytes = c_int::try_from(cap * std::mem::size_of::<libc::statfs>())
            .map_err(|_| io::Error::other("too many mounted volumes"))?;
        // SAFETY: `buf` has room for `cap` entries, which is the byte size we
        // pass; getfsstat writes at most that and returns how many it wrote.
        let got = unsafe { libc::getfsstat(buf.as_mut_ptr(), bytes, libc::MNT_NOWAIT) };
        if got < 0 {
            return Err(io::Error::last_os_error());
        }
        let got = got as usize;
        if got >= cap {
            continue; // More mounts appeared in between; ask again.
        }
        // SAFETY: getfsstat initialised the first `got` entries.
        unsafe { buf.set_len(got) };
        return Ok(buf
            .iter()
            .map(|s| Mount {
                mount_on: PathBuf::from(c_chars(&s.f_mntonname)),
                mount_from: c_chars(&s.f_mntfromname),
                fs_type: c_chars(&s.f_fstypename),
                local: s.f_flags & libc::MNT_LOCAL as u32 != 0,
            })
            .collect());
    }
    Err(io::Error::other("the mount table kept changing"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_become_cfurls_and_back() {
        let u = cf_url("smb://Arch%20Box._smb._tcp.local/media").unwrap();
        // SAFETY: test-only release of the URL created above.
        unsafe { CFRelease(u as CFTypeRef) };
        // A space isn't allowed in a URL; CF refuses it.
        assert!(cf_url("smb://a b/c").is_none());
    }

    #[test]
    fn the_mount_table_has_the_startup_disk() {
        let m = mounts().unwrap();
        assert!(
            m.iter()
                .any(|m| m.mount_on == std::path::Path::new("/") && m.local)
        );
    }

    /// Browsing starts and polls without a network answer (no SMB server is
    /// needed; it may simply find nothing).
    #[test]
    fn browsing_starts_and_polls() {
        match DnsBrowse::start("_smb._tcp") {
            Ok(mut b) => {
                let _ = b.poll(10);
            }
            // Local network access refused (no permission in a test runner).
            Err(e) => assert_ne!(e, 0),
        }
    }
}

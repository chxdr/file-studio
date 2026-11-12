//! Spike 4: which metadata calls touch dataless (iCloud / File Provider) files. READ-ONLY.
//! Usage: s4_icloud <path-to-dataless-file>...   (never opens file contents)
//! We deliberately never call open(2)/read on these files: that is the documented materialise trigger.
use core_foundation_sys::base::*;
use core_foundation_sys::string::*;
use core_foundation_sys::url::*;
use fstudio_spikes::{list::*, ms};
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::Instant;

const SF_DATALESS: u32 = 0x4000_0000;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFURLCopyResourcePropertyForKey(url: CFURLRef, key: CFStringRef, out: *mut CFTypeRef, err: *mut CFTypeRef) -> Boolean;
    fn CFCopyDescription(cf: CFTypeRef) -> CFStringRef;
}

fn flags_of(p: &Path) -> (u32, i64, i64) {
    let c = CString::new(p.as_os_str().as_bytes()).unwrap();
    // SAFETY: valid C string, zeroed stat out-buffer. lstat never materialises dataless files.
    unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        libc::lstat(c.as_ptr(), &mut st);
        (st.st_flags, st.st_size, st.st_blocks)
    }
}

fn cf_key(p: &Path, key: &str) -> (String, f64) {
    let t = Instant::now();
    // SAFETY: CF objects created here are released before return; the URL is built from valid bytes.
    unsafe {
        let b = p.as_os_str().as_bytes();
        let url = CFURLCreateFromFileSystemRepresentation(std::ptr::null(), b.as_ptr(), b.len() as CFIndex, 0);
        let k = CFString::from_static_str_hack(key);
        let mut out: CFTypeRef = std::ptr::null();
        let mut err: CFTypeRef = std::ptr::null();
        let ok = CFURLCopyResourcePropertyForKey(url, k, &mut out, &mut err);
        let desc = if ok != 0 && !out.is_null() {
            let d = CFCopyDescription(out);
            let s = to_string(d); CFRelease(d as _); CFRelease(out); s
        } else { "<none>".into() };
        if !err.is_null() { CFRelease(err); }
        CFRelease(k as _); CFRelease(url as _);
        (desc, ms(t))
    }
}

struct CFString;
impl CFString {
    unsafe fn from_static_str_hack(s: &str) -> CFStringRef {
        CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as CFIndex, kCFStringEncodingUTF8, 0)
    }
}
unsafe fn to_string(s: CFStringRef) -> String {
    let mut buf = [0u8; 512];
    if CFStringGetCString(s, buf.as_mut_ptr() as *mut _, 512, kCFStringEncodingUTF8) != 0 {
        String::from_utf8_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(0)]).into_owned()
    } else { "?".into() }
}

fn xattrs(p: &Path) -> (Vec<String>, f64) {
    let c = CString::new(p.as_os_str().as_bytes()).unwrap();
    let t = Instant::now();
    // SAFETY: two-call listxattr protocol with a correctly sized buffer; XATTR_NOFOLLOW avoids symlink traversal.
    unsafe {
        let n = libc::listxattr(c.as_ptr(), std::ptr::null_mut(), 0, libc::XATTR_NOFOLLOW);
        if n <= 0 { return (vec![], ms(t)); }
        let mut buf = vec![0u8; n as usize];
        libc::listxattr(c.as_ptr(), buf.as_mut_ptr() as *mut _, buf.len(), libc::XATTR_NOFOLLOW);
        (buf.split(|&b| b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect(), ms(t))
    }
}

fn main() {
    for arg in std::env::args().skip(1) {
        let p = Path::new(&arg);
        let (f0, size, blocks) = flags_of(p);
        println!("\n## {}", p.file_name().unwrap().to_string_lossy());
        println!("- lstat: st_flags={f0:#x} dataless={} st_size={size} st_blocks={blocks}", f0 & SF_DATALESS != 0);
        let chk = |what: &str, extra: String| {
            let (f, ..) = flags_of(p);
            println!("- {what}: {extra}; dataless after: {}", f & SF_DATALESS != 0);
        };
        let t = Instant::now(); let _ = std::fs::symlink_metadata(p); chk("std symlink_metadata", format!("{:.3} ms", ms(t)));
        let t = Instant::now(); let _ = std::fs::metadata(p); chk("std metadata (stat)", format!("{:.3} ms", ms(t)));
        let (x, t) = xattrs(p); chk("listxattr", format!("{:.3} ms -> {x:?}", t));
        let parent = p.parent().unwrap();
        let t = Instant::now();
        let es = list_bulk(parent, &mut || {});
        let me = es.iter().find(|e| e.name == p.file_name().unwrap().as_bytes());
        chk("getattrlistbulk(parent, with FLAGS)", format!("{:.3} ms for {} entries; entry flags={:#x} size={}", ms(t), es.len(), me.map(|e| e.flags).unwrap_or(0), me.map(|e| e.size).unwrap_or(0)));
        for key in ["NSURLUbiquitousItemDownloadingStatusKey", "NSURLIsUbiquitousItemKey", "NSURLUbiquitousItemIsDownloadingKey", "NSURLFileSizeKey", "NSURLFileAllocatedSizeKey", "NSURLUbiquitousItemIsUploadedKey"] {
            let (d, t) = cf_key(p, key);
            chk(&format!("CFURL {key}"), format!("{t:.3} ms = {d}"));
        }
    }
}

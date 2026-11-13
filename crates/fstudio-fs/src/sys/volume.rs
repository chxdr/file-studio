//! Mounted-volume queries: `statfs`, `getmntinfo`, `pathconf` and volume capabilities.

use super::cpath;
use std::io;
use std::path::{Path, PathBuf};

/// A raw `statfs` result, with strings already decoded.
#[derive(Debug, Clone)]
pub struct StatFs {
    /// Where the volume is mounted.
    pub mount_on: PathBuf,
    /// The device or remote share it comes from.
    pub mount_from: String,
    /// Filesystem type name, e.g. `apfs`, `hfs`, `smbfs`.
    pub fs_type: String,
    /// `f_flags` (mount flags).
    pub flags: u32,
    /// Block size in bytes.
    pub bsize: u64,
    /// Total blocks.
    pub blocks: u64,
    /// Blocks available to unprivileged users.
    pub bavail: u64,
}

/// `MNT_RDONLY`.
pub const MNT_RDONLY: u32 = 0x1;
/// `MNT_LOCAL`: the volume is on a local device (not a network share).
pub const MNT_LOCAL: u32 = 0x0000_1000;

#[cfg(target_os = "macos")]
fn from_statfs(s: &libc::statfs) -> StatFs {
    use std::ffi::CStr;
    // SAFETY: the kernel NUL-terminates these fixed-size arrays.
    let (on, from, ty) = unsafe {
        (
            CStr::from_ptr(s.f_mntonname.as_ptr()),
            CStr::from_ptr(s.f_mntfromname.as_ptr()),
            CStr::from_ptr(s.f_fstypename.as_ptr()),
        )
    };
    use std::os::unix::ffi::OsStrExt;
    StatFs {
        mount_on: PathBuf::from(std::ffi::OsStr::from_bytes(on.to_bytes())),
        mount_from: from.to_string_lossy().into_owned(),
        fs_type: ty.to_string_lossy().into_owned(),
        flags: s.f_flags,
        bsize: u64::from(s.f_bsize),
        blocks: s.f_blocks,
        bavail: s.f_bavail,
    }
}

/// `statfs(path)`: the volume that contains `path`.
#[cfg(target_os = "macos")]
pub fn statfs(path: &Path) -> io::Result<StatFs> {
    let c = cpath(path)?;
    // SAFETY: zeroed statfs is a valid out-buffer; `c` is a valid path.
    unsafe {
        let mut s: libc::statfs = std::mem::zeroed();
        if libc::statfs(c.as_ptr(), &mut s) != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(from_statfs(&s))
    }
}

/// `getmntinfo(MNT_NOWAIT)`: every mounted volume, without waiting for slow network mounts.
#[cfg(target_os = "macos")]
pub fn mounts() -> io::Result<Vec<StatFs>> {
    let mut p: *mut libc::statfs = std::ptr::null_mut();
    // SAFETY: getmntinfo stores a pointer to an array it owns (valid until the next call
    // on this thread); we copy everything out immediately.
    unsafe {
        let n = libc::getmntinfo(&mut p, libc::MNT_NOWAIT);
        if n <= 0 || p.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(std::slice::from_raw_parts(p, n as usize)
            .iter()
            .map(from_statfs)
            .collect())
    }
}

/// Minimal fallback: report the volume of `path` through `statvfs`.
// `f_flag`/`ST_RDONLY` widths differ by platform, so the casts are needed.
#[allow(clippy::unnecessary_cast)]
#[cfg(not(target_os = "macos"))]
pub fn statfs(path: &Path) -> io::Result<StatFs> {
    let c = cpath(path)?;
    // SAFETY: zeroed statvfs is a valid out-buffer; `c` is a valid path.
    unsafe {
        let mut s: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut s) != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(StatFs {
            mount_on: PathBuf::from("/"),
            mount_from: String::new(),
            fs_type: String::new(),
            flags: if (s.f_flag as u64 & libc::ST_RDONLY as u64) != 0 {
                MNT_RDONLY
            } else {
                0
            } | MNT_LOCAL,
            bsize: s.f_frsize as u64,
            blocks: s.f_blocks as u64,
            bavail: s.f_bavail as u64,
        })
    }
}

/// Minimal fallback: only the root volume.
#[cfg(not(target_os = "macos"))]
pub fn mounts() -> io::Result<Vec<StatFs>> {
    Ok(vec![statfs(Path::new("/"))?])
}

/// `pathconf(path, _PC_CASE_SENSITIVE)`. `None` when the filesystem does not say.
pub fn case_sensitive(path: &Path) -> Option<bool> {
    #[cfg(target_os = "macos")]
    {
        let c = cpath(path).ok()?;
        // SAFETY: valid NUL-terminated path.
        let r = unsafe { libc::pathconf(c.as_ptr(), libc::_PC_CASE_SENSITIVE) };
        if r < 0 { None } else { Some(r != 0) }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Some(true)
    }
}

/// Facts from `getattrlist(ATTR_VOL_NAME | ATTR_VOL_CAPABILITIES)`.
#[derive(Debug, Clone, Default)]
pub struct VolAttrs {
    /// The volume's display name.
    pub name: Option<String>,
    /// The filesystem supports `clonefile` (copy-on-write copies).
    pub supports_clone: bool,
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct AttrList {
    bitmapcount: u16,
    reserved: u16,
    commonattr: u32,
    volattr: u32,
    dirattr: u32,
    fileattr: u32,
    forkattr: u32,
}

/// Reads the volume name and clone capability for the volume mounted at `mount`.
#[cfg(target_os = "macos")]
pub fn vol_attrs(mount: &Path) -> Option<VolAttrs> {
    const ATTR_VOL_INFO: u32 = 0x8000_0000;
    const ATTR_VOL_NAME: u32 = 0x2000;
    const ATTR_VOL_CAPABILITIES: u32 = 0x2_0000;
    const VOL_CAP_INT_CLONE: u32 = 0x0001_0000;
    let c = cpath(mount).ok()?;
    let mut al = AttrList {
        bitmapcount: 5,
        reserved: 0,
        commonattr: 0,
        volattr: ATTR_VOL_INFO | ATTR_VOL_NAME | ATTR_VOL_CAPABILITIES,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut buf = vec![0u64; 1024 / 8];
    // SAFETY: `c` is valid; `al` is a valid attrlist; `buf` is writable for 1024 bytes.
    let r = unsafe {
        libc::getattrlist(
            c.as_ptr(),
            (&mut al as *mut AttrList).cast(),
            buf.as_mut_ptr().cast(),
            1024,
            0,
        )
    };
    if r != 0 {
        return None;
    }
    let base = buf.as_ptr().cast::<u8>();
    // SAFETY: the buffer holds: u32 total length, then attrreference_t (i32 offset, u32 length) for the
    // name, then vol_capabilities_attr_t (u32 capabilities[4], u32 valid[4]); all reads are unaligned-safe
    // and stay inside the 1024-byte buffer the kernel filled.
    unsafe {
        let rd32 = |o: usize| (base.add(o) as *const u32).read_unaligned();
        let total = rd32(0) as usize;
        if !(4 + 8 + 32..=1024).contains(&total) {
            return None;
        }
        let off = (base.add(4) as *const i32).read_unaligned() as isize;
        let len = rd32(8) as usize;
        let name = if len > 1 && (4isize + off) >= 0 && (4 + off) as usize + len <= 1024 {
            let p = base.add(4).offset(off);
            Some(String::from_utf8_lossy(std::slice::from_raw_parts(p, len - 1)).into_owned())
        } else {
            None
        };
        // vol_capabilities_attr_t starts at byte 12; index 1 is VOL_CAPABILITIES_INTERFACES.
        let interfaces = rd32(12 + 4);
        let valid_interfaces = rd32(12 + 16 + 4);
        Some(VolAttrs {
            name,
            supports_clone: valid_interfaces & VOL_CAP_INT_CLONE != 0
                && interfaces & VOL_CAP_INT_CLONE != 0,
        })
    }
}

/// Not available off macOS.
#[cfg(not(target_os = "macos"))]
pub fn vol_attrs(_mount: &Path) -> Option<VolAttrs> {
    None
}

/// What DiskArbitration says about the device behind a volume.
#[derive(Debug, Clone, Copy)]
pub struct DiskFacts {
    /// Built in to this Mac.
    pub internal: bool,
    /// Removable or ejectable media.
    pub removable: bool,
}

/// Asks DiskArbitration about a `/dev/diskNsM` device. `None` when it has no answer (network
/// shares, disk-image internals that DiskArbitration does not describe, …).
#[cfg(target_os = "macos")]
pub fn disk_facts(mount_from: &str) -> Option<DiskFacts> {
    use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::string::CFString;
    use core_foundation_sys::dictionary::{CFDictionaryGetValue, CFDictionaryRef};
    use std::ffi::{CString, c_void};

    #[link(name = "DiskArbitration", kind = "framework")]
    unsafe extern "C" {
        fn DASessionCreate(alloc: *const c_void) -> *const c_void;
        fn DADiskCreateFromBSDName(
            alloc: *const c_void,
            session: *const c_void,
            name: *const libc::c_char,
        ) -> *const c_void;
        fn DADiskCopyDescription(disk: *const c_void) -> CFDictionaryRef;
    }

    let bsd = mount_from.strip_prefix("/dev/")?;
    let cname = CString::new(bsd).ok()?;
    // SAFETY: DiskArbitration objects follow the CF create rule: everything created or copied
    // here is released before returning; `cname` outlives the call; dictionary values are only
    // read as CFBoolean after checking they are non-null (the documented value type of these keys).
    unsafe {
        let session = DASessionCreate(std::ptr::null());
        if session.is_null() {
            return None;
        }
        let disk = DADiskCreateFromBSDName(std::ptr::null(), session, cname.as_ptr());
        let mut result = None;
        if !disk.is_null() {
            let desc = DADiskCopyDescription(disk);
            if !desc.is_null() {
                let get = |key: &str| -> Option<bool> {
                    let k = CFString::new(key);
                    let v = CFDictionaryGetValue(desc, k.as_concrete_TypeRef() as *const c_void);
                    if v.is_null() {
                        None
                    } else {
                        Some(CFBoolean::wrap_under_get_rule(v as _).into())
                    }
                };
                let internal = get("DADeviceInternal");
                let removable = get("DAMediaRemovable");
                let ejectable = get("DAMediaEjectable");
                if internal.is_some() || removable.is_some() {
                    result = Some(DiskFacts {
                        internal: internal.unwrap_or(false),
                        removable: removable.unwrap_or(false) || ejectable.unwrap_or(false),
                    });
                }
                CFRelease(desc as CFTypeRef);
            }
            CFRelease(disk as CFTypeRef);
        }
        CFRelease(session as CFTypeRef);
        result
    }
}

/// Not available off macOS.
#[cfg(not(target_os = "macos"))]
pub fn disk_facts(_mount_from: &str) -> Option<DiskFacts> {
    None
}

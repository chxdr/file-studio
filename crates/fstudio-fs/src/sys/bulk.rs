//! `getattrlistbulk(2)`: one syscall returns many directory entries with their metadata.
//!
//! The attribute bitmap and the record layout follow `man 2 getattrlistbulk`.
//! Fields appear in each record in the order of their bit numbers.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;

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

unsafe extern "C" {
    fn getattrlistbulk(
        dirfd: i32,
        alist: *mut AttrList,
        buf: *mut libc::c_void,
        size: usize,
        options: u64,
    ) -> i32;
}

const ATTR_BIT_MAP_COUNT: u16 = 5;
const ATTR_CMN_NAME: u32 = 0x1;
const ATTR_CMN_DEVID: u32 = 0x2;
const ATTR_CMN_OBJTYPE: u32 = 0x8;
const ATTR_CMN_CRTIME: u32 = 0x200;
const ATTR_CMN_MODTIME: u32 = 0x400;
const ATTR_CMN_FNDRINFO: u32 = 0x4000;
const ATTR_CMN_ACCESSMASK: u32 = 0x2_0000;
const ATTR_CMN_FLAGS: u32 = 0x4_0000;
const ATTR_CMN_FILEID: u32 = 0x0200_0000;
const ATTR_CMN_ERROR: u32 = 0x2000_0000;
const ATTR_CMN_RETURNED_ATTRS: u32 = 0x8000_0000;
const ATTR_FILE_ALLOCSIZE: u32 = 0x4;
const ATTR_FILE_DATALENGTH: u32 = 0x200;

/// `kHasBundle` in the Finder flags (big-endian u16 at byte 8 of `FinderInfo`).
const K_HAS_BUNDLE: u16 = 0x2000;

/// Object types from `<sys/vnode.h>` (`fsobj_type_t`).
pub const VREG: u32 = 1;
/// Directory.
pub const VDIR: u32 = 2;
/// Symbolic link.
pub const VLNK: u32 = 5;

/// One decoded record. Names are raw bytes exactly as the filesystem stored them.
#[derive(Debug, Default)]
pub struct RawEntry {
    /// The name (UTF-8 on APFS/HFS+, raw bytes elsewhere).
    pub name: Vec<u8>,
    /// Non-zero when the kernel could not fetch this entry's attributes (an `errno`).
    pub error: u32,
    /// `fsobj_type_t`.
    pub objtype: u32,
    /// Device id.
    pub dev: u32,
    /// Birth time `(seconds, nanoseconds)`.
    pub crtime: Option<(i64, i64)>,
    /// Modification time `(seconds, nanoseconds)`.
    pub mtime: (i64, i64),
    /// Permission bits (no type bits).
    pub mode: u32,
    /// `st_flags`.
    pub flags: u32,
    /// Inode number.
    pub inode: u64,
    /// Logical (data fork) size.
    pub size: u64,
    /// Allocated size on disk, when returned.
    pub alloc: Option<u64>,
    /// The "has bundle" Finder flag.
    pub has_bundle: bool,
}

/// A directory being read with `getattrlistbulk`.
pub struct Bulk {
    file: File,
    buf: Vec<u64>,
    first_size: usize,
    big_size: usize,
    calls: usize,
    done: bool,
}

/// Why a bulk call could not be used and the caller should fall back.
pub fn is_unsupported(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::ENOTSUP) | Some(libc::ENOSYS) | Some(libc::EINVAL)
    )
}

impl Bulk {
    /// Wraps an already-open directory. The first call uses `first_size` bytes
    /// (fast first rows), later calls use `big_size` (throughput).
    pub fn new(file: File, first_size: usize, big_size: usize) -> Bulk {
        Bulk {
            file,
            buf: vec![0u64; big_size.max(first_size) / 8],
            first_size,
            big_size,
            calls: 0,
            done: false,
        }
    }

    /// Reads the next batch. `Ok(None)` means the directory is exhausted.
    pub fn next_batch(&mut self) -> io::Result<Option<Vec<RawEntry>>> {
        if self.done {
            return Ok(None);
        }
        let mut al = AttrList {
            bitmapcount: ATTR_BIT_MAP_COUNT,
            reserved: 0,
            commonattr: ATTR_CMN_RETURNED_ATTRS
                | ATTR_CMN_ERROR
                | ATTR_CMN_NAME
                | ATTR_CMN_DEVID
                | ATTR_CMN_OBJTYPE
                | ATTR_CMN_CRTIME
                | ATTR_CMN_MODTIME
                | ATTR_CMN_FNDRINFO
                | ATTR_CMN_ACCESSMASK
                | ATTR_CMN_FLAGS
                | ATTR_CMN_FILEID,
            volattr: 0,
            dirattr: 0,
            fileattr: ATTR_FILE_ALLOCSIZE | ATTR_FILE_DATALENGTH,
            forkattr: 0,
        };
        let use_size = if self.calls == 0 {
            self.first_size
        } else {
            self.big_size
        };
        self.calls += 1;
        // SAFETY: `file` is an open directory descriptor; `al` is a valid attrlist;
        // `buf` is an 8-byte-aligned allocation of at least `use_size` bytes, and the
        // kernel writes at most `use_size` bytes.
        let n = unsafe {
            getattrlistbulk(
                self.file.as_raw_fd(),
                &mut al,
                self.buf.as_mut_ptr().cast(),
                use_size,
                0,
            )
        };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n == 0 {
            self.done = true;
            return Ok(None);
        }
        let base = self.buf.as_ptr().cast::<u8>();
        let mut out = Vec::with_capacity(n as usize);
        let mut p = 0usize;
        for _ in 0..n {
            // Defence in depth: never trust a record length that would leave the
            // buffer the kernel was given (or that could loop forever).
            if p + 4 > use_size {
                break;
            }
            // SAFETY: `p + 4 <= use_size`, so the length word is inside the buffer.
            let len = unsafe { (base.add(p) as *const u32).read_unaligned() } as usize;
            if len < 24 || p + len > use_size {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("getattrlistbulk: bad record length {len} at offset {p}"),
                ));
            }
            // SAFETY: the kernel packed `n` records into `buf`; this one starts at `p`
            // and its `len` bytes lie inside the `use_size` bytes the kernel filled
            // (checked above). `parse_record` stays within that length.
            let (rec, _) = unsafe { parse_record(base.add(p)) };
            p += len;
            out.push(rec);
        }
        Ok(Some(out))
    }
}

/// Decodes one record. Returns it and the record's length in bytes.
///
/// # Safety
/// `rec` must point at a complete record produced by `getattrlistbulk` for the attribute
/// set requested in [`Bulk::next_batch`].
unsafe fn parse_record(rec: *const u8) -> (RawEntry, usize) {
    // SAFETY: all reads are unaligned-safe and stay inside the record (the caller's contract).
    unsafe {
        let len = (rec as *const u32).read_unaligned() as usize;
        let rd32 = |o: usize| (rec.add(o) as *const u32).read_unaligned();
        let rd64 = |o: usize| (rec.add(o) as *const i64).read_unaligned();
        let mut o = 4usize;
        let ret_common = rd32(o);
        let ret_file = rd32(o + 12);
        o += 20; // attribute_set_t = 5 x u32
        let mut e = RawEntry::default();
        if ret_common & ATTR_CMN_ERROR != 0 {
            e.error = rd32(o);
            o += 4;
        }
        if ret_common & ATTR_CMN_NAME != 0 {
            let off = (rec.add(o) as *const i32).read_unaligned() as isize;
            let nlen = rd32(o + 4) as usize; // includes the trailing NUL
            let np = rec.add(o).offset(off);
            e.name = std::slice::from_raw_parts(np, nlen.saturating_sub(1)).to_vec();
            o += 8;
        }
        if ret_common & ATTR_CMN_DEVID != 0 {
            e.dev = rd32(o);
            o += 4;
        }
        if ret_common & ATTR_CMN_OBJTYPE != 0 {
            e.objtype = rd32(o);
            o += 4;
        }
        if ret_common & ATTR_CMN_CRTIME != 0 {
            e.crtime = Some((rd64(o), rd64(o + 8)));
            o += 16;
        }
        if ret_common & ATTR_CMN_MODTIME != 0 {
            e.mtime = (rd64(o), rd64(o + 8));
            o += 16;
        }
        if ret_common & ATTR_CMN_FNDRINFO != 0 {
            let hi = rec.add(o + 8).read();
            let lo = rec.add(o + 9).read();
            e.has_bundle = u16::from_be_bytes([hi, lo]) & K_HAS_BUNDLE != 0;
            o += 32;
        }
        if ret_common & ATTR_CMN_ACCESSMASK != 0 {
            e.mode = rd32(o) & 0o7777;
            o += 4;
        }
        if ret_common & ATTR_CMN_FLAGS != 0 {
            e.flags = rd32(o);
            o += 4;
        }
        if ret_common & ATTR_CMN_FILEID != 0 {
            e.inode = rd64(o) as u64;
            o += 8;
        }
        if ret_file & ATTR_FILE_ALLOCSIZE != 0 {
            e.alloc = Some(rd64(o) as u64);
            o += 8;
        }
        if ret_file & ATTR_FILE_DATALENGTH != 0 {
            e.size = rd64(o) as u64;
        }
        (e, len)
    }
}

impl Bulk {
    /// Gives the directory descriptor back, for a fallback that keeps using it.
    pub fn into_file(self) -> File {
        self.file
    }
}

//! The only place in this crate that uses `unsafe`: reading a whole directory
//! with `getattrlistbulk(2)` on macOS. One call returns many entries with the
//! metadata the scanner needs (type, device, inode, link count, logical and
//! allocated size, modification time and `st_flags`), so no per-entry `lstat`
//! is needed. Nothing here ever opens a file's contents, so iCloud
//! placeholders are never downloaded (F-016).
#![allow(unsafe_code)]

#[cfg(target_os = "macos")]
pub use macos::read_dir_bulk;

#[cfg(target_os = "macos")]
mod macos {
    use std::fs::File;
    use std::io;
    use std::os::fd::AsRawFd;

    use crate::read::{EntryKind, RawEntry};

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
    const ATTR_CMN_MODTIME: u32 = 0x400;
    const ATTR_CMN_FNDRINFO: u32 = 0x4000;
    const ATTR_CMN_FLAGS: u32 = 0x4_0000;
    const ATTR_CMN_FILEID: u32 = 0x0200_0000;
    const ATTR_CMN_ERROR: u32 = 0x2000_0000;
    const ATTR_CMN_RETURNED_ATTRS: u32 = 0x8000_0000;
    const ATTR_FILE_LINKCOUNT: u32 = 0x1;
    const ATTR_FILE_TOTALSIZE: u32 = 0x2;
    const ATTR_FILE_ALLOCSIZE: u32 = 0x4;

    /// `kHasBundle` in the Finder flags (big-endian u16 at byte 8 of `FinderInfo`).
    const K_HAS_BUNDLE: u16 = 0x2000;

    const VREG: u32 = 1;
    const VDIR: u32 = 2;
    const VLNK: u32 = 5;

    /// The buffer for one call. 256 KiB holds a few thousand records, so most
    /// folders take one or two calls.
    const BUF_SIZE: usize = 256 * 1024;

    thread_local! {
        static BUF: std::cell::RefCell<Vec<u64>> = std::cell::RefCell::new(vec![0u64; BUF_SIZE / 8]);
    }

    /// Whether this error means "this filesystem has no bulk call; fall back".
    pub fn is_unsupported(e: &io::Error) -> bool {
        matches!(
            e.raw_os_error(),
            Some(libc::ENOTSUP) | Some(libc::ENOSYS) | Some(libc::EINVAL)
        )
    }

    /// Reads every entry of the open directory `dir`, appending to `out`.
    /// `Err` with [`is_unsupported`] true means the caller should read the
    /// folder another way; `out` is then left empty.
    pub fn read_dir_bulk(dir: &File, out: &mut Vec<RawEntry>) -> io::Result<()> {
        let start = out.len();
        let res = BUF.with(|b| {
            let mut buf = b.borrow_mut();
            loop {
                match next_batch(dir, &mut buf, out) {
                    Ok(true) => continue,
                    Ok(false) => return Ok(()),
                    Err(e) => return Err(e),
                }
            }
        });
        if let Err(e) = &res
            && is_unsupported(e)
        {
            out.truncate(start);
        }
        res
    }

    /// One call. `Ok(false)` means the directory is exhausted.
    fn next_batch(dir: &File, buf: &mut [u64], out: &mut Vec<RawEntry>) -> io::Result<bool> {
        let mut al = AttrList {
            bitmapcount: ATTR_BIT_MAP_COUNT,
            reserved: 0,
            commonattr: ATTR_CMN_RETURNED_ATTRS
                | ATTR_CMN_ERROR
                | ATTR_CMN_NAME
                | ATTR_CMN_DEVID
                | ATTR_CMN_OBJTYPE
                | ATTR_CMN_MODTIME
                | ATTR_CMN_FNDRINFO
                | ATTR_CMN_FLAGS
                | ATTR_CMN_FILEID,
            volattr: 0,
            dirattr: 0,
            fileattr: ATTR_FILE_LINKCOUNT | ATTR_FILE_TOTALSIZE | ATTR_FILE_ALLOCSIZE,
            forkattr: 0,
        };
        let size = buf.len() * 8;
        // SAFETY: `dir` is an open directory descriptor for the whole call; `al`
        // is a valid attrlist; `buf` is an 8-byte-aligned allocation of `size`
        // bytes and the kernel writes at most `size` bytes into it.
        let n =
            unsafe { getattrlistbulk(dir.as_raw_fd(), &mut al, buf.as_mut_ptr().cast(), size, 0) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n == 0 {
            return Ok(false);
        }
        let base = buf.as_ptr().cast::<u8>();
        let mut p = 0usize;
        for _ in 0..n {
            if p + 4 > size {
                break;
            }
            // SAFETY: `p + 4 <= size`, so the length word is inside the buffer.
            let len = unsafe { (base.add(p) as *const u32).read_unaligned() } as usize;
            if len < 24 || p + len > size {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("getattrlistbulk: bad record length {len} at offset {p}"),
                ));
            }
            // SAFETY: the record starts at `p` and its `len` bytes lie inside the
            // `size` bytes the kernel filled (checked above); `parse` reads only
            // the attributes requested above, all inside the record, and the name
            // through its in-record offset, which it bounds-checks against `len`.
            let rec = unsafe { std::slice::from_raw_parts(base.add(p), len) };
            out.push(parse(rec));
            p += len;
        }
        Ok(true)
    }

    fn rd32(rec: &[u8], o: usize) -> u32 {
        rec.get(o..o + 4)
            .map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
            .unwrap_or(0)
    }

    fn rd64(rec: &[u8], o: usize) -> i64 {
        rec.get(o..o + 8)
            .map(|b| i64::from_ne_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
            .unwrap_or(0)
    }

    /// Decodes one record, in the order of `man 2 getattrlistbulk`: the
    /// returned-attributes set, the error, then attributes by bit number.
    fn parse(rec: &[u8]) -> RawEntry {
        let ret_common = rd32(rec, 4);
        let ret_file = rd32(rec, 4 + 12);
        let mut o = 4 + 20;
        let mut e = RawEntry::default();
        if ret_common & ATTR_CMN_ERROR != 0 {
            e.error = rd32(rec, o) as i32;
            o += 4;
        }
        if ret_common & ATTR_CMN_NAME != 0 {
            let off = rd32(rec, o) as i32 as isize;
            let nlen = rd32(rec, o + 4) as usize; // includes the trailing NUL
            let start = o as isize + off;
            if start >= 0 {
                let start = start as usize;
                let end = start + nlen.saturating_sub(1);
                if let Some(name) = rec.get(start..end) {
                    e.name = name.to_vec();
                }
            }
            o += 8;
        }
        if ret_common & ATTR_CMN_DEVID != 0 {
            e.dev = u64::from(rd32(rec, o));
            o += 4;
        }
        let mut objtype = 0;
        if ret_common & ATTR_CMN_OBJTYPE != 0 {
            objtype = rd32(rec, o);
            o += 4;
        }
        if ret_common & ATTR_CMN_MODTIME != 0 {
            let s = rd64(rec, o);
            let ns = rd64(rec, o + 8);
            e.mtime_ns = s.saturating_mul(1_000_000_000).saturating_add(ns);
            o += 16;
        }
        if ret_common & ATTR_CMN_FNDRINFO != 0 {
            let hi = rec.get(o + 8).copied().unwrap_or(0);
            let lo = rec.get(o + 9).copied().unwrap_or(0);
            e.has_bundle = u16::from_be_bytes([hi, lo]) & K_HAS_BUNDLE != 0;
            o += 32;
        }
        if ret_common & ATTR_CMN_FLAGS != 0 {
            e.flags = rd32(rec, o);
            o += 4;
        }
        if ret_common & ATTR_CMN_FILEID != 0 {
            e.ino = rd64(rec, o) as u64;
            o += 8;
        }
        if ret_file & ATTR_FILE_LINKCOUNT != 0 {
            e.nlink = rd32(rec, o);
            o += 4;
        }
        if ret_file & ATTR_FILE_TOTALSIZE != 0 {
            e.logical = rd64(rec, o).max(0) as u64;
            o += 8;
        }
        if ret_file & ATTR_FILE_ALLOCSIZE != 0 {
            e.allocated = rd64(rec, o).max(0) as u64;
        }
        e.kind = match objtype {
            VREG => EntryKind::File,
            VDIR => EntryKind::Dir,
            VLNK => EntryKind::Symlink,
            _ => EntryKind::Other,
        };
        e
    }
}

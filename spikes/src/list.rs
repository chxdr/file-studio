//! Four directory-listing strategies. Each calls `on_first100` once when the 100th entry
//! (with metadata) is available, and returns all entries with metadata.
use std::ffi::{CStr, CString};
use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::Path;

pub const K_REG: u8 = 1;
pub const K_DIR: u8 = 2;
pub const K_LNK: u8 = 5;
pub const K_OTHER: u8 = 0;

#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub name: Vec<u8>,
    pub kind: u8,
    pub size: u64,
    pub mtime_s: i64,
    pub mtime_ns: i64,
    pub flags: u32, // st_flags (UF_*/SF_*), e.g. SF_DATALESS
}

fn kind_of(mode: u32) -> u8 {
    match mode & libc::S_IFMT as u32 {
        m if m == libc::S_IFREG as u32 => K_REG,
        m if m == libc::S_IFDIR as u32 => K_DIR,
        m if m == libc::S_IFLNK as u32 => K_LNK,
        _ => K_OTHER,
    }
}

fn entry_from_stat(name: Vec<u8>, st: &libc::stat) -> Entry {
    Entry {
        name,
        kind: kind_of(st.st_mode as u32),
        size: st.st_size as u64,
        mtime_s: st.st_mtime,
        mtime_ns: st.st_mtime_nsec,
        flags: st.st_flags,
    }
}

/// lstat relative to an open directory fd (no path re-resolution).
pub fn lstat_at(dirfd: i32, name: &[u8]) -> Option<libc::stat> {
    let c = CString::new(name).ok()?;
    // SAFETY: zeroed `stat` is a valid out-buffer; `c` is NUL-terminated and outlives the call;
    // `dirfd` is a valid open directory descriptor held by the caller.
    unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        if libc::fstatat(dirfd, c.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) == 0 { Some(st) } else { None }
    }
}

// ---------- 1. std::fs::read_dir + symlink_metadata ----------
pub fn list_std(dir: &Path, first100: &mut dyn FnMut()) -> Vec<Entry> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let e = e.unwrap();
        let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
        out.push(Entry {
            name: e.file_name().into_vec(),
            kind: kind_of(m.mode()),
            size: m.size(),
            mtime_s: m.mtime(),
            mtime_ns: m.mtime_nsec(),
            flags: 0, // std does not expose st_flags
        });
        if out.len() == 100 { first100(); }
    }
    out
}

// ---------- 1b. std::fs::read_dir + DirEntry::metadata (fstatat under the hood) ----------
pub fn list_std_direntry(dir: &Path, first100: &mut dyn FnMut()) -> Vec<Entry> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let e = e.unwrap();
        let Ok(m) = e.metadata() else { continue };
        let _ = m.file_type().is_socket();
        out.push(Entry {
            name: e.file_name().into_vec(),
            kind: kind_of(m.mode()),
            size: m.size(),
            mtime_s: m.mtime(),
            mtime_ns: m.mtime_nsec(),
            flags: 0,
        });
        if out.len() == 100 { first100(); }
    }
    out
}

// ---------- 2. getattrlistbulk(2) ----------
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
extern "C" {
    // man 2 getattrlistbulk. Returns number of entries packed in buf, 0 at end, -1 on error.
    fn getattrlistbulk(dirfd: i32, alist: *mut AttrList, buf: *mut libc::c_void, size: usize, options: u64) -> i32;
}
const ATTR_BIT_MAP_COUNT: u16 = 5;
const ATTR_CMN_NAME: u32 = 0x1;
const ATTR_CMN_OBJTYPE: u32 = 0x8;
const ATTR_CMN_MODTIME: u32 = 0x400;
const ATTR_CMN_FLAGS: u32 = 0x40000;
const ATTR_CMN_ERROR: u32 = 0x2000_0000;
const ATTR_CMN_RETURNED_ATTRS: u32 = 0x8000_0000;
const ATTR_FILE_DATALENGTH: u32 = 0x200;

pub fn list_bulk(dir: &Path, first100: &mut dyn FnMut()) -> Vec<Entry> {
    list_bulk_buf(dir, first100, 256 * 1024, 256 * 1024)
}

/// First call uses a small buffer (fast first rows), later calls the big one (throughput).
pub fn list_bulk_adaptive(dir: &Path, first100: &mut dyn FnMut()) -> Vec<Entry> {
    list_bulk_buf(dir, first100, 16 * 1024, 256 * 1024)
}

pub fn list_bulk_buf(dir: &Path, first100: &mut dyn FnMut(), first_size: usize, bufsize: usize) -> Vec<Entry> {
    let f = File::open(dir).unwrap();
    let mut al = AttrList {
        bitmapcount: ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: ATTR_CMN_RETURNED_ATTRS | ATTR_CMN_ERROR | ATTR_CMN_NAME | ATTR_CMN_OBJTYPE | ATTR_CMN_MODTIME | ATTR_CMN_FLAGS,
        volattr: 0,
        dirattr: 0,
        fileattr: ATTR_FILE_DATALENGTH,
        forkattr: 0,
    };
    let mut buf = vec![0u64; bufsize / 8]; // 8-byte aligned
    let mut out = Vec::new();
    let mut fired = false;
    let mut first_call = true;
    loop {
        // SAFETY: `f` is an open directory fd; `al` is a valid attrlist; `buf` is a writable
        // allocation of `bufsize` bytes. The kernel writes at most `bufsize` bytes.
        let use_size = if first_call { first_size } else { bufsize };
        first_call = false;
        let n = unsafe { getattrlistbulk(f.as_raw_fd(), &mut al, buf.as_mut_ptr() as *mut _, use_size, 0) };
        if n <= 0 {
            assert!(n == 0, "getattrlistbulk: {}", std::io::Error::last_os_error());
            break;
        }
        let base = buf.as_ptr() as *const u8;
        let mut p = 0usize;
        for _ in 0..n {
            // SAFETY: the kernel packed `n` records into buf; each starts with its u32 length and
            // fields follow the documented order for the requested attribute bitmap. All reads are
            // unaligned-safe and stay within the record length.
            unsafe {
                let rec = base.add(p);
                let len = (rec as *const u32).read_unaligned() as usize;
                let mut o = 4usize;
                let rd32 = |o: usize| (rec.add(o) as *const u32).read_unaligned();
                let ret_common = rd32(o);
                let ret_file = rd32(o + 12);
                o += 20; // attribute_set_t = 5 x u32
                let mut e = Entry::default();
                let mut err = 0;
                if ret_common & ATTR_CMN_ERROR != 0 { err = rd32(o); o += 4; }
                if ret_common & ATTR_CMN_NAME != 0 {
                    let off = (rec.add(o) as *const i32).read_unaligned() as isize;
                    let nlen = rd32(o + 4) as usize; // includes trailing NUL
                    let np = rec.add(o).offset(off);
                    e.name = std::slice::from_raw_parts(np, nlen.saturating_sub(1)).to_vec();
                    o += 8;
                }
                if ret_common & ATTR_CMN_OBJTYPE != 0 { e.kind = rd32(o) as u8; o += 4; }
                if ret_common & ATTR_CMN_MODTIME != 0 {
                    e.mtime_s = (rec.add(o) as *const i64).read_unaligned();
                    e.mtime_ns = (rec.add(o + 8) as *const i64).read_unaligned();
                    o += 16;
                }
                if ret_common & ATTR_CMN_FLAGS != 0 { e.flags = rd32(o); o += 4; }
                if ret_file & ATTR_FILE_DATALENGTH != 0 { e.size = (rec.add(o) as *const i64).read_unaligned() as u64; }
                p += len;
                if err == 0 && !e.name.is_empty() { out.push(e); }
            }
            if !fired && out.len() >= 100 { first100(); fired = true; }
        }
    }
    out
}

// ---------- 3. fts(3) ----------
// Layout from <fts_compat.h> (the plain `fts_*` symbols on macOS < 28 deployment targets).
// NB: the macOS 27 SDK also ships fts_new.h (symbols suffixed $NFTS) with a DIFFERENT FTSENT layout.
#[repr(C)]
struct Ftsent {
    fts_cycle: *mut Ftsent,
    fts_parent: *mut Ftsent,
    fts_link: *mut Ftsent,
    fts_number: libc::c_long,
    fts_pointer: *mut libc::c_void,
    fts_accpath: *mut libc::c_char,
    fts_path: *mut libc::c_char,
    fts_errno: libc::c_int,
    fts_symfd: libc::c_int,
    fts_pathlen: u16,
    fts_namelen: u16,
    fts_ino: u64,
    fts_dev: i32,
    fts_nlink: u16,
    fts_level: i16,
    fts_info: u16,
    fts_flags: u16,
    fts_instr: u16,
    fts_statp: *mut libc::stat,
    fts_name: [libc::c_char; 1],
}
enum Fts {}
extern "C" {
    fn fts_open(argv: *const *mut libc::c_char, options: libc::c_int, compar: *const libc::c_void) -> *mut Fts;
    fn fts_read(f: *mut Fts) -> *mut Ftsent;
    fn fts_set(f: *mut Fts, e: *mut Ftsent, instr: libc::c_int) -> libc::c_int;
    fn fts_close(f: *mut Fts) -> libc::c_int;
}
const FTS_NOCHDIR: i32 = 0x4;
const FTS_PHYSICAL: i32 = 0x10;
const FTS_D: u16 = 1;
const FTS_DP: u16 = 6;
const FTS_SKIP: i32 = 4;

pub fn list_fts(dir: &Path, first100: &mut dyn FnMut()) -> Vec<Entry> {
    let root = CString::new(dir.as_os_str().as_bytes()).unwrap();
    let argv = [root.as_ptr() as *mut libc::c_char, std::ptr::null_mut()];
    let mut out = Vec::new();
    // SAFETY: argv is a NULL-terminated array of valid C strings that outlive the stream; the
    // returned FTSENT pointers are only dereferenced before the next fts_read/fts_close, and the
    // struct layout matches <fts_compat.h> for this SDK (verified by the name-length assert).
    unsafe {
        let fts = fts_open(argv.as_ptr(), FTS_PHYSICAL | FTS_NOCHDIR, std::ptr::null());
        assert!(!fts.is_null());
        loop {
            let e = fts_read(fts);
            if e.is_null() { break; }
            let ent = &*e;
            if ent.fts_level == 0 || ent.fts_info == FTS_DP { continue; }
            if ent.fts_info == FTS_D { fts_set(fts, e, FTS_SKIP); } // do not recurse
            if ent.fts_statp.is_null() { continue; }
            let name = CStr::from_ptr(ent.fts_name.as_ptr()).to_bytes();
            debug_assert_eq!(name.len(), ent.fts_namelen as usize);
            out.push(entry_from_stat(name.to_vec(), &*ent.fts_statp));
            if out.len() == 100 { first100(); }
        }
        fts_close(fts);
    }
    out
}

// ---------- 4. readdir + parallel lstat (rayon) ----------
pub fn list_rayon(dir: &Path, first100: &mut dyn FnMut()) -> Vec<Entry> {
    use rayon::prelude::*;
    let f = File::open(dir).unwrap();
    let fd = f.as_raw_fd();
    let mut out = Vec::new();
    let mut rest: Vec<Vec<u8>> = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let name = e.unwrap().file_name().into_vec();
        if out.len() < 100 {
            if let Some(st) = lstat_at(fd, &name) { out.push(entry_from_stat(name, &st)); }
            if out.len() == 100 { first100(); }
        } else {
            rest.push(name);
        }
    }
    let tail: Vec<Entry> = rest
        .into_par_iter()
        .filter_map(|n| lstat_at(fd, &n).map(|st| entry_from_stat(n, &st)))
        .collect();
    out.extend(tail);
    out
}

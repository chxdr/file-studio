//! Streaming directory listing and single-entry stat.
//!
//! Listing never follows symlinks: a link is reported as a link, with the size and
//! times of the link itself (F-007/F-013).

use crate::error::{Error, Result};
use crate::package::is_package_name;
use fstudio_model::{Entry, EntryKind};
use std::fs;
use std::path::{Path, PathBuf};

/// An entry that could not be listed, and why.
#[derive(Debug)]
pub struct Skipped {
    /// The name, as far as it is known.
    pub name: String,
    /// The OS error.
    pub error: std::io::Error,
}

/// How many entries the portable and fallback paths gather per chunk.
const STD_CHUNK: usize = 1024;

enum Inner {
    #[cfg(target_os = "macos")]
    Bulk(crate::sys::bulk::Bulk),
    #[cfg(target_os = "macos")]
    Readdir {
        dirfd: fs::File,
        iter: fs::ReadDir,
    },
    #[cfg(not(target_os = "macos"))]
    Std(fs::ReadDir),
    Done,
}

/// A directory listing that hands out entries in chunks, so the first rows can be shown
/// before the rest are read.
///
/// On macOS this uses `getattrlistbulk` (a 16 KiB buffer for the first call, 256 KiB after)
/// and falls back to `readdir` plus parallel `fstatat` on filesystems that don't support it.
/// Elsewhere it uses `read_dir` and `DirEntry::metadata`.
///
/// Entries that can't be read are skipped and recorded in [`Lister::skipped`].
pub struct Lister {
    dir: PathBuf,
    inner: Inner,
    skipped: Vec<Skipped>,
}

impl Lister {
    /// Opens `dir` for listing. Fails when it can't be opened (missing, not a directory,
    /// permission denied).
    pub fn open(dir: impl AsRef<Path>) -> Result<Lister> {
        let dir = dir.as_ref();
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let file = fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY)
                .open(dir)
                .map_err(|e| Error::io(dir, e))?;
            Ok(Lister {
                dir: dir.to_path_buf(),
                inner: Inner::Bulk(crate::sys::bulk::Bulk::new(file, 16 * 1024, 256 * 1024)),
                skipped: Vec::new(),
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let it = fs::read_dir(dir).map_err(|e| Error::io(dir, e))?;
            Ok(Lister {
                dir: dir.to_path_buf(),
                inner: Inner::Std(it),
                skipped: Vec::new(),
            })
        }
    }

    /// Like [`Lister::open`] but always uses the `readdir` + parallel `fstatat` path that
    /// `getattrlistbulk` falls back to. For testing the fallback and for filesystems known not
    /// to support bulk listing.
    #[doc(hidden)]
    pub fn open_readdir(dir: impl AsRef<Path>) -> Result<Lister> {
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let dir = dir.as_ref();
            let dirfd = fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY)
                .open(dir)
                .map_err(|e| Error::io(dir, e))?;
            let iter = fs::read_dir(dir).map_err(|e| Error::io(dir, e))?;
            Ok(Lister {
                dir: dir.to_path_buf(),
                inner: Inner::Readdir { dirfd, iter },
                skipped: Vec::new(),
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            Lister::open(dir)
        }
    }

    /// The directory being listed.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Entries skipped so far (stat failed, or the entry vanished or was unreadable).
    pub fn skipped(&self) -> &[Skipped] {
        &self.skipped
    }

    /// The next chunk of entries, `None` at the end. After an `Err` the listing is over.
    pub fn next_chunk(&mut self) -> Option<Result<Vec<Entry>>> {
        loop {
            let r = self.step();
            match r {
                // An empty chunk (every entry in the batch was skipped) is not the end.
                Some(Ok(v)) if v.is_empty() => continue,
                Some(Err(_)) => {
                    self.inner = Inner::Done;
                    return r;
                }
                other => return other,
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn step(&mut self) -> Option<Result<Vec<Entry>>> {
        match &mut self.inner {
            Inner::Done => None,
            Inner::Bulk(b) => match b.next_batch() {
                Ok(None) => {
                    self.inner = Inner::Done;
                    None
                }
                Ok(Some(batch)) => {
                    let mut out = Vec::with_capacity(batch.len());
                    for raw in batch {
                        if raw.error != 0 || raw.name.is_empty() {
                            self.skipped.push(Skipped {
                                name: String::from_utf8_lossy(&raw.name).into_owned(),
                                error: std::io::Error::from_raw_os_error(raw.error as i32),
                            });
                            continue;
                        }
                        out.push(entry_from_raw(raw));
                    }
                    Some(Ok(out))
                }
                Err(e) if crate::sys::bulk::is_unsupported(&e) => {
                    // Switch to readdir + fstatat on the same descriptor. Nothing has been
                    // returned yet when the very first call is what failed.
                    let old = std::mem::replace(&mut self.inner, Inner::Done);
                    let Inner::Bulk(b) = old else { unreachable!() };
                    match fs::read_dir(&self.dir) {
                        Ok(iter) => {
                            self.inner = Inner::Readdir {
                                dirfd: b.into_file(),
                                iter,
                            };
                            self.step()
                        }
                        Err(e) => Some(Err(Error::io(&self.dir, e))),
                    }
                }
                Err(e) => Some(Err(Error::io(&self.dir, e))),
            },
            Inner::Readdir { dirfd, iter } => {
                use rayon::prelude::*;
                use std::os::fd::AsRawFd;
                use std::os::unix::ffi::OsStringExt;
                let mut names: Vec<Vec<u8>> = Vec::with_capacity(STD_CHUNK);
                while names.len() < STD_CHUNK {
                    match iter.next() {
                        Some(Ok(e)) => names.push(e.file_name().into_vec()),
                        Some(Err(e)) => {
                            self.skipped.push(Skipped {
                                name: String::new(),
                                error: e,
                            });
                        }
                        None => break,
                    }
                }
                if names.is_empty() {
                    self.inner = Inner::Done;
                    return None;
                }
                let fd = dirfd.as_raw_fd();
                let results: Vec<_> = names
                    .into_par_iter()
                    .map(|n| {
                        let r = crate::sys::lstat_at(fd, &n);
                        (n, r)
                    })
                    .collect();
                let mut out = Vec::with_capacity(results.len());
                for (n, r) in results {
                    match r {
                        Ok(st) => out.push(entry_from_stat(
                            String::from_utf8_lossy(&n).into_owned(),
                            &st,
                        )),
                        Err(e) => self.skipped.push(Skipped {
                            name: String::from_utf8_lossy(&n).into_owned(),
                            error: e,
                        }),
                    }
                }
                Some(Ok(out))
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn step(&mut self) -> Option<Result<Vec<Entry>>> {
        let Inner::Std(it) = &mut self.inner else {
            return None;
        };
        let mut out = Vec::with_capacity(STD_CHUNK);
        let mut ended = false;
        while out.len() < STD_CHUNK {
            match it.next() {
                None => {
                    ended = true;
                    break;
                }
                Some(Err(e)) => self.skipped.push(Skipped {
                    name: String::new(),
                    error: e,
                }),
                Some(Ok(de)) => {
                    let name = de.file_name().to_string_lossy().into_owned();
                    match de.metadata() {
                        Ok(m) => out.push(entry_from_metadata(name, &m)),
                        Err(e) => self.skipped.push(Skipped { name, error: e }),
                    }
                }
            }
        }
        if ended {
            self.inner = Inner::Done;
            if out.is_empty() {
                return None;
            }
        }
        Some(Ok(out))
    }
}

/// Lists a whole directory at once. Unreadable entries are left out.
pub fn list_all(dir: impl AsRef<Path>) -> Result<Vec<Entry>> {
    let mut l = Lister::open(dir)?;
    let mut all = Vec::new();
    while let Some(chunk) = l.next_chunk() {
        all.extend(chunk?);
    }
    Ok(all)
}

/// Stats one path with `lstat` (a symlink is described, not followed).
pub fn stat_entry(path: impl AsRef<Path>) -> Result<Entry> {
    let path = path.as_ref();
    let m = fs::symlink_metadata(path).map_err(|e| Error::io(path, e))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    Ok(entry_from_metadata(name, &m))
}

fn kind_from_mode(mode: u32) -> EntryKind {
    match mode & libc::S_IFMT as u32 {
        m if m == libc::S_IFREG as u32 => EntryKind::File,
        m if m == libc::S_IFDIR as u32 => EntryKind::Dir,
        m if m == libc::S_IFLNK as u32 => EntryKind::Symlink,
        _ => EntryKind::Other,
    }
}

fn ns(sec: i64, nsec: i64) -> i64 {
    sec.saturating_mul(1_000_000_000).saturating_add(nsec)
}

#[cfg(unix)]
fn entry_from_metadata(name: String, m: &fs::Metadata) -> Entry {
    use std::os::unix::fs::MetadataExt;
    let mode = m.mode();
    let kind = kind_from_mode(mode);
    #[cfg(target_os = "macos")]
    let flags = std::os::macos::fs::MetadataExt::st_flags(m);
    #[cfg(not(target_os = "macos"))]
    let flags = 0;
    let btime_ns = m
        .created()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64);
    Entry {
        is_package: kind == EntryKind::Dir && is_package_name(&name),
        name,
        kind,
        size: if kind == EntryKind::Dir { 0 } else { m.size() },
        alloc_size: (kind != EntryKind::Dir).then(|| m.blocks().saturating_mul(512)),
        mtime_ns: ns(m.mtime(), m.mtime_nsec()),
        btime_ns,
        mode,
        flags,
        inode: m.ino(),
        dev: m.dev(),
    }
}

#[cfg(target_os = "macos")]
fn entry_from_stat(name: String, st: &libc::stat) -> Entry {
    let mode = u32::from(st.st_mode);
    let kind = kind_from_mode(mode);
    Entry {
        is_package: kind == EntryKind::Dir && is_package_name(&name),
        name,
        kind,
        size: if kind == EntryKind::Dir {
            0
        } else {
            st.st_size as u64
        },
        alloc_size: (kind != EntryKind::Dir).then(|| (st.st_blocks as u64).saturating_mul(512)),
        mtime_ns: ns(st.st_mtime, st.st_mtime_nsec),
        btime_ns: Some(ns(st.st_birthtime, st.st_birthtime_nsec)),
        mode,
        flags: st.st_flags,
        inode: st.st_ino,
        dev: st.st_dev as u32 as u64,
    }
}

#[cfg(target_os = "macos")]
fn entry_from_raw(raw: crate::sys::bulk::RawEntry) -> Entry {
    use crate::sys::bulk::{VDIR, VLNK, VREG};
    let kind = match raw.objtype {
        VREG => EntryKind::File,
        VDIR => EntryKind::Dir,
        VLNK => EntryKind::Symlink,
        _ => EntryKind::Other,
    };
    let type_bits = match kind {
        EntryKind::File => libc::S_IFREG as u32,
        EntryKind::Dir => libc::S_IFDIR as u32,
        EntryKind::Symlink => libc::S_IFLNK as u32,
        EntryKind::Other => 0,
    };
    let name = String::from_utf8(raw.name)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
    let is_dir = kind == EntryKind::Dir;
    Entry {
        is_package: is_dir && (raw.has_bundle || is_package_name(&name)),
        name,
        kind,
        size: if is_dir { 0 } else { raw.size },
        alloc_size: if is_dir { None } else { raw.alloc },
        mtime_ns: ns(raw.mtime.0, raw.mtime.1),
        btime_ns: raw.crtime.map(|(s, n)| ns(s, n)),
        mode: raw.mode | type_bits,
        flags: raw.flags,
        inode: raw.inode,
        dev: u64::from(raw.dev),
    }
}

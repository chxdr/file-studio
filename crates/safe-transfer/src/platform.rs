//! The platform traits: flushing, uncached I/O, exclusive rename, volume
//! identity, the Trash, cloning and metadata. [`Platform::native`] bundles
//! this machine's implementation; tests swap single parts for fakes.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::sys;

/// How far a flush got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FlushLevel {
    /// Only the operating system's flush worked (`fsync`); the drive may
    /// still hold the data in its own cache.
    OsOnly,
    /// The drive was asked to empty its write cache and said it did
    /// (`F_FULLFSYNC` on macOS).
    Device,
}

/// How a file was put in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameMode {
    /// An atomic rename that can never replace anything (`RENAME_EXCL`).
    Exclusive,
    /// The filesystem can't rename exclusively (exFAT, FAT, SMB): the name
    /// was checked to be free, then renamed. There is a tiny race window.
    CheckThenRename,
}

/// How many of a file's pages are in the operating system's cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Residency {
    /// Pages in the cache.
    pub resident_pages: u64,
    /// Pages in the file.
    pub total_pages: u64,
}

/// A mounted volume, as Safe Transfer recognises it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeInfo {
    /// The volume UUID. Stable across unplugging and remounting, also on
    /// exFAT and FAT (derived from the volume serial number). `None` for
    /// volumes that have none (some network shares).
    pub uuid: Option<String>,
    /// Filesystem type as the OS names it ("apfs", "exfat", "msdos", "hfs"…).
    pub fs_type: String,
    /// The volume's name, for messages ("Untitled"). Never used to identify it.
    pub name: String,
    /// Where it is mounted.
    pub mount: PathBuf,
    /// The device it is mounted from ("/dev/disk4s1").
    pub device: String,
    /// Total size in bytes.
    pub capacity: u64,
    /// Free bytes available to this user.
    pub free: u64,
    /// The allocation unit in bytes (a file takes a whole number of these).
    pub block_size: u64,
}

/// Makes written data durable.
pub trait Flush: Send + Sync {
    /// Flushes the file's data and asks the drive to empty its write cache.
    fn flush_file(&self, f: &File) -> io::Result<FlushLevel>;
    /// The same for a folder, so a rename inside it survives a power cut.
    fn flush_dir(&self, dir: &Path) -> io::Result<FlushLevel>;
    /// Sends the file's data and attributes to the drive without a drive
    /// barrier (`fsync(2)`); a group of these is followed by one
    /// [`Flush::flush_file`]. Defaults to a full flush.
    fn os_flush(&self, f: &File) -> io::Result<()> {
        self.flush_file(f).map(|_| ())
    }
}

/// Reads and writes that don't go through, or stay in, the OS cache.
pub trait Uncached: Send + Sync {
    /// Stops this descriptor's I/O from filling the cache.
    fn set_uncached(&self, f: &File) -> io::Result<()>;
    /// Drops the file's cached pages and says how many are left.
    fn evict(&self, path: &Path) -> io::Result<Residency>;
    /// How many of the file's pages are cached, without changing anything.
    fn residency(&self, path: &Path) -> io::Result<Residency>;
    /// [`Uncached::evict`] through an open descriptor of `path`.
    fn evict_open(&self, _f: &File, path: &Path) -> io::Result<Residency> {
        self.evict(path)
    }
}

/// Puts a file in place without ever replacing something.
pub trait AtomicRename: Send + Sync {
    /// Renames, failing with `AlreadyExists` if `to` is taken.
    fn rename_exclusive(&self, from: &Path, to: &Path) -> io::Result<RenameMode>;
}

/// Recognises volumes across unplugging and remounting.
pub trait VolumeIdentity: Send + Sync {
    /// The volume holding `path`.
    fn volume_of(&self, path: &Path) -> io::Result<VolumeInfo>;
    /// Every mounted volume.
    fn mounted(&self) -> io::Result<Vec<VolumeInfo>>;
    /// Whether names in `dir` are case-sensitive (F-017). Unknown counts as
    /// insensitive, the cautious answer for conflict prediction.
    fn case_sensitive(&self, dir: &Path) -> io::Result<bool>;
}

/// The user's Trash, with Put Back.
pub trait Trash: Send + Sync {
    /// Moves `path` to the Trash and says where it went.
    fn trash(&self, path: &Path) -> io::Result<PathBuf>;
}

/// A Trash that is a folder, for tests and scripts: items are renamed into
/// it (never copied or removed), with Finder's "name 2" for clashes. It must
/// be on the same volume as what it trashes.
#[derive(Debug, Clone)]
pub struct DirTrash {
    dir: PathBuf,
}

impl DirTrash {
    /// Uses (and creates) `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> io::Result<DirTrash> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(DirTrash { dir })
    }
}

impl Trash for DirTrash {
    fn trash(&self, path: &Path) -> io::Result<PathBuf> {
        let md = std::fs::symlink_metadata(path)?;
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?
            .to_string_lossy()
            .into_owned();
        let names =
            std::iter::once(name.clone()).chain(crate::names::keep_both_names(&name, md.is_dir()));
        for n in names {
            let to = self.dir.join(&n);
            match sys::rename_excl(path, &to) {
                Ok(_) => return Ok(to),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        unreachable!("keep_both_names is endless")
    }
}

/// Same-volume cloning (APFS): a new file sharing the original's blocks.
pub trait CloneFile: Send + Sync {
    /// Clones `src` to the new file `dst`. Errors when the volume can't clone.
    fn clone_file(&self, src: &Path, dst: &Path) -> io::Result<()>;
}

/// Copying and inspecting metadata.
pub trait Metadata: Send + Sync {
    /// Extended attributes and stat information (mode, dates, flags).
    fn copy_metadata(&self, src: &Path, dst: &Path) -> io::Result<()>;
    /// Removes ACLs and immutable flags that would block a rename or removal.
    fn clear_security(&self, path: &Path) -> io::Result<()>;
    /// Applies the source's ACL and immutable flags (after the rename).
    fn apply_security(&self, src: &Path, dst: &Path) -> io::Result<()>;
    /// What an item carries, for the "couldn't preserve" report.
    fn snapshot(&self, path: &Path) -> io::Result<MetaSnapshot>;
}

/// The metadata of one item that Safe Transfer tries to preserve.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MetaSnapshot {
    /// Permission bits.
    pub mode: u32,
    /// Owner user id.
    pub uid: u32,
    /// Modification time, ns since the epoch.
    pub mtime_ns: i64,
    /// Creation time, ns since the epoch, if kept.
    pub birth_ns: Option<i64>,
    /// Names of extended attributes (tags, comments, resource fork…).
    pub xattrs: Vec<String>,
    /// Whether an extended ACL is set.
    pub acl: bool,
    /// User-settable BSD flags (locked, hidden…).
    pub flags: u32,
}

/// One implementation of every platform trait.
#[derive(Clone)]
pub struct Platform {
    /// Flushing.
    pub flush: Arc<dyn Flush>,
    /// Uncached I/O and eviction.
    pub uncached: Arc<dyn Uncached>,
    /// Exclusive rename.
    pub rename: Arc<dyn AtomicRename>,
    /// Volume identity.
    pub volumes: Arc<dyn VolumeIdentity>,
    /// The Trash.
    pub trash: Arc<dyn Trash>,
    /// Cloning.
    pub clone: Arc<dyn CloneFile>,
    /// Metadata.
    pub meta: Arc<dyn Metadata>,
}

impl Platform {
    /// This machine's implementation (macOS now; Linux and Windows are stubs).
    pub fn native() -> Platform {
        let n = Arc::new(Native);
        Platform {
            flush: n.clone(),
            uncached: n.clone(),
            rename: n.clone(),
            volumes: n.clone(),
            trash: n.clone(),
            clone: n.clone(),
            meta: n,
        }
    }
}

impl std::fmt::Debug for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Platform")
    }
}

/// The native implementation, on top of `sys`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Native;

/// `UF_IMMUTABLE | UF_APPEND`: flags that stop a rename or removal.
pub const IMMUTABLE_FLAGS: u32 = 0x0000_0002 | 0x0000_0004;
/// The flags a user can set and that we preserve (`UF_SETTABLE`).
const USER_FLAGS: u32 = 0x0000_ffff;

impl Flush for Native {
    fn flush_file(&self, f: &File) -> io::Result<FlushLevel> {
        sys::full_sync(f)
    }
    fn flush_dir(&self, dir: &Path) -> io::Result<FlushLevel> {
        sys::sync_dir(dir)
    }
    fn os_flush(&self, f: &File) -> io::Result<()> {
        sys::os_sync(f)
    }
}

impl Uncached for Native {
    fn set_uncached(&self, f: &File) -> io::Result<()> {
        sys::set_nocache(f)
    }
    fn evict(&self, path: &Path) -> io::Result<Residency> {
        sys::evict(path)
    }
    fn residency(&self, path: &Path) -> io::Result<Residency> {
        sys::residency(path)
    }
    fn evict_open(&self, f: &File, _path: &Path) -> io::Result<Residency> {
        sys::evict_open(f)
    }
}

impl AtomicRename for Native {
    fn rename_exclusive(&self, from: &Path, to: &Path) -> io::Result<RenameMode> {
        sys::rename_excl(from, to)
    }
}

impl VolumeIdentity for Native {
    fn volume_of(&self, path: &Path) -> io::Result<VolumeInfo> {
        sys::volume_info(path)
    }
    fn mounted(&self) -> io::Result<Vec<VolumeInfo>> {
        sys::mounted_volumes()
    }
    fn case_sensitive(&self, dir: &Path) -> io::Result<bool> {
        sys::case_sensitive(dir)
    }
}

impl Trash for Native {
    fn trash(&self, path: &Path) -> io::Result<PathBuf> {
        sys::trash_item(path)
    }
}

impl CloneFile for Native {
    fn clone_file(&self, src: &Path, dst: &Path) -> io::Result<()> {
        sys::clone_file(src, dst)
    }
}

impl Metadata for Native {
    fn copy_metadata(&self, src: &Path, dst: &Path) -> io::Result<()> {
        sys::copy_metadata(src, dst)
    }
    fn clear_security(&self, path: &Path) -> io::Result<()> {
        let flags = sys::file_flags(&std::fs::symlink_metadata(path)?);
        if flags & IMMUTABLE_FLAGS != 0 {
            sys::set_flags(path, flags & !IMMUTABLE_FLAGS)?;
        }
        sys::clear_acl(path)
    }
    fn apply_security(&self, src: &Path, dst: &Path) -> io::Result<()> {
        if sys::has_acl(src) {
            sys::copy_acl(src, dst)?;
        }
        let want = sys::file_flags(&std::fs::symlink_metadata(src)?) & IMMUTABLE_FLAGS;
        if want != 0 {
            let cur = sys::file_flags(&std::fs::symlink_metadata(dst)?);
            sys::set_flags(dst, cur | want)?;
        }
        Ok(())
    }
    fn snapshot(&self, path: &Path) -> io::Result<MetaSnapshot> {
        use std::os::unix::fs::MetadataExt;
        let md = std::fs::symlink_metadata(path)?;
        Ok(MetaSnapshot {
            mode: md.mode() & 0o7777,
            uid: md.uid(),
            mtime_ns: md.mtime() * 1_000_000_000 + md.mtime_nsec(),
            birth_ns: sys::birth_ns(&md),
            xattrs: sys::xattr_names(path)?,
            acl: sys::has_acl(path),
            flags: sys::file_flags(&md) & USER_FLAGS,
        })
    }
}

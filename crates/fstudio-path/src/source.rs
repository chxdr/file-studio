//! Where completion gets folder children from.
//!
//! [`DirSource`] is the seam: [`StdDirSource`] reads the real disk,
//! [`CachedDirSource`] keeps listings in an LRU keyed by `(path, mtime)` so every
//! keystroke after the first matches in memory, and the core will later plug in
//! `fstudio-fs`. A source also says whether a folder "may be slow" (network,
//! iCloud placeholder): the completer never lists those on a keystroke.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use poison_free::Mutex;

use crate::keys::fold;

// std's Mutex with the poison handling hidden, to avoid another dependency.
mod poison_free {
    pub struct Mutex<T>(std::sync::Mutex<T>);
    impl<T> Mutex<T> {
        pub fn new(v: T) -> Self {
            Self(std::sync::Mutex::new(v))
        }
        pub fn lock(&self) -> std::sync::MutexGuard<'_, T> {
            self.0.lock().unwrap_or_else(|e| e.into_inner())
        }
    }
}

/// One sub-folder of a listed folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Child {
    /// The name exactly as the filesystem returned it.
    pub name: String,
    /// The folded comparison key of `name` (NFC + lower case).
    pub key: String,
    /// A package or bundle (`.app`, `.photoslibrary`).
    pub is_package: bool,
    /// The name starts with a dot.
    pub hidden: bool,
}

impl Child {
    /// Builds a child, computing its key and flags.
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            key: fold(&name),
            is_package: is_package_name(&name),
            hidden: name.starts_with('.'),
            name,
        }
    }
}

/// The sub-folders of one folder, sorted by `key`.
#[derive(Debug, Clone, Default)]
pub struct DirListing {
    /// Modification time of the folder when listed, in ns since the epoch.
    pub mtime_ns: i64,
    /// Sub-folders only, sorted by folded key.
    pub children: Vec<Child>,
}

impl DirListing {
    /// Sorts `children` by key. Call after building a listing by hand.
    pub fn sorted(mut self) -> Self {
        self.children.sort_by(|a, b| a.key.cmp(&b.key));
        self
    }
}

/// A provider of folder listings.
pub trait DirSource: Send + Sync {
    /// Sub-folders of `path` (sorted by folded key).
    fn list_dirs(&self, path: &Path) -> io::Result<Arc<DirListing>>;

    /// True if listing `path` could block or trigger a download (network volume,
    /// iCloud placeholder). Must be cheap. Completion skips such folders.
    fn may_be_slow(&self, path: &Path) -> bool;

    /// The folder's current modification time if it is cheap to get; used to
    /// validate caches. `None` means "unknown, treat any cache as stale".
    fn mtime_ns(&self, _path: &Path) -> Option<i64> {
        None
    }
}

/// Extensions that Finder shows as one item (a bundle).
const PACKAGE_EXTS: &[&str] = &[
    "app",
    "bundle",
    "framework",
    "plugin",
    "kext",
    "photoslibrary",
    "xcodeproj",
    "xcworkspace",
    "pkg",
    "rtfd",
    "playground",
    "prefpane",
    "appex",
    "saver",
    "component",
    "musiclibrary",
    "fcpbundle",
    "logicx",
    "band",
    "numbers",
    "pages",
    "key",
];

/// True if a folder name looks like a package by its extension.
pub fn is_package_name(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => {
            PACKAGE_EXTS.iter().any(|e| ext.eq_ignore_ascii_case(e))
        }
        _ => false,
    }
}

fn mtime_of(path: &Path) -> Option<i64> {
    let m = std::fs::metadata(path).ok()?;
    let t = m.modified().ok()?;
    let d = t.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(d.as_nanos() as i64)
}

/// Reads the real disk. Lists sub-folders only (symlinks to folders count).
#[derive(Debug, Clone, Default)]
pub struct StdDirSource {
    include_packages: bool,
}

impl StdDirSource {
    /// A source that skips packages (`.app` and friends).
    pub fn new() -> Self {
        Self::default()
    }

    /// A source that also lists packages, for when the user asks for them.
    pub fn with_packages() -> Self {
        Self {
            include_packages: true,
        }
    }
}

impl DirSource for StdDirSource {
    fn list_dirs(&self, path: &Path) -> io::Result<Arc<DirListing>> {
        let mtime_ns = mtime_of(path).unwrap_or(0);
        let mut children = Vec::new();
        for entry in std::fs::read_dir(path)? {
            let Ok(entry) = entry else { continue };
            let Ok(ft) = entry.file_type() else { continue };
            let is_dir = if ft.is_dir() {
                true
            } else if ft.is_symlink() {
                std::fs::metadata(entry.path())
                    .map(|m| m.is_dir())
                    .unwrap_or(false)
            } else {
                false
            };
            if !is_dir {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let child = Child::new(name);
            if child.is_package && !self.include_packages {
                continue;
            }
            children.push(child);
        }
        Ok(Arc::new(DirListing { mtime_ns, children }.sorted()))
    }

    fn may_be_slow(&self, path: &Path) -> bool {
        if crate::sys::is_non_local_volume(path) {
            return true;
        }
        dataless(path)
    }

    fn mtime_ns(&self, path: &Path) -> Option<i64> {
        mtime_of(path)
    }
}

#[cfg(target_os = "macos")]
fn dataless(path: &Path) -> bool {
    use std::os::macos::fs::MetadataExt;
    std::fs::symlink_metadata(path)
        .map(|m| m.st_flags() & fstudio_model::file_flags::DATALESS != 0)
        .unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
fn dataless(_path: &Path) -> bool {
    false
}

struct CacheEntry {
    listing: Arc<DirListing>,
    last_used: u64,
}

struct CacheState {
    map: HashMap<PathBuf, CacheEntry>,
    tick: u64,
}

/// An LRU cache of listings keyed by `(path, mtime)`. A cached listing is reused
/// while the folder's mtime is unchanged; a hit costs one `stat`.
pub struct CachedDirSource<S: DirSource> {
    inner: S,
    capacity: usize,
    state: Mutex<CacheState>,
}

impl<S: DirSource> CachedDirSource<S> {
    /// Wraps `inner`, keeping at most `capacity` listings (at least 1).
    pub fn new(inner: S, capacity: usize) -> Self {
        Self {
            inner,
            capacity: capacity.max(1),
            state: Mutex::new(CacheState {
                map: HashMap::new(),
                tick: 0,
            }),
        }
    }

    /// How many listings are cached right now.
    pub fn len(&self) -> usize {
        self.state.lock().map.len()
    }

    /// True when nothing is cached.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drops everything (e.g. after a bulk operation).
    pub fn clear(&self) {
        self.state.lock().map.clear();
    }

    /// Forgets one folder, e.g. when the file watcher reports a change.
    pub fn invalidate(&self, path: &Path) {
        self.state.lock().map.remove(path);
    }
}

impl<S: DirSource> DirSource for CachedDirSource<S> {
    fn list_dirs(&self, path: &Path) -> io::Result<Arc<DirListing>> {
        let now_mtime = self.inner.mtime_ns(path);
        {
            let mut st = self.state.lock();
            st.tick += 1;
            let tick = st.tick;
            if let Some(e) = st.map.get_mut(path)
                && now_mtime == Some(e.listing.mtime_ns)
            {
                e.last_used = tick;
                return Ok(e.listing.clone());
            }
        }
        let listing = self.inner.list_dirs(path)?;
        let mut st = self.state.lock();
        st.tick += 1;
        let tick = st.tick;
        if st.map.len() >= self.capacity && !st.map.contains_key(path) {
            let oldest = st
                .map
                .iter()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                st.map.remove(&k);
            }
        }
        st.map.insert(
            path.to_path_buf(),
            CacheEntry {
                listing: listing.clone(),
                last_used: tick,
            },
        );
        Ok(listing)
    }

    fn may_be_slow(&self, path: &Path) -> bool {
        // A folder we already listed was not slow; skip the extra syscalls.
        if self.state.lock().map.contains_key(path) {
            return false;
        }
        self.inner.may_be_slow(path)
    }

    fn mtime_ns(&self, path: &Path) -> Option<i64> {
        self.inner.mtime_ns(path)
    }
}

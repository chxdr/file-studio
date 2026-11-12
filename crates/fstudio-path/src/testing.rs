//! In-memory stand-ins for the disk, for tests and benchmarks (also used by
//! the coordinator's bench harness).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::keys::fold;
use crate::probe::{FsProbe, ProbeKind};
use crate::source::{Child, DirListing, DirSource};

/// A fake filesystem tree for [`FsProbe`]. Paths are absolute and exact-spelled;
/// lookups are case- and normalisation-insensitive like APFS.
#[derive(Debug, Default, Clone)]
pub struct MemFs {
    nodes: BTreeMap<PathBuf, ProbeKind>,
    /// Symlinks: link path -> target path (resolved when probing `kind`/`lookup`).
    links: HashMap<PathBuf, PathBuf>,
}

impl MemFs {
    /// An empty tree containing only `/`.
    pub fn new() -> Self {
        let mut f = Self::default();
        f.nodes.insert(PathBuf::from("/"), ProbeKind::Dir);
        f
    }

    /// Adds a folder and all its ancestors.
    pub fn dir(mut self, path: &str) -> Self {
        self.add(path, ProbeKind::Dir);
        self
    }

    /// Adds a file and the folders above it.
    pub fn file(mut self, path: &str) -> Self {
        self.add(path, ProbeKind::File);
        self
    }

    /// Adds a symlink `link` pointing at `target` (the target must be added separately).
    pub fn symlink(mut self, link: &str, target: &str) -> Self {
        let p = PathBuf::from(link);
        if let Some(parent) = p.parent() {
            self.add(&parent.to_string_lossy(), ProbeKind::Dir);
        }
        self.links.insert(p, PathBuf::from(target));
        self
    }

    fn add(&mut self, path: &str, kind: ProbeKind) {
        let mut cur = PathBuf::from("/");
        let comps: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
        for (i, c) in comps.iter().enumerate() {
            cur.push(c);
            let k = if i + 1 == comps.len() {
                kind
            } else {
                ProbeKind::Dir
            };
            self.nodes.entry(cur.clone()).or_insert(k);
        }
    }

    fn resolve(&self, path: &Path) -> Option<ProbeKind> {
        if let Some(k) = self.nodes.get(path) {
            return Some(*k);
        }
        let t = self.links.get(path)?;
        self.nodes.get(t).copied()
    }
}

impl FsProbe for MemFs {
    fn kind(&self, path: &Path) -> Option<ProbeKind> {
        self.resolve(path)
    }

    fn lookup(&self, dir: &Path, name: &str) -> Option<(String, ProbeKind)> {
        let key = fold(name);
        let mut best: Option<(String, ProbeKind)> = None;
        let candidates = self
            .nodes
            .keys()
            .chain(self.links.keys())
            .filter(|p| p.parent() == Some(dir));
        for p in candidates {
            let n = p.file_name()?.to_str()?;
            if n == name {
                return Some((n.to_owned(), self.resolve(p)?));
            }
            if best.is_none() && fold(n) == key {
                best = Some((n.to_owned(), self.resolve(p)?));
            }
        }
        best
    }
}

/// A fake [`DirSource`] backed by a map from folder to child names.
#[derive(Debug, Default, Clone)]
pub struct MemDirSource {
    dirs: HashMap<PathBuf, Arc<DirListing>>,
    slow: HashSet<PathBuf>,
}

impl MemDirSource {
    /// An empty source.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a folder with the given child folder names.
    pub fn with_dir<S: AsRef<str>>(mut self, path: &str, children: &[S]) -> Self {
        self.insert(path, children.iter().map(|s| s.as_ref().to_owned()));
        self
    }

    /// Adds a folder from any iterator of names (use for the 100k-child case).
    pub fn insert(&mut self, path: &str, names: impl IntoIterator<Item = String>) {
        let listing = DirListing {
            mtime_ns: 1,
            children: names.into_iter().map(Child::new).collect(),
        }
        .sorted();
        self.dirs.insert(PathBuf::from(path), Arc::new(listing));
    }

    /// Marks a folder as slow (network): listing it must never be attempted.
    pub fn with_slow(mut self, path: &str) -> Self {
        self.slow.insert(PathBuf::from(path));
        self
    }
}

impl DirSource for MemDirSource {
    fn list_dirs(&self, path: &Path) -> io::Result<Arc<DirListing>> {
        if self.slow.contains(path) {
            panic!("a slow folder was listed: {}", path.display());
        }
        self.dirs
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such folder"))
    }

    fn may_be_slow(&self, path: &Path) -> bool {
        self.slow.contains(path)
    }

    fn mtime_ns(&self, path: &Path) -> Option<i64> {
        self.dirs.get(path).map(|l| l.mtime_ns)
    }
}

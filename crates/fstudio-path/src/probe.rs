//! The filesystem probe used by parsing. It is a trait so parsing can be tested
//! without a disk, and so the parser touches the disk only through one narrow door.

use std::path::Path;

use crate::keys::fold;

/// What a path turned out to be (symlinks are followed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeKind {
    /// A folder (or a symlink to one).
    Dir,
    /// Anything else that exists.
    File,
}

/// Answers "does this exist, and how is it spelled on disk?".
pub trait FsProbe: Send + Sync {
    /// What `path` is, or `None` if it does not exist or cannot be read.
    fn kind(&self, path: &Path) -> Option<ProbeKind>;

    /// Looks for a child of `dir` whose name equals `name` ignoring case and Unicode
    /// normalisation. Returns the **on-disk spelling** and what it is. An exact
    /// spelling wins over a merely equivalent one.
    fn lookup(&self, dir: &Path, name: &str) -> Option<(String, ProbeKind)>;
}

/// The real filesystem.
#[derive(Debug, Default, Clone, Copy)]
pub struct StdProbe;

impl FsProbe for StdProbe {
    fn kind(&self, path: &Path) -> Option<ProbeKind> {
        let m = std::fs::metadata(path).ok()?;
        Some(if m.is_dir() {
            ProbeKind::Dir
        } else {
            ProbeKind::File
        })
    }

    fn lookup(&self, dir: &Path, name: &str) -> Option<(String, ProbeKind)> {
        let key = fold(name);
        let mut equivalent: Option<String> = None;
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let Some(n) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if n == name {
                equivalent = Some(n);
                break;
            }
            if equivalent.is_none() && fold(&n) == key {
                equivalent = Some(n);
            }
        }
        let n = equivalent?;
        let kind = self.kind(&dir.join(&n))?;
        Some((n, kind))
    }
}

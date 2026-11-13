//! Comparing two trees by content (the CLI's `compare`), uncached.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::chunk::ChunkHasher;
use crate::platform::Platform;

/// One difference between two trees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Difference {
    /// Only in the first tree.
    OnlyInA(PathBuf),
    /// Only in the second tree.
    OnlyInB(PathBuf),
    /// In both, with different content (or a different kind).
    Differs(PathBuf),
    /// Couldn't be read on one side.
    Unreadable(PathBuf, String),
}

/// xxh3-128 of a file's content, read with the cache evicted and bypassed.
pub fn hash_file(platform: &Platform, path: &Path) -> io::Result<u128> {
    let _ = platform.uncached.evict(path);
    let f = std::fs::File::open(path)?;
    let _ = platform.uncached.set_uncached(&f);
    let mut h = ChunkHasher::new(crate::chunk::DEFAULT_CHUNK_SIZE);
    let mut buf = vec![0u8; 4 << 20];
    let mut r = &f;
    loop {
        match io::Read::read(&mut r, &mut buf) {
            Ok(0) => break,
            Ok(n) => {
                h.update(&buf[..n]);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(h.finish().file)
}

/// What an entry is, for comparing.
#[derive(Debug, PartialEq, Eq)]
enum Entry {
    File,
    Dir,
    Link(PathBuf),
    Other,
}

fn walk(root: &Path, rel: &Path, out: &mut BTreeMap<PathBuf, Entry>) -> io::Result<()> {
    let here = root.join(rel);
    let md = std::fs::symlink_metadata(&here)?;
    let ft = md.file_type();
    let e = if ft.is_symlink() {
        Entry::Link(std::fs::read_link(&here)?)
    } else if ft.is_dir() {
        Entry::Dir
    } else if ft.is_file() {
        Entry::File
    } else {
        Entry::Other
    };
    let is_dir = e == Entry::Dir;
    out.insert(rel.to_path_buf(), e);
    if is_dir {
        for ent in std::fs::read_dir(&here)? {
            let ent = ent?;
            let name = ent.file_name();
            // Our own partials and macOS AppleDouble files aren't content.
            let n = name.to_string_lossy();
            if n.ends_with(crate::names::PARTIAL_SUFFIX) || n.starts_with("._") {
                continue;
            }
            walk(root, &rel.join(&name), out)?;
        }
    }
    Ok(())
}

/// Compares `a` and `b` (files or folders, symlinks never followed) by
/// content. An empty result means they're identical.
pub fn compare(platform: &Platform, a: &Path, b: &Path) -> io::Result<Vec<Difference>> {
    let (mut ta, mut tb) = (BTreeMap::new(), BTreeMap::new());
    walk(a, Path::new(""), &mut ta)?;
    walk(b, Path::new(""), &mut tb)?;
    let mut out = Vec::new();
    for (rel, ea) in &ta {
        match tb.get(rel) {
            None => out.push(Difference::OnlyInA(rel.clone())),
            Some(eb) if eb != ea => out.push(Difference::Differs(rel.clone())),
            Some(Entry::File) => {
                match (
                    hash_file(platform, &a.join(rel)),
                    hash_file(platform, &b.join(rel)),
                ) {
                    (Ok(x), Ok(y)) if x == y => {}
                    (Ok(_), Ok(_)) => out.push(Difference::Differs(rel.clone())),
                    (Err(e), _) | (_, Err(e)) => {
                        out.push(Difference::Unreadable(rel.clone(), e.to_string()))
                    }
                }
            }
            Some(_) => {}
        }
    }
    for rel in tb.keys() {
        if !ta.contains_key(rel) {
            out.push(Difference::OnlyInB(rel.clone()));
        }
    }
    Ok(out)
}

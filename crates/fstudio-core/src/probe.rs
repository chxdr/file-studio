//! The path engine's disk access ([`FsProbe`], [`DirSource`]) backed by `fstudio-fs`.

use std::io;
use std::path::Path;
use std::sync::Arc;

use fstudio_fs::Lister;
use fstudio_model::EntryKind;
use fstudio_path::keys::fold;
use fstudio_path::{Child, DirListing, DirSource, FsProbe, ProbeKind};

fn kind_of(path: &Path) -> Option<ProbeKind> {
    let m = std::fs::metadata(path).ok()?;
    Some(if m.is_dir() {
        ProbeKind::Dir
    } else {
        ProbeKind::File
    })
}

/// Answers parse-time questions with `lstat`, `realpath` and the bulk lister.
#[derive(Debug, Default, Clone, Copy)]
pub struct FsCoreProbe;

impl FsProbe for FsCoreProbe {
    fn kind(&self, path: &Path) -> Option<ProbeKind> {
        kind_of(path)
    }

    fn lookup(&self, dir: &Path, name: &str) -> Option<(String, ProbeKind)> {
        let p = dir.join(name);
        if let Ok(md) = std::fs::symlink_metadata(&p) {
            let kind = kind_of(&p).unwrap_or(ProbeKind::File);
            // On macOS realpath returns the on-disk spelling of each component
            // (case and normalisation). Not for a symlink, which it would follow.
            if !md.file_type().is_symlink()
                && let Ok(real) = std::fs::canonicalize(&p)
                && let Some(n) = real.file_name().and_then(|n| n.to_str())
                && fold(n) == fold(name)
            {
                return Some((n.to_string(), kind));
            }
            if md.file_type().is_symlink()
                && let Some(n) = scan(dir, name)
            {
                return Some((n, kind));
            }
            return Some((name.to_string(), kind));
        }
        // Not there under this spelling; a case-sensitive volume may still hold
        // an equivalent name.
        let n = scan(dir, name)?;
        let kind = kind_of(&dir.join(&n))?;
        Some((n, kind))
    }
}

/// Lists `dir` looking for a name equal to `name` ignoring case and normalisation.
fn scan(dir: &Path, name: &str) -> Option<String> {
    let key = fold(name);
    let mut lister = Lister::open(dir).ok()?;
    let mut found = None;
    while let Some(Ok(chunk)) = lister.next_chunk() {
        for e in chunk {
            if e.name == name {
                return Some(e.name);
            }
            if found.is_none() && fold(&e.name) == key {
                found = Some(e.name);
            }
        }
    }
    found
}

/// Completion's folder source: sub-folders from the bulk lister, skipping
/// packages; network volumes and iCloud placeholders count as slow.
#[derive(Debug, Default, Clone, Copy)]
pub struct FsDirSource;

impl DirSource for FsDirSource {
    fn list_dirs(&self, path: &Path) -> io::Result<Arc<DirListing>> {
        let mtime_ns = self.mtime_ns(path).unwrap_or(0);
        let mut lister = Lister::open(path).map_err(to_io)?;
        let mut children = Vec::new();
        while let Some(chunk) = lister.next_chunk() {
            for e in chunk.map_err(to_io)? {
                let is_dir = match e.kind {
                    EntryKind::Dir => true,
                    EntryKind::Symlink => std::fs::metadata(path.join(&e.name))
                        .map(|m| m.is_dir())
                        .unwrap_or(false),
                    _ => false,
                };
                if !is_dir || e.is_package {
                    continue;
                }
                let mut c = Child::new(e.name.clone());
                c.hidden = e.is_hidden();
                children.push(c);
            }
        }
        Ok(Arc::new(DirListing { mtime_ns, children }.sorted()))
    }

    fn may_be_slow(&self, path: &Path) -> bool {
        if let Ok(e) = fstudio_fs::stat_entry(path)
            && e.is_dataless()
        {
            return true;
        }
        fstudio_fs::volume_for(path)
            .map(|v| v.is_network)
            .unwrap_or(false)
    }

    fn mtime_ns(&self, path: &Path) -> Option<i64> {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::metadata(path).ok()?;
        Some(m.mtime() * 1_000_000_000 + m.mtime_nsec())
    }
}

fn to_io(e: fstudio_fs::Error) -> io::Error {
    match e {
        fstudio_fs::Error::Io { source, .. } => source,
        other => io::Error::other(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_returns_on_disk_spelling() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("Photos")).unwrap();
        std::fs::write(d.path().join("Cafe\u{301}.txt"), b"x").unwrap();
        let p = FsCoreProbe;
        assert_eq!(
            p.lookup(d.path(), "photos"),
            Some(("Photos".into(), ProbeKind::Dir))
        );
        assert_eq!(
            p.lookup(d.path(), "caf\u{e9}.TXT").map(|x| x.0),
            Some("Cafe\u{301}.txt".into())
        );
        assert_eq!(p.lookup(d.path(), "nope"), None);
    }

    #[test]
    fn dir_source_lists_folders_only() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("b")).unwrap();
        std::fs::create_dir(d.path().join("A")).unwrap();
        std::fs::create_dir(d.path().join("X.app")).unwrap();
        std::fs::write(d.path().join("f"), b"").unwrap();
        let l = FsDirSource.list_dirs(d.path()).unwrap();
        let names: Vec<_> = l.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["A", "b"]);
        assert!(!FsDirSource.may_be_slow(d.path()));
    }
}

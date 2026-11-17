//! Culling marks from XMP sidecars: read lazily, off the listing path, and
//! cached by (path, mtime, size).
//!
//! A tab lists its folder first. Once the listing is complete, or a watcher
//! patch lands, it hands its `.xmp` entries to [`SidecarCache::table_for`] on a
//! background thread and re-sorts when the marks change. Nothing here ever
//! writes a sidecar.
//!
//! An iCloud placeholder sidecar (`SF_DATALESS`) is never read: reading it
//! would download it (F-016). It counts as unknown, like a malformed one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use fstudio_model::Entry;
use fstudio_view::{MarkTable, Marks};
use fstudio_xmp::SidecarRead;
use parking_lot::Mutex;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// The culling filter presets, as a tab's view keeps them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum CullFilter {
    /// Show everything.
    #[default]
    All,
    /// Three stars or more, not rejected ("★ 3+").
    Rating3Plus,
    /// Picks only.
    Picks,
    /// Rejects only.
    Rejects,
    /// Everything except rejects.
    HideRejects,
}

impl CullFilter {
    /// Every preset, in menu order.
    pub const ALL: [CullFilter; 5] = [
        CullFilter::All,
        CullFilter::Rating3Plus,
        CullFilter::Picks,
        CullFilter::Rejects,
        CullFilter::HideRejects,
    ];

    /// The title the shell shows ("★ 3+").
    pub fn title(self) -> &'static str {
        match self {
            CullFilter::All => "All Shots",
            CullFilter::Rating3Plus => "★ 3+",
            CullFilter::Picks => "Picks",
            CullFilter::Rejects => "Rejects",
            CullFilter::HideRejects => "Hide Rejects",
        }
    }

    /// The command that turns this preset on.
    pub fn command(self) -> &'static str {
        match self {
            CullFilter::All => "cull.all",
            CullFilter::Rating3Plus => "cull.three_stars",
            CullFilter::Picks => "cull.picks",
            CullFilter::Rejects => "cull.rejects",
            CullFilter::HideRejects => "cull.hide_rejects",
        }
    }

    /// The preset a command id names.
    pub fn from_command(id: &str) -> Option<CullFilter> {
        CullFilter::ALL.into_iter().find(|c| c.command() == id)
    }

    pub(crate) fn mark_filter(self) -> fstudio_view::MarkFilter {
        use fstudio_view::MarkFilter as M;
        match self {
            CullFilter::All => M::All,
            CullFilter::Rating3Plus => M::MinRating(3),
            CullFilter::Picks => M::Picks,
            CullFilter::Rejects => M::Rejects,
            CullFilter::HideRejects => M::HideRejects,
        }
    }
}

/// Above this many cached sidecars the cache starts again from empty. Each
/// entry is a path and a few bytes, so this is a few MB at most.
const MAX_CACHED: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cached {
    mtime_ns: i64,
    size: u64,
    read: SidecarRead,
}

/// Sidecar reads, by path, valid while the file's mtime and size are the
/// same. One per engine, shared by every tab.
#[derive(Debug, Default)]
pub(crate) struct SidecarCache {
    map: Mutex<HashMap<PathBuf, Cached>>,
}

impl SidecarCache {
    /// The marks of the sidecars `entries` in `dir` (each entry as listed,
    /// so its mtime and size are known without another `stat`). Reads only
    /// what the cache lacks, in parallel. **Blocks on disk.**
    pub(crate) fn table_for(&self, dir: &Path, entries: &[Entry]) -> MarkTable {
        let mut found: Vec<(String, SidecarRead)> = Vec::with_capacity(entries.len());
        let mut misses: Vec<&Entry> = Vec::new();
        {
            let map = self.map.lock();
            for e in entries {
                if e.is_dataless() {
                    continue; // never read a placeholder (F-016)
                }
                match map.get(&dir.join(&e.name)) {
                    Some(c) if c.mtime_ns == e.mtime_ns && c.size == e.size => {
                        found.push((e.name.clone(), c.read))
                    }
                    _ => misses.push(e),
                }
            }
        }
        let read: Vec<(&Entry, SidecarRead)> = misses
            .par_iter()
            .map(|e| (*e, fstudio_xmp::read_file(&dir.join(&e.name))))
            .collect();
        {
            let mut map = self.map.lock();
            if map.len() + read.len() > MAX_CACHED {
                map.clear();
            }
            for (e, r) in &read {
                map.insert(
                    dir.join(&e.name),
                    Cached {
                        mtime_ns: e.mtime_ns,
                        size: e.size,
                        read: *r,
                    },
                );
            }
        }
        found.extend(read.into_iter().map(|(e, r)| (e.name.clone(), r)));
        table_from(found)
    }

    /// The marks for one file: its own if it is a sidecar, otherwise its
    /// image's sidecar (`DSC0001.ARW` → `DSC0001.xmp`, or `.XMP`).
    /// `None` when there is no sidecar or it can't be read. **Blocks on
    /// disk** (one `lstat`, and a read on a cache miss).
    pub(crate) fn marks_for_file(&self, path: &Path, read_on_miss: bool) -> Option<Marks> {
        let name = path.file_name()?.to_string_lossy();
        let candidates: Vec<PathBuf> = if fstudio_xmp::is_sidecar_name(&name) {
            vec![path.to_path_buf()]
        } else {
            vec![fstudio_xmp::sidecar_path(path), path.with_extension("XMP")]
        };
        for c in candidates {
            let Ok(e) = fstudio_fs::stat_entry(&c) else {
                continue;
            };
            if e.kind != fstudio_model::EntryKind::File {
                continue;
            }
            let dir = c.parent().unwrap_or(Path::new("/"));
            if !read_on_miss {
                let map = self.map.lock();
                let hit = map
                    .get(&dir.join(&e.name))
                    .filter(|h| h.mtime_ns == e.mtime_ns && h.size == e.size)?;
                return match hit.read {
                    SidecarRead::Marks(m) => Some(m),
                    _ => None,
                };
            }
            return self.table_for(dir, &[e]).get(&name);
        }
        None
    }
}

/// A table from read results: missing and unknown sidecars give no marks.
pub(crate) fn table_from(results: impl IntoIterator<Item = (String, SidecarRead)>) -> MarkTable {
    MarkTable::from_sidecars(results.into_iter().filter_map(|(n, r)| match r {
        SidecarRead::Marks(m) => Some((n, m)),
        SidecarRead::Missing | SidecarRead::Unknown => None,
    }))
}

impl crate::engine::Engine {
    /// The culling marks for one file, for the preview pane: its own if it
    /// is a sidecar, otherwise its image's sidecar (`DSC0001.ARW` →
    /// `DSC0001.xmp`). `None` without a readable sidecar. **Blocks on
    /// disk** (an `lstat`, and a small read on a cache miss): call it off
    /// the main thread. Never reads an iCloud placeholder.
    pub fn cull_marks(&self, path: &str) -> Option<Marks> {
        self.shared.sidecars.marks_for_file(Path::new(path), true)
    }

    /// Like [`Engine::cull_marks`] but never reads a sidecar's content: an
    /// `lstat` and a cache lookup only, so the main thread may call it.
    pub fn cull_marks_cached(&self, path: &str) -> Option<Marks> {
        self.shared.sidecars.marks_for_file(Path::new(path), false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fstudio_view::{Flag, Label};

    fn xmp(attrs: &str) -> String {
        format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:istudio="https://importstudio.app/ns/xmp/1.0/" {attrs}/></rdf:RDF></x:xmpmeta>"#
        )
    }

    fn listed(dir: &Path) -> Vec<Entry> {
        let mut v: Vec<Entry> = std::fs::read_dir(dir)
            .unwrap()
            .map(|d| fstudio_fs::stat_entry(d.unwrap().path()).unwrap())
            .filter(|e| fstudio_xmp::is_sidecar_name(&e.name))
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    #[test]
    fn reads_once_and_rereads_when_changed() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("A.XMP");
        std::fs::write(&p, xmp(r#"xmp:Rating="3""#)).unwrap();
        std::fs::write(d.path().join("B.xmp"), "<broken").unwrap();
        std::fs::write(d.path().join("A.ARW"), b"raw").unwrap();
        let cache = SidecarCache::default();
        let t = cache.table_for(d.path(), &listed(d.path()));
        assert_eq!(t.get("A.ARW").map(|m| m.rating), Some(3));
        assert_eq!(t.get("B.ARW"), None); // malformed: unknown, no error
        assert_eq!(cache.map.lock().len(), 2);

        // Same mtime and size: served from the cache even if the bytes differ.
        let mut stale = listed(d.path());
        std::fs::write(&p, xmp(r#"xmp:Rating="5""#)).unwrap();
        stale[0].size = std::fs::metadata(&p).unwrap().len();
        stale[0].mtime_ns = cache.map.lock()[&p].mtime_ns;
        let cached_size = cache.map.lock()[&p].size;
        if stale[0].size == cached_size {
            let t = cache.table_for(d.path(), &stale);
            assert_eq!(t.get("A.ARW").map(|m| m.rating), Some(3));
        }
        // A new mtime: read again.
        let mut fresh = listed(d.path());
        fresh[0].mtime_ns += 1;
        let t = cache.table_for(d.path(), &fresh);
        assert_eq!(t.get("A.ARW").map(|m| m.rating), Some(5));
        // Nothing was written.
        assert_eq!(std::fs::read(d.path().join("A.ARW")).unwrap(), b"raw");
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 3);
    }

    #[test]
    fn dataless_sidecars_are_never_read() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("A.xmp"), xmp(r#"xmp:Rating="3""#)).unwrap();
        let mut e = listed(d.path());
        e[0].flags |= fstudio_model::file_flags::DATALESS;
        let cache = SidecarCache::default();
        assert!(cache.table_for(d.path(), &e).is_empty());
        assert!(cache.map.lock().is_empty());
    }

    #[test]
    fn marks_for_one_file() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("Caf\u{e9}.xmp"),
            xmp(r#"xmp:Rating="-1" istudio:Stars="2" xmp:Label="Blue""#),
        )
        .unwrap();
        let cache = SidecarCache::default();
        let want = Some(Marks {
            rating: 2,
            label: Some(Label::Blue),
            flag: Flag::Reject,
        });
        assert_eq!(
            cache.marks_for_file(&d.path().join("Caf\u{e9}.ARW"), true),
            want
        );
        assert_eq!(
            cache.marks_for_file(&d.path().join("Caf\u{e9}.xmp"), true),
            want
        );
        // No sidecar, no image: nothing, no error.
        assert_eq!(
            cache.marks_for_file(&d.path().join("Other.ARW"), true),
            None
        );
        assert_eq!(
            cache.marks_for_file(&d.path().join("gone/x.ARW"), true),
            None
        );
    }

    #[test]
    fn presets_round_trip_commands() {
        for c in CullFilter::ALL {
            assert_eq!(CullFilter::from_command(c.command()), Some(c));
        }
    }
}

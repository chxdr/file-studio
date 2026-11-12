//! What a drag and drop means (copy, move or nothing), and queuing it.
//!
//! Finder's rules: on the same volume a drop moves, onto another volume it
//! copies; ⌥ forces a copy and ⌘ forces a move. Nothing happens when an item
//! is dropped onto itself or into its own subfolder, when the destination
//! can't be written, or when it isn't a folder. Only `lstat`, `realpath` and
//! `access` are used, so dataless (iCloud) sources are never read (F-016).

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use fstudio_keymap::Mods;
use fstudio_ops::{OpId, OpRequest};
use serde::{Deserialize, Serialize};

use crate::engine::Engine;
use crate::error::{CoreError, Result};

/// What dropping items onto a folder does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DropEffect {
    /// The items are copied.
    Copy,
    /// The items are moved.
    Move,
    /// The drop is refused.
    None,
}

/// One dragged item, as far as the decision needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceFacts {
    /// The item's path with its folder resolved (the item itself is not followed).
    pub(crate) path: PathBuf,
    /// `st_dev` of the item.
    pub(crate) dev: u64,
}

/// The destination, as far as the decision needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DestFacts {
    /// The folder, resolved.
    pub(crate) path: PathBuf,
    /// A folder (symlinks to folders count) that isn't a package.
    pub(crate) is_folder: bool,
    /// We may create entries in it.
    pub(crate) writable: bool,
    /// `st_dev` of the folder.
    pub(crate) dev: u64,
}

/// The pure decision. `sources` is `None` when any source is missing.
pub(crate) fn decide(sources: Option<&[SourceFacts]>, dest: &DestFacts, mods: Mods) -> DropEffect {
    let Some(sources) = sources else {
        return DropEffect::None;
    };
    if sources.is_empty() || !dest.is_folder || !dest.writable {
        return DropEffect::None;
    }
    // Onto itself or into its own subfolder.
    if sources.iter().any(|s| dest.path.starts_with(&s.path)) {
        return DropEffect::None;
    }
    let all_here = sources
        .iter()
        .all(|s| s.path.parent() == Some(dest.path.as_path()));
    if mods.alt {
        // ⌥ copies, even into the same folder (a duplicate, as Finder does).
        return DropEffect::Copy;
    }
    if all_here {
        return DropEffect::None; // moving into its own folder does nothing
    }
    if mods.cmd {
        return DropEffect::Move;
    }
    if sources.iter().all(|s| s.dev == dest.dev) {
        DropEffect::Move
    } else {
        DropEffect::Copy
    }
}

/// Resolves a path's folder but not the item itself (so a dragged symlink is
/// the link, not its target).
fn resolve_parent(p: &Path) -> PathBuf {
    match (p.parent(), p.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => {
            std::fs::canonicalize(parent)
                .map(|c| c.join(name))
                .unwrap_or_else(|_| p.to_path_buf())
        }
        _ => p.to_path_buf(),
    }
}

pub(crate) fn source_facts(paths: &[String]) -> Option<Vec<SourceFacts>> {
    paths
        .iter()
        .map(|s| {
            let p = Path::new(s);
            let md = std::fs::symlink_metadata(p).ok()?;
            Some(SourceFacts {
                path: resolve_parent(p),
                dev: md.dev() as u32 as u64,
            })
        })
        .collect()
}

pub(crate) fn dest_facts(dest: &str) -> DestFacts {
    let path = std::fs::canonicalize(dest).unwrap_or_else(|_| PathBuf::from(dest));
    let md = std::fs::metadata(&path).ok();
    let is_dir = md.as_ref().is_some_and(|m| m.is_dir());
    let is_package = path
        .file_name()
        .is_some_and(|n| fstudio_fs::is_package_name(&n.to_string_lossy()));
    DestFacts {
        is_folder: is_dir && !is_package,
        writable: is_dir && crate::sys::dir_writable(&path),
        dev: md.map(|m| m.dev() as u32 as u64).unwrap_or(0),
        path,
    }
}

impl Engine {
    /// What dropping `sources` onto the folder `dest` with these modifiers
    /// does. Metadata only (`lstat`, `realpath`, `access`); never reads
    /// content, so dataless sources are safe.
    pub fn drop_effect(&self, sources: &[String], dest: &str, mods: Mods) -> DropEffect {
        decide(source_facts(sources).as_deref(), &dest_facts(dest), mods)
    }

    /// Decides the drop as [`Engine::drop_effect`] does and queues the copy or
    /// move. Returns the queued operation, or `NotAvailable` when the drop is
    /// refused.
    pub fn drop_items(&self, sources: &[String], dest: &str, mods: Mods) -> Result<OpId> {
        let effect = self.drop_effect(sources, dest, mods);
        let sources: Vec<PathBuf> = sources.iter().map(PathBuf::from).collect();
        let dest_dir = PathBuf::from(dest);
        let req = match effect {
            DropEffect::Copy => OpRequest::Copy { sources, dest_dir },
            DropEffect::Move => OpRequest::Move { sources, dest_dir },
            DropEffect::None => {
                return Err(CoreError::NotAvailable(
                    "These items can't be dropped here".into(),
                ));
            }
        };
        self.enqueue(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(p: &str, dev: u64) -> SourceFacts {
        SourceFacts {
            path: p.into(),
            dev,
        }
    }
    fn dst(p: &str, dev: u64) -> DestFacts {
        DestFacts {
            path: p.into(),
            is_folder: true,
            writable: true,
            dev,
        }
    }
    const NONE: Mods = Mods::NONE;
    fn alt() -> Mods {
        Mods {
            alt: true,
            ..Mods::NONE
        }
    }
    fn cmd() -> Mods {
        Mods {
            cmd: true,
            ..Mods::NONE
        }
    }

    #[test]
    fn same_volume_moves_other_volume_copies() {
        let s = [src("/a/x.txt", 1)];
        assert_eq!(decide(Some(&s), &dst("/b", 1), NONE), DropEffect::Move);
        assert_eq!(
            decide(Some(&s), &dst("/Volumes/USB", 2), NONE),
            DropEffect::Copy
        );
        // Mixed volumes copy.
        let s = [src("/a/x", 1), src("/Volumes/USB/y", 2)];
        assert_eq!(decide(Some(&s), &dst("/b", 1), NONE), DropEffect::Copy);
    }

    #[test]
    fn modifiers_force() {
        let s = [src("/a/x.txt", 1)];
        assert_eq!(decide(Some(&s), &dst("/b", 1), alt()), DropEffect::Copy);
        assert_eq!(
            decide(Some(&s), &dst("/Volumes/USB", 2), cmd()),
            DropEffect::Move
        );
        // Both held: ⌥ wins (copying is the safe choice).
        let both = Mods {
            alt: true,
            cmd: true,
            ..Mods::NONE
        };
        assert_eq!(decide(Some(&s), &dst("/b", 1), both), DropEffect::Copy);
    }

    #[test]
    fn refuses_itself_its_subfolders_and_its_own_folder() {
        let s = [src("/a/dir", 1)];
        assert_eq!(decide(Some(&s), &dst("/a/dir", 1), NONE), DropEffect::None);
        assert_eq!(
            decide(Some(&s), &dst("/a/dir/sub/deeper", 1), cmd()),
            DropEffect::None
        );
        assert_eq!(
            decide(Some(&s), &dst("/a/dir/sub", 1), alt()),
            DropEffect::None
        );
        // A sibling whose name only starts the same is fine.
        assert_eq!(decide(Some(&s), &dst("/a/dir2", 1), NONE), DropEffect::Move);
        // Into the folder it is already in: nothing, unless ⌥ duplicates.
        assert_eq!(decide(Some(&s), &dst("/a", 1), NONE), DropEffect::None);
        assert_eq!(decide(Some(&s), &dst("/a", 1), cmd()), DropEffect::None);
        assert_eq!(decide(Some(&s), &dst("/a", 1), alt()), DropEffect::Copy);
    }

    #[test]
    fn refuses_unwritable_non_folders_missing_and_empty() {
        let s = [src("/a/x", 1)];
        let mut d = dst("/b", 1);
        d.writable = false;
        assert_eq!(decide(Some(&s), &d, alt()), DropEffect::None);
        let mut d = dst("/b/App.app", 1);
        d.is_folder = false;
        assert_eq!(decide(Some(&s), &d, NONE), DropEffect::None);
        assert_eq!(decide(None, &dst("/b", 1), NONE), DropEffect::None);
        assert_eq!(decide(Some(&[]), &dst("/b", 1), NONE), DropEffect::None);
    }

    fn ps(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn real_files_unicode_packages_symlinks() {
        let d = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(d.path()).unwrap();
        let a = base.join("a");
        let b = base.join("b");
        std::fs::create_dir_all(a.join("Café sub")).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::create_dir_all(b.join("Thing.app")).unwrap();
        // NFD name as source.
        let nfd = a.join("Cafe\u{301}.txt");
        std::fs::write(&nfd, b"x").unwrap();
        let pkg = a.join("Tool.app");
        std::fs::create_dir_all(&pkg).unwrap();
        std::os::unix::fs::symlink(&b, a.join("link to b")).unwrap();
        let e = DropEffect::Move;
        let src = vec![ps(&nfd), ps(&pkg)];
        assert_eq!(decide_paths(&src, &ps(&b), NONE), e, "same volume moves");
        assert_eq!(decide_paths(&src, &ps(&b), alt()), DropEffect::Copy);
        // A package is not a drop folder.
        assert_eq!(
            decide_paths(&src, &ps(&b.join("Thing.app")), NONE),
            DropEffect::None
        );
        // Dropping a folder into its own subfolder (unicode name).
        assert_eq!(
            decide_paths(&[ps(&a)], &ps(&a.join("Café sub")), NONE),
            DropEffect::None
        );
        // A dragged symlink is the link: dropping it into its target is allowed.
        assert_eq!(
            decide_paths(&[ps(&a.join("link to b"))], &ps(&b), NONE),
            DropEffect::Move
        );
        // Missing source.
        assert_eq!(
            decide_paths(&[ps(&a.join("gone"))], &ps(&b), NONE),
            DropEffect::None
        );
        // A file as destination.
        assert_eq!(decide_paths(&[ps(&pkg)], &ps(&nfd), NONE), DropEffect::None);
    }

    #[test]
    fn read_only_destination_refused() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(d.path()).unwrap();
        let ro = base.join("ro");
        std::fs::create_dir(&ro).unwrap();
        std::fs::write(base.join("f"), b"x").unwrap();
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
        let r = decide_paths(&[ps(&base.join("f"))], &ps(&ro), alt());
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(r, DropEffect::None);
    }

    #[test]
    fn dataless_source_is_only_lstated() {
        // A dataless placeholder can't be made in a test; the decision only
        // uses `symlink_metadata`, which never downloads (F-016). Check a
        // file with no read permission is still accepted: content is never opened.
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(d.path()).unwrap();
        let f = base.join("secret");
        std::fs::write(&f, b"x").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o000)).unwrap();
        std::fs::create_dir(base.join("to")).unwrap();
        let r = decide_paths(&[ps(&f)], &ps(&base.join("to")), NONE);
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(r, DropEffect::Move);
    }

    fn decide_paths(sources: &[String], dest: &str, mods: Mods) -> DropEffect {
        decide(source_facts(sources).as_deref(), &dest_facts(dest), mods)
    }
}

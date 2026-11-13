//! Mounted volumes and what they can do.

use crate::error::{Error, Result};
use crate::sys::volume as sv;
use std::path::{Path, PathBuf};

/// One mounted volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeInfo {
    /// Where it is mounted, e.g. `/` or `/Volumes/Backup`.
    pub mount_path: PathBuf,
    /// The display name (`Macintosh HD`).
    pub name: String,
    /// Filesystem type: `apfs`, `hfs`, `msdos`, `exfat`, `smbfs`, …
    pub fs_type: String,
    /// Names differing only by case are different files (F-017).
    pub case_sensitive: bool,
    /// Copy-on-write clones (`clonefile`) work here.
    pub supports_clone: bool,
    /// On removable or ejectable media.
    pub is_removable: bool,
    /// A built-in disk.
    pub is_internal: bool,
    /// A network share.
    pub is_network: bool,
    /// Mounted read-only.
    pub is_read_only: bool,
    /// Capacity in bytes.
    pub total_bytes: u64,
    /// Bytes available to this user.
    pub free_bytes: u64,
    /// `st_dev` of the mount point; equal ids mean the same volume (cross-volume move checks).
    pub device_id: u64,
}

/// `MNT_DONTBROWSE`: system volumes that Finder hides.
const MNT_DONTBROWSE: u32 = 0x0010_0000;

fn info_from(s: sv::StatFs) -> VolumeInfo {
    let is_network = s.flags & sv::MNT_LOCAL == 0;
    let facts = if is_network {
        None
    } else {
        sv::disk_facts(&s.mount_from)
    };
    let attrs = sv::vol_attrs(&s.mount_on).unwrap_or_default();
    // The sealed system volume mounted at `/` has no name of its own; Finder shows the
    // name of its paired Data volume.
    let paired = (s.mount_on == Path::new("/"))
        .then(|| sv::vol_attrs(Path::new("/System/Volumes/Data")).and_then(|a| a.name))
        .flatten();
    let name = attrs
        .name
        .filter(|n| !n.is_empty())
        .or(paired)
        .unwrap_or_else(|| fallback_volume_name(&s.mount_on));
    let device_id = crate::sys::lstat(&s.mount_on)
        .map(|st| st.st_dev as u32 as u64)
        .unwrap_or(0);
    VolumeInfo {
        case_sensitive: sv::case_sensitive(&s.mount_on).unwrap_or(false),
        supports_clone: attrs.supports_clone,
        is_removable: facts.is_some_and(|f| f.removable),
        // Without an answer from DiskArbitration, a local volume is assumed to be internal.
        is_internal: !is_network && facts.is_none_or(|f| f.internal),
        is_network,
        is_read_only: s.flags & sv::MNT_RDONLY != 0,
        total_bytes: s.blocks.saturating_mul(s.bsize),
        free_bytes: s.bavail.saturating_mul(s.bsize),
        mount_path: s.mount_on,
        name,
        fs_type: s.fs_type,
        device_id,
    }
}

/// The volume that contains `path` (the path must exist).
pub fn volume_for(path: impl AsRef<Path>) -> Result<VolumeInfo> {
    let path = path.as_ref();
    let s = sv::statfs(path).map_err(|e| Error::io(path, e))?;
    let v = info_from(s);
    remember(std::slice::from_ref(&v), false);
    Ok(v)
}

/// The startup disk's name when the system won't tell us (macOS's default).
pub const STARTUP_DISK_FALLBACK: &str = "Macintosh HD";

/// A volume's name when it has none of its own: its mount folder's name
/// (`/Volumes/Card` → `Card`), or [`STARTUP_DISK_FALLBACK`] for `/`. Never
/// the raw path "/".
fn fallback_volume_name(mount: &Path) -> String {
    mount
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| STARTUP_DISK_FALLBACK.to_string())
}

/// Volume names by mount point, as last read by [`volumes`] or
/// [`volume_for`] of a mount point.
static NAMES: std::sync::RwLock<Vec<(PathBuf, String)>> = std::sync::RwLock::new(Vec::new());

fn remember(list: &[VolumeInfo], all: bool) {
    if let Ok(mut n) = NAMES.write() {
        if all {
            n.clear();
        }
        for v in list {
            n.retain(|(p, _)| *p != v.mount_path);
            n.push((v.mount_path.clone(), v.name.clone()));
        }
    }
}

/// **The** name to show for a path, everywhere (sidebar, breadcrumbs, tab
/// and window titles, toasts, questions, operation titles): the volume's
/// name for a volume root ("Macintosh HD" for `/`, "Card" for
/// `/Volumes/Card`), else the item's own name exactly as stored, else the
/// path itself. It never answers "/" for the startup disk.
///
/// Uses the names the last [`volumes`] call read; for `/` before any such
/// call it asks the system once (`statfs` + `getattrlist` on `/`, never a
/// slow network mount).
pub fn display_name(path: &Path) -> String {
    let trimmed = path.to_string_lossy();
    let trimmed = trimmed.trim_end_matches('/');
    let path = Path::new(if trimmed.is_empty() { "/" } else { trimmed });
    if let Ok(n) = NAMES.read()
        && let Some((_, name)) = n.iter().find(|(p, _)| p == path)
        && !name.is_empty()
        && name != "/"
    {
        return name.clone();
    }
    if path == Path::new("/") {
        return volume_for(path)
            .map(|v| v.name)
            .ok()
            .filter(|n| !n.is_empty() && n != "/")
            .unwrap_or_else(|| STARTUP_DISK_FALLBACK.to_string());
    }
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// All user-visible mounted volumes, without waiting on slow network mounts.
///
/// System-internal volumes that Finder hides (`MNT_DONTBROWSE`, e.g. VM, Preboot) and
/// pseudo filesystems (`devfs`, `autofs`) are left out; `/` is always kept.
pub fn volumes() -> Result<Vec<VolumeInfo>> {
    let list = sv::mounts().map_err(|e| Error::io(Path::new("/"), e))?;
    Ok(list
        .into_iter()
        .filter(|s| {
            s.mount_on == Path::new("/")
                || (s.flags & MNT_DONTBROWSE == 0
                    && !matches!(s.fs_type.as_str(), "devfs" | "autofs" | "fdesc"))
        })
        .map(info_from)
        .collect::<Vec<_>>())
    .inspect(|v| remember(v, true))
}

#[cfg(test)]
mod display_name_tests {
    use super::*;

    /// The startup disk was shown as "/" in some places (a crumb, a tab
    /// title, "Moved “a” to “/”"). It has a name, always.
    #[test]
    fn the_startup_disk_is_never_called_slash() {
        let name = display_name(Path::new("/"));
        assert_ne!(name, "/");
        assert!(!name.is_empty());
        // The same name the volume list gives, before and after reading it.
        let root = volumes()
            .unwrap()
            .into_iter()
            .find(|v| v.mount_path == Path::new("/"))
            .unwrap();
        assert_eq!(display_name(Path::new("/")), root.name);
        assert_eq!(display_name(Path::new("//")), root.name);
    }

    #[test]
    fn other_paths_keep_their_names_as_stored() {
        assert_eq!(display_name(Path::new("/Volumes/Card")), "Card");
        assert_eq!(display_name(Path::new("/a/b/")), "b");
        // NFD stays NFD (F-017: names exactly as the filesystem returns them).
        let nfd = "Cafe\u{301}";
        assert_eq!(display_name(&Path::new("/x").join(nfd)), nfd);
        assert_eq!(fallback_volume_name(Path::new("/")), STARTUP_DISK_FALLBACK);
        assert_eq!(fallback_volume_name(Path::new("/Volumes/SD")), "SD");
    }
}

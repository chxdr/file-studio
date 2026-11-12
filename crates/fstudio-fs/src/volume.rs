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
        .unwrap_or_else(|| {
            s.mount_on
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".to_string())
        });
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
    Ok(info_from(s))
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
        .collect())
}

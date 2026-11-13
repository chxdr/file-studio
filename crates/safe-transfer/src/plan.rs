//! Looking before copying: what a copy would do, without reading any data.

use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::copy::CopyRequest;
use crate::platform::{Platform, VolumeInfo};
use crate::space;

/// What copying one file would do. Built from metadata only.
#[derive(Debug, Clone)]
pub struct Plan {
    /// Size of the original in bytes.
    pub bytes: u64,
    /// Whether the original is a symlink (copied as a link).
    pub is_link: bool,
    /// Same volume as the destination (an APFS clone, when the volume can).
    pub same_volume: bool,
    /// The destination name is already taken.
    pub conflict: bool,
    /// The original is an iCloud placeholder.
    pub dataless: bool,
    /// The destination volume.
    pub dest_volume: VolumeInfo,
    /// Bytes the copy needs on the destination, rounded to its allocation unit.
    pub need: u64,
    /// The safety margin on top of `need`.
    pub margin: u64,
    /// Whether `need + margin` fits in the destination's free space.
    pub fits: bool,
}

/// Plans `req`: stat only, never reads data or downloads placeholders.
pub fn plan(platform: &Platform, req: &CopyRequest) -> io::Result<Plan> {
    let smd = std::fs::symlink_metadata(&req.source)?;
    let dir = req.dest.parent().unwrap_or(Path::new("/"));
    let dmd = std::fs::metadata(dir)?;
    let vol = platform.volumes.volume_of(dir)?;
    let is_link = smd.file_type().is_symlink();
    let same_volume = smd.dev() == dmd.dev();
    let no_xattrs = matches!(vol.fs_type.as_str(), "exfat" | "msdos");
    let appledouble = no_xattrs
        && !crate::sys::xattr_names(&req.source)
            .unwrap_or_default()
            .is_empty();
    let need = if is_link || (same_volume && vol.fs_type == "apfs") {
        0
    } else {
        space::on_disk(smd.len(), vol.block_size, appledouble)
    };
    Ok(Plan {
        bytes: smd.len(),
        is_link,
        same_volume,
        conflict: std::fs::symlink_metadata(&req.dest).is_ok(),
        dataless: crate::sys::file_flags(&smd) & 0x4000_0000 != 0,
        margin: space::margin(need),
        fits: need == 0 || space::fits(need, vol.free),
        need,
        dest_volume: vol,
    })
}

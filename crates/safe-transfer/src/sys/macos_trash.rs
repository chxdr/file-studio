//! The macOS Trash through `NSFileManager trashItemAtURL:resultingItemURL:error:`,
//! so Finder's Put Back works. Extracted from `fstudio-ops` (`sys/trash.rs`).

use std::io;
use std::path::{Path, PathBuf};

use objc2::rc::autoreleasepool;
use objc2_foundation::{NSFileManager, NSURL};

/// Moves `path` to the Trash of its volume and returns where it landed.
/// A symlink is trashed as a link; its target is untouched. Blocks on disk.
pub fn trash_item(path: &Path) -> io::Result<PathBuf> {
    autoreleasepool(|_| {
        let url = NSURL::from_file_path(path).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "path can't be made into a file URL",
            )
        })?;
        let fm = NSFileManager::defaultManager();
        let mut out = None;
        fm.trashItemAtURL_resultingItemURL_error(&url, Some(&mut out))
            .map_err(|e| {
                let code = e.code();
                let msg = e.localizedDescription().to_string();
                // NSFileNoSuchFileError = 4, NSFileWriteNoPermissionError = 513.
                let kind = match code {
                    4 => io::ErrorKind::NotFound,
                    513 => io::ErrorKind::PermissionDenied,
                    _ => io::ErrorKind::Other,
                };
                io::Error::new(kind, msg)
            })?;
        out.and_then(|u| u.to_file_path())
            .ok_or_else(|| io::Error::other("the Trash did not report where the item went"))
    })
}

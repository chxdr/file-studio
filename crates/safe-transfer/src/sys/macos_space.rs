//! Free space the way Finder counts it: `NSURLVolumeAvailableCapacityForImportantUsageKey`
//! includes purgeable space (local snapshots, caches macOS can free on demand),
//! which `statfs` leaves out.

use std::path::Path;

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::AnyObject;
use objc2_foundation::{NSNumber, NSURL, NSURLVolumeAvailableCapacityForImportantUsageKey};

/// Bytes available for "important" use on the volume holding `path`
/// (purgeable space included), or `None` if the volume doesn't say.
pub fn important_free(path: &Path) -> Option<u64> {
    autoreleasepool(|_| {
        let url = NSURL::from_file_path(path)?;
        let mut value: Option<Retained<AnyObject>> = None;
        // SAFETY: reading an `extern` static NSURLResourceKey that Foundation
        // defines; it is valid for the life of the process.
        let key = unsafe { NSURLVolumeAvailableCapacityForImportantUsageKey };
        // SAFETY: for this key Foundation stores an NSNumber (or nothing) in
        // `value`, which is checked with a downcast before use.
        unsafe { url.getResourceValue_forKey_error(&mut value, key) }.ok()?;
        let n = value?.downcast::<NSNumber>().ok()?;
        Some(n.unsignedLongLongValue())
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_startup_disk_reports_important_free_space() {
        let v = super::important_free(std::path::Path::new("/"));
        assert!(v.is_some_and(|n| n > 0), "{v:?}");
    }
}

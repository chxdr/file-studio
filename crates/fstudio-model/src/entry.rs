//! One directory entry as listed. Produced by `fstudio-fs`, consumed by
//! `fstudio-view`, `fstudio-ops` and the shells.

/// What an entry is, as `lstat` sees it (symlinks are never followed when listing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    /// FIFOs, sockets, devices.
    Other,
}

/// BSD `st_flags` bits we care about (macOS values; Linux shells leave `flags` at 0).
pub mod file_flags {
    /// `UF_HIDDEN`: hidden in Finder.
    pub const HIDDEN: u32 = 0x0000_8000;
    /// `UF_IMMUTABLE`.
    pub const USER_IMMUTABLE: u32 = 0x0000_0002;
    /// `SF_IMMUTABLE`.
    pub const SYSTEM_IMMUTABLE: u32 = 0x0002_0000;
    /// `UF_COMPRESSED`.
    pub const COMPRESSED: u32 = 0x0000_0020;
    /// `SF_DATALESS`: an iCloud / File Provider placeholder. Reading its content
    /// downloads it, so previews, hashing and copying must check this first (F-016).
    pub const DATALESS: u32 = 0x4000_0000;
}

/// One directory entry with the cheap metadata that comes from the bulk listing
/// call. Expensive attributes (Finder tags, Spotlight metadata, git status,
/// folder sizes) are fetched lazily elsewhere and are not part of `Entry`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    /// The name exactly as the filesystem returned it (NFC or NFD, unchanged).
    /// Never normalise this for display or for file operations (F-017).
    pub name: String,
    pub kind: EntryKind,
    /// Logical size in bytes (`st_size`). For directories it is whatever the
    /// filesystem reports and is not shown as a folder size.
    pub size: u64,
    /// Bytes actually allocated on disk, when known (`None` if not fetched).
    /// It is 0 for dataless placeholders.
    pub alloc_size: Option<u64>,
    /// Modification time, in ns since the Unix epoch.
    pub mtime_ns: i64,
    /// Creation (birth) time, in ns since the Unix epoch, when the filesystem has one.
    pub btime_ns: Option<i64>,
    /// `st_mode` permission and type bits.
    pub mode: u32,
    /// BSD `st_flags` (see [`file_flags`]).
    pub flags: u32,
    /// The inode number (`st_ino`); with `dev` it identifies the file for loop detection.
    pub inode: u64,
    pub dev: u64,
    /// The directory is a package or bundle (`.app`, `.photoslibrary`) and is shown as one item.
    /// It is set by `fstudio-fs` from the extension list or the bundle bit.
    pub is_package: bool,
}

impl Entry {
    /// Dot-files and `UF_HIDDEN` items.
    pub fn is_hidden(&self) -> bool {
        self.name.starts_with('.') || self.flags & file_flags::HIDDEN != 0
    }
    /// An iCloud / File Provider placeholder whose content is not on this Mac.
    pub fn is_dataless(&self) -> bool {
        self.flags & file_flags::DATALESS != 0
    }
    pub fn is_dir(&self) -> bool {
        self.kind == EntryKind::Dir
    }
    /// The extension without the dot, as stored ("" when there is none or for dot-files like `.zshrc`).
    pub fn extension(&self) -> &str {
        match self.name.rfind('.') {
            Some(0) | None => "",
            Some(i) => &self.name[i + 1..],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, flags: u32) -> Entry {
        Entry {
            name: name.into(),
            kind: EntryKind::File,
            size: 0,
            alloc_size: None,
            mtime_ns: 0,
            btime_ns: None,
            mode: 0o644,
            flags,
            inode: 1,
            dev: 1,
            is_package: false,
        }
    }

    #[test]
    fn hidden_and_dataless() {
        assert!(e(".zshrc", 0).is_hidden());
        assert!(e("a", file_flags::HIDDEN).is_hidden());
        assert!(!e("a", 0).is_hidden());
        assert!(e("a", 0x4000_0060).is_dataless());
    }

    #[test]
    fn extension() {
        assert_eq!(e("a.tar.gz", 0).extension(), "gz");
        assert_eq!(e(".zshrc", 0).extension(), "");
        assert_eq!(e("README", 0).extension(), "");
        assert_eq!(e("DSC0001.ARW", 0).extension(), "ARW");
    }
}

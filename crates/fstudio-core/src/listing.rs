//! A tab's listing snapshot as the shell sees it, and the rows it asks for.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use fstudio_model::{Entry, EntryKind};
use fstudio_view::{Clock, DateFormat, SizeFormat, format_date, format_size};
use serde::{Deserialize, Serialize};

/// Where a listing is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ListingState {
    /// The folder is still being read; rows so far are sorted.
    Loading,
    /// Every entry is in.
    Complete,
    /// The folder could not be read (see [`Listing::error`]). Rows read before
    /// the error, if any, are kept.
    Error,
}

/// An immutable, versioned, sorted and filtered snapshot of a tab's folder
/// (F-005). Cheap to share; a new version replaces it on every change.
#[derive(Debug, Clone)]
pub struct Listing {
    pub(crate) version: u64,
    pub(crate) dir: PathBuf,
    pub(crate) state: ListingState,
    pub(crate) error: Option<String>,
    pub(crate) inner: fstudio_view::Listing,
}

impl Listing {
    /// The tab's listing version; it only grows.
    pub fn version(&self) -> u64 {
        self.version
    }
    /// The folder listed.
    pub fn dir(&self) -> String {
        self.dir.to_string_lossy().into_owned()
    }
    /// Loading, complete or failed.
    pub fn state(&self) -> ListingState {
        self.state
    }
    /// Why the folder could not be read, when `state` is `Error`.
    pub fn error(&self) -> Option<String> {
        self.error.clone()
    }
    /// Visible rows (after hidden files and the filter).
    pub fn len(&self) -> u32 {
        self.inner.len() as u32
    }
    /// True when no row is visible.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
    /// Every entry of the folder, including hidden and filtered-out ones.
    pub fn total_len(&self) -> u32 {
        self.inner.total_len() as u32
    }
    /// The row of the entry with exactly this name, if visible.
    pub fn index_of(&self, name: &str) -> Option<u32> {
        self.inner.index_of(name).map(|i| i as u32)
    }
    /// The name at row `i`.
    pub fn name_at(&self, i: u32) -> Option<String> {
        self.inner.entry(i as usize).map(|e| e.name.clone())
    }
    /// The full path at row `i`.
    pub fn path_at(&self, i: u32) -> Option<String> {
        self.inner
            .entry(i as usize)
            .map(|e| self.dir.join(&e.name).to_string_lossy().into_owned())
    }
    /// The entry at row `i`.
    pub fn entry(&self, i: u32) -> Option<Entry> {
        self.inner.entry(i as usize).cloned()
    }
    /// Names of all visible rows in order (for tools and tests; the shell uses rows).
    pub fn names(&self) -> Vec<String> {
        self.inner.iter().map(|e| e.name.clone()).collect()
    }
    pub(crate) fn path_of(&self, i: usize) -> Option<PathBuf> {
        self.inner.entry(i).map(|e| self.dir.join(&e.name))
    }
}

/// What a row shows, for choosing its icon and kind column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RowKind {
    /// A folder.
    Folder,
    /// A package or bundle shown as one item (`.app`).
    Package,
    /// A regular file.
    File,
    /// A symbolic link.
    Symlink,
    /// A device, socket or pipe.
    Other,
}

/// One row, ready to draw: names exactly as on disk, strings pre-formatted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowInfo {
    /// The row number.
    pub index: u32,
    /// The name exactly as the filesystem stores it.
    pub name: String,
    /// What it is.
    pub kind: RowKind,
    /// The shell maps this to an icon: `folder`, `package.app`, `file.png`,
    /// `file`, `symlink`, `other`.
    pub icon_key: String,
    /// The kind column text ("Folder", "PNG file").
    pub kind_text: String,
    /// Logical size in bytes (0 for folders).
    pub size: u64,
    /// The size column ("1.2 MB", "--" for folders).
    pub size_text: String,
    /// Modification time, ns since the epoch.
    pub modified_ns: i64,
    /// The date column ("Today at 14:32").
    pub modified_text: String,
    /// A dot-file or `UF_HIDDEN` item.
    pub is_hidden: bool,
    /// An iCloud placeholder whose content is not on this Mac.
    pub is_dataless: bool,
    /// A package shown as one item.
    pub is_package: bool,
    /// A symbolic link.
    pub is_symlink: bool,
    /// Selected.
    pub is_selected: bool,
    /// The cursor row.
    pub is_cursor: bool,
    /// Cut and waiting for Paste (draw dimmed).
    pub is_cut_pending: bool,
    /// Creation time, ns since the epoch (the modification time when the
    /// filesystem keeps no creation time).
    pub created_ns: i64,
    /// The Date Created column, formatted like `modified_text`.
    pub created_text: String,
    /// The Extension column: the extension as on disk ("png"), empty for
    /// folders and names without one; packages show theirs ("app").
    pub ext_text: String,
}

/// Formatting inputs fixed for one `rows` call.
pub(crate) struct RowFormat {
    pub(crate) clock: Clock,
    pub(crate) dates: DateFormat,
    pub(crate) sizes: SizeFormat,
}

impl RowFormat {
    pub(crate) fn now(hour24: bool) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        Self {
            clock: Clock {
                now_ns: now,
                utc_offset_secs: crate::sys::utc_offset_secs(now / 1_000_000_000),
            },
            dates: DateFormat {
                hour24,
                ..DateFormat::default()
            },
            sizes: SizeFormat::default(),
        }
    }
}

pub(crate) fn row_kind(e: &Entry) -> RowKind {
    match e.kind {
        EntryKind::Dir if e.is_package => RowKind::Package,
        EntryKind::Dir => RowKind::Folder,
        EntryKind::File => RowKind::File,
        EntryKind::Symlink => RowKind::Symlink,
        EntryKind::Other => RowKind::Other,
    }
}

pub(crate) fn icon_key(e: &Entry) -> String {
    let ext = e.extension().to_ascii_lowercase();
    match row_kind(e) {
        RowKind::Folder => "folder".into(),
        RowKind::Package => format!("package.{ext}"),
        RowKind::File if ext.is_empty() => "file".into(),
        RowKind::File => format!("file.{ext}"),
        RowKind::Symlink => "symlink".into(),
        RowKind::Other => "other".into(),
    }
}

pub(crate) fn kind_text(e: &Entry) -> String {
    match row_kind(e) {
        RowKind::Folder => "Folder".into(),
        RowKind::Package if e.extension().eq_ignore_ascii_case("app") => "Application".into(),
        RowKind::Package => "Package".into(),
        RowKind::Symlink => "Symbolic Link".into(),
        RowKind::Other => "Special File".into(),
        RowKind::File if e.extension().is_empty() => "Document".into(),
        RowKind::File => format!("{} File", e.extension().to_uppercase()),
    }
}

/// Builds rows `start..start+count` of `l`.
pub(crate) fn build_rows(
    l: &Listing,
    start: u32,
    count: u32,
    selected: &dyn Fn(usize) -> bool,
    cursor: Option<usize>,
    cut: &HashSet<String>,
    f: &RowFormat,
) -> Vec<RowInfo> {
    let start = start as usize;
    l.inner
        .rows(start, count as usize)
        .enumerate()
        .map(|(k, e)| {
            let i = start + k;
            let kind = row_kind(e);
            let folder = matches!(kind, RowKind::Folder);
            RowInfo {
                index: i as u32,
                name: e.name.clone(),
                kind,
                icon_key: icon_key(e),
                kind_text: kind_text(e),
                size: if folder { 0 } else { e.size },
                size_text: if folder {
                    "--".into()
                } else {
                    format_size(e.size, &f.sizes)
                },
                modified_ns: e.mtime_ns,
                modified_text: format_date(e.mtime_ns, f.clock, &f.dates),
                is_hidden: e.is_hidden(),
                is_dataless: e.is_dataless(),
                is_package: e.is_package,
                is_symlink: e.kind == EntryKind::Symlink,
                is_selected: selected(i),
                is_cursor: cursor == Some(i),
                is_cut_pending: !cut.is_empty() && cut.contains(&e.name),
                created_ns: e.btime_ns.unwrap_or(e.mtime_ns),
                created_text: format_date(e.btime_ns.unwrap_or(e.mtime_ns), f.clock, &f.dates),
                ext_text: if folder {
                    String::new()
                } else {
                    e.extension().to_string()
                },
            }
        })
        .collect()
}

pub(crate) fn empty(dir: &Path, opts: fstudio_view::ViewOptions, version: u64) -> Listing {
    Listing {
        version,
        dir: dir.to_path_buf(),
        state: ListingState::Loading,
        error: None,
        inner: fstudio_view::Listing::empty(opts),
    }
}

//! What to search for.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::kind::Kind;

/// How the name text is matched. Every mode compares NFC, case-folded
/// names (F-017).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NameMode {
    /// The name contains the text anywhere.
    #[default]
    Contains,
    /// The whole name matches a shell glob (`*.png`, `IMG_????.*`, `[ab]*`).
    Glob,
    /// A regular expression found anywhere in the name.
    Regex,
}

/// Which date the date range applies to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DateField {
    /// Date Modified.
    #[default]
    Modified,
    /// Date Created (falls back to Modified where the filesystem has none).
    Created,
}

/// Folders a "This Mac" walk never enters: other volumes (they can be slow
/// or networked; Spotlight still covers indexed ones), the APFS system
/// volumes reached again through firmlinks, devices and VM files.
pub const THIS_MAC_SKIP: &[&str] = &[
    "/Volumes",
    "/System/Volumes",
    "/dev",
    "/private/var/vm",
    "/.vol",
    "/net",
    "/home",
    "/cores",
    "/.Spotlight-V100",
    "/.fseventsd",
    "/.DocumentRevisions-V100",
    "/.MobileBackups",
];

/// A search: where, what, and how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchQuery {
    /// Folders to search below (the folders themselves are not results).
    pub roots: Vec<PathBuf>,
    /// Folders never entered (absolute paths; their insides are skipped too).
    pub skip: Vec<PathBuf>,
    /// The name text. Empty matches every name.
    pub name: String,
    /// How `name` is matched.
    pub name_mode: NameMode,
    /// Any of these kinds; empty means any kind.
    pub kinds: Vec<Kind>,
    /// Smallest size in bytes, inclusive. Folders never match a size filter.
    pub min_size: Option<u64>,
    /// Largest size in bytes, inclusive.
    pub max_size: Option<u64>,
    /// Which date `date_from_ns`/`date_to_ns` filter.
    pub date_field: DateField,
    /// Earliest date, ns since the epoch, inclusive.
    pub date_from_ns: Option<i64>,
    /// Latest date, ns since the epoch, inclusive.
    pub date_to_ns: Option<i64>,
    /// Finder tags the item must all have (matched by name, case-folded).
    pub tags: Vec<String>,
    /// Text the file must contain. Off when `None`. Only text files at most
    /// `content_max_bytes` long are read; iCloud placeholders never are.
    pub content: Option<String>,
    /// The size cap for content search.
    pub content_max_bytes: u64,
    /// Also match and enter dot-files and `UF_HIDDEN` items.
    pub include_hidden: bool,
    /// Also enter packages (`.app`, `.photoslibrary`). Packages themselves
    /// are always candidates.
    pub include_packages: bool,
    /// Ask the Spotlight index too (results are re-checked on disk).
    pub use_index: bool,
    /// Walk the folders (the accurate source).
    pub walk: bool,
    /// Stop after this many results (the summary says it was cut short).
    pub max_results: usize,
    /// Walker threads; 0 picks the number of cores, at most 8.
    pub threads: usize,
}

impl Default for SearchQuery {
    fn default() -> Self {
        SearchQuery {
            roots: Vec::new(),
            skip: Vec::new(),
            name: String::new(),
            name_mode: NameMode::Contains,
            kinds: Vec::new(),
            min_size: None,
            max_size: None,
            date_field: DateField::Modified,
            date_from_ns: None,
            date_to_ns: None,
            tags: Vec::new(),
            content: None,
            content_max_bytes: 8 * 1024 * 1024,
            include_hidden: false,
            include_packages: false,
            use_index: true,
            walk: true,
            max_results: 50_000,
            threads: 0,
        }
    }
}

impl SearchQuery {
    /// A query below one folder, everything else at its default.
    pub fn in_folder(root: impl Into<PathBuf>) -> Self {
        SearchQuery {
            roots: vec![root.into()],
            ..Default::default()
        }
    }

    /// A query over the whole Mac: the walk starts at `/` and skips
    /// [`THIS_MAC_SKIP`].
    pub fn this_mac() -> Self {
        SearchQuery {
            roots: vec![PathBuf::from("/")],
            skip: THIS_MAC_SKIP.iter().map(PathBuf::from).collect(),
            ..Default::default()
        }
    }

    /// True when nothing narrows the search (every item would match).
    pub fn is_unfiltered(&self) -> bool {
        self.name.is_empty()
            && self.kinds.is_empty()
            && self.min_size.is_none()
            && self.max_size.is_none()
            && self.date_from_ns.is_none()
            && self.date_to_ns.is_none()
            && self.tags.is_empty()
            && self.content.as_deref().is_none_or(str::is_empty)
    }

    /// The walker thread count to use.
    pub(crate) fn thread_count(&self) -> usize {
        if self.threads > 0 {
            return self.threads;
        }
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(2, 8)
    }
}

/// Why a query can't run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    /// The glob or regular expression doesn't compile.
    BadPattern(String),
    /// No folder to search.
    NoRoots,
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryError::BadPattern(m) => write!(f, "the pattern isn’t valid: {m}"),
            QueryError::NoRoots => f.write_str("there is no folder to search"),
        }
    }
}

impl std::error::Error for QueryError {}

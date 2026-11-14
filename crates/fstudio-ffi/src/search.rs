//! M3 search over the FFI: run a search in a tab, stop it, end it, and read
//! its status. The results themselves are the tab's rows, as for a folder.

use crate::{FfiError, FfiTab};
use fstudio_core as core;

/// Where a search looks.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiSearchScope {
    /// The folder the tab shows, and below.
    ThisFolder,
    /// The whole Mac.
    ThisMac,
}

/// How the name text is matched (NFC, case-folded).
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiNameMode {
    Contains,
    Glob,
    Regex,
}

/// A kind for the kind filter.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiSearchKind {
    Folder,
    Application,
    Image,
    Movie,
    Audio,
    Pdf,
    Text,
    Document,
    Archive,
    Other,
}

/// Which date the date range filters.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiDateField {
    Modified,
    Created,
}

/// A search to run. Unset filters are `None` or empty.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiSearchRequest {
    pub scope: FfiSearchScope,
    pub name: String,
    pub name_mode: FfiNameMode,
    pub kinds: Vec<FfiSearchKind>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub date_field: FfiDateField,
    /// ns since the epoch, inclusive.
    pub date_from_ns: Option<i64>,
    pub date_to_ns: Option<i64>,
    pub tags: Vec<String>,
    /// Text the file must contain (text files only); `None` is off.
    pub content: Option<String>,
    pub include_hidden: bool,
    pub include_packages: bool,
}

/// Where a search is in its life.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiSearchState {
    Searching,
    Done,
    Stopped,
    Truncated,
}

/// A search tab's status, for the search bar.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiSearchStatus {
    pub request: FfiSearchRequest,
    /// The folder the tab showed before (Back returns there).
    pub folder: String,
    pub root: String,
    pub state: FfiSearchState,
    pub found: u64,
    pub dirs_scanned: u64,
    /// The first 200 folders that couldn't be read.
    pub denied: Vec<String>,
    pub denied_count: u64,
    pub cloud_skipped: u64,
    pub index_used: bool,
    pub index_error: Option<String>,
    pub elapsed_ms: u64,
    pub first_hit_ms: Option<u64>,
    /// The status line, ready to show.
    pub text: String,
}

fn kind_to(k: FfiSearchKind) -> core::SearchKind {
    use core::SearchKind as K;
    match k {
        FfiSearchKind::Folder => K::Folder,
        FfiSearchKind::Application => K::Application,
        FfiSearchKind::Image => K::Image,
        FfiSearchKind::Movie => K::Movie,
        FfiSearchKind::Audio => K::Audio,
        FfiSearchKind::Pdf => K::Pdf,
        FfiSearchKind::Text => K::Text,
        FfiSearchKind::Document => K::Document,
        FfiSearchKind::Archive => K::Archive,
        FfiSearchKind::Other => K::Other,
    }
}

fn kind_from(k: core::SearchKind) -> FfiSearchKind {
    use core::SearchKind as K;
    match k {
        K::Folder => FfiSearchKind::Folder,
        K::Application => FfiSearchKind::Application,
        K::Image => FfiSearchKind::Image,
        K::Movie => FfiSearchKind::Movie,
        K::Audio => FfiSearchKind::Audio,
        K::Pdf => FfiSearchKind::Pdf,
        K::Text => FfiSearchKind::Text,
        K::Document => FfiSearchKind::Document,
        K::Archive => FfiSearchKind::Archive,
        K::Other => FfiSearchKind::Other,
    }
}

impl From<FfiSearchRequest> for core::SearchRequest {
    fn from(r: FfiSearchRequest) -> Self {
        core::SearchRequest {
            scope: match r.scope {
                FfiSearchScope::ThisFolder => core::SearchScope::ThisFolder,
                FfiSearchScope::ThisMac => core::SearchScope::ThisMac,
            },
            query: core::SearchQuery {
                name: r.name,
                name_mode: match r.name_mode {
                    FfiNameMode::Contains => core::NameMode::Contains,
                    FfiNameMode::Glob => core::NameMode::Glob,
                    FfiNameMode::Regex => core::NameMode::Regex,
                },
                kinds: r.kinds.into_iter().map(kind_to).collect(),
                min_size: r.min_size,
                max_size: r.max_size,
                date_field: match r.date_field {
                    FfiDateField::Modified => core::DateField::Modified,
                    FfiDateField::Created => core::DateField::Created,
                },
                date_from_ns: r.date_from_ns,
                date_to_ns: r.date_to_ns,
                tags: r.tags,
                content: r.content.filter(|c| !c.is_empty()),
                include_hidden: r.include_hidden,
                include_packages: r.include_packages,
                ..Default::default()
            },
        }
    }
}

fn request_from(scope: core::SearchScope, q: core::SearchQuery) -> FfiSearchRequest {
    FfiSearchRequest {
        scope: match scope {
            core::SearchScope::ThisFolder => FfiSearchScope::ThisFolder,
            core::SearchScope::ThisMac => FfiSearchScope::ThisMac,
        },
        name: q.name,
        name_mode: match q.name_mode {
            core::NameMode::Contains => FfiNameMode::Contains,
            core::NameMode::Glob => FfiNameMode::Glob,
            core::NameMode::Regex => FfiNameMode::Regex,
        },
        kinds: q.kinds.into_iter().map(kind_from).collect(),
        min_size: q.min_size,
        max_size: q.max_size,
        date_field: match q.date_field {
            core::DateField::Modified => FfiDateField::Modified,
            core::DateField::Created => FfiDateField::Created,
        },
        date_from_ns: q.date_from_ns,
        date_to_ns: q.date_to_ns,
        tags: q.tags,
        content: q.content,
        include_hidden: q.include_hidden,
        include_packages: q.include_packages,
    }
}

impl From<core::SearchStatus> for FfiSearchStatus {
    fn from(s: core::SearchStatus) -> Self {
        FfiSearchStatus {
            request: request_from(s.scope, s.query),
            folder: s.folder,
            root: s.root,
            state: match s.state {
                core::SearchState::Searching => FfiSearchState::Searching,
                core::SearchState::Done => FfiSearchState::Done,
                core::SearchState::Stopped => FfiSearchState::Stopped,
                core::SearchState::Truncated => FfiSearchState::Truncated,
            },
            found: s.found,
            dirs_scanned: s.dirs_scanned,
            denied: s.denied,
            denied_count: s.denied_count,
            cloud_skipped: s.cloud_skipped,
            index_used: s.index_used,
            index_error: s.index_error,
            elapsed_ms: s.elapsed_ms,
            first_hit_ms: s.first_hit_ms,
            text: s.text,
        }
    }
}

#[uniffi::export]
impl FfiTab {
    /// Runs a search in this tab; results stream in as its rows
    /// (`ListingChanged` Partial…, then Sorted). Fails on a bad pattern.
    pub fn search(&self, request: FfiSearchRequest) -> Result<(), FfiError> {
        Ok(self.inner.search(request.into())?)
    }
    /// Stops a running search, keeping the results so far.
    pub fn stop_search(&self) -> bool {
        self.inner.stop_search()
    }
    /// Ends the search and shows the folder it started from.
    pub fn end_search(&self) -> bool {
        self.inner.end_search()
    }
    /// The search's status; `None` for a folder tab.
    pub fn search_status(&self) -> Option<FfiSearchStatus> {
        self.inner.search_status().map(Into::into)
    }
}

//! A tab's view state, in a serialisable form, and its mapping onto
//! `fstudio-view`'s options.

use fstudio_keymap::ViewMode;
use fstudio_view::{SortKey, ViewOptions};
use serde::{Deserialize, Serialize};

use crate::settings::ViewSettings;

/// What a listing is sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SortBy {
    /// Natural name order.
    #[default]
    Name,
    /// Logical size.
    Size,
    /// Modification date.
    Modified,
    /// Creation date.
    Created,
    /// Kind (folders, packages, links, then by extension).
    Kind,
    /// Extension, then name.
    Extension,
}

impl SortBy {
    pub(crate) fn key(self) -> SortKey {
        match self {
            SortBy::Name => SortKey::Name,
            SortBy::Size => SortKey::Size,
            SortBy::Modified => SortKey::Modified,
            SortBy::Created => SortKey::Created,
            SortBy::Kind => SortKey::Kind,
            SortBy::Extension => SortKey::Extension,
        }
    }
}

/// Everything about how one tab shows its folder, except the filter text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewState {
    /// List, icons, columns or gallery.
    pub mode: ViewMode,
    /// The column sorted by.
    pub sort: SortBy,
    /// Smallest/oldest/A first.
    pub ascending: bool,
    /// Keep folders above files.
    pub folders_first: bool,
    /// Show dot-files and hidden items.
    pub show_hidden: bool,
    /// Treat packages as files when sorting.
    pub packages_as_files: bool,
}

impl Default for ViewState {
    fn default() -> Self {
        Self::from_settings(&ViewSettings::default())
    }
}

impl ViewState {
    /// The defaults from settings.
    pub fn from_settings(v: &ViewSettings) -> Self {
        Self {
            mode: v.mode,
            sort: v.sort,
            ascending: v.ascending,
            folders_first: v.folders_first,
            show_hidden: v.show_hidden,
            packages_as_files: v.packages_as_files,
        }
    }

    /// The `fstudio-view` options for this state plus a filter.
    pub(crate) fn options(&self, filter: &str) -> ViewOptions {
        ViewOptions {
            sort_key: self.sort.key(),
            ascending: self.ascending,
            folders_first: self.folders_first,
            show_hidden: self.show_hidden,
            filter: filter.to_string(),
            packages_as_files: self.packages_as_files,
        }
    }
}

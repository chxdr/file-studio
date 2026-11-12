//! A listing without a tab: one folder for the column view (Miller columns).
//!
//! A handle runs the same pipeline as a tab (background lister, `Partial`
//! then `Sorted`, watcher patches, stale generations dropped) because it *is*
//! a detached [`Tab`]: one that isn't in the workspace, so it never emits
//! `TabsChanged`, never saves into the session and never records per-folder
//! view memory. Its events are `ListingChanged` / `SelectionChanged` with the
//! handle's id in the `tab` field. Dropping the handle stops the listing and
//! unwatches the folder.

use std::path::PathBuf;
use std::sync::Arc;

use crate::engine::Engine;
use crate::ids::TabId;
use crate::listing::{Listing, RowInfo};
use crate::menus::MenuTarget;
use crate::tab::{SelectionChange, SelectionInfo, Tab, TabState};
use crate::view::ViewState;

/// One folder's listing with its own selection, outside any tab.
#[derive(Debug)]
pub struct ListingHandle {
    tab: Arc<Tab>,
}

impl Drop for ListingHandle {
    fn drop(&mut self) {
        self.tab.close();
    }
}

impl ListingHandle {
    /// The handle's id; its events carry it in the `tab` field.
    pub fn id(&self) -> TabId {
        self.tab.id()
    }
    /// The folder listed.
    pub fn dir(&self) -> String {
        self.tab.location()
    }
    /// The current snapshot. Never blocks.
    pub fn listing(&self) -> Arc<Listing> {
        self.tab.listing()
    }
    /// Rows `start..start+count`, formatted, with selection and cut flags.
    pub fn rows(&self, start: u32, count: u32) -> Vec<RowInfo> {
        self.tab.rows(start, count)
    }
    /// The selection.
    pub fn selection(&self) -> SelectionInfo {
        self.tab.selection()
    }
    /// Changes the selection (emits `SelectionChanged` with the handle's id).
    pub fn set_selection(&self, change: SelectionChange) {
        self.tab.set_selection(change);
    }
    /// Paths of the selected rows (or the cursor row).
    pub fn selected_paths(&self) -> Vec<String> {
        self.tab.selected_paths()
    }
    /// The rows on screen.
    pub fn set_visible_range(&self, start: u32, count: u32) {
        self.tab.set_visible_range(start, count);
    }
    /// The view (sort, hidden files). Not remembered per folder.
    pub fn view(&self) -> ViewState {
        self.tab.view()
    }
    /// Changes sort or hidden files; re-sorts off the calling thread.
    pub fn set_view(&self, v: ViewState) {
        self.tab.set_view(v);
    }
    /// Reads the folder again, keeping the selection.
    pub fn refresh(&self) {
        self.tab.refresh();
    }

    /// The right-click target for `row` (`None`: empty space), by Finder's
    /// rule: a click outside the selection selects only that row first.
    /// Commands from the menu act through `owner`, the tab whose column view
    /// shows this listing; the target's paths are this listing's.
    pub fn menu_target_at(&self, owner: TabId, row: Option<u32>) -> MenuTarget {
        let empty = MenuTarget::EmptySpace { tab: owner };
        self.tab.note_user_selection();
        let l = self.listing();
        let Some(row) = row.filter(|&r| r < l.len()) else {
            self.set_selection(SelectionChange::None);
            return empty;
        };
        if !self.selection().rows.contains(&row) {
            self.set_selection(SelectionChange::Only(row));
        }
        let sel = self.selection();
        let dir = PathBuf::from(self.dir());
        let paths: Vec<String> = sel
            .names
            .iter()
            .map(|n| dir.join(n).to_string_lossy().into_owned())
            .collect();
        if paths.len() > 1 {
            return MenuTarget::Multi { tab: owner, paths };
        }
        let Some(e) = sel
            .names
            .first()
            .and_then(|n| l.index_of(n))
            .and_then(|i| l.entry(i))
        else {
            return empty;
        };
        let path = paths[0].clone();
        if e.is_package {
            MenuTarget::Package { tab: owner, path }
        } else if e.is_dir() {
            MenuTarget::Folder { tab: owner, path }
        } else {
            MenuTarget::File { tab: owner, path }
        }
    }
}

impl Engine {
    /// Lists `path` without a tab (a column of the column view). The view
    /// (sort, columns) is the folder's remembered one or the defaults;
    /// `show_hidden` usually follows the owning tab. The listing starts at
    /// once on a background thread; follow it with `ListingChanged` events
    /// carrying [`ListingHandle::id`].
    pub fn open_listing(&self, path: &str, show_hidden: bool) -> Arc<ListingHandle> {
        let dir = PathBuf::from(path);
        let defaults = ViewState::from_settings(&self.shared.settings.read().view);
        let mut view = self.shared.folder_view(&dir).unwrap_or(defaults);
        view.show_hidden = show_hidden;
        let state = TabState {
            location: path.to_string(),
            view,
            ..TabState::default()
        };
        Arc::new(ListingHandle {
            tab: Tab::create_with(self.shared.clone(), state, true),
        })
    }

    /// The folders the shared watcher watches now (tools and tests).
    pub fn watched_folders(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .shared
            .watch
            .watched()
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }
}

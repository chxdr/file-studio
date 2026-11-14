//! Context menus defined by the core (F-022, `docs/CONTEXT-MENUS.md`).
//!
//! Each kind of thing you can right-click ([`MenuKind`]) has a default list
//! of *entries*: a command id (`file.duplicate`), a group (`menu.copy_path`,
//! a submenu the core fills), a system submenu (`system.share`, drawn by the
//! shell) or a separator (`---`). The user's changes are stored as a diff per
//! kind ([`MenuOverride`]: removed, added, moved), like keymap overrides, so
//! new defaults still reach people who customised a menu.
//!
//! [`Engine::context_menu`] turns the effective entries into [`MenuItem`]s
//! for one [`MenuTarget`]: it hides what doesn't apply (dual-pane-only items
//! in a single pane, Delete Immediately while it is off, later-milestone
//! commands…), greys out Paste when there is nothing to paste, ticks checked
//! items (sort order, view mode, tags), fills in each item's current
//! shortcut and the exact [`CommandInvocation`] to send back with
//! [`Engine::perform`]. The shell only draws the list.
//!
//! For rows of a list, call [`Engine::menu_target_at`] first: it applies
//! Finder's right-click rule to the selection (a click on an unselected row
//! selects only that row) and says which target the click is.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fstudio_keymap::{CommandArg, ViewMode, registry};
use serde::{Deserialize, Serialize};

use crate::commands::{CommandInvocation, LATER_COMMANDS};
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::ids::TabId;
use crate::tab::{SelectionChange, Tab};
use crate::view::{SortBy, ViewState};

/// The separator entry.
pub const SEPARATOR: &str = "---";

/// What a sidebar row stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SidebarItemKind {
    /// A favourite folder (can be removed and renamed).
    Favourite,
    /// A mounted volume (can show drive info; ejectable ones can be ejected).
    Volume,
}

/// What was right-clicked, with the paths involved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MenuTarget {
    /// One file in a tab's list.
    File {
        /// The tab.
        tab: TabId,
        /// The file.
        path: String,
    },
    /// One folder in a tab's list.
    Folder {
        /// The tab.
        tab: TabId,
        /// The folder.
        path: String,
    },
    /// One package (`.app`, `.bundle`…) in a tab's list.
    Package {
        /// The tab.
        tab: TabId,
        /// The package.
        path: String,
    },
    /// Several selected items in a tab's list.
    Multi {
        /// The tab.
        tab: TabId,
        /// The items.
        paths: Vec<String>,
    },
    /// Empty space in a tab's list (the folder itself).
    EmptySpace {
        /// The tab.
        tab: TabId,
    },
    /// A segment of a tab's address bar.
    Crumb {
        /// The tab.
        tab: TabId,
        /// The segment's folder.
        path: String,
    },
    /// A sidebar row.
    SidebarItem {
        /// The folder or volume.
        path: String,
        /// What the row is.
        kind: SidebarItemKind,
    },
    /// A tab in the tab bar.
    Tab {
        /// The tab.
        tab: TabId,
    },
}

impl MenuTarget {
    /// Which menu this target shows.
    pub fn kind(&self) -> MenuKind {
        match self {
            MenuTarget::File { .. } => MenuKind::File,
            MenuTarget::Folder { .. } => MenuKind::Folder,
            MenuTarget::Package { .. } => MenuKind::Package,
            MenuTarget::Multi { .. } => MenuKind::Multi,
            MenuTarget::EmptySpace { .. } => MenuKind::EmptySpace,
            MenuTarget::Crumb { .. } => MenuKind::Crumb,
            MenuTarget::SidebarItem { .. } => MenuKind::SidebarItem,
            MenuTarget::Tab { .. } => MenuKind::Tab,
        }
    }

    fn tab(&self) -> Option<TabId> {
        match self {
            MenuTarget::File { tab, .. }
            | MenuTarget::Folder { tab, .. }
            | MenuTarget::Package { tab, .. }
            | MenuTarget::Multi { tab, .. }
            | MenuTarget::EmptySpace { tab }
            | MenuTarget::Crumb { tab, .. }
            | MenuTarget::Tab { tab } => Some(*tab),
            MenuTarget::SidebarItem { .. } => None,
        }
    }

    fn paths(&self) -> Vec<PathBuf> {
        match self {
            MenuTarget::File { path, .. }
            | MenuTarget::Folder { path, .. }
            | MenuTarget::Package { path, .. }
            | MenuTarget::Crumb { path, .. }
            | MenuTarget::SidebarItem { path, .. } => vec![PathBuf::from(path)],
            MenuTarget::Multi { paths, .. } => paths.iter().map(PathBuf::from).collect(),
            MenuTarget::EmptySpace { .. } | MenuTarget::Tab { .. } => Vec::new(),
        }
    }
}

/// The kinds of context menu, each with its own contents and overrides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MenuKind {
    /// One file.
    File,
    /// One folder.
    Folder,
    /// One package.
    Package,
    /// Several items.
    Multi,
    /// Empty space in a folder.
    EmptySpace,
    /// An address-bar segment.
    Crumb,
    /// A sidebar row.
    SidebarItem,
    /// A tab.
    Tab,
}

impl MenuKind {
    /// Every kind, in Settings order.
    pub const ALL: [MenuKind; 8] = [
        MenuKind::File,
        MenuKind::Folder,
        MenuKind::Package,
        MenuKind::Multi,
        MenuKind::EmptySpace,
        MenuKind::Crumb,
        MenuKind::SidebarItem,
        MenuKind::Tab,
    ];

    /// The heading for Settings.
    pub fn title(self) -> &'static str {
        match self {
            MenuKind::File => "File",
            MenuKind::Folder => "Folder",
            MenuKind::Package => "Package",
            MenuKind::Multi => "Multiple Items",
            MenuKind::EmptySpace => "Empty Space",
            MenuKind::Crumb => "Address Bar Segment",
            MenuKind::SidebarItem => "Sidebar Item",
            MenuKind::Tab => "Tab",
        }
    }
}

/// What the shell knows that the core doesn't, at the moment of the click.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MenuContext {
    /// The system pasteboard holds file URLs from another app (so Paste
    /// applies even when the core's own clipboard is empty).
    pub pasteboard_has_files: bool,
}

/// A submenu the system provides, drawn by the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SystemMenu {
    /// Open With ›: the default app, the other apps for this type, then an
    /// "Other…" item that performs `file.open_with`.
    OpenWith,
    /// Share › (`NSSharingServicePicker`).
    Share,
    /// Quick Actions › (the Finder extension actions).
    QuickActions,
    /// Services › (`NSServices`).
    Services,
}

/// One clickable menu item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuCommand {
    /// The entry it came from (its command id).
    pub entry: String,
    /// The title to show.
    pub title: String,
    /// The user's current shortcut, as key-cap text.
    pub shortcut: Option<String>,
    /// Clickable. Only Paste is ever shown greyed; other items that don't
    /// apply are left out.
    pub enabled: bool,
    /// Shown with a tick (the current sort, view mode, a tag that is set).
    pub checked: bool,
    /// A Finder tag colour index (1–7) to draw as a dot, for tag items.
    pub tag_color: Option<u8>,
    /// What to send to `Engine::perform` when it is chosen.
    pub invocation: CommandInvocation,
}

/// One row of a context menu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MenuItem {
    /// A command.
    Command(MenuCommand),
    /// A separator line.
    Separator,
    /// A submenu the core filled.
    Submenu {
        /// The entry it came from (`menu.copy_path`).
        entry: String,
        /// Its title.
        title: String,
        /// Its items.
        items: Vec<MenuItem>,
    },
    /// A submenu the shell fills from the system.
    System {
        /// Which one.
        kind: SystemMenu,
        /// Its title.
        title: String,
        /// The items it is for.
        paths: Vec<String>,
    },
}

/// Where an added or moved entry goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuPlacement {
    /// The entry.
    pub entry: String,
    /// The entry it follows; `None` puts it first. An anchor that is no
    /// longer in the menu puts it last.
    pub after: Option<String>,
}

/// The user's changes to one menu, relative to its defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MenuOverride {
    /// Default entries taken out.
    pub removed: Vec<String>,
    /// Entries added (separators may repeat).
    pub added: Vec<MenuPlacement>,
    /// Entries moved, replayed in order.
    pub moved: Vec<MenuPlacement>,
}

impl MenuOverride {
    /// No changes.
    pub fn is_empty(&self) -> bool {
        self.removed.is_empty() && self.added.is_empty() && self.moved.is_empty()
    }
}

/// Context-menu settings: the overrides per menu kind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MenuSettings {
    /// The changes per kind; kinds without changes are absent.
    pub overrides: BTreeMap<MenuKind, MenuOverride>,
}

/// One entry of a menu as Settings lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuEntryInfo {
    /// The entry id (`file.duplicate`, `menu.copy_path`, `---`).
    pub entry: String,
    /// Its title (`Duplicate`, `Copy Path`, empty for a separator).
    pub title: String,
}

// ───────────────────────── defaults (the spec) ─────────────────────────

const OPEN_WITH: &str = "menu.open_with";
const COPY_PATH: &str = "menu.copy_path";
const TAGS: &str = "menu.tags";
const VIEW_AS: &str = "menu.view_as";
const SORT_BY: &str = "menu.sort_by";
const SHARE: &str = "system.share";
const QUICK_ACTIONS: &str = "system.quick_actions";
const SERVICES: &str = "system.services";

/// The groups (submenus) an entry can name.
pub const GROUPS: &[&str] = &[
    OPEN_WITH,
    COPY_PATH,
    TAGS,
    VIEW_AS,
    SORT_BY,
    SHARE,
    QUICK_ACTIONS,
    SERVICES,
];

const FILE_TAIL: &[&str] = &[
    "file.rename",
    "file.duplicate",
    "file.make_alias",
    "file.compress",
    SEPARATOR,
    "edit.copy",
    "edit.cut",
    "edit.paste",
    "file.copy_to_other_pane",
    "file.move_to_other_pane",
    SEPARATOR,
    COPY_PATH,
    SEPARATOR,
    TAGS,
    SEPARATOR,
    "file.reveal_in_finder",
    "terminal.open",
    "file.open_in_editor",
    SHARE,
    QUICK_ACTIONS,
    SERVICES,
    SEPARATOR,
    "file.trash",
    "file.delete_immediately",
];

/// Add to / Remove from Favourites: only one of the two shows, by whether
/// the folder is already a favourite.
const FAVOURITES: [&str; 2] = ["favourites.add", "favourites.remove"];

/// The default entries of a menu, exactly as `docs/CONTEXT-MENUS.md` lists them.
pub fn default_entries(kind: MenuKind) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = Vec::new();
    match kind {
        MenuKind::File => {
            v.extend([
                "nav.open",
                OPEN_WITH,
                "nav.open_new_tab",
                "nav.open_other_pane",
                SEPARATOR,
                "preview.quick_look",
                "preview.get_info",
                SEPARATOR,
            ]);
            v.extend(FILE_TAIL);
        }
        MenuKind::Folder | MenuKind::Package => {
            v.push("nav.open");
            if kind == MenuKind::Package {
                v.push("nav.show_package_contents");
            }
            v.extend([
                "nav.open_new_tab",
                "nav.open_other_pane",
                OPEN_WITH,
                SEPARATOR,
                "preview.quick_look",
                "preview.get_info",
                "file.folder_size",
                "nav.disk_usage",
                SEPARATOR,
            ]);
            v.extend(FILE_TAIL);
            let at = v
                .iter()
                .position(|e| *e == "file.open_in_editor")
                .map_or(v.len(), |i| i + 1);
            v.splice(at..at, FAVOURITES);
        }
        MenuKind::Multi => v.extend([
            "nav.open",
            "preview.quick_look",
            "preview.get_info",
            SEPARATOR,
            "file.batch_rename",
            "file.duplicate",
            "file.compress",
            "file.new_folder_with_selection",
            SEPARATOR,
            "edit.copy",
            "edit.cut",
            "edit.paste",
            "file.copy_to_other_pane",
            "file.move_to_other_pane",
            COPY_PATH,
            SEPARATOR,
            "favourites.add",
            "favourites.remove",
            SEPARATOR,
            TAGS,
            SHARE,
            SEPARATOR,
            "file.trash",
        ]),
        MenuKind::EmptySpace => v.extend([
            "file.new_folder",
            "file.new_file",
            "edit.paste",
            "file.move_here",
            SEPARATOR,
            VIEW_AS,
            SORT_BY,
            "view.show_hidden",
            SEPARATOR,
            "select.all",
            "select.invert",
            "select.by_pattern",
            SEPARATOR,
            "path.copy_folder_path",
            "terminal.open",
            "file.reveal_in_finder",
            "view.refresh",
        ]),
        MenuKind::Crumb => v.extend([
            "nav.open",
            "nav.open_new_tab",
            "nav.open_other_pane",
            SEPARATOR,
            COPY_PATH,
            SEPARATOR,
            "terminal.open",
            "file.reveal_in_finder",
            "favourites.add",
            "favourites.remove",
            "preview.get_info",
        ]),
        MenuKind::SidebarItem => v.extend([
            "nav.open",
            "nav.open_new_tab",
            "nav.open_other_pane",
            "file.reveal_in_finder",
            "preview.get_info",
            "path.copy_path",
            SEPARATOR,
            "sidebar.remove",
            "sidebar.rename",
            "drive.eject",
            "drive.info",
            "nav.disk_usage",
            "transfer.resume",
            "transfer.resume_all",
        ]),
        MenuKind::Tab => v.extend([
            "tab.new",
            "tab.duplicate",
            "tab.reopen_closed",
            "tab.move_to_other_pane",
            SEPARATOR,
            "tab.close",
            "tab.close_others",
            SEPARATOR,
            "path.copy_path",
            "favourites.add",
            "favourites.remove",
            "workspace.save",
        ]),
    }
    v
}

/// Drops leading, trailing and doubled separators.
pub fn tidy_separators<T: Clone>(items: Vec<T>, is_sep: impl Fn(&T) -> bool) -> Vec<T> {
    let mut out: Vec<T> = Vec::with_capacity(items.len());
    for it in items {
        if is_sep(&it) && out.last().is_none_or(&is_sep) {
            continue;
        }
        out.push(it);
    }
    while out.last().is_some_and(&is_sep) {
        out.pop();
    }
    out
}

fn place(list: &mut Vec<String>, p: &MenuPlacement) {
    let at = match &p.after {
        None => 0,
        Some(a) => list
            .iter()
            .position(|e| e == a)
            .map(|i| i + 1)
            .unwrap_or(list.len()),
    };
    list.insert(at, p.entry.clone());
}

/// Applies a menu's overrides to its defaults: remove, then replay moves,
/// then adds, then tidy separators. Pure, so Settings previews use it too.
pub fn apply_override(defaults: &[&str], ov: &MenuOverride) -> Vec<String> {
    let mut list: Vec<String> = defaults
        .iter()
        .filter(|e| **e == SEPARATOR || !ov.removed.iter().any(|r| r == *e))
        .map(|e| e.to_string())
        .collect();
    for m in &ov.moved {
        let Some(i) = list.iter().position(|e| *e == m.entry) else {
            continue;
        };
        list.remove(i);
        place(&mut list, m);
    }
    for a in &ov.added {
        if a.entry != SEPARATOR && list.contains(&a.entry) {
            continue;
        }
        place(&mut list, a);
    }
    tidy_separators(list, |e| e == SEPARATOR)
}

/// Is `entry` something a menu can show (a group, a separator or a
/// registered command)?
pub fn is_known_entry(entry: &str) -> bool {
    entry == SEPARATOR || GROUPS.contains(&entry) || registry::get(entry).is_some()
}

/// The title Settings shows for an entry.
pub fn entry_title(entry: &str) -> String {
    match entry {
        SEPARATOR => String::new(),
        OPEN_WITH => "Open With".into(),
        COPY_PATH => "Copy Path".into(),
        TAGS => "Tags".into(),
        VIEW_AS => "View as".into(),
        SORT_BY => "Sort by".into(),
        SHARE => "Share".into(),
        QUICK_ACTIONS => "Quick Actions".into(),
        SERVICES => "Services".into(),
        "file.move_here" => "Paste as Move".into(),
        "edit.cut" => "Cut".into(),
        "path.copy_path" => "Copy Path".into(),
        id => registry::get(id)
            .map(|c| c.title.to_string())
            .unwrap_or_else(|| id.to_string()),
    }
}

/// The title of an entry in one menu, with the item count where the spec
/// asks for it ("Move 3 Items to Trash").
fn title_in(kind: MenuKind, entry: &str, n: usize) -> String {
    if kind == MenuKind::Multi {
        match entry {
            "preview.quick_look" => return format!("Quick Look {n} Items"),
            "file.batch_rename" => return format!("Batch Rename {n} Items…"),
            "file.compress" => return format!("Compress {n} Items"),
            "file.trash" => return format!("Move {n} Items to Trash"),
            COPY_PATH => return "Copy Paths".into(),
            _ => {}
        }
    }
    entry_title(entry)
}

/// Finder's seven colour tags, in Finder's menu order.
const TAG_COLOURS: &[(&str, u8)] = &[
    ("Red", 6),
    ("Orange", 7),
    ("Yellow", 5),
    ("Green", 2),
    ("Blue", 4),
    ("Purple", 3),
    ("Gray", 1),
];

/// Most items whose tags are read to tick the Tags submenu.
const TAG_READ_LIMIT: usize = 64;

// ───────────────────────── building ─────────────────────────

/// Everything the rules need, gathered once per menu.
struct Ctx {
    kind: MenuKind,
    tab: Option<Arc<Tab>>,
    /// Where commands act: the menu's tab, or the focused one for the sidebar.
    act_tab: Option<TabId>,
    paths: Vec<PathBuf>,
    /// The items' paths travel with each invocation (crumb, sidebar, tab).
    explicit: bool,
    dual: bool,
    can_paste: bool,
    allow_delete: bool,
    view: Option<ViewState>,
    sidebar: Option<SidebarItemKind>,
    removable: bool,
    /// The target folder is a favourite (for several items: every folder
    /// among them is).
    favourite: bool,
    can_reopen: bool,
    tabs_in_pane: usize,
    any_folder: bool,
    /// Interrupted transfers to or from the sidebar disk (Resume Transfer).
    transfers: usize,
}

impl Engine {
    /// Applies Finder's right-click rule to a tab's list and says what was
    /// clicked: `row = None` is empty space (the selection is cleared); a row
    /// that is selected keeps the selection; any other row becomes the only
    /// selection. Then one item is a file, folder or package, and several
    /// are `Multi`.
    pub fn menu_target_at(&self, tab: TabId, row: Option<u32>) -> Result<MenuTarget> {
        let t = self.target(Some(tab))?;
        t.note_user_selection();
        let Some(row) = row else {
            t.set_selection(SelectionChange::None);
            return Ok(MenuTarget::EmptySpace { tab });
        };
        let l = t.listing();
        if row >= l.len() {
            t.set_selection(SelectionChange::None);
            return Ok(MenuTarget::EmptySpace { tab });
        }
        let sel = t.selection();
        if !sel.rows.contains(&row) {
            t.set_selection(SelectionChange::Only(row));
        }
        let names = t.selection().names;
        let dir = t.location_path();
        let paths: Vec<String> = names
            .iter()
            .map(|n| dir.join(n).to_string_lossy().into_owned())
            .collect();
        if paths.len() > 1 {
            return Ok(MenuTarget::Multi { tab, paths });
        }
        let Some(i) = l.index_of(&names[0]) else {
            return Ok(MenuTarget::EmptySpace { tab });
        };
        let e = l
            .entry(i)
            .ok_or_else(|| CoreError::NotFound(names[0].clone()))?;
        let path = paths[0].clone();
        Ok(if e.is_package {
            MenuTarget::Package { tab, path }
        } else if e.is_dir() {
            MenuTarget::Folder { tab, path }
        } else {
            MenuTarget::File { tab, path }
        })
    }

    /// The effective entries of a menu kind (defaults plus the user's changes).
    pub fn menu_entries(&self, kind: MenuKind) -> Vec<MenuEntryInfo> {
        let ov = self
            .settings()
            .menus
            .overrides
            .get(&kind)
            .cloned()
            .unwrap_or_default();
        apply_override(&default_entries(kind), &ov)
            .into_iter()
            .map(|e| MenuEntryInfo {
                title: entry_title(&e),
                entry: e,
            })
            .collect()
    }

    /// Entries that could be added to a menu kind and aren't in it: the
    /// groups, a separator, and every command that shows in menus or the
    /// palette.
    pub fn menu_choices(&self, kind: MenuKind) -> Vec<MenuEntryInfo> {
        let have: Vec<String> = self
            .menu_entries(kind)
            .into_iter()
            .map(|e| e.entry)
            .collect();
        let mut out = vec![MenuEntryInfo {
            entry: SEPARATOR.into(),
            title: "Separator".into(),
        }];
        let ids = GROUPS.iter().copied().chain(
            registry::all()
                .iter()
                .filter(|c| c.shows_in_menu || c.shows_in_palette)
                .map(|c| c.id.as_str()),
        );
        for id in ids {
            if !have.iter().any(|h| h == id) {
                out.push(MenuEntryInfo {
                    entry: id.to_string(),
                    title: entry_title(id),
                });
            }
        }
        out
    }

    fn edit_menu(&self, kind: MenuKind, f: impl FnOnce(&mut MenuOverride)) -> Result<()> {
        let mut s = self.settings();
        let mut ov = s.menus.overrides.remove(&kind).unwrap_or_default();
        f(&mut ov);
        if !ov.is_empty() {
            s.menus.overrides.insert(kind, ov);
        }
        self.update_settings(s)
    }

    /// Takes an entry out of a menu.
    pub fn menu_remove(&self, kind: MenuKind, entry: &str) -> Result<()> {
        let defaults = default_entries(kind);
        self.edit_menu(kind, |ov| {
            ov.moved.retain(|m| m.entry != entry);
            if let Some(i) = ov.added.iter().position(|a| a.entry == entry) {
                ov.added.remove(i);
            } else if defaults.contains(&entry) && !ov.removed.iter().any(|r| r == entry) {
                ov.removed.push(entry.to_string());
            }
        })
    }

    /// Adds an entry to a menu after `after` (first when `None`).
    pub fn menu_add(&self, kind: MenuKind, entry: &str, after: Option<String>) -> Result<()> {
        if !is_known_entry(entry) {
            return Err(CoreError::UnknownCommand(entry.to_string()));
        }
        let p = MenuPlacement {
            entry: entry.to_string(),
            after,
        };
        self.edit_menu(kind, |ov| {
            if let Some(i) = ov.removed.iter().position(|r| r == entry) {
                // Bring a removed default back, where it was asked for.
                ov.removed.remove(i);
                ov.moved.retain(|m| m.entry != entry);
                ov.moved.push(p);
            } else {
                ov.added.push(p);
            }
        })
    }

    /// Moves an entry of a menu to just after `after` (first when `None`).
    pub fn menu_move(&self, kind: MenuKind, entry: &str, after: Option<String>) -> Result<()> {
        let p = MenuPlacement {
            entry: entry.to_string(),
            after,
        };
        self.edit_menu(kind, |ov| {
            if let Some(a) = ov.added.iter_mut().find(|a| a.entry == entry) {
                a.after = p.after;
            } else {
                ov.moved.retain(|m| m.entry != entry);
                ov.moved.push(p);
            }
        })
    }

    /// Puts one menu back to its defaults.
    pub fn reset_menu(&self, kind: MenuKind) -> Result<()> {
        self.edit_menu(kind, |ov| *ov = MenuOverride::default())
    }

    /// Puts every menu back to its defaults.
    pub fn reset_menus(&self) -> Result<()> {
        let mut s = self.settings();
        s.menus = MenuSettings::default();
        self.update_settings(s)
    }

    /// Builds the context menu for a target. For list rows the target's
    /// items become the tab's selection, so the commands act on exactly what
    /// the menu was opened for. Reading tags for the Tags submenu is the only
    /// disk access (an `xattr` read, which never downloads placeholders).
    pub fn context_menu(&self, target: MenuTarget, mctx: MenuContext) -> Result<Vec<MenuItem>> {
        let kind = target.kind();
        let tab = match target.tab() {
            Some(id) => Some(self.target(Some(id))?),
            None => self.active_tab(),
        };
        let paths = target.paths();
        if matches!(
            kind,
            MenuKind::File | MenuKind::Folder | MenuKind::Package | MenuKind::Multi
        ) && let Some(t) = &tab
        {
            sync_selection(t, &paths);
        }
        let settings = self.settings();
        let tabs_in_pane = tab
            .as_ref()
            .and_then(|t| {
                let m = self.model.lock();
                m.find(t.id())
                    .map(|pl| m.windows[pl.window].panes[pl.pane].tabs.len())
            })
            .unwrap_or(0);
        let (removable, favourite) = match &target {
            MenuTarget::SidebarItem { path, .. } | MenuTarget::Crumb { path, .. } => (
                self.volumes()
                    .iter()
                    .any(|v| v.mount_path == *path && v.is_removable),
                self.is_favourite(path),
            ),
            MenuTarget::Folder { path, .. } | MenuTarget::Package { path, .. } => {
                (false, self.is_favourite(path))
            }
            MenuTarget::Multi { paths, .. } => (
                false,
                paths
                    .iter()
                    .filter(|p| Path::new(p).is_dir())
                    .all(|p| self.is_favourite(p)),
            ),
            MenuTarget::Tab { .. } => (
                false,
                tab.as_ref()
                    .is_some_and(|t| self.is_favourite(&t.location_path().to_string_lossy())),
            ),
            _ => (false, false),
        };
        let any_folder = match kind {
            MenuKind::Folder
            | MenuKind::Package
            | MenuKind::Crumb
            | MenuKind::SidebarItem
            | MenuKind::Tab => true,
            MenuKind::Multi => paths.iter().any(|p| p.is_dir()),
            _ => false,
        };
        // Only a sidebar disk asks Safe Transfer (it lists volumes).
        let transfers = match &target {
            MenuTarget::SidebarItem {
                path,
                kind: SidebarItemKind::Volume,
            } => self
                .ops()
                .interrupted_on_volume(Path::new(path))
                .map(|v| v.len())
                .unwrap_or(0),
            _ => 0,
        };
        let ctx = Ctx {
            transfers,
            kind,
            act_tab: tab.as_ref().map(|t| t.id()),
            dual: tab
                .as_ref()
                .is_some_and(|t| self.other_pane_tab(t.id()).is_some()),
            view: tab.as_ref().map(|t| t.view()),
            tab,
            explicit: matches!(kind, MenuKind::Crumb | MenuKind::SidebarItem),
            paths,
            can_paste: self.shared.clipboard.lock().is_some() || mctx.pasteboard_has_files,
            allow_delete: settings.operations.allow_delete_immediately,
            sidebar: match &target {
                MenuTarget::SidebarItem { kind, .. } => Some(*kind),
                _ => None,
            },
            removable,
            favourite,
            can_reopen: self.can_reopen_tab(),
            tabs_in_pane,
            any_folder,
        };
        let entries = self.menu_entries(kind);
        let items = entries
            .iter()
            .filter_map(|e| self.build_entry(&e.entry, &ctx))
            .collect();
        Ok(tidy_separators(items, |i| matches!(i, MenuItem::Separator)))
    }

    fn command_item(
        &self,
        ctx: &Ctx,
        id: &str,
        title: String,
        arg: Option<CommandArg>,
    ) -> Option<MenuItem> {
        let (enabled, checked) = self.rule(id, ctx)?;
        let arg = arg.or_else(|| self.default_arg(id, ctx));
        Some(MenuItem::Command(MenuCommand {
            entry: id.to_string(),
            title,
            shortcut: self.shortcut_text(id),
            enabled,
            checked,
            tag_color: None,
            invocation: CommandInvocation {
                id: id.to_string(),
                arg,
                tab: ctx.act_tab,
            },
        }))
    }

    fn shortcut_text(&self, id: &str) -> Option<String> {
        self.keymap
            .lock()
            .shortcut_text(&fstudio_keymap::CommandId::new(id))
    }

    /// The argument an item carries in this menu, so it acts on the menu's
    /// target rather than whatever is selected.
    fn default_arg(&self, id: &str, ctx: &Ctx) -> Option<CommandArg> {
        let first = ctx.paths.first().cloned();
        match ctx.kind {
            MenuKind::Crumb | MenuKind::SidebarItem => first.map(CommandArg::Path),
            MenuKind::Folder | MenuKind::Package if id == "terminal.open" => {
                first.map(CommandArg::Path)
            }
            MenuKind::Folder | MenuKind::Package if id.starts_with("favourites.") => {
                first.map(CommandArg::Path)
            }
            MenuKind::Folder | MenuKind::Package
                if id == "nav.disk_usage" || id == "file.folder_size" =>
            {
                first.map(CommandArg::Path)
            }
            MenuKind::Tab if id.starts_with("path.") || id.starts_with("favourites.") => ctx
                .tab
                .as_ref()
                .map(|t| CommandArg::Path(t.location_path())),
            _ => None,
        }
    }

    /// `None` hides the item; otherwise `(enabled, checked)`.
    fn rule(&self, id: &str, ctx: &Ctx) -> Option<(bool, bool)> {
        let info = registry::get(id)?;
        if LATER_COMMANDS.contains(&id) {
            return None;
        }
        let view = ctx.view.clone().unwrap_or_default();
        let shown = |b: bool| b.then_some((true, false));
        let check = |c: bool| Some((true, c));
        match info.id.as_str() {
            "nav.open_other_pane" | "tab.move_to_other_pane" => {
                shown(ctx.dual && (ctx.kind == MenuKind::Tab || ctx.any_folder))
            }
            "nav.open_new_tab" => shown(ctx.any_folder),
            "file.copy_to_other_pane" | "file.move_to_other_pane" => shown(ctx.dual),
            "file.delete_immediately" => shown(ctx.allow_delete),
            "edit.paste" | "file.move_here" => Some((ctx.can_paste, false)),
            "sidebar.remove" | "sidebar.rename" => {
                shown(ctx.sidebar == Some(SidebarItemKind::Favourite))
            }
            "drive.eject" => shown(ctx.sidebar == Some(SidebarItemKind::Volume) && ctx.removable),
            "drive.info" => shown(ctx.sidebar == Some(SidebarItemKind::Volume)),
            // Disk usage (M3): folders, and volumes in the sidebar.
            "nav.disk_usage" => shown(
                ctx.kind != MenuKind::SidebarItem || ctx.sidebar == Some(SidebarItemKind::Volume),
            ),
            "transfer.resume" => {
                shown(ctx.sidebar == Some(SidebarItemKind::Volume) && ctx.transfers > 0)
            }
            "transfer.resume_all" => {
                shown(ctx.sidebar == Some(SidebarItemKind::Volume) && ctx.transfers > 1)
            }
            "addressbar.add_favourite" => shown(!ctx.favourite),
            "favourites.add" => shown(ctx.any_folder && !ctx.favourite),
            "favourites.remove" => shown(ctx.any_folder && ctx.favourite),
            "tab.reopen_closed" => shown(ctx.can_reopen),
            "tab.close_others" => shown(ctx.tabs_in_pane > 1),
            "view.show_hidden" => check(view.show_hidden),
            "view.icons" => check(view.mode == ViewMode::Icons),
            "view.list" => check(view.mode == ViewMode::List),
            "view.columns" => check(view.mode == ViewMode::Columns),
            "view.gallery" => check(view.mode == ViewMode::Gallery),
            "sort.name" => check(view.sort == SortBy::Name),
            "sort.kind" => check(view.sort == SortBy::Kind),
            "sort.size" => check(view.sort == SortBy::Size),
            "sort.date_modified" => check(view.sort == SortBy::Modified),
            "sort.date_created" => check(view.sort == SortBy::Created),
            "sort.reverse" => check(!view.ascending),
            "sort.folders_first" => check(view.folders_first),
            _ => Some((true, false)),
        }
    }

    fn build_entry(&self, entry: &str, ctx: &Ctx) -> Option<MenuItem> {
        let kind = ctx.kind;
        let n = ctx.paths.len();
        let strings = || -> Vec<String> {
            ctx.paths
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect()
        };
        let sub = |items: Vec<MenuItem>| -> Option<MenuItem> {
            let items = tidy_separators(items, |i| matches!(i, MenuItem::Separator));
            (!items.is_empty()).then(|| MenuItem::Submenu {
                entry: entry.to_string(),
                title: title_in(kind, entry, n),
                items,
            })
        };
        let sys = |k: SystemMenu| {
            Some(MenuItem::System {
                kind: k,
                title: title_in(kind, entry, n),
                paths: strings(),
            })
        };
        match entry {
            SEPARATOR => Some(MenuItem::Separator),
            OPEN_WITH => sys(SystemMenu::OpenWith),
            SHARE => sys(SystemMenu::Share),
            QUICK_ACTIONS => sys(SystemMenu::QuickActions),
            SERVICES => sys(SystemMenu::Services),
            COPY_PATH => {
                let mut styles = vec![
                    ("path.copy_path", "Path"),
                    ("path.copy_name", "Name"),
                    ("path.copy_escaped", "Escaped"),
                    ("path.copy_home_relative", "Relative to ~"),
                    ("path.copy_quoted", "Quoted"),
                    ("path.copy_file_url", "File URL"),
                ];
                if kind == MenuKind::Crumb {
                    styles.retain(|(id, _)| *id != "path.copy_name");
                }
                sub(styles
                    .into_iter()
                    .filter_map(|(id, t)| self.command_item(ctx, id, t.into(), None))
                    .collect())
            }
            VIEW_AS => sub([
                ("view.icons", "Icons"),
                ("view.list", "List"),
                ("view.columns", "Columns"),
                ("view.gallery", "Gallery"),
            ]
            .into_iter()
            .filter_map(|(id, t)| self.command_item(ctx, id, t.into(), None))
            .collect()),
            SORT_BY => {
                let mut items: Vec<MenuItem> = [
                    ("sort.name", "Name"),
                    ("sort.kind", "Kind"),
                    ("sort.size", "Size"),
                    ("sort.date_modified", "Date Modified"),
                    ("sort.date_created", "Date Created"),
                ]
                .into_iter()
                .filter_map(|(id, t)| self.command_item(ctx, id, t.into(), None))
                .collect();
                items.push(MenuItem::Separator);
                for (id, t) in [
                    ("sort.reverse", "Reverse"),
                    ("sort.folders_first", "Folders First"),
                ] {
                    items.extend(self.command_item(ctx, id, t.into(), None));
                }
                sub(items)
            }
            TAGS => {
                let on = tags_on(&ctx.paths);
                let mut items: Vec<MenuItem> = TAG_COLOURS
                    .iter()
                    .filter_map(|(name, color)| {
                        let mut it = self.command_item(
                            ctx,
                            "tags.toggle",
                            (*name).into(),
                            Some(CommandArg::Text((*name).into())),
                        )?;
                        if let MenuItem::Command(c) = &mut it {
                            c.tag_color = Some(*color);
                            c.checked = on.iter().any(|t| t == name);
                        }
                        Some(it)
                    })
                    .collect();
                items.push(MenuItem::Separator);
                items.extend(self.command_item(ctx, "tags.edit", "Tags…".into(), None));
                sub(items)
            }
            id => {
                if ctx.explicit && !explicit_ok(id) {
                    return None;
                }
                self.command_item(ctx, id, title_in(kind, id, n), None)
            }
        }
    }
}

/// Commands that understand a path argument, so they can appear in menus
/// whose target is not the selection (address bar, sidebar).
fn explicit_ok(id: &str) -> bool {
    matches!(
        id,
        "nav.open"
            | "nav.open_new_tab"
            | "nav.open_other_pane"
            | "file.reveal_in_finder"
            | "preview.get_info"
            | "preview.quick_look"
            | "terminal.open"
            | "addressbar.add_favourite"
            | "favourites.add"
            | "favourites.remove"
            | "sidebar.remove"
            | "sidebar.rename"
            | "drive.eject"
            | "drive.info"
    ) || id.starts_with("path.copy_")
        // Disk usage (M3) takes the menu's path.
        || id == "nav.disk_usage"
        || id == "file.folder_size"
}

/// Selects exactly these items in the tab, if they aren't already.
fn sync_selection(t: &Tab, paths: &[PathBuf]) {
    let dir = t.location_path();
    let names: Vec<String> = paths
        .iter()
        .filter(|p| p.parent() == Some(dir.as_path()))
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    if names.is_empty() {
        return;
    }
    let mut have = t.selection().names;
    let mut want = names.clone();
    have.sort();
    want.sort();
    if have != want {
        t.set_selection(SelectionChange::Names(names));
    }
}

/// Tag names set on *every* item (so the menu ticks them).
fn tags_on(paths: &[PathBuf]) -> Vec<String> {
    if paths.is_empty() || paths.len() > TAG_READ_LIMIT {
        return Vec::new();
    }
    let mut common: Option<Vec<String>> = None;
    for p in paths {
        let names: Vec<String> = fstudio_fs::finder_tags(p)
            .unwrap_or_default()
            .into_iter()
            .map(|t| t.name)
            .collect();
        common = Some(match common {
            None => names,
            Some(c) => c.into_iter().filter(|n| names.contains(n)).collect(),
        });
    }
    common.unwrap_or_default()
}

/// Toggles a Finder tag on items: if every item has it, it is removed from
/// all; otherwise it is added where missing. Known colour names keep their
/// colour. Returns how many items changed.
pub(crate) fn toggle_tag(paths: &[PathBuf], name: &str) -> Result<usize> {
    let color = TAG_COLOURS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, c)| fstudio_fs::TagColor::from_index(*c))
        .unwrap_or_default();
    let current: Vec<Vec<fstudio_fs::FinderTag>> = paths
        .iter()
        .map(|p| fstudio_fs::finder_tags(p).unwrap_or_default())
        .collect();
    let all_have = current.iter().all(|t| t.iter().any(|x| x.name == name));
    let mut changed = 0;
    for (p, mut tags) in paths.iter().zip(current) {
        let has = tags.iter().any(|x| x.name == name);
        if all_have {
            tags.retain(|x| x.name != name);
        } else if !has {
            tags.push(fstudio_fs::FinderTag::new(name, color));
        } else {
            continue;
        }
        fstudio_fs::set_finder_tags(p, &tags)?;
        changed += 1;
    }
    Ok(changed)
}

/// The folder a path argument means for "open in Terminal": itself when it
/// is a folder, else its parent.
pub(crate) fn folder_of(p: &Path) -> PathBuf {
    if p.is_dir() {
        p.to_path_buf()
    } else {
        p.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| p.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn tidy_drops_doubled_and_edge_separators() {
        let v = tidy_separators(s(&["---", "a", "---", "---", "b", "---"]), |e| e == "---");
        assert_eq!(v, s(&["a", "---", "b"]));
    }

    #[test]
    fn overrides_remove_move_add() {
        let d = ["a", "b", SEPARATOR, "c"];
        let ov = MenuOverride {
            removed: s(&["b"]),
            moved: vec![MenuPlacement {
                entry: "c".into(),
                after: None,
            }],
            added: vec![
                MenuPlacement {
                    entry: "x".into(),
                    after: Some("a".into()),
                },
                MenuPlacement {
                    entry: "y".into(),
                    after: Some("gone".into()),
                },
            ],
        };
        assert_eq!(apply_override(&d, &ov), s(&["c", "a", "x", SEPARATOR, "y"]));
    }

    #[test]
    fn every_default_entry_is_known() {
        for k in MenuKind::ALL {
            for e in default_entries(k) {
                assert!(is_known_entry(e), "{k:?}: {e}");
            }
        }
    }
}

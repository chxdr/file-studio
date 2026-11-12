//! Commands: the single entry point for keys, menus, the palette, the toolbar
//! and the CLI (F-006). The shell turns input into a [`KeyChord`] or a
//! [`CommandInvocation`]; the core decides what it means and either does it
//! or returns a [`ShellRequest`] for work only the shell can do.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fstudio_keymap::{
    CommandArg, CommandId, Context, Focus, HeldShortcuts, Mods, Resolution, ViewMode, VimMode,
    registry,
};
use fstudio_model::{KeyChord, PathStyle, Via};
use fstudio_ops::{OpId, OpRequest};
use serde::{Deserialize, Serialize};

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::events::{CoreEvent, HintInfo};
use crate::ids::{TabId, WindowId};
use crate::outcome::{Outcome, ShellRequest};
use crate::shared::Clipboard;
use crate::tab::{CursorMove, SelectionChange, Tab};
use crate::view::SortBy;

/// A command to run, with its argument and the tab it is for (the focused
/// tab when `None`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandInvocation {
    /// The command id (`file.new_folder`).
    pub id: String,
    /// Its argument, if it takes one.
    pub arg: Option<CommandArg>,
    /// The tab it acts on.
    pub tab: Option<TabId>,
}

impl CommandInvocation {
    /// A command with no argument, for the focused tab.
    pub fn new(id: &str) -> Self {
        Self {
            id: id.into(),
            arg: None,
            tab: None,
        }
    }
    /// The same command with an argument.
    pub fn with_arg(mut self, arg: CommandArg) -> Self {
        self.arg = Some(arg);
        self
    }
    /// The same command for a given tab.
    pub fn for_tab(mut self, tab: TabId) -> Self {
        self.tab = Some(tab);
        self
    }
}

/// Where a key press happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct InputContext {
    /// Which part of the UI has focus.
    pub focus: Focus,
    /// The tab whose pane has focus (the focused tab when `None`).
    pub tab: Option<TabId>,
}

/// A registered command, for menus, the palette and Settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSummary {
    /// The id.
    pub id: String,
    /// Menu and palette title.
    pub title: String,
    /// The group heading.
    pub category: String,
    /// One plain sentence.
    pub description: String,
    /// The primary shortcut as key-cap text.
    pub shortcut: Option<String>,
    /// Shown in menus.
    pub shows_in_menu: bool,
    /// Shown in the palette.
    pub shows_in_palette: bool,
    /// The user changed its keys.
    pub customized: bool,
}

/// One binding in force, for `fstudio keys`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingSummary {
    /// The command.
    pub command: String,
    /// Key-cap text of the keys.
    pub keys: String,
    /// Where it applies (debug text of the context rule).
    pub when: String,
    /// Default, Vim layer or user.
    pub tier: String,
}

/// How the core handles a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handling {
    /// The core does it.
    Core,
    /// The core decides, the shell carries it out (a [`ShellRequest`]).
    Shell,
    /// Planned for a later milestone; returns `NotAvailable` for now.
    Later,
}

/// Commands that come in a later milestone (M3+).
pub const LATER_COMMANDS: &[&str] = &[
    "file.compress",
    "file.folder_size",
    "file.batch_rename",
    "file.make_alias",
    "search.global",
    "nav.connect_server",
];

const UNTITLED_FOLDER: &str = "untitled folder";
const UNTITLED_FILE: &str = "untitled";
const NEW_FOLDER_WITH_ITEMS: &str = "New Folder With Items";

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn strings(v: &[PathBuf]) -> Vec<String> {
    v.iter().map(|p| s(p)).collect()
}

fn hint_info(h: fstudio_keymap::Hint) -> HintInfo {
    HintInfo {
        command: h.command.as_str().to_string(),
        keys: h.keys,
        label: h.label,
    }
}

impl Engine {
    // ───────────────────────── keymap ─────────────────────────

    fn key_context(&self, ctx: &InputContext) -> Context {
        let tab = ctx.tab.or_else(|| self.active_tab().map(|t| t.id()));
        let dual = tab.is_some_and(|t| self.other_pane_tab(t).is_some());
        Context {
            focus: ctx.focus,
            dual_pane: dual,
            vim: *self.vim.lock(),
        }
    }

    /// Decides what a key press means and does it. `None` means "not ours":
    /// let the focused control handle the key (typing in a text field).
    pub fn handle_key(&self, chord: KeyChord, ctx: InputContext) -> Option<Outcome> {
        if let Some(o) = self.column_key(&chord, &ctx) {
            return Some(o);
        }
        let kctx = self.key_context(&ctx);
        let res = self.keymap.lock().resolve(chord, &kctx, Instant::now());
        self.run_resolution(res, ctx)
    }

    /// Call from a ~100 ms timer while a key sequence is pending: runs a
    /// command whose keys are also the start of a longer one once the
    /// sequence times out.
    pub fn poll_keys(&self, ctx: InputContext) -> Option<Outcome> {
        let res = self.keymap.lock().poll(Instant::now())?;
        self.run_resolution(res, ctx)
    }

    /// The unfinished key sequence, if any (`g`).
    pub fn pending_keys(&self) -> Option<String> {
        self.keymap.lock().pending_text()
    }

    fn run_resolution(&self, res: Resolution, ctx: InputContext) -> Option<Outcome> {
        match res {
            Resolution::Command(id, arg) => {
                let inv = CommandInvocation {
                    id: id.as_str().to_string(),
                    arg,
                    tab: ctx.tab,
                };
                Some(
                    self.perform(inv, Via::Keyboard)
                        .unwrap_or_else(|e| Outcome::message(e.to_string())),
                )
            }
            Resolution::Pending(text) => Some(Outcome {
                pending_keys: Some(text),
                ..Outcome::none()
            }),
            Resolution::TypeSelect(c) => {
                let tab = self.target(ctx.tab).ok()?;
                tab.type_char(c);
                Some(Outcome::none().with_tab(tab.id()))
            }
            Resolution::Filter(c) => {
                let tab = self.target(ctx.tab).ok()?;
                let f = format!("{}{c}", tab.filter());
                tab.set_filter(&f);
                Some(Outcome::shell(ShellRequest::FocusFilter).with_tab(tab.id()))
            }
            Resolution::None => None,
        }
    }

    /// Every registered command with its current shortcut.
    pub fn commands(&self) -> Vec<CommandSummary> {
        let k = self.keymap.lock();
        registry::all()
            .iter()
            .map(|c| CommandSummary {
                id: c.id.as_str().to_string(),
                title: c.title.to_string(),
                category: c.category.title().to_string(),
                description: c.description.to_string(),
                shortcut: k.shortcut_text(&c.id),
                shows_in_menu: c.shows_in_menu,
                shows_in_palette: c.shows_in_palette,
                customized: k.is_customized(&c.id),
            })
            .collect()
    }

    /// The overlay rows for held modifiers, or `None` when off or empty.
    pub fn held_shortcuts(&self, mods: Mods, ctx: InputContext) -> Option<HeldShortcuts> {
        let kctx = self.key_context(&ctx);
        self.keymap.lock().held_shortcuts(mods, &kctx)
    }

    /// Records that a command ran (keyboard uses count toward "learned") and
    /// returns a hint if a mouse use should show one. `perform` calls this
    /// itself; shells call it only for actions they run without `perform`.
    pub fn hint_for(&self, command: &str, via: Via) -> Option<HintInfo> {
        self.keymap
            .lock()
            .hint_for(&CommandId::new(command), via, Instant::now())
            .map(hint_info)
    }

    /// Has this shortcut been used from the keyboard enough times?
    pub fn is_learned(&self, command: &str) -> bool {
        self.keymap.lock().is_learned(&CommandId::new(command))
    }

    /// Every binding in force.
    pub fn bindings(&self) -> Vec<BindingSummary> {
        self.keymap
            .lock()
            .entries()
            .iter()
            .map(|e| BindingSummary {
                command: e.command.as_str().to_string(),
                keys: e.binding.text(),
                when: format!("{:?}", e.binding.when),
                tier: format!("{:?}", e.tier),
            })
            .collect()
    }

    /// Problems with the effective keymap, as plain sentences.
    pub fn keymap_conflicts(&self) -> Vec<String> {
        self.keymap
            .lock()
            .conflicts()
            .into_iter()
            .map(|c| {
                let ids: Vec<&str> = c.commands.iter().map(|i| i.as_str()).collect();
                format!("{:?}: {} → {}", c.kind, c.keys, ids.join(", "))
            })
            .collect()
    }

    /// The user's keymap changes as a JSON file.
    pub fn export_keymap(&self) -> String {
        self.settings().keymap.export_json()
    }

    /// Replaces the keymap changes from an exported file. Returns warnings
    /// (unknown commands, bindings dropped).
    pub fn import_keymap(&self, json: &str) -> Result<Vec<String>> {
        let imported = fstudio_keymap::KeymapSettings::import_json(json)
            .map_err(|e| CoreError::InvalidSetting(e.to_string()))?;
        let mut s = self.settings();
        s.keymap.profile = imported.profile;
        s.keymap.overrides = imported.overrides;
        self.update_settings(s)?;
        Ok(imported.warnings)
    }

    /// The key-cap text of each binding a command has now, primary first.
    pub fn command_keys(&self, command: &str) -> Vec<String> {
        self.keymap
            .lock()
            .bindings_for(&CommandId::new(command))
            .iter()
            .map(|b| b.text())
            .collect()
    }

    /// Sets (or with `None` clears) a command's primary or secondary key in
    /// the active profile. `keys` is key-cap text (`⇧⌘N`, `g g`, `F5`); the
    /// binding applies where the command does. Refuses macOS shortcuts.
    pub fn set_command_key(&self, command: &str, primary: bool, keys: Option<&str>) -> Result<()> {
        let info =
            registry::get(command).ok_or_else(|| CoreError::UnknownCommand(command.to_string()))?;
        let binding = match keys {
            Some(k) => Some(
                fstudio_keymap::Binding::parse(k, info.contexts[0].clone())
                    .ok_or_else(|| CoreError::InvalidSetting(format!("can't read keys “{k}”")))?,
            ),
            None => None,
        };
        let slot = if primary {
            fstudio_keymap::Slot::Primary
        } else {
            fstudio_keymap::Slot::Secondary
        };
        let mut s = self.settings();
        s.keymap
            .set_slot(&info.id, slot, binding)
            .map_err(|e| CoreError::InvalidSetting(e.to_string()))?;
        self.update_settings(s)
    }

    /// Puts one command's keys back to the profile's defaults (all commands
    /// when `command` is `None`).
    pub fn reset_command_keys(&self, command: Option<&str>) -> Result<()> {
        let mut s = self.settings();
        match command {
            Some(c) => s.keymap.reset_command(&CommandId::new(c)),
            None => s.keymap.reset_all(),
        }
        self.update_settings(s)
    }

    /// Vim's modal state (`Off` with the Standard profile).
    pub fn vim_mode(&self) -> VimMode {
        *self.vim.lock()
    }

    /// How the core handles a command id.
    pub fn handling(id: &str) -> Handling {
        if LATER_COMMANDS.contains(&id) {
            return Handling::Later;
        }
        match id {
            "nav.quick_open"
            | "nav.address_bar"
            | "nav.other_address_bar"
            | "file.rename"
            | "file.rename_dialog"
            | "file.open_with"
            | "file.open_in_editor"
            | "file.reveal_in_finder"
            | "select.by_pattern"
            | "select.deselect_pattern"
            | "view.toggle_sidebar"
            | "view.toggle_inspector"
            | "window.new"
            | "window.close"
            | "workspace.save"
            | "workspace.manage"
            | "preview.quick_look"
            | "preview.get_info"
            | "path.copy_path"
            | "path.copy_name"
            | "path.copy_folder_path"
            | "path.copy_escaped"
            | "path.copy_home_relative"
            | "path.copy_file_url"
            | "path.copy_quoted"
            | "terminal.open"
            | "palette.open"
            | "filter.focus"
            | "filter.commit"
            | "app.settings"
            | "app.keyboard_shortcuts"
            | "app.power_tour"
            | "edit.copy"
            | "tags.edit"
            | "sidebar.rename"
            | "drive.eject"
            | "drive.info" => Handling::Shell,
            _ if id.starts_with("addressbar.")
                || id.starts_with("palette.")
                || id.starts_with("rename.") =>
            {
                Handling::Shell
            }
            _ => Handling::Core,
        }
    }

    // ───────────────────────── helpers ─────────────────────────

    pub(crate) fn target(&self, tab: Option<TabId>) -> Result<Arc<Tab>> {
        match tab {
            Some(id) => self
                .tab(id)
                .ok_or_else(|| CoreError::NotFound(format!("tab {id}"))),
            None => self
                .active_tab()
                .ok_or_else(|| CoreError::NotAvailable("no window is open".into())),
        }
    }

    fn other(&self, t: &Tab) -> Result<Arc<Tab>> {
        self.other_pane_tab(t.id())
            .ok_or_else(|| CoreError::NotAvailable("dual pane is off".into()))
    }

    fn window_of(&self, t: TabId) -> Result<WindowId> {
        let m = self.model.lock();
        let pl = m
            .find(t)
            .ok_or_else(|| CoreError::NotFound(format!("tab {t}")))?;
        Ok(m.windows[pl.window].id)
    }

    fn pane_of(&self, t: TabId) -> Result<crate::ids::PaneId> {
        let m = self.model.lock();
        let pl = m
            .find(t)
            .ok_or_else(|| CoreError::NotFound(format!("tab {t}")))?;
        Ok(m.windows[pl.window].panes[pl.pane].id)
    }

    /// The items a command acts on: its path argument when it has one (a
    /// context menu on an address-bar segment or a sidebar row), else the
    /// selection.
    fn arg_or_selection(inv: &CommandInvocation, t: &Tab) -> Vec<PathBuf> {
        match &inv.arg {
            Some(CommandArg::Path(p)) => vec![p.clone()],
            _ => t.selected_pathbufs(),
        }
    }

    fn need_targets(inv: &CommandInvocation, t: &Tab) -> Result<Vec<PathBuf>> {
        let v = Self::arg_or_selection(inv, t);
        if v.is_empty() {
            return Err(CoreError::NotAvailable("nothing is selected".into()));
        }
        Ok(v)
    }

    /// `favourites.add` (⌃⌘T) and `favourites.remove`: the path argument
    /// (a context menu's folder, crumb or tab), else the selected folders,
    /// else the tab's own folder. Only folders are added; files in the
    /// selection are skipped.
    fn favourites_command(&self, id: &str, inv: &CommandInvocation) -> Result<Outcome> {
        let paths: Vec<PathBuf> = match &inv.arg {
            Some(CommandArg::Path(p)) => vec![p.clone()],
            _ => {
                let t = self.target(inv.tab)?;
                let sel = t.selected_pathbufs();
                if sel.is_empty() {
                    vec![t.location_path()]
                } else {
                    sel
                }
            }
        };
        let name_of = |p: &Path| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| s(p))
        };
        if id == "favourites.add" {
            let folders: Vec<&PathBuf> = paths.iter().filter(|p| p.is_dir()).collect();
            if folders.is_empty() {
                return Ok(Outcome::message("Only folders can be favourites"));
            }
            let mut added = Vec::new();
            for p in &folders {
                if self.add_favourite_reporting(&s(p), None)? {
                    added.push(name_of(p));
                }
            }
            return Ok(Outcome::message(match (added.len(), folders.len()) {
                (0, 1) => format!("“{}” is already in Favourites", name_of(folders[0])),
                (0, _) => "They’re already in Favourites".to_string(),
                (1, _) => format!("Added “{}” to Favourites", added[0]),
                (n, _) => format!("Added {n} folders to Favourites"),
            }));
        }
        let mut removed = Vec::new();
        for p in &paths {
            if let Some(f) = self.favourite_for(&s(p)) {
                self.remove_favourite(&f.path)?;
                removed.push(f.name);
            }
        }
        Ok(Outcome::message(match removed.len() {
            0 if paths.len() == 1 => format!("“{}” isn’t in Favourites", name_of(&paths[0])),
            0 => "None of them are in Favourites".to_string(),
            1 => format!("Removed “{}” from Favourites", removed[0]),
            n => format!("Removed {n} folders from Favourites"),
        }))
    }

    fn sidebar_command(&self, id: &str, arg: Option<&CommandArg>) -> Result<Outcome> {
        let Some(CommandArg::Path(p)) = arg else {
            return Err(CoreError::NotAvailable("no sidebar item given".into()));
        };
        let path = s(p);
        Ok(match id {
            "sidebar.remove" => match self.favourite_for(&path) {
                Some(f) => {
                    self.remove_favourite(&f.path)?;
                    Outcome::message(format!("Removed “{}” from Favourites", f.name))
                }
                None => Outcome::message("That folder isn’t in Favourites"),
            },
            "sidebar.rename" => {
                let (path, name) = match self.favourite_for(&path) {
                    Some(f) => (f.path, f.name),
                    None => {
                        let name = p
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        (path, name)
                    }
                };
                Outcome::shell(ShellRequest::AskFavouriteName { path, name })
            }
            _ => {
                let mount = fstudio_fs::volume_for(p)
                    .map(|v| s(&v.mount_path))
                    .unwrap_or(path);
                Outcome::shell(if id == "drive.eject" {
                    ShellRequest::Eject { path: mount }
                } else {
                    ShellRequest::ShowDriveInfo { path: mount }
                })
            }
        })
    }

    fn need_selection(t: &Tab) -> Result<Vec<PathBuf>> {
        let v = t.selected_pathbufs();
        if v.is_empty() {
            return Err(CoreError::NotAvailable("nothing is selected".into()));
        }
        Ok(v)
    }

    /// The first of `base`, `base 2`, `base 3`… free in the tab's folder,
    /// checked against the listing and with an `lstat` (the listing may lag
    /// behind an op that just finished).
    fn free_name(t: &Tab, base: &str) -> String {
        let dir = t.location_path();
        let free = |n: &str| !t.name_taken(n) && std::fs::symlink_metadata(dir.join(n)).is_err();
        if free(base) {
            return base.to_string();
        }
        fstudio_ops::names::keep_both_names(base, true)
            .take(10_000)
            .find(|n| free(n))
            .unwrap_or_else(|| base.to_string())
    }

    fn go_dir(&self, t: &Tab, dir: PathBuf) -> Outcome {
        t.navigate(dir, None);
        Outcome::none().with_tab(t.id())
    }

    /// Puts paths on the core's clipboard (Copy, or Cut pending a move).
    /// Shells call this for files pasted in from other apps.
    pub fn set_clipboard(&self, paths: Vec<String>, cut: bool) {
        let items: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        let dirs: Vec<PathBuf> = items
            .iter()
            .filter_map(|p| p.parent().map(Path::to_path_buf))
            .collect();
        *self.shared.clipboard.lock() = (!items.is_empty()).then_some(Clipboard { items, cut });
        self.redraw_dirs(&dirs);
    }

    /// What is on the core's clipboard: `(paths, cut)`.
    pub fn clipboard(&self) -> Option<(Vec<String>, bool)> {
        self.shared
            .clipboard
            .lock()
            .as_ref()
            .map(|c| (strings(&c.items), c.cut))
    }

    /// Tabs showing these folders get a selection event so they redraw cut marks.
    fn redraw_dirs(&self, dirs: &[PathBuf]) {
        let tabs: Vec<Arc<Tab>> = self.model.lock().tabs.values().cloned().collect();
        for t in tabs {
            if dirs.iter().any(|d| *d == t.location_path()) {
                let sel = t.selection();
                self.shared.emit(CoreEvent::SelectionChanged {
                    tab: t.id(),
                    count: sel.rows.len() as u32,
                    cursor: sel.cursor,
                });
            }
        }
    }

    /// Validates a new name typed in place and queues the rename. The row
    /// follows the item under its new name.
    pub fn rename_item(&self, tab: TabId, old_name: &str, new_name: &str) -> Result<Outcome> {
        let t = self.target(Some(tab))?;
        if old_name == new_name {
            return Ok(Outcome::none().with_tab(tab));
        }
        let path = t.rename_target(old_name, new_name)?;
        let id = self.enqueue(OpRequest::Rename {
            path,
            new_name: new_name.to_string(),
        })?;
        t.select_when_present(vec![new_name.to_string()]);
        Ok(Outcome::op(id).with_tab(tab))
    }

    /// Creates a missing folder (and any missing parents) through the queue,
    /// then shows it in the tab. For the "Create Folder" offer.
    pub fn create_folder_and_go(&self, tab: TabId, path: &str) -> Result<Outcome> {
        let t = self.target(Some(tab))?;
        let target = PathBuf::from(path);
        let mut missing = Vec::new();
        let mut cur = target.clone();
        while !cur.exists() {
            missing.push(cur.clone());
            match cur.parent() {
                Some(p) => cur = p.to_path_buf(),
                None => break,
            }
        }
        missing.reverse();
        for p in &missing {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .ok_or_else(|| CoreError::InvalidName(s(p)))?;
            fstudio_ops::names::validate_name(&name).map_err(CoreError::InvalidName)?;
        }
        let ops = self.ops();
        let me = self.me.clone();
        let mut first = None;
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("fstudio-create-folder".into())
            .spawn(move || {
                for (k, p) in missing.iter().enumerate() {
                    let req = OpRequest::NewFolder {
                        parent: p.parent().map(Path::to_path_buf).unwrap_or_default(),
                        name: p
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    };
                    let Ok(id) = ops.enqueue(req) else { return };
                    if k == 0 {
                        let _ = tx.send(id);
                    }
                    ops.wait(id, Duration::from_secs(30));
                }
                if me.upgrade().is_some() {
                    t.navigate(target, None);
                }
            })?;
        if let Ok(id) = rx.recv_timeout(Duration::from_secs(5)) {
            first = Some(id);
        }
        Ok(Outcome {
            ops: first.into_iter().collect(),
            ..Outcome::none().with_tab(tab)
        })
    }

    /// What dropping `sources` onto the folder `dest_dir` would do, for the
    /// drag cursor and the drop highlight. Finder's rule: a move on the same
    /// volume, a copy across volumes; ⌥ forces a copy and ⌘ forces a move.
    /// `None` when the drop makes no sense (onto itself or into its own
    /// subfolder, a move into the folder it is already in, or a destination
    /// that is not a folder). Does `lstat` on each source.
    pub fn drop_action(&self, sources: &[PathBuf], dest_dir: &Path, mods: Mods) -> DropAction {
        // One rule for the whole app: `drop.rs` (also used by the list view).
        let sources: Vec<String> = sources
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        match self.drop_effect(&sources, &dest_dir.to_string_lossy(), mods) {
            crate::drop::DropEffect::Copy => DropAction::Copy,
            crate::drop::DropEffect::Move => DropAction::Move,
            crate::drop::DropEffect::None => DropAction::None,
        }
    }

    /// Drops `sources` onto `dest_dir` by [`Engine::drop_action`]'s rule and
    /// queues the copy or move. `NotAvailable` when the drop makes no sense.
    pub fn drop_files(
        &self,
        sources: Vec<PathBuf>,
        dest_dir: PathBuf,
        mods: Mods,
    ) -> Result<Outcome> {
        let req = match self.drop_action(&sources, &dest_dir, mods) {
            DropAction::None => {
                return Err(CoreError::NotAvailable(
                    "the items can’t be dropped there".into(),
                ));
            }
            DropAction::Copy => OpRequest::Copy { sources, dest_dir },
            DropAction::Move => {
                let req = OpRequest::Move {
                    sources: sources.clone(),
                    dest_dir: dest_dir.clone(),
                };
                if let Some(ask) = self.guard_move(&sources, &dest_dir, || {
                    crate::protect::PendingMove::Request(req.clone())
                }) {
                    return Ok(ask);
                }
                let mut out = Outcome::op(self.enqueue(req)?);
                out.shell = Some(ShellRequest::Moved {
                    text: crate::protect::moved_text(&sources, &dest_dir),
                });
                return Ok(out);
            }
        };
        Ok(Outcome::op(self.enqueue(req)?))
    }

    /// F-023 for commands that move (F6, paste, Paste as Move, New Folder
    /// with Selection): unless the user already confirmed, a protected item
    /// stops the command and asks; on yes the command runs again with
    /// `Confirmed`.
    fn guard_command(
        &self,
        inv: &CommandInvocation,
        sources: &[PathBuf],
        dest: &Path,
    ) -> Option<Outcome> {
        if matches!(inv.arg, Some(CommandArg::Confirmed)) {
            return None;
        }
        self.guard_move(sources, dest, || {
            let mut again = inv.clone();
            again.arg = Some(CommandArg::Confirmed);
            crate::protect::PendingMove::Command(again)
        })
    }

    /// The paste guard: only a cut (or Paste as Move) moves.
    fn guard_paste(
        &self,
        inv: &CommandInvocation,
        t: &Tab,
        as_move: Option<bool>,
    ) -> Option<Outcome> {
        let cb = self.shared.clipboard.lock().clone()?;
        if !as_move.unwrap_or(cb.cut) {
            return None;
        }
        let dest = t.location_path();
        if cb.items.iter().all(|p| p.parent() == Some(dest.as_path())) {
            return None;
        }
        self.guard_command(inv, &cb.items, &dest)
            .map(|o| o.with_tab(t.id()))
    }

    fn paste_into(&self, t: &Tab, as_move: Option<bool>) -> Result<Outcome> {
        let Some(cb) = self.shared.clipboard.lock().clone() else {
            return Ok(Outcome::message("Nothing to paste"));
        };
        let dest = t.location_path();
        let mv = as_move.unwrap_or(cb.cut);
        let names: Vec<String> = cb
            .items
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        let here = cb.items.iter().all(|p| p.parent() == Some(dest.as_path()));
        if here && mv {
            *self.shared.clipboard.lock() = None;
            self.redraw_dirs(&[dest]);
            return Ok(Outcome::message("The items are already here").with_tab(t.id()));
        }
        let req = if here {
            // Pasting a copy into its own folder makes a copy next to it, as Finder does.
            OpRequest::Duplicate {
                paths: cb.items.clone(),
            }
        } else if mv {
            OpRequest::Move {
                sources: cb.items.clone(),
                dest_dir: dest,
            }
        } else {
            OpRequest::Copy {
                sources: cb.items.clone(),
                dest_dir: dest,
            }
        };
        let moved = mv.then(|| crate::protect::moved_text(&cb.items, &t.location_path()));
        let id = self.enqueue(req)?;
        if mv {
            // A cut is used up by its paste; nothing was deleted before now.
            let dirs: Vec<PathBuf> = cb
                .items
                .iter()
                .filter_map(|p| p.parent().map(Path::to_path_buf))
                .collect();
            *self.shared.clipboard.lock() = None;
            self.redraw_dirs(&dirs);
        }
        t.select_when_present(names);
        let mut out = Outcome::op(id).with_tab(t.id());
        out.shell = moved.map(|text| ShellRequest::Moved { text });
        Ok(out)
    }

    fn copy_paths(
        &self,
        t: &Tab,
        inv: &CommandInvocation,
        style: PathStyle,
        folder: bool,
    ) -> Outcome {
        let paths: Vec<String> = if let Some(CommandArg::Path(p)) = &inv.arg {
            vec![s(p)]
        } else if folder {
            vec![t.location()]
        } else {
            let sel = t.selected_paths();
            if sel.is_empty() {
                vec![t.location()]
            } else {
                sel
            }
        };
        let text = paths
            .iter()
            .map(|p| self.copy_path_as(p, style))
            .collect::<Vec<_>>()
            .join("\n");
        Outcome::shell(ShellRequest::CopyText { text })
    }

    fn set_sort(&self, t: &Tab, f: impl FnOnce(&mut crate::view::ViewState)) -> Outcome {
        let mut v = t.view();
        f(&mut v);
        t.set_view(v);
        Outcome::none().with_tab(t.id())
    }

    fn special_dir(&self, id: &str) -> PathBuf {
        let h = &self.shared.home;
        match id {
            "nav.go_home" => h.clone(),
            "nav.go_desktop" => h.join("Desktop"),
            "nav.go_documents" => h.join("Documents"),
            "nav.go_downloads" => h.join("Downloads"),
            "nav.go_applications" => PathBuf::from("/Applications"),
            "nav.go_icloud" => h.join("Library/Mobile Documents/com~apple~CloudDocs"),
            "nav.go_trash" => h.join(".Trash"),
            _ => PathBuf::from("/"),
        }
    }

    // ───────────────────────── perform ─────────────────────────

    /// Runs a command. `via` says how it was issued (mouse uses may hint;
    /// keyboard uses count toward "learned").
    pub fn perform(&self, inv: CommandInvocation, via: Via) -> Result<Outcome> {
        let info =
            registry::get(&inv.id).ok_or_else(|| CoreError::UnknownCommand(inv.id.clone()))?;
        let hint = self
            .keymap
            .lock()
            .hint_for(&info.id, via, Instant::now())
            .map(hint_info);
        if let Some(h) = &hint {
            self.shared.emit(CoreEvent::Hint { hint: h.clone() });
        }
        let mut out = self.dispatch(&inv)?;
        out.command = Some(inv.id.clone());
        if out.hint.is_none() {
            out.hint = hint;
        }
        Ok(out)
    }

    fn dispatch(&self, inv: &CommandInvocation) -> Result<Outcome> {
        let id = inv.id.as_str();
        if LATER_COMMANDS.contains(&id) {
            return Err(CoreError::NotAvailable(format!(
                "“{}” comes in a later version",
                registry::get(id).map(|c| c.title).unwrap_or(id)
            )));
        }
        // Commands that work without any window.
        match id {
            "window.new" => {
                let w = self.new_window(None)?;
                return Ok(Outcome::shell(ShellRequest::OpenWindow { window: w }));
            }
            "tab.reopen_closed" => {
                return Ok(match self.reopen_closed_tab()? {
                    Some(t) => Outcome::none().with_tab(t.id()),
                    None => Outcome::message("No closed tab to reopen"),
                });
            }
            "edit.undo" => return self.undo(),
            "edit.redo" => return self.redo(),
            "app.settings" => {
                return Ok(Outcome::shell(ShellRequest::ShowSettings {
                    section: String::new(),
                }));
            }
            "app.keyboard_shortcuts" => {
                return Ok(Outcome::shell(ShellRequest::ShowSettings {
                    section: "shortcuts".into(),
                }));
            }
            "palette.open" => return Ok(Outcome::shell(ShellRequest::ShowPalette)),
            "nav.quick_open" => return Ok(Outcome::shell(ShellRequest::ShowQuickOpen)),
            "view.toggle_sidebar" => return Ok(Outcome::shell(ShellRequest::ToggleSidebar)),
            "view.toggle_inspector" => return Ok(Outcome::shell(ShellRequest::ToggleInspector)),
            "workspace.save" => return Ok(Outcome::shell(ShellRequest::AskWorkspaceName)),
            "workspace.manage" => return Ok(Outcome::shell(ShellRequest::ManageWorkspaces)),
            "workspace.switch" => {
                return match &inv.arg {
                    Some(CommandArg::Index(i)) => {
                        let list = self.saved_workspaces()?;
                        let w = list
                            .get(*i)
                            .ok_or_else(|| CoreError::NotFound(format!("workspace {}", i + 1)))?;
                        self.open_workspace(w.id)?;
                        Ok(Outcome::message(format!("Opened “{}”", w.name)))
                    }
                    _ => Ok(Outcome::shell(ShellRequest::ManageWorkspaces)),
                };
            }
            _ if id.starts_with("addressbar.")
                || id.starts_with("palette.")
                || id.starts_with("rename.") =>
            {
                if let (Some(CommandArg::Path(_)), "addressbar.add_favourite") = (&inv.arg, id) {
                    return self.favourites_command("favourites.add", inv);
                }
                if let Some(CommandArg::Path(p)) = &inv.arg {
                    let t = self.target(inv.tab)?;
                    let t = if id == "addressbar.commit_other_pane" {
                        self.other(&t)?
                    } else {
                        t
                    };
                    if id.starts_with("addressbar.commit") {
                        return t.go(&p.to_string_lossy());
                    }
                }
                return Ok(Outcome::shell(ShellRequest::TextField {
                    command: id.to_string(),
                }));
            }
            "filter.commit" => return Ok(Outcome::shell(ShellRequest::FocusList)),
            "favourites.add" | "favourites.remove" => return self.favourites_command(id, inv),
            "app.power_tour" => return Ok(Outcome::shell(ShellRequest::ShowPowerTour)),
            "sidebar.remove" | "sidebar.rename" | "drive.eject" | "drive.info" => {
                return self.sidebar_command(id, inv.arg.as_ref());
            }
            _ => {}
        }

        let t = self.target(inv.tab)?;
        let tid = t.id();
        let done = || Outcome::none().with_tab(tid);
        let vim_visual = *self.vim.lock() == VimMode::Visual;
        let mv = |by: CursorMove, extend: bool| {
            t.set_selection(SelectionChange::Move { by, extend });
            Outcome::none().with_tab(tid)
        };

        Ok(match id {
            // ── navigation ──
            "nav.open" => {
                let sel = Self::need_targets(inv, &t)?;
                let (dirs, files): (Vec<PathBuf>, Vec<PathBuf>) = sel.into_iter().partition(|p| {
                    let l = t.listing();
                    p.file_name()
                        .and_then(|n| l.inner.get(&n.to_string_lossy()).cloned())
                        .is_some_and(|e| e.is_dir() && !e.is_package)
                        || (p.is_dir() && !fstudio_fs::is_package_name(&s(p)))
                });
                if files.is_empty() && dirs.len() == 1 {
                    self.go_dir(&t, dirs[0].clone())
                } else {
                    let pane = self.pane_of(tid)?;
                    for d in &dirs {
                        self.open_tab_in(pane, Some(s(d)), None)?;
                    }
                    if files.is_empty() {
                        done()
                    } else {
                        Outcome::shell(ShellRequest::Open {
                            paths: strings(&files),
                        })
                        .with_tab(tid)
                    }
                }
            }
            "nav.open_new_tab" => {
                let sel = Self::arg_or_selection(inv, &t);
                let dirs: Vec<PathBuf> = sel.into_iter().filter(|p| p.is_dir()).collect();
                let pane = self.pane_of(tid)?;
                if dirs.is_empty() {
                    self.open_tab_in(pane, Some(t.location()), None)?;
                }
                for d in dirs {
                    self.open_tab_in(pane, Some(s(&d)), None)?;
                }
                done()
            }
            "nav.open_other_pane" => {
                let o = self.other(&t)?;
                let d = Self::need_targets(inv, &t)?
                    .into_iter()
                    .find(|p| p.is_dir())
                    .ok_or_else(|| CoreError::NotAvailable("no folder is selected".into()))?;
                self.go_dir(&o, d)
            }
            "nav.up" => t.up()?,
            "nav.back" => t.back()?,
            "nav.forward" => t.forward()?,
            "nav.go_to" => match &inv.arg {
                Some(CommandArg::Path(p)) => t.go(&p.to_string_lossy())?,
                _ => Outcome::shell(ShellRequest::FocusAddressBar {
                    pane: self.pane_of(tid)?,
                    text: String::new(),
                })
                .with_tab(tid),
            },
            "nav.address_bar" => Outcome::shell(ShellRequest::FocusAddressBar {
                pane: self.pane_of(tid)?,
                text: match &inv.arg {
                    Some(CommandArg::Char(c)) => c.to_string(),
                    _ => String::new(),
                },
            })
            .with_tab(tid),
            "nav.other_address_bar" => {
                let o = self.other(&t)?;
                Outcome::shell(ShellRequest::FocusAddressBar {
                    pane: self.pane_of(o.id())?,
                    text: String::new(),
                })
                .with_tab(o.id())
            }
            "nav.go_home"
            | "nav.go_desktop"
            | "nav.go_documents"
            | "nav.go_downloads"
            | "nav.go_applications"
            | "nav.go_icloud"
            | "nav.go_root"
            | "nav.go_trash" => {
                let d = self.special_dir(id);
                if !d.is_dir() {
                    return Err(CoreError::NotFound(s(&d)));
                }
                self.shared.frecency.visit(&d);
                self.go_dir(&t, d)
            }
            "nav.show_package_contents" => {
                let d = Self::need_targets(inv, &t)?
                    .into_iter()
                    .find(|p| p.is_dir())
                    .ok_or_else(|| CoreError::NotAvailable("no package is selected".into()))?;
                self.go_dir(&t, d)
            }
            "nav.mark_set" => {
                let Some(CommandArg::Char(c)) = inv.arg else {
                    return Err(CoreError::NotAvailable("no mark letter given".into()));
                };
                let json = {
                    let mut m = self.marks.lock();
                    m.insert(c, t.location());
                    serde_json::to_string(&*m)?
                };
                self.db.kv_set("core.marks", &json)?;
                Outcome::message(format!("Mark ‘{c}’ set")).with_tab(tid)
            }
            "nav.mark_jump" => {
                let Some(CommandArg::Char(c)) = inv.arg else {
                    return Err(CoreError::NotAvailable("no mark letter given".into()));
                };
                let p = self
                    .marks
                    .lock()
                    .get(&c)
                    .cloned()
                    .ok_or_else(|| CoreError::NotFound(format!("mark ‘{c}’")))?;
                t.go(&p)?
            }

            // ── selection and cursor ──
            "select.all" => {
                t.set_selection(SelectionChange::All);
                done()
            }
            "select.none" => {
                t.set_selection(SelectionChange::None);
                done()
            }
            "select.invert" => {
                t.set_selection(SelectionChange::Invert);
                done()
            }
            "select.by_pattern" => {
                Outcome::shell(ShellRequest::AskPattern { select: true }).with_tab(tid)
            }
            "select.deselect_pattern" => {
                Outcome::shell(ShellRequest::AskPattern { select: false }).with_tab(tid)
            }
            "select.extend_up" => mv(CursorMove::Up, true),
            "select.extend_down" => mv(CursorMove::Down, true),
            "select.visual" | "select.visual_line" => {
                *self.vim.lock() = VimMode::Visual;
                if let Some(c) = t.selection().cursor {
                    t.set_selection(SelectionChange::Only(c));
                } else {
                    t.set_selection(SelectionChange::Move {
                        by: CursorMove::Down,
                        extend: false,
                    });
                }
                done()
            }
            "select.exit_visual" => {
                let mut v = self.vim.lock();
                if *v == VimMode::Visual {
                    *v = VimMode::Normal;
                }
                done()
            }
            "cursor.up" => mv(CursorMove::Up, vim_visual),
            "cursor.down" => mv(CursorMove::Down, vim_visual),
            "cursor.top" => mv(CursorMove::Home, vim_visual),
            "cursor.bottom" => mv(CursorMove::End, vim_visual),
            "cursor.page_up" => mv(CursorMove::PageUp, vim_visual),
            "cursor.page_down" => mv(CursorMove::PageDown, vim_visual),

            // ── file ──
            "file.rename" => {
                let sel = t.selection();
                let name = match (sel.names.len(), sel.cursor) {
                    (1, _) => sel.names[0].clone(),
                    (0, Some(c)) => t.listing().name_at(c).unwrap_or_default(),
                    _ => {
                        return Ok(Outcome::shell(ShellRequest::RenameDialog {
                            tab: tid,
                            paths: t.selected_paths(),
                        })
                        .with_tab(tid));
                    }
                };
                if name.is_empty() {
                    return Err(CoreError::NotAvailable("nothing is selected".into()));
                }
                Outcome::shell(ShellRequest::BeginRename { tab: tid, name }).with_tab(tid)
            }
            "file.rename_dialog" => {
                Self::need_selection(&t)?;
                Outcome::shell(ShellRequest::RenameDialog {
                    tab: tid,
                    paths: t.selected_paths(),
                })
                .with_tab(tid)
            }
            "file.new_folder" | "file.new_file" => {
                let folder = id == "file.new_folder";
                let name = Self::free_name(
                    &t,
                    if folder {
                        UNTITLED_FOLDER
                    } else {
                        UNTITLED_FILE
                    },
                );
                let parent = t.location_path();
                let req = if folder {
                    OpRequest::NewFolder {
                        parent,
                        name: name.clone(),
                    }
                } else {
                    OpRequest::NewFile {
                        parent,
                        name: name.clone(),
                    }
                };
                let op = self.enqueue(req)?;
                t.select_when_present(vec![name.clone()]);
                Outcome {
                    ops: vec![op],
                    shell: Some(ShellRequest::BeginRename { tab: tid, name }),
                    ..done()
                }
            }
            "file.new_folder_with_selection" => {
                let sel = Self::need_selection(&t)?;
                let name = Self::free_name(&t, NEW_FOLDER_WITH_ITEMS);
                let parent = t.location_path();
                if let Some(ask) = self.guard_command(inv, &sel, &parent.join(&name)) {
                    return Ok(ask.with_tab(tid));
                }
                let ops = self.ops();
                let first = ops.enqueue(OpRequest::NewFolder {
                    parent: parent.clone(),
                    name: name.clone(),
                })?;
                let dest = parent.join(&name);
                // The move needs the folder to exist (it picks its volume from it).
                std::thread::Builder::new()
                    .name("fstudio-new-folder-with-items".into())
                    .spawn(move || {
                        ops.wait(first, Duration::from_secs(30));
                        if dest.is_dir() {
                            let _ = ops.enqueue(OpRequest::Move {
                                sources: sel,
                                dest_dir: dest,
                            });
                        }
                    })?;
                t.select_when_present(vec![name.clone()]);
                Outcome {
                    ops: vec![first],
                    shell: Some(ShellRequest::BeginRename { tab: tid, name }),
                    ..done()
                }
            }
            "file.trash" => {
                let sel = Self::need_selection(&t)?;
                // `Confirmed` is the shell's "yes" to our own question: carry on.
                let confirmed = matches!(inv.arg, Some(CommandArg::Confirmed));
                let ask = matches!(inv.arg, Some(CommandArg::Confirm))
                    || self.settings().general.confirm_trash;
                if ask && !confirmed {
                    return Ok(Outcome::shell(ShellRequest::ConfirmTrash {
                        paths: strings(&sel),
                    })
                    .with_tab(tid));
                }
                t.select_after_removing(&sel);
                Outcome::op(self.enqueue(OpRequest::Trash { paths: sel })?).with_tab(tid)
            }
            "file.delete_immediately" => {
                let sel = Self::need_selection(&t)?;
                if !self.settings().operations.allow_delete_immediately {
                    return Err(CoreError::NotAvailable(
                        "Delete Immediately is turned off in Settings".into(),
                    ));
                }
                if !matches!(inv.arg, Some(CommandArg::Confirm | CommandArg::Confirmed)) {
                    return Ok(Outcome::shell(ShellRequest::ConfirmDeleteImmediately {
                        paths: strings(&sel),
                    })
                    .with_tab(tid));
                }
                t.select_after_removing(&sel);
                Outcome::op(self.enqueue(OpRequest::DeleteImmediately { paths: sel })?)
                    .with_tab(tid)
            }
            "file.duplicate" => {
                let sel = Self::need_selection(&t)?;
                Outcome::op(self.enqueue(OpRequest::Duplicate { paths: sel })?).with_tab(tid)
            }
            "file.copy_to_other_pane" | "file.move_to_other_pane" => {
                let sel = Self::need_selection(&t)?;
                let o = self.other(&t)?;
                let dest_dir = o.location_path();
                if id == "file.move_to_other_pane"
                    && let Some(ask) = self.guard_command(inv, &sel, &dest_dir)
                {
                    return Ok(ask.with_tab(tid));
                }
                let moved = (id == "file.move_to_other_pane")
                    .then(|| crate::protect::moved_text(&sel, &dest_dir));
                let names: Vec<String> = sel
                    .iter()
                    .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .collect();
                let req = if id == "file.copy_to_other_pane" {
                    OpRequest::Copy {
                        sources: sel,
                        dest_dir,
                    }
                } else {
                    t.select_after_removing(&sel);
                    OpRequest::Move {
                        sources: sel,
                        dest_dir,
                    }
                };
                let op = self.enqueue(req)?;
                o.select_when_present(names);
                let mut out = Outcome::op(op).with_tab(tid);
                out.shell = moved.map(|text| ShellRequest::Moved { text });
                out
            }
            "file.move_here" => {
                if let Some(ask) = self.guard_paste(inv, &t, Some(true)) {
                    return Ok(ask);
                }
                self.paste_into(&t, Some(true))?
            }
            "file.open_with" => Outcome::shell(ShellRequest::OpenWith {
                paths: strings(&Self::need_selection(&t)?),
            })
            .with_tab(tid),
            "file.open_in_editor" => Outcome::shell(ShellRequest::OpenInEditor {
                paths: strings(&Self::need_selection(&t)?),
            })
            .with_tab(tid),
            "file.reveal_in_finder" => {
                let sel = Self::arg_or_selection(inv, &t);
                let paths = if sel.is_empty() {
                    vec![t.location()]
                } else {
                    strings(&sel)
                };
                Outcome::shell(ShellRequest::RevealInFinder { paths }).with_tab(tid)
            }

            // ── edit ──
            "edit.copy" => {
                let sel = Self::need_selection(&t)?;
                self.set_clipboard(strings(&sel), false);
                Outcome::shell(ShellRequest::WriteFilePasteboard {
                    paths: strings(&sel),
                })
                .with_tab(tid)
            }
            "edit.cut" => {
                let sel = Self::need_selection(&t)?;
                let n = sel.len();
                self.set_clipboard(strings(&sel), true);
                Outcome::message(format!(
                    "{} marked to move; paste to move {}",
                    if n == 1 {
                        "1 item".to_string()
                    } else {
                        format!("{n} items")
                    },
                    if n == 1 { "it" } else { "them" }
                ))
                .with_tab(tid)
            }
            "edit.paste" => {
                if let Some(ask) = self.guard_paste(inv, &t, None) {
                    return Ok(ask);
                }
                self.paste_into(&t, None)?
            }

            // ── view ──
            "view.set_mode" => {
                let Some(CommandArg::ViewMode(m)) = inv.arg else {
                    return Err(CoreError::NotAvailable("no view mode given".into()));
                };
                self.set_sort(&t, |v| v.mode = m)
            }
            "view.icons" => self.set_sort(&t, |v| v.mode = ViewMode::Icons),
            "view.list" => self.set_sort(&t, |v| v.mode = ViewMode::List),
            "view.columns" => self.set_sort(&t, |v| v.mode = ViewMode::Columns),
            "view.gallery" => self.set_sort(&t, |v| v.mode = ViewMode::Gallery),
            "view.show_hidden" => self.set_sort(&t, |v| v.show_hidden = !v.show_hidden),
            "view.refresh" => {
                t.refresh();
                done()
            }
            "sort.name" => self.set_sort(&t, |v| v.sort = SortBy::Name),
            "sort.kind" => self.set_sort(&t, |v| v.sort = SortBy::Kind),
            "sort.size" => self.set_sort(&t, |v| v.sort = SortBy::Size),
            "sort.date_modified" => self.set_sort(&t, |v| v.sort = SortBy::Modified),
            "sort.date_created" => self.set_sort(&t, |v| v.sort = SortBy::Created),
            "sort.reverse" => self.set_sort(&t, |v| v.ascending = !v.ascending),
            "sort.folders_first" => self.set_sort(&t, |v| v.folders_first = !v.folders_first),

            // ── tabs and windows ──
            "tab.new" => {
                let pane = self.pane_of(tid)?;
                let nt = self.open_tab_in(pane, None, None)?;
                Outcome::none().with_tab(nt.id())
            }
            "tab.close" => {
                self.close_tab(tid)?;
                Outcome::none()
            }
            "tab.next" | "tab.previous" => {
                let next = {
                    let m = self.model.lock();
                    let pl = m
                        .find(tid)
                        .ok_or_else(|| CoreError::NotFound(format!("tab {tid}")))?;
                    let p = &m.windows[pl.window].panes[pl.pane];
                    let n = p.tabs.len();
                    let i = if id == "tab.next" {
                        (pl.index + 1) % n
                    } else {
                        (pl.index + n - 1) % n
                    };
                    p.tabs[i]
                };
                self.activate_tab(next)?;
                Outcome::none().with_tab(next)
            }
            "tab.duplicate" => {
                let pane = self.pane_of(tid)?;
                let mut st = t.state();
                st.id = TabId(0);
                let idx = {
                    let m = self.model.lock();
                    m.find(tid).map(|p| p.index + 1)
                };
                let nt =
                    self.open_tab_in(pane, Some(st.location.clone()), idx.map(|i| i as u32))?;
                nt.set_view(st.view);
                Outcome::none().with_tab(nt.id())
            }
            "tab.select" => {
                let Some(CommandArg::Index(i)) = inv.arg else {
                    return Err(CoreError::NotAvailable("no tab number given".into()));
                };
                let target = {
                    let m = self.model.lock();
                    let pl = m
                        .find(tid)
                        .ok_or_else(|| CoreError::NotFound(format!("tab {tid}")))?;
                    let p = &m.windows[pl.window].panes[pl.pane];
                    // Like Safari: the last number means the last tab.
                    let i = if i >= 8 { p.tabs.len() - 1 } else { i };
                    p.tabs.get(i).copied()
                };
                let target = target.ok_or_else(|| CoreError::NotFound(format!("tab {}", i + 1)))?;
                self.activate_tab(target)?;
                Outcome::none().with_tab(target)
            }
            "tab.close_others" => {
                self.close_other_tabs(tid)?;
                done()
            }
            "tab.move_to_other_pane" => {
                let o = self.other(&t)?;
                let pane = self.pane_of(o.id())?;
                self.move_tab(tid, pane, u32::MAX)?;
                done()
            }
            "window.close" => {
                let w = self.window_of(tid)?;
                self.close_window(w)?;
                Outcome::shell(ShellRequest::CloseWindow { window: w })
            }

            // ── panes ──
            "pane.toggle_dual" => {
                let w = self.window_of(tid)?;
                let dual = self.toggle_dual(w)?;
                {
                    let _ = dual; // the toolbar button and status line show the layout
                    Outcome::none().with_tab(tid)
                }
            }
            "pane.switch" => {
                let o = self.other(&t)?;
                self.activate_tab(o.id())?;
                Outcome::none().with_tab(o.id())
            }
            "pane.swap" => {
                self.other(&t)?;
                self.swap_panes(self.window_of(tid)?)?;
                done()
            }
            "pane.same_folder" => {
                let o = self.other(&t)?;
                self.go_dir(&o, t.location_path());
                done()
            }
            "pane.same_folder_reverse" => {
                let o = self.other(&t)?;
                self.go_dir(&t, o.location_path())
            }
            "pane.equalize" => {
                self.other(&t)?;
                let w = self.window_of(tid)?;
                let orientation = {
                    let m = self.model.lock();
                    m.window_index(w)
                        .map(|i| m.windows[i].orientation)
                        .unwrap_or_default()
                };
                self.set_layout(w, orientation, 0.5)?;
                done()
            }

            // ── preview, paths, terminal ──
            "preview.quick_look" => Outcome::shell(ShellRequest::QuickLook {
                paths: strings(&Self::need_targets(inv, &t)?),
            })
            .with_tab(tid),
            "preview.get_info" => {
                let sel = Self::arg_or_selection(inv, &t);
                let paths = if sel.is_empty() {
                    vec![t.location()]
                } else {
                    strings(&sel)
                };
                Outcome::shell(ShellRequest::GetInfo { paths }).with_tab(tid)
            }
            "path.copy_path" => self
                .copy_paths(&t, inv, PathStyle::Posix, false)
                .with_tab(tid),
            "path.copy_escaped" => self
                .copy_paths(&t, inv, PathStyle::Escaped, false)
                .with_tab(tid),
            "path.copy_home_relative" => self
                .copy_paths(&t, inv, PathStyle::HomeRelative, false)
                .with_tab(tid),
            "path.copy_file_url" => self
                .copy_paths(&t, inv, PathStyle::FileUrl, false)
                .with_tab(tid),
            "path.copy_quoted" => self
                .copy_paths(&t, inv, PathStyle::Quoted, false)
                .with_tab(tid),
            "path.copy_folder_path" => self
                .copy_paths(&t, inv, PathStyle::Posix, true)
                .with_tab(tid),
            "path.copy_name" => {
                let sel = Self::arg_or_selection(inv, &t);
                let text = if sel.is_empty() {
                    t.location_path()
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "/".into())
                } else {
                    sel.iter()
                        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                Outcome::shell(ShellRequest::CopyText { text }).with_tab(tid)
            }
            "terminal.open" => {
                let dir = match &inv.arg {
                    Some(CommandArg::Path(p)) => s(&crate::menus::folder_of(p)),
                    _ => t.location(),
                };
                Outcome::shell(ShellRequest::OpenTerminal { dir }).with_tab(tid)
            }
            "tags.toggle" => {
                let Some(CommandArg::Text(name)) = &inv.arg else {
                    return Err(CoreError::NotAvailable("no tag given".into()));
                };
                let sel = Self::need_selection(&t)?;
                let n = crate::menus::toggle_tag(&sel, name)?;
                Outcome::message(format!(
                    "Tag “{name}” changed on {n} item{}",
                    if n == 1 { "" } else { "s" }
                ))
                .with_tab(tid)
            }
            "tags.edit" => Outcome::shell(ShellRequest::EditTags {
                paths: strings(&Self::need_selection(&t)?),
            })
            .with_tab(tid),

            // ── filter ──
            "filter.focus" => Outcome::shell(ShellRequest::FocusFilter).with_tab(tid),
            "filter.clear" => {
                t.set_filter("");
                Outcome::shell(ShellRequest::FocusList).with_tab(tid)
            }

            other => return Err(CoreError::UnknownCommand(other.to_string())),
        })
    }

    /// Records a queued op's id for callers that want to wait (CLI, tests).
    pub fn wait_op(&self, id: OpId, timeout: Duration) -> Option<fstudio_ops::OpStatus> {
        self.ops().wait(id, timeout)
    }
}

/// What a drop does (see [`Engine::drop_action`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DropAction {
    /// Nothing: the drop is refused.
    None,
    /// Copy the items into the folder.
    Copy,
    /// Move the items into the folder.
    Move,
}

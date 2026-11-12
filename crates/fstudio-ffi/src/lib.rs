//! UniFFI exports: a thin, lossless translation of `fstudio-core` for the
//! Swift shell (F-004). No app logic lives here; every method forwards to the
//! core. Paths cross as `String`, ids as integers, and core types that carry
//! `PathBuf`s or generics get plain mirror records in [`types`].
//!
//! Threads: every call may be made from any thread. Calls are quick and never
//! block on disk unless their docs say so. Events arrive on the core's event
//! thread through [`FfiEventSink`]; the shell hops them to its main thread.

#![allow(missing_docs)]

mod settings;
mod types;

use std::sync::Arc;

use fstudio_core as core;
use fstudio_core::{Engine, Tab, TabId};

pub use settings::*;
pub use types::*;

uniffi::setup_scaffolding!();

/// The core's version.
#[uniffi::export]
pub fn core_version() -> String {
    fstudio_core::version().to_string()
}

// ───────────────────────── callbacks ─────────────────────────

/// Receives core events. Implemented by the shell. Called on one core
/// thread, in order; return quickly (hop to the main thread).
#[uniffi::export(with_foreign)]
pub trait FfiEventSink: Send + Sync {
    /// Handles one event.
    fn on_event(&self, event: FfiEvent);
}

struct SinkBridge(Arc<dyn FfiEventSink>);

impl core::EventSink for SinkBridge {
    fn on_event(&self, e: core::CoreEvent) {
        self.0.on_event(e.into());
    }
}

// ───────────────────────── tab ─────────────────────────

/// One tab: a location, its history, view state, selection and listing.
#[derive(uniffi::Object)]
pub struct FfiTab {
    inner: Arc<Tab>,
}

impl FfiTab {
    fn wrap(t: Arc<Tab>) -> Arc<Self> {
        Arc::new(FfiTab { inner: t })
    }
}

#[uniffi::export]
impl FfiTab {
    /// The tab's id.
    pub fn id(&self) -> u64 {
        self.inner.id().0
    }
    /// The folder shown.
    pub fn location(&self) -> String {
        self.inner.location()
    }
    /// The persistent state (history, view, selection names, scroll).
    pub fn state(&self) -> FfiTabState {
        self.inner.state().into()
    }
    /// The current listing's version, size and state.
    pub fn listing_info(&self) -> FfiListingInfo {
        FfiListingInfo::from(&*self.inner.listing())
    }
    /// Rows `start..start+count` of the current listing, formatted. Cheap:
    /// call it for the visible rows only and cache by listing version.
    pub fn rows(&self, start: u32, count: u32) -> Vec<FfiRow> {
        self.inner
            .rows(start, count)
            .into_iter()
            .map(Into::into)
            .collect()
    }
    /// The same rows as [`FfiTab::rows`], packed into one byte buffer, for
    /// shells whose generated record lifting is slow (Swift: ~150 µs for 60
    /// records against ~15 µs for this). Little endian:
    ///
    /// ```text
    /// u32 count
    /// per row: u32 index, u8 kind (0 folder, 1 package, 2 file, 3 symlink, 4 other),
    ///          u8 flags (1 hidden, 2 dataless, 4 package, 8 symlink, 16 selected,
    ///                    32 cursor, 64 cut pending),
    ///          u64 size, i64 modified_ns,
    ///          5 × (u32 byte length, UTF-8 bytes): name, icon_key, kind_text,
    ///                                                size_text, modified_text
    /// ```
    pub fn rows_packed(&self, start: u32, count: u32) -> Vec<u8> {
        pack_rows(&self.inner.rows(start, count))
    }
    /// The row of the entry with exactly this name.
    pub fn index_of(&self, name: String) -> Option<u32> {
        self.inner.listing().index_of(&name)
    }
    /// The full path at row `i`.
    pub fn path_at(&self, i: u32) -> Option<String> {
        self.inner.listing().path_at(i)
    }
    /// Goes to typed or pasted text (parsed by the path engine).
    pub fn go(&self, to: String) -> R<FfiOutcome> {
        Ok(self.inner.go(&to)?.into())
    }
    pub fn back(&self) -> R<FfiOutcome> {
        Ok(self.inner.back()?.into())
    }
    pub fn forward(&self) -> R<FfiOutcome> {
        Ok(self.inner.forward()?.into())
    }
    pub fn up(&self) -> R<FfiOutcome> {
        Ok(self.inner.up()?.into())
    }
    /// Can go back / forward.
    pub fn can_go_back(&self) -> bool {
        self.inner.can_go().0
    }
    pub fn can_go_forward(&self) -> bool {
        self.inner.can_go().1
    }
    /// Re-reads the folder.
    pub fn refresh(&self) {
        self.inner.refresh();
    }
    pub fn view(&self) -> FfiViewState {
        self.inner.view().into()
    }
    /// Changes sort, mode, hidden files…; re-sorts off the calling thread.
    pub fn set_view(&self, view: FfiViewState) {
        self.inner.set_view(view.into());
    }
    pub fn filter(&self) -> String {
        self.inner.filter()
    }
    /// Filters as you type (off the calling thread).
    pub fn set_filter(&self, text: String) {
        self.inner.set_filter(&text);
    }
    pub fn selection(&self) -> FfiSelection {
        self.inner.selection().into()
    }
    /// Selected items' full paths.
    pub fn selected_paths(&self) -> Vec<String> {
        self.inner.selected_paths()
    }
    pub fn set_selection(&self, change: FfiSelectionChange) {
        self.inner.set_selection(change.into());
    }
    pub fn select_pattern(&self, pattern: String, select: bool) {
        self.inner.select_pattern(&pattern, select);
    }
    /// Type-to-select with a whole string; returns the row jumped to.
    pub fn type_select(&self, text: String) -> Option<u32> {
        self.inner.type_select(&text)
    }
    /// The rows on screen (page size, and priority for lazy metadata).
    pub fn set_visible_range(&self, start: u32, count: u32) {
        self.inner.set_visible_range(start, count);
    }
    pub fn set_scroll_anchor(&self, anchor: Option<FfiScrollAnchor>) {
        self.inner
            .set_scroll_anchor(anchor.map(|a| core::ScrollAnchor {
                name: a.name,
                offset: a.offset,
            }));
    }
    pub fn scroll_anchor(&self) -> Option<FfiScrollAnchor> {
        self.inner.scroll_anchor().map(|a| FfiScrollAnchor {
            name: a.name,
            offset: a.offset,
        })
    }
    /// The address bar's crumbs for this tab's folder.
    pub fn breadcrumbs(&self) -> Vec<FfiCrumb> {
        self.inner
            .breadcrumbs()
            .into_iter()
            .map(Into::into)
            .collect()
    }
    /// Completions for the address bar text, `cursor` a UTF-8 byte offset.
    /// Lists at most one cached local folder.
    pub fn complete(&self, input: String, cursor: u32) -> FfiCompletions {
        self.inner.complete(&input, cursor).into()
    }
}

/// Packs rows in the layout documented on [`FfiTab::rows_packed`].
fn pack_rows(rows: &[core::RowInfo]) -> Vec<u8> {
    let mut b = Vec::with_capacity(16 + rows.len() * 96);
    b.extend_from_slice(&(rows.len() as u32).to_le_bytes());
    for r in rows {
        b.extend_from_slice(&r.index.to_le_bytes());
        b.push(match r.kind {
            core::RowKind::Folder => 0,
            core::RowKind::Package => 1,
            core::RowKind::File => 2,
            core::RowKind::Symlink => 3,
            core::RowKind::Other => 4,
        });
        let flags = [
            r.is_hidden,
            r.is_dataless,
            r.is_package,
            r.is_symlink,
            r.is_selected,
            r.is_cursor,
            r.is_cut_pending,
        ]
        .iter()
        .enumerate()
        .fold(0u8, |f, (i, on)| if *on { f | (1 << i) } else { f });
        b.push(flags);
        b.extend_from_slice(&r.size.to_le_bytes());
        b.extend_from_slice(&r.modified_ns.to_le_bytes());
        for s in [
            &r.name,
            &r.icon_key,
            &r.kind_text,
            &r.size_text,
            &r.modified_text,
        ] {
            b.extend_from_slice(&(s.len() as u32).to_le_bytes());
            b.extend_from_slice(s.as_bytes());
        }
    }
    b
}

// ───────────────────────── engine ─────────────────────────

/// The engine: one per process.
#[derive(uniffi::Object)]
pub struct FfiEngine {
    inner: Arc<Engine>,
}

fn tab_id(t: u64) -> TabId {
    TabId(t)
}

#[uniffi::export]
impl FfiEngine {
    /// Opens the database in `config.data_dir` and starts the core's threads.
    #[uniffi::constructor]
    pub fn new(config: FfiEngineConfig, sink: Arc<dyn FfiEventSink>) -> R<Arc<Self>> {
        let e = Engine::new(config.into(), Arc::new(SinkBridge(sink)))?;
        Ok(Arc::new(FfiEngine { inner: e }))
    }

    pub fn data_dir(&self) -> String {
        self.inner.data_dir()
    }
    pub fn home(&self) -> String {
        self.inner.home()
    }

    // ── settings ──

    pub fn settings(&self) -> FfiSettings {
        FfiSettings::from(&self.inner.settings())
    }
    /// Validates and applies settings (keymap and menu customisations are kept).
    pub fn update_settings(&self, s: FfiSettings) -> R<()> {
        let mut base = self.inner.settings();
        settings::apply(&mut base, s);
        Ok(self.inner.update_settings(base)?)
    }
    /// One setting as JSON, by dotted key (`view.show_hidden`).
    pub fn setting(&self, key: String) -> R<String> {
        Ok(self.inner.setting(&key)?)
    }
    /// Sets one setting from JSON text.
    pub fn set_setting(&self, key: String, value: String) -> R<()> {
        Ok(self.inner.set_setting(&key, &value)?)
    }
    pub fn export_settings(&self) -> String {
        self.inner.export_settings()
    }
    pub fn import_settings(&self, json: String) -> R<()> {
        Ok(self.inner.import_settings(&json)?)
    }

    // ── workspace ──

    /// Restores the last session, or opens one default window.
    pub fn restore_session(&self) -> FfiWorkspaceState {
        self.inner.restore_session().into()
    }
    pub fn workspace_state(&self) -> FfiWorkspaceState {
        self.inner.workspace_state().into()
    }
    pub fn tab(&self, id: u64) -> Option<Arc<FfiTab>> {
        self.inner.tab(tab_id(id)).map(FfiTab::wrap)
    }
    pub fn active_tab(&self) -> Option<Arc<FfiTab>> {
        self.inner.active_tab().map(FfiTab::wrap)
    }
    /// The active tab of the other pane, in dual-pane mode.
    pub fn other_pane_tab(&self, of: u64) -> Option<Arc<FfiTab>> {
        self.inner.other_pane_tab(tab_id(of)).map(FfiTab::wrap)
    }
    pub fn new_window(&self, path: Option<String>) -> R<u64> {
        Ok(self.inner.new_window(path)?.0)
    }
    pub fn close_window(&self, id: u64) -> R<()> {
        Ok(self.inner.close_window(core::WindowId(id))?)
    }
    pub fn open_tab(&self, path: Option<String>) -> R<Arc<FfiTab>> {
        Ok(FfiTab::wrap(self.inner.open_tab(path)?))
    }
    pub fn open_tab_in(
        &self,
        pane: u64,
        path: Option<String>,
        index: Option<u32>,
    ) -> R<Arc<FfiTab>> {
        Ok(FfiTab::wrap(self.inner.open_tab_in(
            core::PaneId(pane),
            path,
            index,
        )?))
    }
    pub fn close_tab(&self, id: u64) -> R<()> {
        Ok(self.inner.close_tab(tab_id(id))?)
    }
    pub fn close_other_tabs(&self, keep: u64) -> R<()> {
        Ok(self.inner.close_other_tabs(tab_id(keep))?)
    }
    pub fn reopen_closed_tab(&self) -> R<Option<Arc<FfiTab>>> {
        Ok(self.inner.reopen_closed_tab()?.map(FfiTab::wrap))
    }
    pub fn activate_tab(&self, id: u64) -> R<()> {
        Ok(self.inner.activate_tab(tab_id(id))?)
    }
    pub fn move_tab(&self, id: u64, pane: u64, index: u32) -> R<()> {
        Ok(self.inner.move_tab(tab_id(id), core::PaneId(pane), index)?)
    }
    pub fn focus_pane(&self, window: u64, index: u32) -> R<()> {
        Ok(self.inner.focus_pane(core::WindowId(window), index)?)
    }
    /// Turns dual pane on or off; returns whether it is on.
    pub fn toggle_dual(&self, window: u64) -> R<bool> {
        Ok(self.inner.toggle_dual(core::WindowId(window))?)
    }
    pub fn set_layout(&self, window: u64, orientation: FfiOrientation, split_ratio: f64) -> R<()> {
        Ok(self
            .inner
            .set_layout(core::WindowId(window), orientation.into(), split_ratio)?)
    }
    pub fn swap_panes(&self, window: u64) -> R<()> {
        Ok(self.inner.swap_panes(core::WindowId(window))?)
    }
    pub fn save_workspace(&self, name: String) -> R<u64> {
        Ok(self.inner.save_workspace(&name)?.0)
    }
    pub fn saved_workspaces(&self) -> R<Vec<FfiWorkspaceSummary>> {
        Ok(self
            .inner
            .saved_workspaces()?
            .into_iter()
            .map(Into::into)
            .collect())
    }
    pub fn open_workspace(&self, id: u64) -> R<FfiWorkspaceState> {
        Ok(self.inner.open_workspace(core::WorkspaceId(id))?.into())
    }
    pub fn rename_workspace(&self, id: u64, name: String) -> R<()> {
        Ok(self.inner.rename_workspace(core::WorkspaceId(id), &name)?)
    }
    pub fn delete_workspace(&self, id: u64) -> R<()> {
        Ok(self.inner.delete_workspace(core::WorkspaceId(id))?)
    }

    // ── commands and keys ──

    /// What a key press means; `None` means "not ours", let the focused
    /// control handle it.
    pub fn handle_key(&self, chord: FfiKeyChord, ctx: FfiInputContext) -> Option<FfiOutcome> {
        let c = chord.to_core()?;
        self.inner.handle_key(c, ctx.into()).map(Into::into)
    }
    /// Call from a ~100 ms timer while a key sequence is pending.
    pub fn poll_keys(&self, ctx: FfiInputContext) -> Option<FfiOutcome> {
        self.inner.poll_keys(ctx.into()).map(Into::into)
    }
    pub fn pending_keys(&self) -> Option<String> {
        self.inner.pending_keys()
    }
    /// Runs a command.
    pub fn perform(&self, inv: FfiInvocation, via: FfiVia) -> R<FfiOutcome> {
        Ok(self.inner.perform(inv.to_core(), via.into())?.into())
    }
    /// Every registered command with its current shortcut.
    pub fn commands(&self) -> Vec<FfiCommandInfo> {
        self.inner
            .commands()
            .into_iter()
            .map(|c| FfiCommandInfo {
                handling: Engine::handling(&c.id).into(),
                id: c.id,
                title: c.title,
                category: c.category,
                description: c.description,
                shortcut: c.shortcut,
                shows_in_menu: c.shows_in_menu,
                shows_in_palette: c.shows_in_palette,
                customized: c.customized,
            })
            .collect()
    }
    /// The overlay rows for held modifiers.
    pub fn held_shortcuts(&self, mods: FfiMods, ctx: FfiInputContext) -> Option<FfiHeldShortcuts> {
        self.inner
            .held_shortcuts(mods.into(), ctx.into())
            .map(Into::into)
    }
    /// Records a command run outside `perform`; may return a hint.
    pub fn hint_for(&self, command: String, via: FfiVia) -> Option<FfiHint> {
        self.inner.hint_for(&command, via.into()).map(Into::into)
    }
    pub fn vim_mode(&self) -> FfiVimMode {
        self.inner.vim_mode().into()
    }
    pub fn keymap_conflicts(&self) -> Vec<String> {
        self.inner.keymap_conflicts()
    }
    pub fn command_keys(&self, command: String) -> Vec<String> {
        self.inner.command_keys(&command)
    }
    /// Sets or clears a command's primary/secondary key (key-cap text).
    pub fn set_command_key(&self, command: String, primary: bool, keys: Option<String>) -> R<()> {
        Ok(self
            .inner
            .set_command_key(&command, primary, keys.as_deref())?)
    }
    /// Resets one command's keys, or all with `None`.
    pub fn reset_command_keys(&self, command: Option<String>) -> R<()> {
        Ok(self.inner.reset_command_keys(command.as_deref())?)
    }
    pub fn export_keymap(&self) -> String {
        self.inner.export_keymap()
    }
    /// Returns warnings.
    pub fn import_keymap(&self, json: String) -> R<Vec<String>> {
        Ok(self.inner.import_keymap(&json)?)
    }
    /// Validates a name typed in place and queues the rename.
    pub fn rename_item(&self, tab: u64, old_name: String, new_name: String) -> R<FfiOutcome> {
        Ok(self
            .inner
            .rename_item(tab_id(tab), &old_name, &new_name)?
            .into())
    }
    /// Creates a missing folder (through the queue) and shows it.
    pub fn create_folder_and_go(&self, tab: u64, path: String) -> R<FfiOutcome> {
        Ok(self.inner.create_folder_and_go(tab_id(tab), &path)?.into())
    }
    /// What dropping `sources` onto the folder `dest_dir` with these
    /// modifiers would do (for the drag cursor). Does `lstat` on each source.
    pub fn drop_action(
        &self,
        sources: Vec<String>,
        dest_dir: String,
        mods: FfiMods,
    ) -> FfiDropAction {
        let src: Vec<std::path::PathBuf> = sources.into_iter().map(Into::into).collect();
        self.inner
            .drop_action(&src, std::path::Path::new(&dest_dir), mods.into())
            .into()
    }
    /// Drops `sources` onto `dest_dir`: queues the copy or move the core decides.
    pub fn drop_files(
        &self,
        sources: Vec<String>,
        dest_dir: String,
        mods: FfiMods,
    ) -> R<FfiOutcome> {
        let src = sources.into_iter().map(Into::into).collect();
        Ok(self
            .inner
            .drop_files(src, dest_dir.into(), mods.into())?
            .into())
    }
    /// Files copied in from another app (or cut, pending a move on paste).
    pub fn set_clipboard(&self, paths: Vec<String>, cut: bool) {
        self.inner.set_clipboard(paths, cut);
    }
    /// The core clipboard's paths (empty when nothing is copied).
    pub fn clipboard_paths(&self) -> Vec<String> {
        self.inner.clipboard().map(|c| c.0).unwrap_or_default()
    }

    // ── context menus ──

    /// Applies Finder's right-click rule (`row = None` is empty space) and
    /// returns what the click is on.
    pub fn menu_target_at(&self, tab: u64, row: Option<u32>) -> R<FfiMenuTarget> {
        Ok(self.inner.menu_target_at(tab_id(tab), row)?.into())
    }
    /// The context menu for a target. Reads Finder tags (an xattr) of the
    /// target items for the Tags submenu.
    pub fn context_menu(&self, target: FfiMenuTarget, ctx: FfiMenuContext) -> R<Vec<FfiMenuItem>> {
        Ok(self
            .inner
            .context_menu(
                target.into(),
                core::MenuContext {
                    pasteboard_has_files: ctx.pasteboard_has_files,
                },
            )?
            .into_iter()
            .map(Into::into)
            .collect())
    }
    pub fn menu_entries(&self, kind: FfiMenuKind) -> Vec<FfiMenuEntry> {
        self.inner
            .menu_entries(kind.into())
            .into_iter()
            .map(Into::into)
            .collect()
    }
    pub fn menu_choices(&self, kind: FfiMenuKind) -> Vec<FfiMenuEntry> {
        self.inner
            .menu_choices(kind.into())
            .into_iter()
            .map(Into::into)
            .collect()
    }
    pub fn menu_add(&self, kind: FfiMenuKind, entry: String, after: Option<String>) -> R<()> {
        Ok(self.inner.menu_add(kind.into(), &entry, after)?)
    }
    pub fn menu_remove(&self, kind: FfiMenuKind, entry: String) -> R<()> {
        Ok(self.inner.menu_remove(kind.into(), &entry)?)
    }
    pub fn menu_move(&self, kind: FfiMenuKind, entry: String, after: Option<String>) -> R<()> {
        Ok(self.inner.menu_move(kind.into(), &entry, after)?)
    }
    pub fn reset_menu(&self, kind: FfiMenuKind) -> R<()> {
        Ok(self.inner.reset_menu(kind.into())?)
    }
    pub fn reset_menus(&self) -> R<()> {
        Ok(self.inner.reset_menus()?)
    }

    // ── path bar ──

    /// Parses typed or pasted text against `cwd`. Does `stat`-like checks.
    pub fn parse_location(&self, raw: String, cwd: String) -> FfiParsedLocation {
        self.inner.parse_location(&raw, &cwd).into()
    }
    pub fn breadcrumbs(&self, path: String) -> Vec<FfiCrumb> {
        self.inner
            .breadcrumbs(&path)
            .into_iter()
            .map(Into::into)
            .collect()
    }
    /// A folder's subfolders, for a crumb's chevron menu. Lists the folder.
    pub fn subfolders(&self, path: String) -> Vec<FfiCrumb> {
        self.inner
            .subfolders(&path)
            .into_iter()
            .map(Into::into)
            .collect()
    }
    pub fn complete(&self, input: String, cursor: u32, cwd: String) -> FfiCompletions {
        self.inner.complete(&input, cursor, &cwd).into()
    }
    pub fn copy_path_as(&self, path: String, style: FfiPathStyle) -> String {
        self.inner.copy_path_as(&path, style.into())
    }
    pub fn frecent(&self, query: String, limit: u32) -> Vec<FfiFrecentDir> {
        self.inner
            .frecent(&query, limit)
            .into_iter()
            .map(|f| FfiFrecentDir {
                path: f.path,
                score: f.score,
                last_accessed: f.last_accessed,
            })
            .collect()
    }
    /// Imports zoxide's database once; returns (added, merged).
    pub fn import_zoxide(&self, db_path: Option<String>) -> R<Vec<u32>> {
        let (a, b) = self.inner.import_zoxide(db_path)?;
        Ok(vec![a, b])
    }

    // ── operations ──

    /// Queues an operation (drag and drop).
    pub fn enqueue(&self, req: FfiOpRequest) -> R<i64> {
        Ok(self.inner.enqueue(req.into())?.0)
    }
    /// Every op of this session, in queue order.
    pub fn ops_list(&self) -> Vec<FfiOpStatus> {
        self.inner
            .ops()
            .list()
            .into_iter()
            .map(Into::into)
            .collect()
    }
    pub fn op_status(&self, id: i64) -> Option<FfiOpStatus> {
        self.inner.ops().status(core::OpId(id)).map(Into::into)
    }
    pub fn op_pause(&self, id: i64) -> R<()> {
        Ok(self.inner.ops().pause(core::OpId(id))?)
    }
    /// Resumes a paused op, or an interrupted one from an earlier run.
    pub fn op_resume(&self, id: i64) -> R<()> {
        Ok(self.inner.ops().resume(core::OpId(id))?)
    }
    /// Cancels an op (interrupted ones too, without resuming them).
    pub fn op_cancel(&self, id: i64) -> R<()> {
        Ok(self.inner.ops().cancel(core::OpId(id))?)
    }
    pub fn op_reorder(&self, id: i64, new_index: u32) -> R<()> {
        Ok(self
            .inner
            .ops()
            .reorder(core::OpId(id), new_index as usize)?)
    }
    pub fn resolve_conflict(
        &self,
        op: i64,
        conflict: i64,
        decision: FfiConflictDecision,
        apply_to_all: bool,
    ) -> R<()> {
        Ok(self.inner.ops().resolve_conflict(
            core::OpId(op),
            core::ConflictId(conflict),
            decision.into(),
            apply_to_all,
        )?)
    }
    /// Ops left unfinished by an earlier run, offered for resume. Reads the journal.
    pub fn interrupted_ops(&self) -> R<Vec<FfiOpStatus>> {
        Ok(self
            .inner
            .ops()
            .interrupted()?
            .into_iter()
            .map(Into::into)
            .collect())
    }
    pub fn clear_finished_ops(&self) {
        self.inner.ops().clear_finished();
    }
    pub fn undo(&self) -> R<FfiOutcome> {
        Ok(self.inner.undo()?.into())
    }
    pub fn redo(&self) -> R<FfiOutcome> {
        Ok(self.inner.redo()?.into())
    }
    pub fn undo_state(&self) -> FfiUndoState {
        self.inner.undo_state().into()
    }

    // ── sidebar ──

    pub fn volumes(&self) -> Vec<FfiVolume> {
        self.inner.volumes().into_iter().map(Into::into).collect()
    }
    pub fn favourites(&self) -> Vec<FfiFavourite> {
        self.inner
            .favourites()
            .into_iter()
            .map(Into::into)
            .collect()
    }
    pub fn add_favourite(&self, path: String, index: Option<u32>) -> R<()> {
        Ok(self.inner.add_favourite(&path, index)?)
    }
    pub fn remove_favourite(&self, path: String) -> R<()> {
        Ok(self.inner.remove_favourite(&path)?)
    }
    pub fn rename_favourite(&self, path: String, name: String) -> R<()> {
        Ok(self.inner.rename_favourite(&path, &name)?)
    }
    pub fn move_favourite(&self, path: String, index: u32) -> R<()> {
        Ok(self.inner.move_favourite(&path, index)?)
    }

    // ── OS hooks ──

    /// A volume was mounted or unmounted.
    pub fn volumes_changed(&self) {
        self.inner.volumes_changed();
    }
    /// The app is quitting: save the session and flush.
    pub fn app_will_quit(&self) {
        self.inner.app_will_quit();
    }
    /// Delivers held events now (tests and scripts).
    pub fn flush_events(&self) {
        self.inner.flush_events();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Collect(Mutex<Vec<FfiEvent>>);
    impl FfiEventSink for Collect {
        fn on_event(&self, e: FfiEvent) {
            self.0.lock().unwrap().push(e);
        }
    }

    fn engine(dir: &std::path::Path) -> (Arc<FfiEngine>, Arc<Collect>) {
        let sink = Arc::new(Collect(Mutex::new(Vec::new())));
        std::fs::create_dir_all(dir.join("home")).unwrap();
        std::fs::create_dir_all(dir.join("trash")).unwrap();
        let e = FfiEngine::new(
            FfiEngineConfig {
                data_dir: ps(&dir.join("data")),
                home: Some(ps(&dir.join("home"))),
                trash_dir: Some(ps(&dir.join("trash"))),
                poll_watcher: false,
            },
            sink.clone(),
        )
        .unwrap();
        (e, sink)
    }

    /// A reference decoder for `rows_packed` (the Swift one mirrors it).
    fn unpack(b: &[u8]) -> Vec<FfiRow> {
        let mut at = 0usize;
        let mut take = |n: usize| {
            let s = &b[at..at + n];
            at += n;
            s
        };
        let n = u32::from_le_bytes(take(4).try_into().unwrap());
        (0..n)
            .map(|_| {
                let index = u32::from_le_bytes(take(4).try_into().unwrap());
                let kind = [
                    FfiRowKind::Folder,
                    FfiRowKind::Package,
                    FfiRowKind::File,
                    FfiRowKind::Symlink,
                    FfiRowKind::Other,
                ][take(1)[0] as usize];
                let f = take(1)[0];
                let size = u64::from_le_bytes(take(8).try_into().unwrap());
                let modified_ns = i64::from_le_bytes(take(8).try_into().unwrap());
                let mut s = || {
                    let len = u32::from_le_bytes(take(4).try_into().unwrap()) as usize;
                    String::from_utf8(take(len).to_vec()).unwrap()
                };
                let (name, icon_key, kind_text, size_text, modified_text) =
                    (s(), s(), s(), s(), s());
                FfiRow {
                    index,
                    name,
                    kind,
                    icon_key,
                    kind_text,
                    size,
                    size_text,
                    modified_ns,
                    modified_text,
                    is_hidden: f & 1 != 0,
                    is_dataless: f & 2 != 0,
                    is_package: f & 4 != 0,
                    is_symlink: f & 8 != 0,
                    is_selected: f & 16 != 0,
                    is_cursor: f & 32 != 0,
                    is_cut_pending: f & 64 != 0,
                }
            })
            .collect()
    }

    fn wait_sorted(t: &FfiTab) {
        let start = std::time::Instant::now();
        while t.listing_info().state != FfiListingState::Complete {
            assert!(start.elapsed().as_secs() < 10, "listing never completed");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn round_trip_through_the_ffi_types() {
        let d = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(d.path()).unwrap();
        let (e, sink) = engine(&base);
        for i in 0..100 {
            std::fs::write(base.join(format!("home/f{i:03}.txt")), b"x").unwrap();
        }
        std::fs::create_dir(base.join("home/Café")).unwrap();
        e.restore_session();
        let t = e.active_tab().unwrap();
        t.go(ps(&base.join("home"))).unwrap();
        wait_sorted(&t);
        let rows = t.rows(0, 60);
        assert_eq!(rows.len(), 60);
        assert_eq!(rows[0].name, "Café", "folders first, name as on disk");
        let packed = t.rows_packed(0, 60);
        assert_eq!(unpack(&packed), rows, "packed rows match the records");
        // Keys and commands.
        let out = e
            .handle_key(
                FfiKeyChord {
                    key: FfiKey::Char { value: "a".into() },
                    mods: FfiMods {
                        cmd: true,
                        ..Default::default()
                    },
                },
                FfiInputContext {
                    focus: FfiFocus::List,
                    tab: Some(t.id()),
                },
            )
            .unwrap();
        assert_eq!(out.command.as_deref(), Some("select.all"));
        assert_eq!(t.selection().rows.len(), 101);
        // Context menus with nested submenus cross intact.
        let target = e.menu_target_at(t.id(), Some(1)).unwrap();
        assert!(matches!(target, FfiMenuTarget::Multi { .. }));
        let target = FfiMenuTarget::File {
            tab: t.id(),
            path: ps(&base.join("home/f000.txt")),
        };
        let m = e.context_menu(target, FfiMenuContext::default()).unwrap();
        let cp = m.iter().find(|i| i.entry == "menu.copy_path").unwrap();
        assert_eq!(cp.kind, FfiMenuItemKind::Submenu);
        assert!(cp.children.len() >= 5);
        // Settings survive the round trip.
        let mut s = e.settings();
        s.general.terminal_app = FfiTerminalApp::Ghostty;
        e.update_settings(s.clone()).unwrap();
        assert_eq!(e.settings(), s);
        e.flush_events();
        assert!(
            sink.0
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, FfiEvent::ListingChanged { .. }))
        );
    }

    /// The per-call cost of `rows(start, 60)` through the FFI types (the
    /// Swift side adds the buffer (de)serialisation measured in the app).
    #[test]
    #[ignore = "timing; run with --ignored --nocapture"]
    fn rows_cost() {
        let d = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(d.path()).unwrap();
        let (e, _sink) = engine(&base);
        for i in 0..10_000 {
            std::fs::write(base.join(format!("home/file {i:05}.txt")), b"x").unwrap();
        }
        e.restore_session();
        let t = e.active_tab().unwrap();
        t.go(ps(&base.join("home"))).unwrap();
        wait_sorted(&t);
        let n = 5_000;
        let start = std::time::Instant::now();
        for k in 0..n {
            let r = t.rows((k * 37) % 9_000, 60);
            assert_eq!(r.len(), 60);
        }
        let per = start.elapsed() / n;
        println!("rows(start, 60) via FFI types: {per:?} per call");
    }
}

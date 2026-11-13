//! Live workspaces: separate setups ("Everyday", "Photo work") that each keep
//! their own windows, panes, tabs, history, view options and layout (F-002).
//!
//! Exactly one workspace is active: the open windows *are* its live state,
//! saved with the session. Every other workspace keeps its state in
//! `core_workspaces` until it is switched to. A switch saves the active
//! workspace's state, then loads the other one, so no tab is ever lost by
//! switching (the way browser profiles work).
//!
//! Workspaces only hold layouts. Nothing here reads, moves or deletes a file
//! (F-007): deleting a workspace forgets its saved layout and nothing else.

use std::collections::HashMap;

use fstudio_db::Migrations;
use fstudio_db::rusqlite::{self, OptionalExtension, params};
use fstudio_keymap::{CommandArg, CommandId};

use crate::confirm::PendingAction;
use crate::engine::{IdMode, now_ns};
use crate::error::{CoreError, Result};
use crate::events::CoreEvent;
use crate::ids::{PaneId, WindowId, WorkspaceId};
use crate::outcome::{Outcome, ShellRequest};
use crate::workspace::{
    Closed, DEFAULT_SIDEBAR_WIDTH, Orientation, PaneState, WindowState, WorkspaceState,
    WorkspaceSummary, clamp_sidebar,
};

/// The workspace columns added after `core_workspaces` was first created.
/// A module of its own, so the core's first migration stays as it shipped.
pub(crate) const MIGRATIONS: Migrations = Migrations {
    module: "core.workspaces",
    steps: &[(
        1,
        "ALTER TABLE core_workspaces ADD COLUMN icon TEXT NOT NULL DEFAULT 'square.grid.2x2';
         ALTER TABLE core_workspaces ADD COLUMN position INTEGER NOT NULL DEFAULT 0;
         UPDATE core_workspaces SET position = (
             SELECT COUNT(*) FROM core_workspaces AS o
             WHERE o.name < core_workspaces.name COLLATE NOCASE
                OR (o.name = core_workspaces.name COLLATE NOCASE AND o.id < core_workspaces.id)
         );",
    )],
};

/// The key holding the active workspace's id.
const ACTIVE_KEY: &str = "core.workspaces.active";

/// The first workspace's name.
pub const DEFAULT_WORKSPACE_NAME: &str = "Everyday";
/// The first workspace's icon.
const DEFAULT_WORKSPACE_ICON: &str = "house";
/// The icon a workspace gets when none is chosen.
const FALLBACK_ICON: &str = "square.grid.2x2";
/// The longest name, in characters.
const MAX_NAME_CHARS: usize = 60;

/// The SF Symbols offered for workspaces, in the order the picker shows them.
pub const WORKSPACE_ICONS: &[&str] = &[
    "house",
    "sun.max",
    "camera",
    "photo.on.rectangle",
    "film",
    "music.note",
    "paintbrush",
    "briefcase",
    "doc.text",
    "books.vertical",
    "hammer",
    "chevron.left.forwardslash.chevron.right",
    "terminal",
    "folder",
    "tray.full",
    "star",
    "heart",
    "leaf",
    "airplane",
    "gamecontroller",
    "square.grid.2x2",
];

/// How a new workspace starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NewWorkspace {
    /// One window with one tab at the home folder.
    #[default]
    Empty,
    /// A copy of the active workspace's windows, tabs and layout.
    CopyCurrent,
}

/// Engine-side memory for workspaces: which one is active and, per
/// workspace, the tabs closed in it (for "Reopen Closed Tab").
#[derive(Default)]
pub(crate) struct Live {
    pub(crate) active: Option<WorkspaceId>,
    closed: HashMap<WorkspaceId, Vec<Closed>>,
}

/// One row of `core_workspaces`.
#[derive(Debug, Clone)]
struct Row {
    id: WorkspaceId,
    name: String,
    icon: String,
    state: String,
}

// ───────────────────────── pure rules ─────────────────────────

/// Checks a typed workspace name. Keeps it exactly as typed apart from
/// surrounding white space (F-017); refuses empty, overlong and multi-line
/// names.
pub(crate) fn clean_name(raw: &str) -> Result<String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(CoreError::InvalidName(
            "A workspace needs a name.".to_string(),
        ));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(CoreError::InvalidName(format!(
            "Workspace names can have at most {MAX_NAME_CHARS} characters."
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(CoreError::InvalidName(
            "Workspace names can't contain line breaks or tabs.".to_string(),
        ));
    }
    Ok(name.to_string())
}

/// True when `icon` looks like an SF Symbol name (`photo.on.rectangle`).
pub(crate) fn icon_ok(icon: &str) -> bool {
    !icon.is_empty()
        && icon.len() <= 64
        && icon
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.')
        && !icon.starts_with('.')
        && !icon.ends_with('.')
}

/// The icon to store: the given one if it is a symbol name, else the default.
fn clean_icon(icon: &str) -> String {
    let icon = icon.trim();
    if icon.is_empty() {
        FALLBACK_ICON.to_string()
    } else if icon_ok(icon) {
        icon.to_string()
    } else {
        FALLBACK_ICON.to_string()
    }
}

/// True when `name` is already used by a workspace other than `except`,
/// comparing NFC + case-folded (so "Photo Work" and "photo work" clash).
fn name_taken<'a>(
    mut names: impl Iterator<Item = (WorkspaceId, &'a str)>,
    name: &str,
    except: Option<WorkspaceId>,
) -> bool {
    let key = fstudio_path::keys::fold(name);
    names.any(|(id, n)| Some(id) != except && fstudio_path::keys::fold(n) == key)
}

/// `base`, or `base 2`, `base 3`… — the first one no workspace uses.
fn unique_name<'a>(
    names: impl Iterator<Item = (WorkspaceId, &'a str)> + Clone,
    base: &str,
) -> String {
    if !name_taken(names.clone(), base, None) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base} {n}"))
        .find(|n| !name_taken(names.clone(), n, None))
        .expect("an unused number exists")
}

/// The neighbour to show when the workspace at `index` of `len` goes away:
/// the next one, or the previous one when it was last.
fn neighbour(index: usize, len: usize) -> Option<usize> {
    if len < 2 {
        None
    } else if index + 1 < len {
        Some(index + 1)
    } else {
        Some(index - 1)
    }
}

/// The index `delta` steps from `index`, wrapping around `len`.
fn cycle(index: usize, len: usize, delta: i64) -> usize {
    let len = len as i64;
    (((index as i64 + delta) % len + len) % len) as usize
}

// ───────────────────────── storage ─────────────────────────

fn read_rows(c: &rusqlite::Connection) -> rusqlite::Result<Vec<Row>> {
    let mut st =
        c.prepare("SELECT id, name, icon, state FROM core_workspaces ORDER BY position, id")?;
    st.query_map([], |r| {
        Ok(Row {
            id: WorkspaceId(r.get::<_, i64>(0)? as u64),
            name: r.get(1)?,
            icon: r.get(2)?,
            state: r.get(3)?,
        })
    })?
    .collect()
}

/// Gives the rows positions 0, 1, 2… in the given order.
fn renumber(c: &rusqlite::Connection, order: &[WorkspaceId]) -> rusqlite::Result<()> {
    for (i, id) in order.iter().enumerate() {
        c.execute(
            "UPDATE core_workspaces SET position = ?1 WHERE id = ?2",
            params![i as i64, id.0 as i64],
        )?;
    }
    Ok(())
}

/// Inserts a workspace at `index` (clamped), moving later ones down.
fn insert_row(
    c: &mut rusqlite::Connection,
    name: &str,
    icon: &str,
    state: &str,
    index: usize,
) -> rusqlite::Result<WorkspaceId> {
    let tx = c.transaction()?;
    let mut order: Vec<WorkspaceId> = read_rows(&tx)?.into_iter().map(|r| r.id).collect();
    let now = now_ns();
    tx.execute(
        "INSERT INTO core_workspaces(name, icon, state, position, created_ns, updated_ns)
         VALUES (?1, ?2, ?3, 0, ?4, ?4)",
        params![name, icon, state, now],
    )?;
    let id = WorkspaceId(tx.last_insert_rowid() as u64);
    order.insert(index.min(order.len()), id);
    renumber(&tx, &order)?;
    tx.commit()?;
    Ok(id)
}

fn names(rows: &[Row]) -> impl Iterator<Item = (WorkspaceId, &str)> + Clone {
    rows.iter().map(|r| (r.id, r.name.as_str()))
}

// ───────────────────────── engine API ─────────────────────────

impl crate::Engine {
    fn rows(&self) -> Result<Vec<Row>> {
        Ok(self.db.with(|c| read_rows(c))?)
    }

    pub(crate) fn workspaces_changed(&self) {
        self.shared.emit(CoreEvent::WorkspacesChanged);
    }

    /// The active workspace from the database, creating "Everyday" when
    /// there is none (first launch, or an upgrade from saved snapshots). Also
    /// returns the active workspace's stored state, when it has one.
    pub(crate) fn active_workspace_or_create(
        &self,
        live: &mut Live,
    ) -> (Option<WorkspaceId>, Option<WorkspaceState>) {
        let stored: Option<(WorkspaceId, String)> = self
            .db
            .kv_get(ACTIVE_KEY)
            .ok()
            .flatten()
            .and_then(|t| t.parse::<u64>().ok())
            .and_then(|id| {
                self.db
                    .with(|c| {
                        c.query_row(
                            "SELECT state FROM core_workspaces WHERE id = ?1",
                            [id as i64],
                            |r| r.get::<_, String>(0),
                        )
                        .optional()
                    })
                    .ok()
                    .flatten()
                    .map(|s| (WorkspaceId(id), s))
            });
        if let Some((id, state)) = stored {
            live.active = Some(id);
            let ws = serde_json::from_str::<WorkspaceState>(&state).ok();
            return (Some(id), ws.filter(|w| w.tab_count() > 0));
        }
        // First launch: the session (whatever it is) becomes "Everyday".
        let session = self
            .db
            .kv_get("core.session")
            .ok()
            .flatten()
            .unwrap_or_else(|| "{\"windows\":[],\"active_window\":null}".to_string());
        match self.create_row_first(DEFAULT_WORKSPACE_NAME, DEFAULT_WORKSPACE_ICON, &session) {
            Ok(id) => {
                live.active = Some(id);
                (Some(id), None)
            }
            Err(_) => (None, None),
        }
    }

    /// Adds a workspace in first place and makes it the active one.
    fn create_row_first(&self, base: &str, icon: &str, state: &str) -> Result<WorkspaceId> {
        let rows = self.rows()?;
        let name = unique_name(names(&rows), base);
        let id = self.db.with(|c| insert_row(c, &name, icon, state, 0))?;
        self.db.kv_set(ACTIVE_KEY, &id.0.to_string())?;
        Ok(id)
    }

    /// The active workspace, creating "Everyday" from the open windows when
    /// there is none yet (the CLI and tests never restore a session).
    fn ensure_active(&self, live: &mut Live) -> Result<WorkspaceId> {
        if let Some(id) = live.active
            && self.rows()?.iter().any(|r| r.id == id)
        {
            return Ok(id);
        }
        let (id, _) = self.active_workspace_or_create(live);
        let id = id.ok_or_else(|| CoreError::NotAvailable("workspaces are unavailable".into()))?;
        // The windows on screen are this workspace's state from now on.
        let json = serde_json::to_string(&self.workspace_state())?;
        self.db.with(|c| {
            c.execute(
                "UPDATE core_workspaces SET state = ?1 WHERE id = ?2",
                params![json, id.0 as i64],
            )
        })?;
        Ok(id)
    }

    fn summaries(&self, rows: Vec<Row>, active: WorkspaceId) -> Vec<WorkspaceSummary> {
        let live = self.workspace_state();
        let switch_keys = self
            .keymap
            .lock()
            .bindings_for(&CommandId::new("workspace.switch"));
        rows.into_iter()
            .enumerate()
            .map(|(i, r)| {
                let ws = if r.id == active {
                    live.clone()
                } else {
                    serde_json::from_str(&r.state).unwrap_or_default()
                };
                WorkspaceSummary {
                    id: r.id,
                    name: r.name,
                    icon: r.icon,
                    position: i as u32,
                    active: r.id == active,
                    shortcut: switch_keys
                        .iter()
                        .find(|b| b.arg == Some(CommandArg::Index(i)))
                        .map(|b| b.text()),
                    windows: ws.windows.len() as u32,
                    tabs: ws.tab_count() as u32,
                    updated_ns: 0,
                }
            })
            .collect()
    }

    /// Every workspace, in switcher order. There is always at least one.
    pub fn workspaces(&self) -> Result<Vec<WorkspaceSummary>> {
        let mut live = self.live.lock();
        let active = self.ensure_active(&mut live)?;
        let rows = self.rows()?;
        drop(live);
        Ok(self.summaries(rows, active))
    }

    /// The workspace on screen now.
    pub fn active_workspace(&self) -> Result<WorkspaceSummary> {
        self.workspaces()?
            .into_iter()
            .find(|w| w.active)
            .ok_or_else(|| CoreError::NotAvailable("no active workspace".into()))
    }

    /// The SF Symbols the "New Workspace" sheet offers.
    pub fn workspace_icons(&self) -> Vec<String> {
        WORKSPACE_ICONS.iter().map(|s| s.to_string()).collect()
    }

    /// A workspace with one window holding one tab at the home folder.
    fn empty_workspace(&self) -> Result<WorkspaceState> {
        let home = self.shared.home.to_string_lossy().into_owned();
        let tab = self.tab_state_at(Some(&home))?;
        let sidebar = self.focused_sidebar();
        Ok(WorkspaceState {
            windows: vec![WindowState {
                id: WindowId(0),
                panes: vec![PaneState {
                    id: PaneId(0),
                    tabs: vec![tab],
                    active_tab: 0,
                }],
                active_pane: 0,
                orientation: Orientation::Horizontal,
                split_ratio: 0.5,
                hidden_pane: None,
                sidebar_width: sidebar.0,
                sidebar_hidden: sidebar.1,
            }],
            active_window: Some(WindowId(0)),
        })
    }

    fn focused_sidebar(&self) -> (f64, bool) {
        let m = self.model.lock();
        m.active_window_index()
            .map(|wi| (m.windows[wi].sidebar_width, m.windows[wi].sidebar_hidden))
            .unwrap_or((DEFAULT_SIDEBAR_WIDTH, false))
    }

    /// Creates a workspace after the active one and switches to it. The name
    /// must not match another workspace's, ignoring case and Unicode form.
    pub fn create_workspace(
        &self,
        name: &str,
        icon: &str,
        start: NewWorkspace,
    ) -> Result<WorkspaceId> {
        let name = clean_name(name)?;
        let icon = clean_icon(icon);
        let id = {
            let mut live = self.live.lock();
            let active = self.ensure_active(&mut live)?;
            let rows = self.rows()?;
            if name_taken(names(&rows), &name, None) {
                return Err(CoreError::InvalidName(format!(
                    "There's already a workspace called “{name}”."
                )));
            }
            let state = match start {
                NewWorkspace::Empty => self.empty_workspace()?,
                NewWorkspace::CopyCurrent => self.workspace_state(),
            };
            let json = serde_json::to_string(&state)?;
            let after = rows
                .iter()
                .position(|r| r.id == active)
                .map_or(rows.len(), |i| i + 1);
            self.db
                .with(|c| insert_row(c, &name, &icon, &json, after))?
        };
        self.switch_workspace(id)?;
        Ok(id)
    }

    /// Saves the active workspace's windows, then shows workspace `id`'s
    /// windows, tabs and layout in their place. Switching to the active
    /// workspace does nothing.
    pub fn switch_workspace(&self, id: WorkspaceId) -> Result<WorkspaceState> {
        let mut live = self.live.lock();
        let current = self.ensure_active(&mut live)?;
        if current == id {
            return Ok(self.workspace_state());
        }
        let target: String = self
            .db
            .with(|c| {
                c.query_row(
                    "SELECT state FROM core_workspaces WHERE id = ?1",
                    [id.0 as i64],
                    |r| r.get(0),
                )
                .optional()
            })?
            .ok_or_else(|| CoreError::NotFound(format!("workspace {id}")))?;
        let target: WorkspaceState = serde_json::from_str(&target).unwrap_or_default();
        let now = self.workspace_state();
        let now_json = serde_json::to_string(&now)?;
        // The old workspace's state and the new active id land together, so
        // a crash at any point restores one consistent workspace.
        self.db.with(|c| {
            let tx = c.transaction()?;
            tx.execute(
                "UPDATE core_workspaces SET state = ?1, updated_ns = ?2 WHERE id = ?3",
                params![now_json, now_ns(), current.0 as i64],
            )?;
            tx.execute(
                "INSERT INTO db_kv(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![ACTIVE_KEY, id.0.to_string()],
            )?;
            tx.commit()
        })?;
        let closed = std::mem::take(&mut self.model.lock().closed);
        live.closed.insert(current, closed);
        live.active = Some(id);
        // The shell's windows stay open and show the new workspace.
        let windows: Vec<WindowId> = now.windows.iter().map(|w| w.id).collect();
        self.load_workspace(target, IdMode::Reuse(windows));
        self.model.lock().closed = live.closed.remove(&id).unwrap_or_default();
        self.save_session_locked(&live, Some(id));
        drop(live);
        self.workspaces_changed();
        Ok(self.workspace_state())
    }

    /// Switches to the workspace at `index` in switcher order.
    pub fn switch_workspace_at(&self, index: usize) -> Result<WorkspaceSummary> {
        let list = self.workspaces()?;
        let w = list
            .get(index)
            .cloned()
            .ok_or_else(|| CoreError::NotFound(format!("workspace {}", index + 1)))?;
        self.switch_workspace(w.id)?;
        Ok(w)
    }

    /// Switches `delta` places along the switcher, wrapping around.
    pub fn switch_workspace_by(&self, delta: i64) -> Result<WorkspaceSummary> {
        let list = self.workspaces()?;
        let at = list.iter().position(|w| w.active).unwrap_or(0);
        if list.len() < 2 {
            return Err(CoreError::NotAvailable(
                "There's only one workspace.".into(),
            ));
        }
        self.switch_workspace_at(cycle(at, list.len(), delta))
    }

    /// Renames a workspace. Names follow the rules of [`Self::create_workspace`].
    pub fn rename_workspace(&self, id: WorkspaceId, name: &str) -> Result<()> {
        let name = clean_name(name)?;
        {
            let mut live = self.live.lock();
            self.ensure_active(&mut live)?;
            let rows = self.rows()?;
            if !rows.iter().any(|r| r.id == id) {
                return Err(CoreError::NotFound(format!("workspace {id}")));
            }
            if name_taken(names(&rows), &name, Some(id)) {
                return Err(CoreError::InvalidName(format!(
                    "There's already a workspace called “{name}”."
                )));
            }
            self.db.with(|c| {
                c.execute(
                    "UPDATE core_workspaces SET name = ?1 WHERE id = ?2",
                    params![name, id.0 as i64],
                )
            })?;
        }
        self.workspaces_changed();
        Ok(())
    }

    /// Gives a workspace another SF Symbol. Anything that isn't a symbol
    /// name falls back to the default icon.
    pub fn set_workspace_icon(&self, id: WorkspaceId, icon: &str) -> Result<()> {
        let n = self.db.with(|c| {
            c.execute(
                "UPDATE core_workspaces SET icon = ?1 WHERE id = ?2",
                params![clean_icon(icon), id.0 as i64],
            )
        })?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("workspace {id}")));
        }
        self.workspaces_changed();
        Ok(())
    }

    /// Moves a workspace to `index` in the switcher (clamped). The ⌃⌘1…
    /// shortcuts follow the order.
    pub fn move_workspace(&self, id: WorkspaceId, index: u32) -> Result<()> {
        {
            let _live = self.live.lock();
            self.db
                .with(|c| {
                    let tx = c.transaction()?;
                    let mut order: Vec<WorkspaceId> =
                        read_rows(&tx)?.into_iter().map(|r| r.id).collect();
                    let Some(from) = order.iter().position(|x| *x == id) else {
                        return Ok(false);
                    };
                    order.remove(from);
                    order.insert((index as usize).min(order.len()), id);
                    renumber(&tx, &order)?;
                    tx.commit()?;
                    Ok(true)
                })?
                .then_some(())
                .ok_or_else(|| CoreError::NotFound(format!("workspace {id}")))?;
        }
        self.workspaces_changed();
        Ok(())
    }

    /// Copies a workspace (its current tabs and layout) to a new one right
    /// after it, named "… copy". Stays on the active workspace.
    pub fn duplicate_workspace(&self, id: WorkspaceId) -> Result<WorkspaceId> {
        let new = {
            let mut live = self.live.lock();
            let active = self.ensure_active(&mut live)?;
            let rows = self.rows()?;
            let at = rows
                .iter()
                .position(|r| r.id == id)
                .ok_or_else(|| CoreError::NotFound(format!("workspace {id}")))?;
            let src = &rows[at];
            let state = if id == active {
                serde_json::to_string(&self.workspace_state())?
            } else {
                src.state.clone()
            };
            let name = unique_name(names(&rows), &format!("{} copy", src.name));
            let name: String = name.chars().take(MAX_NAME_CHARS).collect();
            let icon = src.icon.clone();
            self.db
                .with(|c| insert_row(c, &name, &icon, &state, at + 1))?
        };
        self.workspaces_changed();
        Ok(new)
    }

    /// Deletes a workspace: forgets its saved tabs and layout. No file or
    /// folder is touched. Deleting the active workspace first switches to its
    /// neighbour; the last workspace can't be deleted.
    pub fn delete_workspace(&self, id: WorkspaceId) -> Result<()> {
        let list = self.workspaces()?;
        let at = list
            .iter()
            .position(|w| w.id == id)
            .ok_or_else(|| CoreError::NotFound(format!("workspace {id}")))?;
        let Some(next) = neighbour(at, list.len()) else {
            return Err(CoreError::NotAvailable(
                "The last workspace can't be deleted.".into(),
            ));
        };
        if list[at].active {
            self.switch_workspace(list[next].id)?;
        }
        {
            let live = self.live.lock();
            if live.active == Some(id) {
                // Another switch got in first; try again from the top.
                drop(live);
                return self.delete_workspace(id);
            }
            self.db.with(|c| {
                let tx = c.transaction()?;
                tx.execute("DELETE FROM core_workspaces WHERE id = ?1", [id.0 as i64])?;
                let order: Vec<WorkspaceId> = read_rows(&tx)?.into_iter().map(|r| r.id).collect();
                renumber(&tx, &order)?;
                tx.commit()
            })?;
        }
        self.live.lock().closed.remove(&id);
        self.workspaces_changed();
        Ok(())
    }

    /// Asks before deleting a workspace (the Manage sheet and the
    /// `workspace.delete` command). The answer comes back through
    /// [`crate::Engine::confirm`].
    pub fn ask_delete_workspace(&self, id: WorkspaceId) -> Result<Outcome> {
        let list = self.workspaces()?;
        let w = list
            .iter()
            .find(|w| w.id == id)
            .ok_or_else(|| CoreError::NotFound(format!("workspace {id}")))?;
        if list.len() < 2 {
            return Err(CoreError::NotAvailable(
                "The last workspace can't be deleted.".into(),
            ));
        }
        let tabs = match w.tabs {
            1 => "its tab".to_string(),
            n => format!("its {n} tabs"),
        };
        let token = self.ask(PendingAction::DeleteWorkspace(id));
        Ok(Outcome::shell(ShellRequest::Confirm {
            token,
            title: format!("Delete the workspace “{}”?", w.name),
            detail: format!(
                "File Studio forgets {tabs} and layout. No files or folders are touched."
            ),
            yes: "Delete Workspace".into(),
            no: "Cancel".into(),
        }))
    }

    /// The user said yes to deleting a workspace.
    pub(crate) fn confirmed_delete_workspace(&self, id: WorkspaceId) -> Result<Outcome> {
        let name = self
            .rows()?
            .into_iter()
            .find(|r| r.id == id)
            .map(|r| r.name)
            .ok_or_else(|| CoreError::NotFound(format!("workspace {id}")))?;
        self.delete_workspace(id)?;
        Ok(Outcome::message(format!("Deleted the workspace “{name}”")))
    }

    /// Records the sidebar's width and whether it is collapsed, for this
    /// window in this workspace. The width is kept within 160–320 pt.
    pub fn set_sidebar(&self, window: WindowId, width: f64, hidden: bool) -> Result<()> {
        {
            let mut m = self.model.lock();
            let wi = m
                .window_index(window)
                .ok_or_else(|| CoreError::NotFound(format!("window {window}")))?;
            let w = &mut m.windows[wi];
            let width = clamp_sidebar(width);
            if (w.sidebar_width - width).abs() < 0.5 && w.sidebar_hidden == hidden {
                return Ok(());
            }
            w.sidebar_width = width;
            w.sidebar_hidden = hidden;
        }
        self.changed();
        Ok(())
    }

    // ───────────── snapshots (kept for the CLI and older callers) ─────────────

    /// Saves the open windows as a workspace called `name`, without switching
    /// to it. A workspace of that name (any case) gets this layout instead.
    pub fn save_workspace(&self, name: &str) -> Result<WorkspaceId> {
        let name = clean_name(name)?;
        let json = serde_json::to_string(&self.workspace_state())?;
        let id = {
            let mut live = self.live.lock();
            let active = self.ensure_active(&mut live)?;
            let rows = self.rows()?;
            let key = fstudio_path::keys::fold(&name);
            match rows
                .iter()
                .find(|r| fstudio_path::keys::fold(&r.name) == key)
            {
                // The active workspace's state is the open windows already.
                Some(r) if r.id == active => r.id,
                Some(r) => {
                    self.db.with(|c| {
                        c.execute(
                            "UPDATE core_workspaces SET state = ?1, updated_ns = ?2 WHERE id = ?3",
                            params![json, now_ns(), r.id.0 as i64],
                        )
                    })?;
                    r.id
                }
                None => {
                    let n = rows.len();
                    self.db
                        .with(|c| insert_row(c, &name, FALLBACK_ICON, &json, n))?
                }
            }
        };
        self.workspaces_changed();
        Ok(id)
    }

    /// Every workspace (the older name for [`Self::workspaces`]).
    pub fn saved_workspaces(&self) -> Result<Vec<WorkspaceSummary>> {
        self.workspaces()
    }

    /// Loads a workspace's last saved layout into the open windows, replacing
    /// them, without making it the active workspace. To change workspaces use
    /// [`Self::switch_workspace`].
    pub fn open_workspace(&self, id: WorkspaceId) -> Result<WorkspaceState> {
        let state: String = self
            .db
            .with(|c| {
                c.query_row(
                    "SELECT state FROM core_workspaces WHERE id = ?1",
                    [id.0 as i64],
                    |r| r.get(0),
                )
                .optional()
            })?
            .ok_or_else(|| CoreError::NotFound(format!("workspace {id}")))?;
        let ws: WorkspaceState = serde_json::from_str(&state)?;
        let windows: Vec<WindowId> = self.model.lock().windows.iter().map(|w| w.id).collect();
        self.load_workspace(ws, IdMode::Reuse(windows));
        Ok(self.workspace_state())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(list: &[(u64, &'static str)]) -> Vec<(WorkspaceId, &'static str)> {
        list.iter().map(|(i, n)| (WorkspaceId(*i), *n)).collect()
    }

    #[test]
    fn names_are_kept_as_typed_but_checked() {
        assert_eq!(clean_name("  Photo work ").unwrap(), "Photo work");
        assert!(clean_name("   ").is_err());
        assert!(clean_name("").is_err());
        assert!(clean_name("two\nlines").is_err());
        assert!(clean_name(&"x".repeat(61)).is_err());
        assert!(clean_name(&"é".repeat(60)).is_ok());
        // NFD stays NFD (F-017).
        let nfd = "Cafe\u{301}";
        assert_eq!(clean_name(nfd).unwrap(), nfd);
    }

    #[test]
    fn duplicate_names_clash_across_case_and_unicode_form() {
        let list = ids(&[(1, "Everyday"), (2, "Caf\u{e9}")]);
        let it = || list.iter().map(|(i, n)| (*i, *n));
        assert!(name_taken(it(), "everyday", None));
        assert!(name_taken(it(), "CAFE\u{301}", None));
        assert!(!name_taken(it(), "everyday", Some(WorkspaceId(1))));
        assert!(!name_taken(it(), "Photo work", None));
        assert_eq!(unique_name(it(), "Photo work"), "Photo work");
        assert_eq!(unique_name(it(), "EVERYDAY"), "EVERYDAY 2");
        let more = ids(&[(1, "A"), (2, "a 2")]);
        assert_eq!(unique_name(more.iter().map(|(i, n)| (*i, *n)), "A"), "A 3");
    }

    #[test]
    fn icons() {
        assert!(WORKSPACE_ICONS.iter().all(|i| icon_ok(i)));
        assert!(!icon_ok(""));
        assert!(!icon_ok("../etc"));
        assert!(!icon_ok("Camera"));
        assert_eq!(clean_icon(""), FALLBACK_ICON);
        assert_eq!(clean_icon("camera"), "camera");
        assert_eq!(clean_icon("rm -rf"), FALLBACK_ICON);
    }

    #[test]
    fn neighbours_and_cycling() {
        assert_eq!(neighbour(0, 1), None);
        assert_eq!(neighbour(0, 3), Some(1));
        assert_eq!(neighbour(2, 3), Some(1));
        assert_eq!(cycle(0, 3, -1), 2);
        assert_eq!(cycle(2, 3, 1), 0);
        assert_eq!(cycle(1, 3, 1), 2);
    }
}

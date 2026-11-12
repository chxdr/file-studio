//! The engine: one per process. It owns the database, settings, the
//! workspace (windows → panes → tabs), the shared watcher, the operation
//! queue, frecency and completion, and the keymap.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use crossbeam_channel::RecvTimeoutError;
use fstudio_db::Db;
use fstudio_keymap::{Keymap, Profile, VimMode};
use fstudio_ops::{
    DirTrasher, EnqueueOptions, OpId, OpQueue, OpRequest, OpState, SystemTrasher, Trasher,
    UndoState,
};
use fstudio_path::{
    Completions, Crumb, Frecency, FrecencyConfig, Location, ParseContext, ParsedLocation, PathStyle,
};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::events::{CoreEvent, EventHub, EventSink, OpsBridge, VolumeSummary};
use crate::ids::{PaneId, TabId, WindowId, WorkspaceId};
use crate::outcome::Outcome;
use crate::probe::FsCoreProbe;
use crate::settings::{SETTINGS_KEY, Settings};
use crate::shared::{self, Shared};
use crate::tab::{Tab, TabState};
use crate::view::ViewState;
use crate::workspace::{
    Closed, Model, Orientation, PaneRt, PaneState, WindowRt, WindowState, WorkspaceState,
    WorkspaceSummary,
};

const SESSION_KEY: &str = "core.session";
const MARKS_KEY: &str = "core.marks";
/// How long the session waits for changes to stop before it is saved.
const SAVE_QUIET: Duration = Duration::from_millis(250);
/// The longest a busy session goes unsaved.
const SAVE_MAX_DELAY: Duration = Duration::from_secs(2);

/// How the engine is set up.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineConfig {
    /// The folder for the database (`fstudio.sqlite3`). The app uses
    /// `~/Library/Application Support/File Studio`; tools use their own.
    pub data_dir: String,
    /// The home folder (`~`). `None` uses `$HOME`.
    pub home: Option<String>,
    /// Use this folder as the Trash instead of the system Trash (tests and
    /// headless tools; it must be on the same volume as what is trashed).
    pub trash_dir: Option<String>,
    /// Watch folders by polling instead of FSEvents (for filesystems FSEvents
    /// doesn't cover).
    pub poll_watcher: bool,
}

/// A remembered folder, for `z`-style jumping.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrecentDir {
    /// The folder.
    pub path: String,
    /// Its zoxide-style score now.
    pub score: f64,
    /// Last visit, seconds since the epoch.
    pub last_accessed: u64,
}

/// The engine. Create one with [`Engine::new`].
pub struct Engine {
    pub(crate) me: Weak<Engine>,
    pub(crate) shared: Arc<Shared>,
    pub(crate) db: Db,
    data_dir: PathBuf,
    ops: RwLock<OpQueue>,
    ops_pending: Mutex<Option<fstudio_ops::OpsConfig>>,
    trasher: Arc<dyn Trasher>,
    ops_bridge: Arc<OpsBridge>,
    pub(crate) keymap: Mutex<Keymap>,
    pub(crate) model: Mutex<Model>,
    pub(crate) vim: Mutex<VimMode>,
    pub(crate) marks: Mutex<HashMap<char, String>>,
    last_saved: Mutex<String>,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("data_dir", &self.data_dir)
            .finish()
    }
}

fn now_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

impl Engine {
    /// Opens (or creates) the database in `config.data_dir`, runs every
    /// crate's migrations, loads settings and starts the event pump, the
    /// watcher and the operation queue. Interrupted operations from an
    /// earlier run are listed by `ops().interrupted()`.
    pub fn new(config: EngineConfig, events: Arc<dyn EventSink>) -> Result<Arc<Engine>> {
        let data_dir = PathBuf::from(&config.data_dir);
        std::fs::create_dir_all(&data_dir)?;
        let db = Db::open(&data_dir.join("fstudio.sqlite3"))?;
        db.migrate(&shared::MIGRATIONS)?;
        db.migrate(&fstudio_ops::MIGRATIONS)?;
        db.migrate(&fstudio_keymap::MIGRATIONS)?;
        db.migrate(&fstudio_path::frecency::MIGRATIONS)?;

        let settings = match db.kv_get(SETTINGS_KEY)? {
            Some(text) => match Settings::from_json(&text) {
                Ok(s) if s.validate().is_ok() => s,
                _ => {
                    // Keep the unreadable copy for support, start from defaults.
                    db.kv_set(&format!("{SETTINGS_KEY}.unreadable"), &text)?;
                    Settings::default()
                }
            },
            None => Settings::default(),
        };

        let home = config
            .home
            .clone()
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("/"));
        let hub = EventHub::start(events);
        let frecency = Frecency::open(
            db.clone(),
            FrecencyConfig {
                excluded: settings.path_bar.frecency_excluded.clone(),
                ..FrecencyConfig::default()
            },
        )?;
        let watcher: Box<dyn fstudio_fs::Watcher> = if config.poll_watcher {
            Box::new(fstudio_fs::PollWatcher::new(shared::watch_config()))
        } else {
            fstudio_fs::default_watcher(shared::watch_config())?
        };
        let shared = Shared::new(
            db.clone(),
            hub.clone(),
            settings.clone(),
            frecency,
            home,
            watcher,
        )?;

        let trasher: Arc<dyn Trasher> = match &config.trash_dir {
            Some(d) => Arc::new(DirTrasher::new(d)?),
            None => Arc::new(SystemTrasher),
        };
        let ops_bridge = Arc::new(OpsBridge { hub: hub.clone() });
        let ops = OpQueue::open(
            db.clone(),
            settings.ops_config(),
            trasher.clone(),
            ops_bridge.clone(),
        )?;

        let mut keymap = Keymap::new(settings.keymap.clone());
        keymap.set_overlay(settings.overlay);
        keymap.set_hints(settings.hints);
        keymap.attach_db(&db)?;
        let vim = if settings.keymap.profile == Profile::Vim {
            VimMode::Normal
        } else {
            VimMode::Off
        };
        let marks: HashMap<char, String> = db
            .kv_get(MARKS_KEY)?
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();

        let (dirty_tx, dirty_rx) = crossbeam_channel::bounded::<()>(1);
        shared.set_dirty_channel(dirty_tx);
        let engine = Arc::new_cyclic(|me: &Weak<Engine>| Engine {
            me: me.clone(),
            shared,
            db,
            data_dir,
            ops: RwLock::new(ops),
            ops_pending: Mutex::new(None),
            trasher,
            ops_bridge,
            keymap: Mutex::new(keymap),
            model: Mutex::new(Model::default()),
            vim: Mutex::new(vim),
            marks: Mutex::new(marks),
            last_saved: Mutex::new(String::new()),
        });
        let weak = Arc::downgrade(&engine);
        std::thread::Builder::new()
            .name("fstudio-session-saver".into())
            .spawn(move || session_saver(weak, dirty_rx))?;
        Ok(engine)
    }

    /// The folder the database lives in.
    pub fn data_dir(&self) -> String {
        self.data_dir.to_string_lossy().into_owned()
    }

    /// The home folder used for `~`.
    pub fn home(&self) -> String {
        self.shared.home.to_string_lossy().into_owned()
    }

    // ───────────────────────── settings ─────────────────────────

    /// The current settings.
    pub fn settings(&self) -> Settings {
        self.shared.settings.read().clone()
    }

    /// Validates, stores and applies new settings, then emits `SettingsChanged`.
    /// Queue settings apply as soon as no operation is running.
    pub fn update_settings(&self, s: Settings) -> Result<()> {
        s.validate()?;
        let old = self.settings();
        if old == s {
            return Ok(());
        }
        self.db.kv_set(SETTINGS_KEY, &s.to_json())?;
        *self.shared.settings.write() = s.clone();
        if old.keymap != s.keymap || old.overlay != s.overlay || old.hints != s.hints {
            let mut k = self.keymap.lock();
            k.set_settings(s.keymap.clone());
            k.set_overlay(s.overlay);
            k.set_hints(s.hints);
            let mut vim = self.vim.lock();
            *vim = match (s.keymap.profile, *vim) {
                (Profile::Vim, VimMode::Off) => VimMode::Normal,
                (Profile::Vim, m) => m,
                (Profile::Standard, _) => VimMode::Off,
            };
        }
        if old.path_bar.frecency_excluded != s.path_bar.frecency_excluded {
            self.shared
                .frecency
                .set_excluded(s.path_bar.frecency_excluded.clone());
        }
        if old.path_bar != s.path_bar {
            self.shared.rebuild_completer();
        }
        if old.operations != s.operations {
            *self.ops_pending.lock() = Some(s.ops_config());
            self.apply_pending_ops_config();
        }
        self.shared.emit(CoreEvent::SettingsChanged);
        Ok(())
    }

    /// One setting by dotted key (`view.show_hidden`), as JSON text.
    pub fn setting(&self, key: &str) -> Result<String> {
        self.settings()
            .get_key(key)
            .map(|v| v.to_string())
            .ok_or_else(|| CoreError::InvalidSetting(format!("no setting '{key}'")))
    }

    /// Changes one setting by dotted key; `value` is JSON or plain text.
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let s = self.settings().with_key(key, value)?;
        self.update_settings(s)
    }

    /// Every setting as a JSON document (with its format version).
    pub fn export_settings(&self) -> String {
        self.settings().to_json()
    }

    /// Replaces the settings from an exported JSON document.
    pub fn import_settings(&self, json: &str) -> Result<()> {
        let s = Settings::from_json(json)?;
        self.update_settings(s)
    }

    fn apply_pending_ops_config(&self) {
        let mut pending = self.ops_pending.lock();
        let Some(cfg) = pending.clone() else { return };
        let busy = self.ops.read().list().iter().any(|s| {
            matches!(
                s.state,
                OpState::Queued | OpState::Running | OpState::Paused | OpState::WaitingForDecision
            )
        });
        if busy {
            return;
        }
        if let Ok(q) = OpQueue::open(
            self.db.clone(),
            cfg,
            self.trasher.clone(),
            self.ops_bridge.clone(),
        ) {
            *self.ops.write() = q;
            *pending = None;
        }
    }

    // ───────────────────────── operations ─────────────────────────

    /// The operation queue (progress, pause, resume, cancel, conflicts, interrupted ops).
    pub fn ops(&self) -> OpQueue {
        self.apply_pending_ops_config();
        self.ops.read().clone()
    }

    /// Queues a file operation.
    pub fn enqueue(&self, req: OpRequest) -> Result<OpId> {
        Ok(self.ops().enqueue(req)?)
    }

    /// Queues a file operation with conflict rules.
    pub fn enqueue_with(&self, req: OpRequest, opts: EnqueueOptions) -> Result<OpId> {
        Ok(self.ops().enqueue_with(req, opts)?)
    }

    /// Undoes the last file operation by queuing its inverse.
    /// A refusal ("“a.txt” was changed after it was copied") comes back as
    /// the outcome's message, not an error.
    pub fn undo(&self) -> Result<Outcome> {
        let ids = match self.ops().undo() {
            Ok(ids) => ids,
            Err(fstudio_ops::OpsError::UndoRefused(why)) => {
                return Ok(Outcome::message(format!("Can’t undo: {why}")));
            }
            Err(e) => return Err(e.into()),
        };
        Ok(if ids.is_empty() {
            Outcome::message("Nothing to undo")
        } else {
            Outcome {
                ops: ids,
                ..Outcome::none()
            }
        })
    }

    /// Redoes the last undone operation.
    pub fn redo(&self) -> Result<Outcome> {
        Ok(match self.ops().redo()? {
            Some(id) => Outcome::op(id),
            None => Outcome::message("Nothing to redo"),
        })
    }

    /// The Edit menu's undo and redo labels.
    pub fn undo_state(&self) -> UndoState {
        self.ops().undo_state()
    }

    // ───────────────────────── workspace ─────────────────────────

    /// Restores the last session (or opens one window at the start folder)
    /// and returns it. Missing folders fall back to the nearest existing parent.
    pub fn restore_session(&self) -> WorkspaceState {
        let saved = if self.settings().general.restore_session {
            self.db
                .kv_get(SESSION_KEY)
                .ok()
                .flatten()
                .and_then(|t| serde_json::from_str::<WorkspaceState>(&t).ok())
                .filter(|w| w.tab_count() > 0)
        } else {
            None
        };
        match saved {
            Some(ws) => self.load_workspace(ws, true),
            None => {
                self.close_all();
                self.new_window(None).ok();
            }
        }
        self.workspace_state()
    }

    /// Every window, pane and tab as plain data.
    pub fn workspace_state(&self) -> WorkspaceState {
        let m = self.model.lock();
        let pane = |p: &PaneRt| PaneState {
            id: p.id,
            tabs: p
                .tabs
                .iter()
                .filter_map(|t| m.tabs.get(t))
                .map(|t| t.state())
                .collect(),
            active_tab: p.active as u32,
        };
        WorkspaceState {
            windows: m
                .windows
                .iter()
                .map(|w| WindowState {
                    id: w.id,
                    panes: w.panes.iter().map(pane).collect(),
                    active_pane: w.active_pane as u32,
                    orientation: w.orientation,
                    split_ratio: w.split_ratio,
                    hidden_pane: w.hidden.as_ref().map(pane),
                })
                .collect(),
            active_window: m.active_window,
        }
    }

    /// A tab by id.
    pub fn tab(&self, id: TabId) -> Option<Arc<Tab>> {
        self.model.lock().tabs.get(&id).cloned()
    }

    /// The focused tab of the focused pane of the focused window.
    pub fn active_tab(&self) -> Option<Arc<Tab>> {
        let m = self.model.lock();
        m.active_tab().and_then(|t| m.tabs.get(&t).cloned())
    }

    /// The active tab of the other pane, when the tab's window is dual-pane.
    pub fn other_pane_tab(&self, of: TabId) -> Option<Arc<Tab>> {
        let m = self.model.lock();
        m.other_pane_tab(of).and_then(|t| m.tabs.get(&t).cloned())
    }

    fn start_dir(&self) -> PathBuf {
        let s = self.settings().general.start_folder;
        let p = self.resolve_dir(&s).map(|x| x.0);
        p.unwrap_or_else(|_| self.shared.home.clone())
    }

    /// Resolves typed text to an existing folder (and a name to select in it).
    pub(crate) fn resolve_dir(&self, raw: &str) -> Result<(PathBuf, Option<String>)> {
        let cwd = self
            .active_tab()
            .map(|t| t.location_path())
            .unwrap_or_else(|| self.shared.home.clone());
        let ctx = ParseContext::new(cwd, self.shared.home.clone(), Box::new(FsCoreProbe))
            .with_env(|k| std::env::var(k).ok());
        match fstudio_path::parse_location(raw, &ctx).location {
            Location::Dir(p) => Ok((p, None)),
            Location::File { dir, name } => Ok((dir, Some(name))),
            Location::Missing { path, .. } => {
                Err(CoreError::NotFound(path.to_string_lossy().into_owned()))
            }
            Location::Remote(u) => Err(CoreError::NotAvailable(format!(
                "remote locations are not supported yet: {u}"
            ))),
        }
    }

    fn tab_state_at(&self, raw: Option<&str>) -> Result<TabState> {
        let (dir, select) = match raw {
            Some(r) => self.resolve_dir(r)?,
            None => (self.start_dir(), None),
        };
        let v = &self.settings().view;
        Ok(TabState {
            id: TabId(0),
            location: dir.to_string_lossy().into_owned(),
            view: self
                .shared
                .folder_view(&dir)
                .unwrap_or_else(|| ViewState::from_settings(v)),
            selected: select.iter().cloned().collect(),
            cursor: select,
            ..TabState::default()
        })
    }

    fn make_tab(&self, state: TabState) -> Arc<Tab> {
        let mut state = state;
        state.location = nearest_existing(Path::new(&state.location))
            .to_string_lossy()
            .into_owned();
        Tab::create(self.shared.clone(), state)
    }

    fn changed(&self) {
        self.shared.emit(CoreEvent::TabsChanged);
        self.shared.mark_dirty();
    }

    /// Opens a new window with one tab at `path` (or the start folder), and a
    /// second pane if dual-pane is the default.
    pub fn new_window(&self, path: Option<String>) -> Result<WindowId> {
        let state = self.tab_state_at(path.as_deref())?;
        let dual = self.settings().general.dual_pane_by_default;
        let t1 = self.make_tab(state.clone());
        let mut panes = vec![PaneRt {
            id: PaneId(self.shared.ids.next()),
            tabs: vec![t1.id()],
            active: 0,
        }];
        let mut new_tabs = vec![t1];
        if dual {
            let t2 = self.make_tab(TabState {
                id: TabId(0),
                ..state
            });
            panes.push(PaneRt {
                id: PaneId(self.shared.ids.next()),
                tabs: vec![t2.id()],
                active: 0,
            });
            new_tabs.push(t2);
        }
        let id = WindowId(self.shared.ids.next());
        {
            let mut m = self.model.lock();
            for t in new_tabs {
                m.tabs.insert(t.id(), t);
            }
            m.windows.push(WindowRt {
                id,
                panes,
                active_pane: 0,
                orientation: Orientation::Horizontal,
                split_ratio: 0.5,
                hidden: None,
            });
            m.active_window = Some(id);
        }
        self.changed();
        Ok(id)
    }

    /// Closes a window and its tabs.
    pub fn close_window(&self, id: WindowId) -> Result<()> {
        let closed: Vec<Arc<Tab>> = {
            let mut m = self.model.lock();
            let wi = m
                .window_index(id)
                .ok_or_else(|| CoreError::NotFound(format!("window {id}")))?;
            let w = m.windows.remove(wi);
            if m.active_window == Some(id) {
                m.active_window = m.windows.first().map(|w| w.id);
            }
            w.panes
                .iter()
                .chain(w.hidden.iter())
                .flat_map(|p| p.tabs.clone())
                .filter_map(|t| m.tabs.remove(&t))
                .collect()
        };
        for t in closed {
            t.close();
        }
        self.changed();
        Ok(())
    }

    /// Opens a tab at `path` (or the start folder) after the focused tab, and
    /// focuses it. Opens a window if there is none.
    pub fn open_tab(&self, path: Option<String>) -> Result<Arc<Tab>> {
        let target = {
            let m = self.model.lock();
            m.active_window_index().map(|wi| {
                let w = &m.windows[wi];
                (w.panes[w.active_pane].id, w.panes[w.active_pane].active + 1)
            })
        };
        match target {
            Some((pane, idx)) => self.open_tab_in(pane, path, Some(idx as u32)),
            None => {
                let w = self.new_window(path)?;
                let m = self.model.lock();
                let wi = m.window_index(w).expect("just created");
                let t = m.windows[wi].panes[0].tabs[0];
                Ok(m.tabs[&t].clone())
            }
        }
    }

    /// Opens a tab in a given pane at `index` (the end if `None`).
    pub fn open_tab_in(
        &self,
        pane: PaneId,
        path: Option<String>,
        index: Option<u32>,
    ) -> Result<Arc<Tab>> {
        let state = self.tab_state_at(path.as_deref())?;
        self.insert_tab(pane, state, index.map(|i| i as usize))
    }

    fn insert_tab(&self, pane: PaneId, state: TabState, index: Option<usize>) -> Result<Arc<Tab>> {
        let tab = self.make_tab(state);
        {
            let mut m = self.model.lock();
            let Some((wi, pi)) = m.pane_place(pane) else {
                drop(m);
                tab.close();
                return Err(CoreError::NotFound(format!("pane {pane}")));
            };
            m.tabs.insert(tab.id(), tab.clone());
            m.attach(tab.id(), wi, pi, index.unwrap_or(usize::MAX));
        }
        self.changed();
        Ok(tab)
    }

    /// Closes a tab (remembered for "Reopen Closed Tab"). A pane left empty
    /// closes; a window left empty closes.
    pub fn close_tab(&self, id: TabId) -> Result<()> {
        let keep = self.settings().general.closed_tabs_kept as usize;
        let tab = {
            let mut m = self.model.lock();
            let tab = m
                .tabs
                .remove(&id)
                .ok_or_else(|| CoreError::NotFound(format!("tab {id}")))?;
            if let Some((pl, window, pane, _)) = m.detach(id) {
                m.closed.push(Closed {
                    state: tab.state(),
                    window,
                    pane,
                    index: pl.index,
                });
                if m.closed.len() > keep {
                    m.closed.remove(0);
                }
            }
            tab
        };
        tab.close();
        self.changed();
        Ok(())
    }

    /// Closes every tab of the pane except `keep`.
    pub fn close_other_tabs(&self, keep: TabId) -> Result<()> {
        let others: Vec<TabId> = {
            let m = self.model.lock();
            let pl = m
                .find(keep)
                .ok_or_else(|| CoreError::NotFound(format!("tab {keep}")))?;
            m.windows[pl.window].panes[pl.pane]
                .tabs
                .iter()
                .copied()
                .filter(|t| *t != keep)
                .collect()
        };
        for t in others {
            self.close_tab(t)?;
        }
        Ok(())
    }

    /// Brings back the most recently closed tab, where it was if possible.
    pub fn reopen_closed_tab(&self) -> Result<Option<Arc<Tab>>> {
        let (closed, target) = {
            let mut m = self.model.lock();
            let Some(c) = m.closed.pop() else {
                return Ok(None);
            };
            let target = m.pane_place(c.pane).map(|_| c.pane).or_else(|| {
                m.window_index(c.window)
                    .or(m.active_window_index())
                    .map(|wi| m.windows[wi].panes[m.windows[wi].active_pane].id)
            });
            (c, target)
        };
        let mut state = closed.state;
        if self.tab(state.id).is_some() {
            state.id = TabId(0);
        }
        let tab = match target {
            Some(p) => self.insert_tab(p, state, Some(closed.index))?,
            None => {
                let w = self.new_window(Some(state.location.clone()))?;
                let t = {
                    let m = self.model.lock();
                    let wi = m.window_index(w).expect("just created");
                    m.windows[wi].panes[0].tabs[0]
                };
                self.tab(t).expect("just created")
            }
        };
        Ok(Some(tab))
    }

    /// Focuses a tab (and its pane and window).
    pub fn activate_tab(&self, id: TabId) -> Result<()> {
        if !self.model.lock().activate(id) {
            return Err(CoreError::NotFound(format!("tab {id}")));
        }
        self.changed();
        Ok(())
    }

    /// Moves a tab to `pane` at `index` (reorder within a pane, or between panes).
    pub fn move_tab(&self, id: TabId, pane: PaneId, index: u32) -> Result<()> {
        {
            let mut m = self.model.lock();
            let (wi, pi) = m
                .pane_place(pane)
                .ok_or_else(|| CoreError::NotFound(format!("pane {pane}")))?;
            if !m.move_tab(id, wi, pi, index as usize) {
                return Err(CoreError::NotFound(format!("tab {id}")));
            }
        }
        self.changed();
        Ok(())
    }

    /// Focuses pane `index` (0 or 1) of a window.
    pub fn focus_pane(&self, window: WindowId, index: u32) -> Result<()> {
        {
            let mut m = self.model.lock();
            let wi = m
                .window_index(window)
                .ok_or_else(|| CoreError::NotFound(format!("window {window}")))?;
            let w = &mut m.windows[wi];
            if index as usize >= w.panes.len() {
                return Err(CoreError::NotFound(format!("pane {index}")));
            }
            w.active_pane = index as usize;
            m.active_window = Some(window);
        }
        self.changed();
        Ok(())
    }

    /// Turns the second pane on or off. Turned off, it is remembered and comes
    /// back as it was; the first time, it opens at the focused tab's folder.
    pub fn toggle_dual(&self, window: WindowId) -> Result<bool> {
        let new_tab_state = {
            let m = self.model.lock();
            let wi = m
                .window_index(window)
                .ok_or_else(|| CoreError::NotFound(format!("window {window}")))?;
            let w = &m.windows[wi];
            if w.panes.len() == 1 && w.hidden.is_none() {
                let t = w.panes[0].tabs[w.panes[0].active];
                Some(m.tabs[&t].state())
            } else {
                None
            }
        };
        let fresh = new_tab_state.map(|s| {
            self.make_tab(TabState {
                id: TabId(0),
                back: Vec::new(),
                forward: Vec::new(),
                selected: Vec::new(),
                cursor: None,
                scroll: None,
                ..s
            })
        });
        let dual = {
            let mut m = self.model.lock();
            let wi = m.window_index(window).expect("checked");
            if let Some(t) = &fresh {
                m.tabs.insert(t.id(), t.clone());
            }
            let pane_id = PaneId(self.shared.ids.next());
            let w = &mut m.windows[wi];
            if w.panes.len() == 2 {
                let p = w.panes.remove(1);
                w.hidden = Some(p);
                w.active_pane = 0;
                false
            } else {
                let p = match (w.hidden.take(), fresh) {
                    (Some(p), _) => p,
                    (None, Some(t)) => PaneRt {
                        id: pane_id,
                        tabs: vec![t.id()],
                        active: 0,
                    },
                    (None, None) => unreachable!("a fresh tab is made when nothing is hidden"),
                };
                w.panes.push(p);
                true
            }
        };
        self.changed();
        Ok(dual)
    }

    /// Sets how a dual-pane window is split.
    pub fn set_layout(
        &self,
        window: WindowId,
        orientation: Orientation,
        split_ratio: f64,
    ) -> Result<()> {
        {
            let mut m = self.model.lock();
            let wi = m
                .window_index(window)
                .ok_or_else(|| CoreError::NotFound(format!("window {window}")))?;
            let w = &mut m.windows[wi];
            w.orientation = orientation;
            w.split_ratio = split_ratio.clamp(0.1, 0.9);
        }
        self.changed();
        Ok(())
    }

    /// Swaps the two panes' tabs.
    pub fn swap_panes(&self, window: WindowId) -> Result<()> {
        {
            let mut m = self.model.lock();
            let wi = m
                .window_index(window)
                .ok_or_else(|| CoreError::NotFound(format!("window {window}")))?;
            let w = &mut m.windows[wi];
            if w.panes.len() != 2 {
                return Err(CoreError::NotAvailable("only one pane is open".into()));
            }
            w.panes.swap(0, 1);
            w.active_pane = 1 - w.active_pane;
        }
        self.changed();
        Ok(())
    }

    fn close_all(&self) {
        let tabs: Vec<Arc<Tab>> = {
            let mut m = self.model.lock();
            m.windows.clear();
            m.active_window = None;
            m.tabs.drain().map(|(_, t)| t).collect()
        };
        for t in tabs {
            t.close();
        }
    }

    /// Replaces the open windows with `ws`. `keep_ids` keeps tab, pane and
    /// window ids (session restore); otherwise fresh ids are used.
    fn load_workspace(&self, ws: WorkspaceState, keep_ids: bool) {
        self.close_all();
        let fresh = |id: u64| {
            if keep_ids {
                self.shared.ids.bump_past(id);
                id
            } else {
                self.shared.ids.next()
            }
        };
        let mut m_windows = Vec::new();
        let mut tabs = Vec::new();
        let mut active_window = None;
        for w in ws.windows {
            let wid = WindowId(fresh(w.id.0));
            if ws.active_window == Some(w.id) {
                active_window = Some(wid);
            }
            let mut pane = |p: PaneState| -> Option<PaneRt> {
                let ids: Vec<TabId> = p
                    .tabs
                    .into_iter()
                    .map(|mut t| {
                        if !keep_ids {
                            t.id = TabId(0);
                        }
                        let tab = self.make_tab(t);
                        let id = tab.id();
                        tabs.push(tab);
                        id
                    })
                    .collect();
                (!ids.is_empty()).then(|| PaneRt {
                    id: PaneId(fresh(p.id.0)),
                    active: (p.active_tab as usize).min(ids.len() - 1),
                    tabs: ids,
                })
            };
            let panes: Vec<PaneRt> = w.panes.into_iter().filter_map(&mut pane).collect();
            let hidden = w.hidden_pane.and_then(&mut pane);
            if panes.is_empty() {
                continue;
            }
            m_windows.push(WindowRt {
                id: wid,
                active_pane: (w.active_pane as usize).min(panes.len() - 1),
                panes,
                orientation: w.orientation,
                split_ratio: w.split_ratio.clamp(0.1, 0.9),
                hidden,
            });
        }
        {
            let mut m = self.model.lock();
            m.active_window = active_window.or(m_windows.first().map(|w| w.id));
            m.windows = m_windows;
            for t in tabs {
                m.tabs.insert(t.id(), t);
            }
        }
        if self.model.lock().windows.is_empty() {
            let _ = self.new_window(None);
        }
        self.changed();
    }

    /// Saves the session now (normally this happens a moment after each change).
    pub fn save_session_now(&self) {
        let ws = self.workspace_state();
        if ws.windows.is_empty() {
            return;
        }
        let Ok(json) = serde_json::to_string(&ws) else {
            return;
        };
        let mut last = self.last_saved.lock();
        if *last == json {
            return;
        }
        if self.db.kv_set(SESSION_KEY, &json).is_ok() {
            *last = json;
        }
    }

    // ───────────────────────── saved workspaces ─────────────────────────

    /// Saves the open windows under `name`, replacing a workspace of that name.
    pub fn save_workspace(&self, name: &str) -> Result<WorkspaceId> {
        let name = name.trim();
        if name.is_empty() {
            return Err(CoreError::InvalidName("the name is empty".into()));
        }
        let json = serde_json::to_string(&self.workspace_state())?;
        let now = now_ns();
        let id: i64 = self.db.with(|c| {
            c.execute(
                "INSERT INTO core_workspaces(name, state, created_ns, updated_ns) VALUES (?1, ?2, ?3, ?3)
                 ON CONFLICT(name) DO UPDATE SET state = excluded.state, updated_ns = excluded.updated_ns",
                fstudio_db::rusqlite::params![name, json, now],
            )?;
            c.query_row("SELECT id FROM core_workspaces WHERE name = ?1", [name], |r| r.get(0))
        })?;
        Ok(WorkspaceId(id as u64))
    }

    /// Saved workspaces, by name.
    pub fn saved_workspaces(&self) -> Result<Vec<WorkspaceSummary>> {
        let rows: Vec<(i64, String, String, i64)> = self.db.with(|c| {
            let mut st = c.prepare(
                "SELECT id, name, state, updated_ns FROM core_workspaces ORDER BY name COLLATE NOCASE",
            )?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect()
        })?;
        Ok(rows
            .into_iter()
            .map(|(id, name, state, updated_ns)| {
                let ws: WorkspaceState = serde_json::from_str(&state).unwrap_or_default();
                WorkspaceSummary {
                    id: WorkspaceId(id as u64),
                    name,
                    windows: ws.windows.len() as u32,
                    tabs: ws.tab_count() as u32,
                    updated_ns,
                }
            })
            .collect())
    }

    /// Replaces the open windows with a saved workspace.
    pub fn open_workspace(&self, id: WorkspaceId) -> Result<WorkspaceState> {
        let state: String = self
            .db
            .with(|c| {
                c.query_row(
                    "SELECT state FROM core_workspaces WHERE id = ?1",
                    [id.0 as i64],
                    |r| r.get(0),
                )
            })
            .map_err(|_| CoreError::NotFound(format!("workspace {id}")))?;
        let ws: WorkspaceState = serde_json::from_str(&state)?;
        self.load_workspace(ws, false);
        Ok(self.workspace_state())
    }

    /// Renames a saved workspace.
    pub fn rename_workspace(&self, id: WorkspaceId, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(CoreError::InvalidName("the name is empty".into()));
        }
        let n = self.db.with(|c| {
            c.execute(
                "UPDATE core_workspaces SET name = ?1 WHERE id = ?2",
                fstudio_db::rusqlite::params![name, id.0 as i64],
            )
        })?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("workspace {id}")));
        }
        Ok(())
    }

    /// Deletes a saved workspace (only the saved layout; no files are touched).
    pub fn delete_workspace(&self, id: WorkspaceId) -> Result<()> {
        let n = self
            .db
            .with(|c| c.execute("DELETE FROM core_workspaces WHERE id = ?1", [id.0 as i64]))?;
        if n == 0 {
            return Err(CoreError::NotFound(format!("workspace {id}")));
        }
        Ok(())
    }

    // ───────────────────────── path engine ─────────────────────────

    /// Parses typed or pasted text relative to `cwd` (no navigation).
    pub fn parse_location(&self, raw: &str, cwd: &str) -> ParsedLocation {
        let ctx = ParseContext::new(cwd, self.shared.home.clone(), Box::new(FsCoreProbe))
            .with_env(|k| std::env::var(k).ok())
            .with_user_home(|u| {
                if u.is_empty() || u.contains('/') {
                    return None;
                }
                let p = PathBuf::from("/Users").join(u);
                p.is_dir().then_some(p)
            });
        fstudio_path::parse_location(raw, &ctx)
    }

    /// Breadcrumbs for a path.
    pub fn breadcrumbs(&self, path: &str) -> Vec<Crumb> {
        fstudio_path::breadcrumbs(
            Path::new(path),
            &self.shared.home,
            &self.shared.crumb_volumes(),
        )
    }

    /// Sub-folders of a crumb, for its chevron menu. Lists the folder.
    pub fn subfolders(&self, path: &str) -> Vec<Crumb> {
        fstudio_path::subfolders(Path::new(path), self.shared.dir_source.as_ref())
    }

    /// Completions for address-bar text relative to `cwd`.
    pub fn complete(&self, input: &str, cursor: u32, cwd: &str) -> Completions {
        let ctx = ParseContext::new(cwd, self.shared.home.clone(), Box::new(FsCoreProbe))
            .with_env(|k| std::env::var(k).ok());
        let c = self.shared.completer.read().clone();
        c.complete(input, cursor as usize, &ctx)
    }

    /// A path written in a "Copy Path" style.
    pub fn copy_path_as(&self, path: &str, style: PathStyle) -> String {
        fstudio_path::copy_path_as(Path::new(path), style, &self.shared.home)
    }

    /// Records a folder visit for frecency (navigation does this itself).
    pub fn record_visit(&self, path: &str) -> bool {
        self.shared.frecency.visit(Path::new(path))
    }

    /// Remembered folders matching every word of `query` in order (zoxide's
    /// rule: the last word must match the last component), best first.
    pub fn frecent(&self, query: &str, limit: u32) -> Vec<FrecentDir> {
        let words: Vec<String> = query
            .split_whitespace()
            .map(fstudio_path::keys::fold)
            .collect();
        let now = self.shared.frecency.now();
        self.shared
            .frecency
            .ranked()
            .into_iter()
            .filter(|e| zoxide_match(&fstudio_path::keys::fold(&e.path), &words))
            .take(limit as usize)
            .map(|e| FrecentDir {
                score: e.score(now),
                path: e.path,
                last_accessed: e.last_accessed,
            })
            .collect()
    }

    /// How many folders frecency remembers.
    pub fn frecency_len(&self) -> u32 {
        self.shared.frecency.len() as u32
    }

    /// Imports zoxide's database (default: its usual location). Returns
    /// `(imported, skipped)`.
    pub fn import_zoxide(&self, db_path: Option<String>) -> Result<(u32, u32)> {
        let path = match db_path {
            Some(p) => PathBuf::from(p),
            None => default_zoxide_db(&self.shared.home),
        };
        let r = self.shared.frecency.import_zoxide(&path)?;
        self.shared.frecency.flush();
        Ok((r.imported as u32, r.skipped as u32))
    }

    /// Adds a remembered folder with a given rank (imports and benchmarks).
    pub fn add_frecent(&self, path: &str, rank: f64, last_accessed: u64) -> bool {
        self.shared.frecency.add_imported(path, rank, last_accessed)
    }

    // ───────────────────────── OS hooks ─────────────────────────

    /// The shell saw a volume mount or unmount. Re-reads volumes, emits
    /// `VolumesChanged` and re-lists tabs whose folder is gone.
    pub fn volumes_changed(&self) {
        let vols = fstudio_fs::volumes().unwrap_or_default();
        *self.shared.volumes.write() = vols;
        self.shared.emit(CoreEvent::VolumesChanged {
            volumes: self.volumes(),
        });
        let tabs: Vec<Arc<Tab>> = self.model.lock().tabs.values().cloned().collect();
        for t in tabs {
            if !t.location_path().exists() {
                t.refresh();
            }
        }
    }

    /// Mounted volumes.
    pub fn volumes(&self) -> Vec<VolumeSummary> {
        self.shared
            .volumes
            .read()
            .iter()
            .map(|v| VolumeSummary {
                name: v.name.clone(),
                mount_path: v.mount_path.to_string_lossy().into_owned(),
                is_removable: v.is_removable,
                is_network: v.is_network,
                is_read_only: v.is_read_only,
                total_bytes: v.total_bytes,
                free_bytes: v.free_bytes,
            })
            .collect()
    }

    /// The app is quitting: saves the session, flushes frecency and delivers
    /// pending events. The journal is already durable; running operations are
    /// offered for resume next time.
    pub fn app_will_quit(&self) {
        self.save_session_now();
        self.shared.frecency.flush();
        self.shared.hub.flush();
    }

    /// Delivers every held event now and waits for the sink to see them.
    pub fn flush_events(&self) {
        self.shared.hub.flush();
    }
}

fn nearest_existing(p: &Path) -> PathBuf {
    let mut cur = p.to_path_buf();
    loop {
        if cur.is_dir() {
            return cur;
        }
        match cur.parent() {
            Some(parent) => cur = parent.to_path_buf(),
            None => return PathBuf::from("/"),
        }
    }
}

fn zoxide_match(path: &str, words: &[String]) -> bool {
    if words.is_empty() {
        return true;
    }
    let mut at = 0;
    for w in words {
        match path[at..].find(w.as_str()) {
            Some(i) => at += i + w.len(),
            None => return false,
        }
    }
    let last = path.rsplit('/').next().unwrap_or("");
    last.contains(words.last().map(String::as_str).unwrap_or(""))
}

fn default_zoxide_db(home: &Path) -> PathBuf {
    match std::env::var_os("_ZO_DATA_DIR") {
        Some(d) => PathBuf::from(d).join("db.zo"),
        None => home.join("Library/Application Support/zoxide/db.zo"),
    }
}

fn session_saver(engine: Weak<Engine>, rx: crossbeam_channel::Receiver<()>) {
    while rx.recv().is_ok() {
        let start = Instant::now();
        loop {
            match rx.recv_timeout(SAVE_QUIET) {
                Ok(()) if start.elapsed() < SAVE_MAX_DELAY => continue,
                Ok(()) | Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    if let Some(e) = engine.upgrade() {
                        e.save_session_now();
                    }
                    return;
                }
            }
        }
        match engine.upgrade() {
            Some(e) => e.save_session_now(),
            None => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoxide_rule() {
        assert!(zoxide_match("/users/me/code/fstudio", &["fst".into()]));
        assert!(zoxide_match(
            "/users/me/code/fstudio",
            &["code".into(), "stu".into()]
        ));
        assert!(!zoxide_match("/users/me/code/fstudio", &["code".into()]));
        assert!(!zoxide_match(
            "/users/me/code/fstudio",
            &["stu".into(), "code".into()]
        ));
    }
}

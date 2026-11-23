//! Services shared by the engine and its tabs: events, the watcher, frecency,
//! completion, settings, the clipboard and per-folder view memory.
//!
//! Tabs hold an `Arc<Shared>`; `Shared` never holds a tab strongly, so there
//! are no reference cycles.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::Duration;

use crossbeam_channel::Sender;
use fstudio_db::{Db, Migrations};
use fstudio_fs::{FsEvent, FsEventKind, WatchConfig, Watcher};
use fstudio_path::{CachedDirSource, Completer, CompleterConfig, Frecency, Volume};
use parking_lot::{Mutex, RwLock};

use crate::events::{CoreEvent, EventHub};
use crate::ids::{IdGen, TabId};
use crate::probe::FsDirSource;
use crate::settings::{FolderMemory, Settings};
use crate::tab::Tab;
use crate::view::ViewState;

/// The core's own tables.
pub const MIGRATIONS: Migrations = Migrations {
    module: "core",
    steps: &[(
        1,
        "CREATE TABLE core_workspaces (
             id         INTEGER PRIMARY KEY,
             name       TEXT NOT NULL UNIQUE,
             state      TEXT NOT NULL,
             created_ns INTEGER NOT NULL,
             updated_ns INTEGER NOT NULL
         );
         CREATE TABLE core_folder_views (
             path TEXT PRIMARY KEY,
             view TEXT NOT NULL
         );",
    )],
};

/// Items on the core's clipboard: copied, or cut (pending a move on paste).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Clipboard {
    pub(crate) items: Vec<PathBuf>,
    pub(crate) cut: bool,
    /// The system pasteboard's `changeCount` once the shell wrote these items
    /// there (`None` until it does, and with no shell).
    pub(crate) written: Option<i64>,
}

/// What the shell last told us is on the system pasteboard (S7).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SystemPasteboard {
    /// `NSPasteboard.changeCount`.
    pub(crate) change_count: i64,
    /// The file URLs on it, as paths (empty for text and other data).
    pub(crate) files: Vec<PathBuf>,
}

pub(crate) struct Shared {
    pub(crate) db: Db,
    pub(crate) hub: EventHub,
    pub(crate) settings: RwLock<Settings>,
    pub(crate) frecency: Frecency,
    pub(crate) completer: RwLock<Arc<Completer>>,
    pub(crate) dir_source: Arc<CachedDirSource<FsDirSource>>,
    pub(crate) watch: Arc<WatchRouter>,
    pub(crate) ids: IdGen,
    pub(crate) home: PathBuf,
    pub(crate) clipboard: Mutex<Option<Clipboard>>,
    /// The system pasteboard as the shell last reported it; `None` without a
    /// shell (CLI, tests), where the core's clipboard is the only one.
    pub(crate) pasteboard: Mutex<Option<SystemPasteboard>>,
    pub(crate) volumes: RwLock<Vec<fstudio_fs::VolumeInfo>>,
    folder_views: RwLock<HashMap<String, ViewState>>,
    /// Sidecar reads for culling marks, shared by every tab.
    pub(crate) sidecars: crate::sidecars::SidecarCache,
    /// Connect to Server: connections in progress and nearby computers.
    pub(crate) network: crate::network::Network,
    dirty: Mutex<Option<Sender<()>>>,
}

impl Shared {
    pub(crate) fn new(
        db: Db,
        hub: EventHub,
        settings: Settings,
        frecency: Frecency,
        home: PathBuf,
        watcher: Box<dyn Watcher>,
    ) -> crate::Result<Arc<Shared>> {
        let dir_source = Arc::new(CachedDirSource::new(FsDirSource, 256));
        let completer = Arc::new(Completer::new(
            dir_source.clone(),
            frecency.clone(),
            completer_config(&settings),
        ));
        let views: Vec<(String, String)> = db.with(|c| {
            let mut st = c.prepare("SELECT path, view FROM core_folder_views")?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
        })?;
        let folder_views = views
            .into_iter()
            .filter_map(|(p, v)| serde_json::from_str(&v).ok().map(|v| (p, v)))
            .collect();
        let volumes = fstudio_fs::volumes().unwrap_or_default();
        Ok(Arc::new(Shared {
            db,
            hub,
            settings: RwLock::new(settings),
            frecency,
            completer: RwLock::new(completer),
            dir_source,
            watch: WatchRouter::start(watcher),
            ids: IdGen::default(),
            home,
            clipboard: Mutex::new(None),
            pasteboard: Mutex::new(None),
            volumes: RwLock::new(volumes),
            folder_views: RwLock::new(folder_views),
            sidecars: crate::sidecars::SidecarCache::default(),
            network: crate::network::Network::default(),
            dirty: Mutex::new(None),
        }))
    }

    pub(crate) fn emit(&self, e: CoreEvent) {
        self.hub.emit(e);
    }

    pub(crate) fn set_dirty_channel(&self, tx: Sender<()>) {
        *self.dirty.lock() = Some(tx);
    }

    /// Marks the session as changed; it is saved after a short quiet period.
    pub(crate) fn mark_dirty(&self) {
        if let Some(tx) = &*self.dirty.lock() {
            let _ = tx.try_send(());
        }
    }

    pub(crate) fn rebuild_completer(&self) {
        let cfg = completer_config(&self.settings.read());
        *self.completer.write() = Arc::new(Completer::new(
            self.dir_source.clone(),
            self.frecency.clone(),
            cfg,
        ));
    }

    /// The remembered view of a folder, if per-folder memory is on.
    pub(crate) fn folder_view(&self, dir: &Path) -> Option<ViewState> {
        if self.settings.read().view.folder_memory == FolderMemory::Off {
            return None;
        }
        self.folder_views
            .read()
            .get(&*dir.to_string_lossy())
            .cloned()
    }

    /// Remembers a folder's view (sort and mode), if per-folder memory is on.
    pub(crate) fn remember_view(&self, dir: &Path, v: &ViewState) {
        if self.settings.read().view.folder_memory == FolderMemory::Off {
            return;
        }
        let key = dir.to_string_lossy().into_owned();
        self.folder_views.write().insert(key.clone(), v.clone());
        if let Ok(json) = serde_json::to_string(v) {
            let _ = self.db.with(|c| {
                c.execute(
                    "INSERT INTO core_folder_views(path, view) VALUES (?1, ?2)
                     ON CONFLICT(path) DO UPDATE SET view = excluded.view",
                    [&key, &json],
                )
            });
        }
    }

    /// Volumes as the path engine wants them.
    pub(crate) fn crumb_volumes(&self) -> Vec<Volume> {
        let mut v: Vec<Volume> = self
            .volumes
            .read()
            .iter()
            .map(|v| Volume {
                name: fstudio_fs::display_name(&v.mount_path),
                mount: v.mount_path.clone(),
            })
            .collect();
        // The address bar's first crumb is never "/" (it was when the volume
        // list didn't have the startup disk).
        if !v.iter().any(|v| v.mount == std::path::Path::new("/")) {
            v.push(Volume {
                name: fstudio_fs::display_name(std::path::Path::new("/")),
                mount: "/".into(),
            });
        }
        v
    }

    /// The paths currently cut, as `(folder, names)`.
    pub(crate) fn cut_names_in(&self, dir: &Path) -> HashSet<String> {
        let cb = self.clipboard.lock();
        match &*cb {
            Some(c) if c.cut => c
                .items
                .iter()
                .filter(|p| p.parent() == Some(dir))
                .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .collect(),
            _ => HashSet::new(),
        }
    }
}

fn completer_config(s: &Settings) -> CompleterConfig {
    CompleterConfig {
        limit: s.path_bar.completion_limit as usize,
        show_hidden: s.path_bar.complete_hidden,
        ..CompleterConfig::default()
    }
}

/// One shared watcher for every open tab's folder. Events are routed to the
/// tabs showing the folder they belong to.
pub(crate) struct WatchRouter {
    watcher: Mutex<Box<dyn Watcher>>,
    state: Mutex<RouteState>,
}

#[derive(Default)]
struct RouteState {
    /// Canonical folder → tabs showing it.
    dirs: HashMap<PathBuf, HashSet<TabId>>,
    /// Tab → its canonical folder.
    of_tab: HashMap<TabId, PathBuf>,
    tabs: HashMap<TabId, Weak<Tab>>,
}

impl WatchRouter {
    fn start(watcher: Box<dyn Watcher>) -> Arc<WatchRouter> {
        let rx = watcher.events();
        let me = Arc::new(WatchRouter {
            watcher: Mutex::new(watcher),
            state: Mutex::new(RouteState::default()),
        });
        let weak = Arc::downgrade(&me);
        std::thread::Builder::new()
            .name("fstudio-watch-router".into())
            .spawn(move || {
                loop {
                    match rx.recv_timeout(Duration::from_millis(500)) {
                        Ok(batch) => {
                            let Some(me) = weak.upgrade() else { break };
                            me.route(batch);
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                            if weak.strong_count() == 0 {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
            .expect("spawn watch router");
        me
    }

    /// Records that `tab` now shows `dir` (canonical) and updates the watched set.
    pub(crate) fn register(&self, tab: &Arc<Tab>, dir: &Path) {
        let changed = {
            let mut st = self.state.lock();
            st.tabs.insert(tab.id(), Arc::downgrade(tab));
            let before: HashSet<PathBuf> = st.dirs.keys().cloned().collect();
            if let Some(old) = st.of_tab.insert(tab.id(), dir.to_path_buf())
                && let Some(set) = st.dirs.get_mut(&old)
            {
                set.remove(&tab.id());
                if set.is_empty() {
                    st.dirs.remove(&old);
                }
            }
            st.dirs
                .entry(dir.to_path_buf())
                .or_default()
                .insert(tab.id());
            let after: HashSet<PathBuf> = st.dirs.keys().cloned().collect();
            (before != after).then(|| after.into_iter().collect::<Vec<_>>())
        };
        if let Some(paths) = changed {
            let _ = self.watcher.lock().set_paths(&paths);
        }
    }

    /// The canonical folders being watched now (tools and tests).
    pub(crate) fn watched(&self) -> Vec<PathBuf> {
        self.state.lock().dirs.keys().cloned().collect()
    }

    /// Forgets a closed tab.
    pub(crate) fn unregister(&self, tab: TabId) {
        let paths = {
            let mut st = self.state.lock();
            st.tabs.remove(&tab);
            let Some(old) = st.of_tab.remove(&tab) else {
                return;
            };
            let mut gone = false;
            if let Some(set) = st.dirs.get_mut(&old) {
                set.remove(&tab);
                if set.is_empty() {
                    st.dirs.remove(&old);
                    gone = true;
                }
            }
            if !gone {
                return;
            }
            st.dirs.keys().cloned().collect::<Vec<_>>()
        };
        let _ = self.watcher.lock().set_paths(&paths);
    }

    fn route(&self, batch: Vec<FsEvent>) {
        // Group by the watched folder each event belongs to.
        let mut per_dir: HashMap<PathBuf, Vec<FsEvent>> = HashMap::new();
        {
            let st = self.state.lock();
            for ev in batch {
                if st.dirs.contains_key(&ev.path) {
                    // The folder itself: moved, deleted or flagged for a rescan means
                    // re-list; its own metadata changing means nothing for the rows.
                    let gone = matches!(ev.kind, FsEventKind::Removed | FsEventKind::Renamed);
                    if ev.must_rescan || gone {
                        per_dir.entry(ev.path.clone()).or_default().push(FsEvent {
                            must_rescan: true,
                            ..ev
                        });
                    }
                } else if let Some(parent) = ev.path.parent()
                    && st.dirs.contains_key(parent)
                {
                    per_dir.entry(parent.to_path_buf()).or_default().push(ev);
                } else if ev.must_rescan {
                    // A rescan flag for a path we can't place: rescan everything under it.
                    for d in st.dirs.keys().filter(|d| d.starts_with(&ev.path)) {
                        per_dir.entry(d.clone()).or_default().push(FsEvent {
                            path: d.clone(),
                            ..ev.clone()
                        });
                    }
                }
            }
        }
        for (dir, events) in per_dir {
            let tabs: Vec<Arc<Tab>> = {
                let st = self.state.lock();
                st.dirs
                    .get(&dir)
                    .map(|set| {
                        set.iter()
                            .filter_map(|id| st.tabs.get(id).and_then(Weak::upgrade))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            for t in tabs {
                t.on_fs_events(&dir, &events);
            }
        }
    }
}

/// The watcher config used by the engine (F-014).
pub(crate) fn watch_config() -> WatchConfig {
    WatchConfig {
        latency: Duration::from_millis(30),
        watch_root: true,
        recursive: false,
    }
}

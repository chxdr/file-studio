//! A usage session: one scan of one folder or volume, plus everything the
//! view asks of it (the tree list's rows and selection, the treemap and its
//! zoom, the type breakdown). Scans and rescans run on background threads;
//! every method here returns quickly and never touches the disk unless its
//! docs say so.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

use crate::outline::{Move, Outline, OutlineRow};
use crate::scan::{Progress, ScanControl, ScanOptions, scan};
use crate::squarify::{Rect, Tile, TreemapOptions, hit, treemap};
use crate::tree::{NO_NODE, Node, NodeId, SizeMode, Tree};
use crate::types::{Palette, TypeStat, breakdown};

/// What changed, for [`UsageObserver::changed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageChange {
    /// A scan started (the tree may be empty until it ends).
    Started,
    /// The scan ended (done, cancelled or failed: see [`UsageSession::state`]).
    Finished,
    /// The tree changed (a folder was rescanned or an item removed).
    TreeChanged,
}

/// Receives a session's news. Called on a background thread; keep it quick.
pub trait UsageObserver: Send + Sync {
    /// Progress, about ten times a second while scanning.
    fn progress(&self, p: Progress);
    /// The scan started, ended, or the tree changed.
    fn changed(&self, change: UsageChange);
}

/// An observer that ignores everything (tests, the CLI).
pub struct NullObserver;

impl UsageObserver for NullObserver {
    fn progress(&self, _: Progress) {}
    fn changed(&self, _: UsageChange) {}
}

/// Where a session is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanState {
    /// Scanning; the tree shows the previous scan, if any.
    Scanning,
    /// Finished.
    Done,
    /// Stopped by the user; the tree holds what was read.
    Cancelled,
    /// The root could not be scanned.
    Failed(String),
}

struct Inner {
    tree: Option<Tree>,
    outline: Outline,
    selected: Option<NodeId>,
    zoom: NodeId,
    mode: SizeMode,
    palette: Palette,
    stats: Option<(NodeId, SizeMode, Arc<Vec<TypeStat>>)>,
    tiles: Arc<Vec<Tile>>,
    tiles_key: Option<(NodeId, SizeMode, u64, u64)>,
    state: ScanState,
    elapsed_ms: u64,
    rescans: u32,
}

/// One disk-usage scan and its view state.
pub struct UsageSession {
    root: PathBuf,
    opts: ScanOptions,
    observer: Arc<dyn UsageObserver>,
    inner: Mutex<Inner>,
    ctl: Mutex<Arc<ScanControl>>,
    generation: AtomicU64,
}

impl std::fmt::Debug for UsageSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsageSession")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl Drop for UsageSession {
    fn drop(&mut self) {
        self.ctl.lock().cancel();
    }
}

/// How often progress is reported.
const TICK: Duration = Duration::from_millis(100);

impl UsageSession {
    /// Starts scanning `root` in the background and returns at once.
    pub fn start(
        root: impl Into<PathBuf>,
        opts: ScanOptions,
        observer: Arc<dyn UsageObserver>,
    ) -> Arc<UsageSession> {
        let s = Arc::new(UsageSession {
            root: root.into(),
            observer,
            inner: Mutex::new(Inner {
                tree: None,
                outline: Outline::default(),
                selected: None,
                zoom: 0,
                mode: opts.mode,
                palette: Palette::default(),
                stats: None,
                tiles: Arc::new(Vec::new()),
                tiles_key: None,
                state: ScanState::Scanning,
                elapsed_ms: 0,
                rescans: 0,
            }),
            opts,
            ctl: Mutex::new(Arc::new(ScanControl::new())),
            generation: AtomicU64::new(0),
        });
        s.rescan();
        s
    }

    /// The folder or volume being scanned.
    pub fn root_path(&self) -> &Path {
        &self.root
    }

    /// Scans the whole root again (in the background).
    pub fn rescan(self: &Arc<Self>) {
        let ctl = Arc::new(ScanControl::new());
        {
            let mut cur = self.ctl.lock();
            cur.cancel();
            *cur = ctl.clone();
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.inner.lock().state = ScanState::Scanning;
        self.observer.changed(UsageChange::Started);
        let me = Arc::downgrade(self);
        let root = self.root.clone();
        let mut opts = self.opts.clone();
        opts.mode = self.inner.lock().mode;
        let observer = self.observer.clone();
        // The progress ticker.
        {
            let ctl = ctl.clone();
            let observer = observer.clone();
            let me = me.clone();
            std::thread::Builder::new()
                .name("fstudio-usage-progress".into())
                .spawn(move || {
                    loop {
                        std::thread::sleep(TICK);
                        let Some(s) = me.upgrade() else { return };
                        if s.generation.load(Ordering::SeqCst) != generation
                            || s.inner.lock().state != ScanState::Scanning
                        {
                            return;
                        }
                        drop(s);
                        observer.progress(ctl.progress());
                    }
                })
                .ok();
        }
        std::thread::Builder::new()
            .name("fstudio-usage-scan".into())
            .stack_size(64 << 20)
            .spawn(move || {
                let res = scan(&root, &opts, &ctl);
                let Some(s) = me.upgrade() else { return };
                if s.generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                let p = ctl.progress();
                {
                    let mut inner = s.inner.lock();
                    inner.elapsed_ms = p.elapsed_ms;
                    match res {
                        Ok(tree) => {
                            inner.state = if ctl.is_cancelled() {
                                ScanState::Cancelled
                            } else {
                                ScanState::Done
                            };
                            inner.install(tree);
                        }
                        Err(e) => inner.state = ScanState::Failed(e.to_string()),
                    }
                }
                observer.progress(p);
                observer.changed(UsageChange::Finished);
            })
            .ok();
    }

    /// Stops the running scan; what was read so far stays.
    pub fn cancel(&self) {
        self.ctl.lock().cancel();
    }

    /// Where the session is.
    pub fn state(&self) -> ScanState {
        self.inner.lock().state.clone()
    }

    /// How long the last full scan took, in ms.
    pub fn elapsed_ms(&self) -> u64 {
        self.inner.lock().elapsed_ms
    }

    /// The progress of the running scan.
    pub fn progress(&self) -> Progress {
        self.ctl.lock().progress()
    }

    /// Runs `f` with the tree, if a scan has finished.
    pub fn with_tree<R>(&self, f: impl FnOnce(&Tree) -> R) -> Option<R> {
        self.inner.lock().tree.as_ref().map(f)
    }

    /// The size mode.
    pub fn mode(&self) -> SizeMode {
        self.inner.lock().mode
    }

    /// Shows, sorts and lays out by `mode`.
    pub fn set_mode(&self, mode: SizeMode) {
        let mut inner = self.inner.lock();
        if inner.mode == mode {
            return;
        }
        inner.mode = mode;
        if let Some(t) = inner.tree.as_mut() {
            t.sort(mode);
        }
        inner.after_change();
    }

    // ───────────── tree list ─────────────

    /// How many rows the tree list shows.
    pub fn row_count(&self) -> usize {
        self.inner.lock().outline.rows().len()
    }

    /// Rows `start..start+count`.
    pub fn rows(&self, start: usize, count: usize) -> Vec<OutlineRow> {
        let inner = self.inner.lock();
        let rows = inner.outline.rows();
        let end = start.saturating_add(count).min(rows.len());
        rows.get(start.min(end)..end).unwrap_or(&[]).to_vec()
    }

    /// Expands or collapses a folder's row.
    pub fn set_expanded(&self, id: NodeId, open: bool) {
        let mut inner = self.inner.lock();
        let Inner { tree, outline, .. } = &mut *inner;
        if let Some(t) = tree.as_ref() {
            outline.set_expanded(t, id, open);
        }
    }

    /// The row of a node, if visible.
    pub fn row_of(&self, id: NodeId) -> Option<usize> {
        self.inner.lock().outline.row_of(id)
    }

    /// The selected node.
    pub fn selected(&self) -> Option<NodeId> {
        self.inner.lock().selected
    }

    /// Selects a node, expanding the folders above it; returns its row.
    pub fn select(&self, id: Option<NodeId>) -> Option<usize> {
        let mut inner = self.inner.lock();
        let Inner {
            tree,
            outline,
            selected,
            ..
        } = &mut *inner;
        let t = tree.as_ref()?;
        match id {
            None => {
                *selected = None;
                None
            }
            Some(id) => {
                t.get(id)?;
                *selected = Some(id);
                outline.reveal(t, id)
            }
        }
    }

    /// Moves the selection by a key; returns the new row.
    pub fn move_selection(&self, mv: Move) -> Option<usize> {
        let mut inner = self.inner.lock();
        let Inner {
            tree,
            outline,
            selected,
            ..
        } = &mut *inner;
        let t = tree.as_ref()?;
        let to = outline.step(t, *selected, mv)?;
        *selected = Some(to);
        outline.row_of(to)
    }

    // ───────────── treemap ─────────────

    /// The node the treemap shows.
    pub fn zoom(&self) -> NodeId {
        self.inner.lock().zoom
    }

    /// Shows `id` (a folder) in the whole treemap. A file zooms to its folder.
    pub fn set_zoom(&self, id: NodeId) {
        let mut inner = self.inner.lock();
        let Some(t) = inner.tree.as_ref() else { return };
        let Some(n) = t.get(id) else { return };
        let target = if n.kind.is_dir() { id } else { n.parent };
        if target != NO_NODE {
            inner.zoom = target;
            inner.tiles_key = None;
        }
    }

    /// Zooms out one level; false at the root.
    pub fn zoom_out(&self) -> bool {
        let mut inner = self.inner.lock();
        let Some(t) = inner.tree.as_ref() else {
            return false;
        };
        let p = t.node(inner.zoom).parent;
        if p == NO_NODE {
            return false;
        }
        inner.zoom = p;
        inner.tiles_key = None;
        true
    }

    /// The treemap tiles for a `width` × `height` view, cached until the
    /// size, zoom, mode or tree changes.
    pub fn layout(&self, width: f64, height: f64) -> Arc<Vec<Tile>> {
        let mut inner = self.inner.lock();
        let key = (inner.zoom, inner.mode, width.to_bits(), height.to_bits());
        if inner.tiles_key == Some(key) {
            return inner.tiles.clone();
        }
        let tiles = match inner.tree.as_ref() {
            Some(t) => {
                let palette = &inner.palette;
                treemap(
                    t,
                    inner.zoom,
                    Rect::new(0.0, 0.0, width, height),
                    inner.mode,
                    &TreemapOptions::default(),
                    &|n: &Node| palette.slot(n.ext),
                )
            }
            None => Vec::new(),
        };
        inner.tiles = Arc::new(tiles);
        inner.tiles_key = Some(key);
        inner.tiles.clone()
    }

    /// The node under a point of the last layout.
    pub fn hit(&self, x: f64, y: f64) -> Option<NodeId> {
        hit(&self.inner.lock().tiles, x, y)
    }

    /// The types with their own treemap colour, slot 1 first.
    pub fn palette(&self) -> Vec<u16> {
        self.inner.lock().palette.top.clone()
    }

    /// Totals per file type below the treemap's node, largest first.
    pub fn types(&self) -> Arc<Vec<TypeStat>> {
        let mut inner = self.inner.lock();
        let (zoom, mode) = (inner.zoom, inner.mode);
        if let Some((z, m, s)) = &inner.stats
            && *z == zoom
            && *m == mode
        {
            return s.clone();
        }
        let s = Arc::new(
            inner
                .tree
                .as_ref()
                .map(|t| breakdown(t, zoom, mode))
                .unwrap_or_default(),
        );
        inner.stats = Some((zoom, mode, s.clone()));
        s
    }

    /// The colour slot of a type.
    pub fn slot(&self, ext: u16) -> u8 {
        self.inner.lock().palette.slot(ext)
    }

    // ───────────── changes ─────────────

    /// The node at a path, if scanned.
    pub fn find(&self, path: &Path) -> Option<NodeId> {
        self.inner.lock().tree.as_ref()?.find(path)
    }

    /// The full path of a node.
    pub fn path(&self, id: NodeId) -> Option<PathBuf> {
        let inner = self.inner.lock();
        let t = inner.tree.as_ref()?;
        t.get(id)?;
        Some(t.path(id))
    }

    /// Rescans one folder (in the background) and puts the result in place.
    pub fn refresh(self: &Arc<Self>, id: NodeId) {
        let Some(path) = self.path(id) else { return };
        let is_dir = self
            .with_tree(|t| t.node(id).kind.is_dir())
            .unwrap_or(false);
        if !is_dir || id == self.with_tree(|t| t.root()).unwrap_or(0) {
            if is_dir {
                self.rescan();
            }
            return;
        }
        let me = Arc::downgrade(self);
        let mut opts = self.opts.clone();
        opts.mode = self.mode();
        let observer = self.observer.clone();
        let generation = self.generation.load(Ordering::SeqCst);
        std::thread::Builder::new()
            .name("fstudio-usage-refresh".into())
            .stack_size(64 << 20)
            .spawn(move || {
                let ctl = ScanControl::new();
                let res = scan(&path, &opts, &ctl);
                let Some(s) = me.upgrade() else { return };
                if s.generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                {
                    let mut inner = s.inner.lock();
                    let Some(tree) = inner.tree.as_mut() else {
                        return;
                    };
                    // Find it again: the tree may have changed meanwhile.
                    let Some(id) = tree.find(&path) else { return };
                    match res {
                        Ok(sub) => tree.graft(id, sub),
                        Err(_) => {
                            tree.remove(id);
                        }
                    }
                    inner.rescans += 1;
                    inner.after_change();
                }
                observer.changed(UsageChange::TreeChanged);
            })
            .ok();
    }

    /// Brings the tree in line with the disk for these paths (after a trash,
    /// an undo or a move): items that are gone are removed, items that are
    /// new get their folder rescanned. Touches the disk (`lstat` per path).
    pub fn sync_paths(self: &Arc<Self>, paths: &[PathBuf]) {
        let mut removed = false;
        let mut to_refresh = Vec::new();
        {
            let mut inner = self.inner.lock();
            let Some(tree) = inner.tree.as_mut() else {
                return;
            };
            for p in paths {
                let exists = p.symlink_metadata().is_ok();
                match (tree.find(p), exists) {
                    (Some(id), false) => removed |= tree.remove(id),
                    (None, true) => {
                        if let Some(parent) = p.parent()
                            && let Some((near, _)) = tree.find_nearest(parent)
                        {
                            to_refresh.push(near);
                        }
                    }
                    _ => {}
                }
            }
            if removed {
                inner.after_change();
            }
        }
        if removed {
            self.observer.changed(UsageChange::TreeChanged);
        }
        to_refresh.sort_unstable();
        to_refresh.dedup();
        for id in to_refresh {
            self.refresh(id);
        }
    }

    /// Takes a node out of the tree (it was trashed), fixing the totals.
    pub fn remove(&self, id: NodeId) -> bool {
        let ok = {
            let mut inner = self.inner.lock();
            let ok = inner.tree.as_mut().is_some_and(|t| t.remove(id));
            if ok {
                inner.after_change();
            }
            ok
        };
        if ok {
            self.observer.changed(UsageChange::TreeChanged);
        }
        ok
    }

    /// How many subtree rescans have been applied (for tests).
    pub fn rescans(&self) -> u32 {
        self.inner.lock().rescans
    }
}

impl Inner {
    fn install(&mut self, tree: Tree) {
        // Keep the view's place when the same root is scanned again.
        let old = self.tree.take();
        let keep = |id: Option<NodeId>| -> Option<PathBuf> {
            let t = old.as_ref()?;
            let id = id?;
            t.get(id)?;
            Some(t.path(id))
        };
        let sel = keep(self.selected);
        let zoom = keep(Some(self.zoom));
        self.outline = Outline::new(&tree);
        self.selected = sel.and_then(|p| tree.find(&p));
        self.zoom = zoom.and_then(|p| tree.find(&p)).unwrap_or(tree.root());
        if let Some(s) = self.selected {
            self.outline.reveal(&tree, s);
        }
        self.tree = Some(tree);
        self.after_change();
    }

    /// Fix selection, zoom, palette and caches after the tree changed.
    fn after_change(&mut self) {
        let Some(t) = self.tree.as_ref() else { return };
        // A removed selection moves to its nearest remaining folder.
        if let Some(mut s) = self.selected {
            while s != NO_NODE && t.get(s).is_none() {
                s = t.node(s).parent;
            }
            self.selected = (s != NO_NODE).then_some(s);
        }
        if t.get(self.zoom).is_none() {
            self.zoom = t.root();
        }
        self.outline.rebuild(t);
        self.palette = Palette::from_stats(&breakdown(t, t.root(), self.mode));
        self.stats = None;
        self.tiles_key = None;
    }
}

//! A tab: one location, its history, view state, selection and the listing
//! pipeline (PROPOSAL §6).
//!
//! Navigating starts a background lister thread. The first chunk publishes a
//! `Partial` listing and the end publishes the `Sorted` one; results from an
//! older navigation are dropped by a generation counter. Watcher events patch
//! the snapshot incrementally (`Patched`) or trigger a re-list. Re-sorting runs
//! off the calling thread. The selection is carried to every new listing
//! version by name.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use fstudio_fs::{FsEvent, Lister};
use fstudio_path::{Completions, Crumb, Location, ParseContext, parse_location};
use fstudio_view::{Change, ListingBuilder, Move, Selection};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::events::{ChangeKind, CoreEvent};
use crate::ids::TabId;
use crate::listing::{self, Listing, ListingState, RowFormat, RowInfo};
use crate::outcome::{Outcome, ShellRequest};
use crate::probe::FsCoreProbe;
use crate::shared::Shared;
use crate::view::ViewState;

/// How often a still-loading listing republishes its partial rows.
const PARTIAL_EVERY: Duration = Duration::from_millis(100);
/// A watcher batch touching more than this share of the folder triggers a re-list.
const RESCAN_FRACTION: f64 = 0.05;
/// …but never for fewer names than this.
const RESCAN_MIN: usize = 64;
/// History entries kept per direction.
const HISTORY_MAX: usize = 100;

/// Where the shell last scrolled to, so it can be restored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScrollAnchor {
    /// The name of the top visible row.
    pub name: String,
    /// The offset of that row from the top, in points.
    pub offset: f64,
}

/// A tab's persistent state (session restore, saved workspaces).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TabState {
    /// The tab.
    pub id: TabId,
    /// The folder shown.
    pub location: String,
    /// Back history, oldest first.
    pub back: Vec<String>,
    /// Forward history, nearest first.
    pub forward: Vec<String>,
    /// View options.
    pub view: ViewState,
    /// The filter text.
    pub filter: String,
    /// Selected names.
    pub selected: Vec<String>,
    /// The cursor's name.
    pub cursor: Option<String>,
    /// Where the shell was scrolled to.
    pub scroll: Option<ScrollAnchor>,
}

impl Default for TabState {
    fn default() -> Self {
        Self {
            id: TabId(0),
            location: "/".into(),
            back: Vec::new(),
            forward: Vec::new(),
            view: ViewState::default(),
            filter: String::new(),
            selected: Vec::new(),
            cursor: None,
            scroll: None,
        }
    }
}

/// The selection as the shell sees it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionInfo {
    /// Selected rows, ascending.
    pub rows: Vec<u32>,
    /// Their names, in row order.
    pub names: Vec<String>,
    /// The cursor row.
    pub cursor: Option<u32>,
    /// The fixed end of a Shift range.
    pub anchor: Option<u32>,
    /// The listing version these rows refer to.
    pub version: u64,
}

/// A cursor movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CursorMove {
    /// One row up.
    Up,
    /// One row down.
    Down,
    /// One page up (the visible row count).
    PageUp,
    /// One page down.
    PageDown,
    /// The first row.
    Home,
    /// The last row.
    End,
}

/// A change to the selection, from a click or a key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectionChange {
    /// Plain click: only this row.
    Only(u32),
    /// ⌘-click: toggle this row.
    Toggle(u32),
    /// ⇧-click: the range from the anchor.
    ExtendTo(u32),
    /// Every row.
    All,
    /// No row (the cursor stays).
    None,
    /// Swap selected and unselected.
    Invert,
    /// Exactly these rows (a rubber-band drag); the cursor goes to the first.
    Rows(Vec<u32>),
    /// Exactly these names, where present; the cursor goes to the first found.
    Names(Vec<String>),
    /// Move the cursor, extending the selection when `extend` is set.
    Move {
        /// Which way.
        by: CursorMove,
        /// Shift held (or Vim visual mode).
        extend: bool,
    },
}

/// The history step a navigation makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hist {
    Push,
    Keep,
}

struct TypeAhead {
    text: String,
    from: usize,
    at: Instant,
}

struct Inner {
    location: PathBuf,
    canonical: PathBuf,
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
    view: ViewState,
    filter: String,
    listing: Arc<Listing>,
    selection: Selection,
    /// Names to select once they appear, and the cursor's name.
    pending: Option<(Vec<String>, Option<String>)>,
    /// A pending selection from an operation (paste, new folder) waits for
    /// its items across several watcher patches, until this time.
    pending_until: Option<Instant>,
    generation: u64,
    view_generation: u64,
    version: u64,
    scroll: Option<ScrollAnchor>,
    visible: (u32, u32),
    type_ahead: Option<TypeAhead>,
    /// Watcher changes that arrived while the folder was still being read.
    queued: Vec<Change>,
    closed: bool,
    /// The next navigation keeps the current view mode (column view browsing).
    keep_mode: bool,
}

/// One tab. Every method is quick and never blocks on disk, except where noted.
pub struct Tab {
    id: TabId,
    me: Weak<Tab>,
    shared: Arc<Shared>,
    st: Mutex<Inner>,
    /// A tab-less listing (`ListingHandle`): not in the workspace, so it never
    /// emits `TabsChanged`, marks the session dirty or records view memory.
    detached: bool,
}

impl std::fmt::Debug for Tab {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tab").field("id", &self.id).finish()
    }
}

fn to_s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

impl Tab {
    pub(crate) fn create(shared: Arc<Shared>, state: TabState) -> Arc<Tab> {
        Self::create_with(shared, state, false)
    }

    /// Creates a tab, or a detached listing (see [`crate::ListingHandle`]).
    pub(crate) fn create_with(shared: Arc<Shared>, state: TabState, detached: bool) -> Arc<Tab> {
        let id = if state.id.0 == 0 {
            TabId(shared.ids.next())
        } else {
            shared.ids.bump_past(state.id.0);
            state.id
        };
        let location = PathBuf::from(&state.location);
        let opts = state.view.options(&state.filter);
        let tab = Arc::new_cyclic(|me| Tab {
            id,
            me: me.clone(),
            shared,
            detached,
            st: Mutex::new(Inner {
                canonical: location.clone(),
                listing: Arc::new(listing::empty(&location, opts, 0)),
                location: location.clone(),
                back: state.back.iter().map(PathBuf::from).collect(),
                forward: state.forward.iter().map(PathBuf::from).collect(),
                view: state.view.clone(),
                filter: state.filter.clone(),
                selection: Selection::new(),
                pending: None,
                pending_until: None,
                generation: 0,
                view_generation: 0,
                version: 0,
                scroll: state.scroll.clone(),
                visible: (0, 40),
                type_ahead: None,
                queued: Vec::new(),
                closed: false,
                keep_mode: false,
            }),
        });
        let pending = (!state.selected.is_empty() || state.cursor.is_some())
            .then(|| (state.selected.clone(), state.cursor.clone()));
        tab.start(location, Hist::Keep, pending, true, false);
        tab
    }

    fn arc(&self) -> Arc<Tab> {
        self.me.upgrade().expect("tab is alive while a method runs")
    }

    /// The tab's id.
    pub fn id(&self) -> TabId {
        self.id
    }

    /// The folder shown.
    pub fn location(&self) -> String {
        to_s(&self.st.lock().location)
    }

    pub(crate) fn location_path(&self) -> PathBuf {
        self.st.lock().location.clone()
    }

    /// The current snapshot. Never blocks.
    pub fn listing(&self) -> Arc<Listing> {
        self.st.lock().listing.clone()
    }

    /// The view options (without the filter).
    pub fn view(&self) -> ViewState {
        self.st.lock().view.clone()
    }

    /// The filter text.
    pub fn filter(&self) -> String {
        self.st.lock().filter.clone()
    }

    /// Back and forward history (oldest first, nearest first).
    pub fn history(&self) -> (Vec<String>, Vec<String>) {
        let st = self.st.lock();
        (
            st.back.iter().map(|p| to_s(p)).collect(),
            st.forward.iter().rev().map(|p| to_s(p)).collect(),
        )
    }

    /// Can go back / forward.
    pub fn can_go(&self) -> (bool, bool) {
        let st = self.st.lock();
        (!st.back.is_empty(), !st.forward.is_empty())
    }

    /// The persistent state.
    pub fn state(&self) -> TabState {
        let st = self.st.lock();
        let l = &st.listing;
        TabState {
            id: self.id,
            location: to_s(&st.location),
            back: st.back.iter().map(|p| to_s(p)).collect(),
            forward: st.forward.iter().map(|p| to_s(p)).collect(),
            view: st.view.clone(),
            filter: st.filter.clone(),
            selected: match &st.pending {
                Some((names, _)) if st.selection.is_empty() => names.clone(),
                _ => st.selection.names(&l.inner),
            },
            cursor: st
                .selection
                .cursor()
                .and_then(|c| l.inner.entry(c).map(|e| e.name.clone()))
                .or_else(|| st.pending.as_ref().and_then(|p| p.1.clone())),
            scroll: st.scroll.clone(),
        }
    }

    // ───────────────────────── navigation ─────────────────────────

    /// Goes to what the user typed or pasted (through `parse_location`). A
    /// folder is shown; a file is selected in its folder; a missing path
    /// returns `OfferCreateFolder`. Records a frecency visit.
    ///
    /// Blocks briefly on `lstat`/`realpath` of the typed path's components.
    pub fn go(&self, raw: &str) -> Result<Outcome> {
        let cwd = self.location_path();
        let parsed = parse_location(raw, &self.parse_context(cwd.clone()));
        let out = Outcome::none().with_tab(self.id);
        let chip = parsed.chip_text();
        match parsed.location {
            Location::Dir(p) => {
                // Going up (a crumb, `..`): select the child we came from.
                let select = child_on_way(&p, &cwd);
                self.navigate(p, select);
                Ok(Outcome {
                    message: chip,
                    ..out
                })
            }
            Location::File { dir, name } => {
                self.navigate(dir, Some((vec![name.clone()], Some(name))));
                Ok(Outcome {
                    message: chip,
                    ..out
                })
            }
            Location::Missing {
                path,
                nearest_existing,
            } => Ok(Outcome {
                shell: Some(ShellRequest::OfferCreateFolder {
                    path: to_s(&path),
                    nearest_existing: to_s(&nearest_existing),
                }),
                message: Some(format!("“{}” doesn’t exist", to_s(&path))),
                ..out
            }),
            Location::Remote(url) => Err(CoreError::NotAvailable(format!(
                "remote locations are not supported yet: {url}"
            ))),
        }
    }

    pub(crate) fn parse_context(&self, cwd: PathBuf) -> ParseContext {
        ParseContext::new(cwd, self.shared.home.clone(), Box::new(FsCoreProbe))
            .with_env(|k| std::env::var(k).ok())
            .with_user_home(|u| {
                if u.is_empty() || u.contains('/') || u.contains('\0') {
                    return None;
                }
                let p = PathBuf::from("/Users").join(u);
                p.is_dir().then_some(p)
            })
    }

    /// Shows `dir` (already resolved), pushing history and recording frecency.
    pub(crate) fn navigate(&self, dir: PathBuf, select: Option<(Vec<String>, Option<String>)>) {
        self.shared.frecency.visit(&dir);
        self.start(dir, Hist::Push, select, true, false);
    }

    /// Like [`Tab::navigate`], keeping the view mode (column view browsing).
    pub(crate) fn navigate_keeping_mode(
        &self,
        dir: PathBuf,
        select: Option<(Vec<String>, Option<String>)>,
    ) {
        self.st.lock().keep_mode = true;
        self.navigate(dir, select);
    }

    /// Goes back in history.
    pub fn back(&self) -> Result<Outcome> {
        let (to, from) = {
            let mut st = self.st.lock();
            let Some(to) = st.back.pop() else {
                return Ok(Outcome::none()); // as Finder: nothing happens
            };
            let cur = st.location.clone();
            st.forward.push(cur.clone());
            (to, cur)
        };
        let select = child_name(&to, &from);
        self.start(to, Hist::Keep, select, true, false);
        Ok(Outcome::none().with_tab(self.id))
    }

    /// Goes forward in history.
    pub fn forward(&self) -> Result<Outcome> {
        let to = {
            let mut st = self.st.lock();
            let Some(to) = st.forward.pop() else {
                return Ok(Outcome::none());
            };
            let cur = st.location.clone();
            st.back.push(cur);
            to
        };
        self.start(to, Hist::Keep, None, true, false);
        Ok(Outcome::none().with_tab(self.id))
    }

    /// Goes to the enclosing folder and selects the folder we came from.
    pub fn up(&self) -> Result<Outcome> {
        let cur = self.location_path();
        let Some(parent) = cur.parent().map(Path::to_path_buf) else {
            return Ok(Outcome::none());
        };
        let name = cur.file_name().map(|n| n.to_string_lossy().into_owned());
        self.shared.frecency.visit(&parent);
        self.start(
            parent,
            Hist::Push,
            name.map(|n| (vec![n.clone()], Some(n))),
            true,
            false,
        );
        Ok(Outcome::none().with_tab(self.id))
    }

    /// Reads the folder again, keeping the rows on screen until the new
    /// listing is ready, and keeping the selection.
    pub fn refresh(&self) {
        let loc = self.location_path();
        self.start(loc, Hist::Keep, None, false, true);
    }

    /// Starts listing `dir`. `partial` publishes rows before the end;
    /// `keep_rows` leaves the current rows up until the new listing is ready.
    fn start(
        &self,
        dir: PathBuf,
        hist: Hist,
        select: Option<(Vec<String>, Option<String>)>,
        partial: bool,
        keep_rows: bool,
    ) {
        let (gen_, opts) = {
            let mut st = self.st.lock();
            if st.closed {
                return;
            }
            let moved = st.location != dir;
            if moved && hist == Hist::Push {
                let cur = st.location.clone();
                st.back.push(cur);
                if st.back.len() > HISTORY_MAX {
                    st.back.remove(0);
                }
                st.forward.clear();
            }
            if moved {
                // A new folder: its own remembered view (sort, mode), a clean filter.
                let show_hidden = st.view.show_hidden;
                let defaults = ViewState::from_settings(&self.shared.settings.read().view);
                let mut v = self.shared.folder_view(&dir).unwrap_or(defaults);
                v.show_hidden = show_hidden;
                if std::mem::take(&mut st.keep_mode) {
                    v.mode = st.view.mode;
                }
                st.view = v;
                st.filter.clear();
                st.scroll = None;
            }
            st.generation += 1;
            st.location = dir.clone();
            st.queued.clear();
            st.type_ahead = None;
            if select.is_some() {
                st.pending = select;
                st.pending_until = None;
            }
            let opts = st.view.options(&st.filter);
            if !keep_rows {
                st.version += 1;
                let mut l = listing::empty(&dir, opts.clone(), st.version);
                if moved {
                    st.selection = Selection::new();
                } else {
                    // Same folder listed again from scratch: keep the selection by name.
                    let names = st.selection.names(&st.listing.inner);
                    let cursor = st
                        .selection
                        .cursor()
                        .and_then(|c| st.listing.inner.entry(c).map(|e| e.name.clone()));
                    if st.pending.is_none() && (!names.is_empty() || cursor.is_some()) {
                        st.pending = Some((names, cursor));
                    }
                    st.selection = Selection::new();
                }
                l.dir = dir.clone();
                st.listing = Arc::new(l);
            }
            (st.generation, opts)
        };
        if !self.detached {
            self.shared.emit(CoreEvent::TabsChanged);
            self.shared.mark_dirty();
        }
        let tab = self.arc();
        let spawned = std::thread::Builder::new()
            .name("fstudio-lister".into())
            .spawn(move || tab.list_job(gen_, dir, opts, partial));
        if let Err(e) = spawned {
            let l = self.listing();
            self.publish_error(gen_, (*l).clone(), &e.to_string());
        }
    }

    fn is_current(&self, gen_: u64) -> bool {
        let st = self.st.lock();
        st.generation == gen_ && !st.closed
    }

    /// The background half of a navigation.
    fn list_job(
        self: Arc<Self>,
        gen_: u64,
        dir: PathBuf,
        opts: fstudio_view::ViewOptions,
        partial: bool,
    ) {
        let canonical = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        {
            let mut st = self.st.lock();
            if st.generation != gen_ {
                return;
            }
            st.canonical = canonical.clone();
        }
        // Watch before listing, so no change between the two is missed.
        self.shared.watch.register(&self, &canonical);
        let mut lister = match Lister::open(&dir) {
            Ok(l) => l,
            Err(e) => {
                let base = listing::empty(&dir, opts, 0);
                self.publish_error(gen_, base, &e.to_string());
                return;
            }
        };
        let mut b = ListingBuilder::new(opts);
        let mut first = true;
        let mut last = Instant::now();
        while let Some(chunk) = lister.next_chunk() {
            if !self.is_current(gen_) {
                return; // stale: a newer navigation won
            }
            match chunk {
                Ok(entries) => b.push_chunk(entries),
                Err(e) => {
                    let l = b.finish();
                    let base = Listing {
                        version: 0,
                        dir: dir.clone(),
                        state: ListingState::Error,
                        error: None,
                        inner: l,
                    };
                    self.publish_error(gen_, base, &e.to_string());
                    return;
                }
            }
            if partial && (first || last.elapsed() >= PARTIAL_EVERY) {
                first = false;
                last = Instant::now();
                let snap = b.snapshot();
                self.publish(gen_, None, snap, ListingState::Loading, ChangeKind::Partial);
            }
        }
        let l = b.finish();
        self.publish(gen_, None, l, ListingState::Complete, ChangeKind::Sorted);
    }

    fn publish_error(&self, gen_: u64, base: Listing, msg: &str) {
        let mut st = self.st.lock();
        if st.generation != gen_ || st.closed {
            return;
        }
        st.version += 1;
        let l = Listing {
            version: st.version,
            dir: st.location.clone(),
            state: ListingState::Error,
            error: Some(msg.to_string()),
            inner: base.inner,
        };
        st.listing = Arc::new(l);
        st.pending = None;
        let v = st.version;
        drop(st);
        self.shared.emit(CoreEvent::ListingChanged {
            tab: self.id,
            version: v,
            kind: ChangeKind::Error,
        });
    }

    /// Installs a new snapshot if it is still current, carrying the selection over.
    /// `base_version` (for patches and re-sorts) must still be the current version.
    fn publish(
        &self,
        gen_: u64,
        base_version: Option<u64>,
        mut l: fstudio_view::Listing,
        state: ListingState,
        kind: ChangeKind,
    ) -> bool {
        let mut st = self.st.lock();
        if st.generation != gen_ || st.closed {
            return false;
        }
        if let Some(v) = base_version
            && st.listing.version != v
        {
            return false;
        }
        let want = st.view.options(&st.filter);
        if *l.options() != want {
            l = l.with_options(want);
        }
        if state == ListingState::Complete && !st.queued.is_empty() {
            let q = std::mem::take(&mut st.queued);
            l = l.apply(q);
        }
        let old_sel = st.selection.clone();
        let mut sel = st.selection.remap(&st.listing.inner, &l);
        let final_ = state != ListingState::Loading;
        if let Some((names, cursor)) = st.pending.take() {
            let mut left = Vec::new();
            for n in &names {
                match l.index_of(n) {
                    Some(i) if !sel.is_selected(i) => sel.toggle(i),
                    Some(_) => {}
                    None => left.push(n.clone()),
                }
            }
            if let Some(c) = cursor.as_ref().and_then(|c| l.index_of(c)) {
                let rows: Vec<usize> = sel.selected().collect();
                sel = make_selection(&rows, Some(c));
            }
            let cursor_left = cursor.filter(|c| l.index_of(c).is_none());
            let waiting = st.pending_until.is_some_and(|u| Instant::now() < u);
            if (!final_ || waiting) && (!left.is_empty() || cursor_left.is_some()) {
                st.pending = Some((left, cursor_left));
            } else {
                st.pending_until = None;
            }
        }
        st.version += 1;
        let version = st.version;
        st.listing = Arc::new(Listing {
            version,
            dir: st.location.clone(),
            state,
            error: None,
            inner: l,
        });
        let sel_changed = sel != old_sel;
        st.selection = sel;
        let (count, cursor) = (st.selection.count() as u32, st.selection.cursor());
        drop(st);
        self.shared.emit(CoreEvent::ListingChanged {
            tab: self.id,
            version,
            kind,
        });
        if sel_changed {
            self.shared.emit(CoreEvent::SelectionChanged {
                tab: self.id,
                count,
                cursor: cursor.map(|c| c as u32),
            });
        }
        true
    }

    /// Watcher events for this tab's folder (called on the router thread).
    pub(crate) fn on_fs_events(&self, canonical: &Path, events: &[FsEvent]) {
        let (gen_, dir, total, loading) = {
            let st = self.st.lock();
            if st.canonical != canonical || st.closed {
                return;
            }
            (
                st.generation,
                st.location.clone(),
                st.listing.inner.total_len(),
                st.listing.state == ListingState::Loading,
            )
        };
        if events.iter().any(|e| e.must_rescan) {
            self.rescan(gen_);
            return;
        }
        let names: BTreeSet<String> = events
            .iter()
            .filter_map(|e| e.path.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .collect();
        let limit = RESCAN_MIN.max((total as f64 * RESCAN_FRACTION) as usize);
        if names.len() > limit {
            self.rescan(gen_);
            return;
        }
        let changes: Vec<Change> = names
            .into_iter()
            .filter_map(|n| match fstudio_fs::stat_entry(dir.join(&n)) {
                Ok(e) => Some(Change::Upsert(e)),
                Err(e) if e.kind() == Some(std::io::ErrorKind::NotFound) => Some(Change::Remove(n)),
                Err(_) => None,
            })
            .collect();
        if changes.is_empty() {
            return;
        }
        // Keep the listing's dir name for stat'd entries: the dir is the tab's spelling.
        let mut st = self.st.lock();
        if st.generation != gen_ || st.closed {
            return;
        }
        if loading || st.listing.state == ListingState::Loading {
            st.queued.extend(changes);
            return;
        }
        let base = st.listing.clone();
        drop(st);
        let next = base.inner.apply(changes.clone());
        if !self.publish(
            gen_,
            Some(base.version),
            next,
            base.state,
            ChangeKind::Patched,
        ) {
            // Something else replaced the listing meanwhile; apply on top of that.
            let st = self.st.lock();
            if st.generation != gen_ {
                return;
            }
            let cur = st.listing.clone();
            drop(st);
            let next = cur.inner.apply(changes);
            self.publish(
                gen_,
                Some(cur.version),
                next,
                cur.state,
                ChangeKind::Patched,
            );
        }
    }

    fn rescan(&self, gen_: u64) {
        if !self.is_current(gen_) {
            return;
        }
        let loc = self.location_path();
        self.start(loc, Hist::Keep, None, false, true);
    }

    pub(crate) fn close(&self) {
        let mut st = self.st.lock();
        st.closed = true;
        st.generation += 1;
        drop(st);
        self.shared.watch.unregister(self.id);
    }

    // ───────────────────────── view ─────────────────────────

    /// Changes sort, hidden files, folders-first or view mode. Re-sorts off the
    /// calling thread and publishes `Resorted`.
    pub fn set_view(&self, v: ViewState) {
        let remember = {
            let mut st = self.st.lock();
            if st.view == v {
                return;
            }
            let r = st.view.sort != v.sort
                || st.view.ascending != v.ascending
                || st.view.folders_first != v.folders_first
                || st.view.mode != v.mode
                || st.view.columns != v.columns;
            st.view = v.clone();
            r.then(|| st.location.clone())
        };
        if let Some(dir) = remember.filter(|_| !self.detached) {
            self.shared.remember_view(&dir, &v);
        }
        self.shared.mark_dirty();
        self.resort();
    }

    /// Sets the filter text (filter-as-you-type). Re-filters off the calling thread.
    pub fn set_filter(&self, text: &str) {
        {
            let mut st = self.st.lock();
            if st.filter == text {
                return;
            }
            st.filter = text.to_string();
        }
        self.shared.mark_dirty();
        self.resort();
    }

    /// Like [`Tab::set_filter`] but computes the new listing on this thread
    /// and returns once it is installed. For tools and benchmarks.
    pub fn set_filter_now(&self, text: &str) {
        let (gen_, base, opts) = {
            let mut st = self.st.lock();
            st.filter = text.to_string();
            st.view_generation += 1;
            (
                st.generation,
                st.listing.clone(),
                st.view.options(&st.filter),
            )
        };
        let next = base.inner.with_options(opts);
        self.publish(
            gen_,
            Some(base.version),
            next,
            base.state,
            ChangeKind::Resorted,
        );
    }

    fn resort(&self) {
        let (gen_, vgen, base, opts) = {
            let mut st = self.st.lock();
            st.view_generation += 1;
            (
                st.generation,
                st.view_generation,
                st.listing.clone(),
                st.view.options(&st.filter),
            )
        };
        let tab = self.arc();
        let _ = std::thread::Builder::new()
            .name("fstudio-resort".into())
            .spawn(move || {
                let next = base.inner.with_options(opts.clone());
                if tab.st.lock().view_generation != vgen {
                    return; // a newer view change will publish
                }
                if !tab.publish(
                    gen_,
                    Some(base.version),
                    next,
                    base.state,
                    ChangeKind::Resorted,
                ) {
                    // Patched meanwhile: re-sort the newer snapshot.
                    let cur = tab.listing();
                    if tab.st.lock().view_generation != vgen {
                        return;
                    }
                    let next = cur.inner.with_options(opts);
                    tab.publish(
                        gen_,
                        Some(cur.version),
                        next,
                        cur.state,
                        ChangeKind::Resorted,
                    );
                }
            });
    }

    // ───────────────────────── selection ─────────────────────────

    /// The current selection.
    pub fn selection(&self) -> SelectionInfo {
        let st = self.st.lock();
        let l = &st.listing.inner;
        SelectionInfo {
            rows: st.selection.selected().map(|i| i as u32).collect(),
            names: st.selection.names(l),
            cursor: st.selection.cursor().map(|c| c as u32),
            anchor: st.selection.anchor().map(|c| c as u32),
            version: st.listing.version,
        }
    }

    /// Paths of the selected items (or the cursor row when nothing is selected).
    pub fn selected_paths(&self) -> Vec<String> {
        self.selected_pathbufs().iter().map(|p| to_s(p)).collect()
    }

    pub(crate) fn selected_pathbufs(&self) -> Vec<PathBuf> {
        let st = self.st.lock();
        let l = &st.listing;
        let mut rows: Vec<usize> = st.selection.selected().collect();
        if rows.is_empty()
            && let Some(c) = st.selection.cursor()
        {
            rows.push(c);
        }
        rows.into_iter().filter_map(|i| l.path_of(i)).collect()
    }

    /// Changes the selection and emits `SelectionChanged`.
    pub fn set_selection(&self, change: SelectionChange) {
        let mut st = self.st.lock();
        let before = st.selection.clone();
        let len = st.listing.inner.len();
        let page = st.visible.1.max(1) as usize;
        let l = st.listing.clone();
        let sel = &mut st.selection;
        let ok = |i: u32| (i as usize) < len;
        match change {
            SelectionChange::Only(i) if ok(i) => sel.select_only(i as usize),
            SelectionChange::Toggle(i) if ok(i) => sel.toggle(i as usize),
            SelectionChange::ExtendTo(i) if ok(i) => sel.extend_to(i as usize),
            SelectionChange::All => sel.select_all(len),
            SelectionChange::None => sel.select_none(),
            SelectionChange::Invert => sel.invert(len),
            SelectionChange::Rows(rows) => {
                let rows: Vec<usize> = rows
                    .into_iter()
                    .filter(|&i| ok(i))
                    .map(|i| i as usize)
                    .collect();
                *sel = make_selection(&rows, rows.first().copied());
            }
            SelectionChange::Names(names) => {
                let rows: Vec<usize> = names.iter().filter_map(|n| l.inner.index_of(n)).collect();
                *sel = make_selection(&rows, rows.first().copied());
            }
            SelectionChange::Move { by, extend } => {
                let mv = match by {
                    CursorMove::Up => Move::Up,
                    CursorMove::Down => Move::Down,
                    CursorMove::PageUp => Move::PageUp,
                    CursorMove::PageDown => Move::PageDown,
                    CursorMove::Home => Move::Home,
                    CursorMove::End => Move::End,
                };
                sel.move_cursor(len, mv, page, extend);
            }
            _ => {}
        }
        st.type_ahead = None;
        let changed = st.selection != before;
        let (count, cursor) = (st.selection.count() as u32, st.selection.cursor());
        drop(st);
        if changed {
            self.shared.emit(CoreEvent::SelectionChanged {
                tab: self.id,
                count,
                cursor: cursor.map(|c| c as u32),
            });
            self.shared.mark_dirty();
        }
    }

    /// Selects (or deselects) rows whose names match a glob (`*.png`).
    pub fn select_pattern(&self, pattern: &str, select: bool) {
        let mut st = self.st.lock();
        let l = st.listing.clone();
        let before = st.selection.clone();
        if select {
            st.selection.select_glob(&l.inner, pattern);
        } else {
            st.selection.deselect_glob(&l.inner, pattern);
        }
        let changed = before != st.selection;
        let (count, cursor) = (st.selection.count() as u32, st.selection.cursor());
        drop(st);
        if changed {
            self.shared.emit(CoreEvent::SelectionChanged {
                tab: self.id,
                count,
                cursor: cursor.map(|c| c as u32),
            });
        }
    }

    /// Type-to-select with the whole typed text: selects and returns the first
    /// row from where the typing started whose name starts with `text`.
    pub fn type_select(&self, text: &str) -> Option<u32> {
        let timeout = Duration::from_millis(
            self.shared.settings.read().general.type_select_timeout_ms as u64,
        );
        let mut st = self.st.lock();
        let now = Instant::now();
        let from = match &st.type_ahead {
            Some(t) if text.starts_with(&t.text) && now.duration_since(t.at) < timeout => t.from,
            _ => st.selection.cursor().unwrap_or(0),
        };
        st.type_ahead = Some(TypeAhead {
            text: text.to_string(),
            from,
            at: now,
        });
        let l = st.listing.clone();
        let row = fstudio_view::type_select(&l.inner, text, from)?;
        st.selection.select_only(row);
        drop(st);
        self.shared.emit(CoreEvent::SelectionChanged {
            tab: self.id,
            count: 1,
            cursor: Some(row as u32),
        });
        Some(row as u32)
    }

    /// Adds one typed character to the type-to-select text (a key nobody binds).
    pub(crate) fn type_char(&self, c: char) -> Option<u32> {
        let timeout = Duration::from_millis(
            self.shared.settings.read().general.type_select_timeout_ms as u64,
        );
        let text = {
            let st = self.st.lock();
            match &st.type_ahead {
                Some(t) if t.at.elapsed() < timeout => format!("{}{c}", t.text),
                _ => c.to_string(),
            }
        };
        self.type_select(&text)
    }

    /// Selects the item that follows `removed` once they are gone (Finder's
    /// behaviour after Trash): returns their paths.
    pub(crate) fn select_after_removing(&self, removed: &[PathBuf]) {
        let mut st = self.st.lock();
        let l = st.listing.clone();
        let rows: Vec<usize> = removed
            .iter()
            .filter(|p| p.parent() == Some(l.dir.as_path()))
            .filter_map(|p| p.file_name())
            .filter_map(|n| l.inner.index_of(&n.to_string_lossy()))
            .collect();
        if rows.is_empty() {
            return;
        }
        let mut probe = st.selection.clone();
        let new_row = probe.after_remove(&rows, l.inner.len());
        // Map the new-listing row back to the name in the current listing.
        let follower = new_row.and_then(|r| {
            let gone: BTreeSet<usize> = rows.iter().copied().collect();
            (0..l.inner.len())
                .filter(|i| !gone.contains(i))
                .nth(r)
                .and_then(|i| l.inner.entry(i).map(|e| e.name.clone()))
        });
        st.selection = Selection::new();
        if let Some(name) = follower
            && let Some(i) = l.inner.index_of(&name)
        {
            st.selection.select_only(i);
        }
        let (count, cursor) = (st.selection.count() as u32, st.selection.cursor());
        drop(st);
        self.shared.emit(CoreEvent::SelectionChanged {
            tab: self.id,
            count,
            cursor: cursor.map(|c| c as u32),
        });
    }

    /// Selects `names` as soon as they appear (after a create, paste or rename).
    pub(crate) fn select_when_present(&self, names: Vec<String>) {
        let mut st = self.st.lock();
        let l = st.listing.clone();
        let found: Vec<usize> = names.iter().filter_map(|n| l.inner.index_of(n)).collect();
        if found.len() == names.len() && !found.is_empty() {
            st.selection = make_selection(&found, Some(found[0]));
            let (count, cursor) = (st.selection.count() as u32, st.selection.cursor());
            drop(st);
            self.shared.emit(CoreEvent::SelectionChanged {
                tab: self.id,
                count,
                cursor: cursor.map(|c| c as u32),
            });
        } else {
            let cursor = names.first().cloned();
            st.pending = Some((names, cursor));
            st.pending_until = Some(Instant::now() + Duration::from_secs(5));
            // Applied by the watcher patches that bring the items in.
            st.selection = Selection::new();
        }
    }

    // ───────────────────────── rows and shell state ─────────────────────────

    /// Rows `start..start+count`, formatted, with selection and cut flags.
    pub fn rows(&self, start: u32, count: u32) -> Vec<RowInfo> {
        let (l, sel) = {
            let st = self.st.lock();
            (st.listing.clone(), st.selection.clone())
        };
        let cut = self.shared.cut_names_in(&l.dir);
        let f = RowFormat::now(self.shared.settings.read().view.hour24);
        listing::build_rows(
            &l,
            start,
            count,
            &|i| sel.is_selected(i),
            sel.cursor(),
            &cut,
            &f,
        )
    }

    /// The rows on screen; page up/down move by this many. Later milestones
    /// fetch tags and folder sizes for this range first.
    pub fn set_visible_range(&self, start: u32, count: u32) {
        self.st.lock().visible = (start, count);
    }

    /// The rows on screen, as last reported.
    pub fn visible_range(&self) -> (u32, u32) {
        self.st.lock().visible
    }

    /// Records where the shell is scrolled to, for restore.
    pub fn set_scroll_anchor(&self, anchor: Option<ScrollAnchor>) {
        self.st.lock().scroll = anchor;
        self.shared.mark_dirty();
    }

    /// Where the shell should scroll to after a restore.
    pub fn scroll_anchor(&self) -> Option<ScrollAnchor> {
        self.st.lock().scroll.clone()
    }

    /// The address bar's resting crumbs for this tab's folder.
    pub fn breadcrumbs(&self) -> Vec<Crumb> {
        let loc = self.location_path();
        fstudio_path::breadcrumbs(&loc, &self.shared.home, &self.shared.crumb_volumes())
    }

    /// Completions for the address bar text with the cursor at byte `cursor`.
    /// Lists at most one (cached, local) folder; slow and dataless folders are skipped.
    pub fn complete(&self, input: &str, cursor: u32) -> Completions {
        let cwd = self.location_path();
        let ctx = self.parse_context(cwd);
        let c = self.shared.completer.read().clone();
        c.complete(input, cursor as usize, &ctx)
    }

    /// Validates a new name for `old_name` in this folder and returns the paths.
    pub(crate) fn rename_target(&self, old_name: &str, new_name: &str) -> Result<PathBuf> {
        fstudio_ops::names::validate_name(new_name).map_err(CoreError::InvalidName)?;
        let st = self.st.lock();
        if st.listing.inner.get(old_name).is_none() {
            return Err(CoreError::NotFound(format!("“{old_name}”")));
        }
        Ok(st.location.join(old_name))
    }

    /// True if a name is taken in the current listing (ignoring case and normalisation).
    pub(crate) fn name_taken(&self, name: &str) -> bool {
        let key = fstudio_path::keys::fold(name);
        let st = self.st.lock();
        st.listing.inner.get(name).is_some()
            || st
                .listing
                .inner
                .iter()
                .any(|e| fstudio_path::keys::fold(&e.name) == key)
    }
}

/// The name of `from` inside `to`, if `from` is a direct child (for selecting
/// the folder we came back from).
/// When `to` is an ancestor of `from`, the child of `to` on the way to
/// `from` (`/a` from `/a/b/c` is `b`), as a selection.
fn child_on_way(to: &Path, from: &Path) -> Option<(Vec<String>, Option<String>)> {
    let rest = from.strip_prefix(to).ok()?;
    let n = rest.components().next()?;
    let n = n.as_os_str().to_string_lossy().into_owned();
    Some((vec![n.clone()], Some(n)))
}

fn child_name(to: &Path, from: &Path) -> Option<(Vec<String>, Option<String>)> {
    if from.parent() == Some(to) {
        let n = from.file_name()?.to_string_lossy().into_owned();
        return Some((vec![n.clone()], Some(n)));
    }
    None
}

/// A selection of `rows` with the cursor (and anchor) on `cursor`.
fn make_selection(rows: &[usize], cursor: Option<usize>) -> Selection {
    let mut s = Selection::new();
    let mut seen = BTreeSet::new();
    for &i in rows {
        if Some(i) != cursor && seen.insert(i) {
            s.toggle(i);
        }
    }
    if let Some(c) = cursor {
        s.toggle(c);
        if !rows.contains(&c) {
            s.toggle(c);
        }
    }
    s
}

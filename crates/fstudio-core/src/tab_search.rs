//! Search results as a tab (M3).
//!
//! A search tab is an ordinary [`Tab`] whose listing holds the results of
//! an [`fstudio_search`] run instead of one folder's entries. Each result's
//! `Entry::name` is its path **relative to the search root**
//! (`Documents/Work/invoice.pdf`), which is unique, so everything keyed by
//! name keeps working unchanged: selection, scroll anchors, `path_at`,
//! drags, Quick Look, context menus and file operations (the full path is
//! always `root.join(name)`). Rows show the item's own name
//! ([`RowInfo::display_name`]) and the folder it is in
//! ([`RowInfo::location_text`]); sorting by name sorts by the item's own
//! name first ([`LeafCollation`]).
//!
//! The tab isn't watched (results come from many folders). Instead it is
//! re-checked on disk when an operation finishes and when a rename made
//! from it is applied, so trashed or moved items drop out and renamed ones
//! stay. Back, Esc (once stopped), or going anywhere else ends the search
//! and shows the folder it started from again.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use fstudio_search::{IndexSource, SearchEvent, SearchHandle, SearchQuery, Summary};
use fstudio_view::{Change, Collation, ListingBuilder, Selection, natural_cmp};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::{Hist, PARTIAL_EVERY, Tab};
use crate::error::{CoreError, Result};
use crate::events::{ChangeKind, CoreEvent};
use crate::listing::{self, ListingState, RowInfo};

/// At most this many unreadable folders are kept for the status popover.
const DENIED_KEPT: usize = 200;
/// Re-checking more results than this after an operation is skipped.
const REVERIFY_MAX: usize = 20_000;

/// Where a search looks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SearchScope {
    /// The folder the tab shows (and everything below it).
    #[default]
    ThisFolder,
    /// The whole Mac (Spotlight everywhere; the walk from `/`, skipping
    /// other volumes).
    ThisMac,
}

/// A search to run in a tab. The query's `roots` and `skip` are set from
/// the scope.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchRequest {
    /// Where.
    pub scope: SearchScope,
    /// What (name, filters, options).
    pub query: SearchQuery,
}

/// Where a search is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SearchState {
    /// Still looking; results keep arriving.
    Searching,
    /// Every source is done.
    Done,
    /// Stopped by the user; the results so far stay.
    Stopped,
    /// Stopped at the result limit.
    Truncated,
}

/// What a search tab is doing, for the search bar's status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchStatus {
    /// The scope.
    pub scope: SearchScope,
    /// The query as run (roots filled in).
    pub query: SearchQuery,
    /// The folder the tab showed before (Back returns there).
    pub folder: String,
    /// The folder searched below ("/" for This Mac).
    pub root: String,
    /// Searching, done, stopped or cut short.
    pub state: SearchState,
    /// Results so far.
    pub found: u64,
    /// Folders the walk listed so far.
    pub dirs_scanned: u64,
    /// Folders that couldn't be read (at most the first 200).
    pub denied: Vec<String>,
    /// How many folders couldn't be read.
    pub denied_count: u64,
    /// iCloud placeholder folders not entered.
    pub cloud_skipped: u64,
    /// Spotlight was asked and answered.
    pub index_used: bool,
    /// Why Spotlight couldn't be asked, if it couldn't.
    pub index_error: Option<String>,
    /// Time so far, or the total.
    pub elapsed_ms: u64,
    /// Time to the first result.
    pub first_hit_ms: Option<u64>,
    /// The status line: "Searching… 120 found", "1,204 found in 2.1 s".
    pub text: String,
}

/// Orders search results by the item's own name (the last path
/// component), then by the whole relative path. Natural and case-folded
/// like the folder view (F-017).
#[derive(Debug, Clone, Copy, Default)]
pub struct LeafCollation;

impl Collation for LeafCollation {
    fn key(&self, _name: &str, folded: &str) -> Option<Box<[u8]>> {
        let leaf = folded.rsplit('/').next().unwrap_or(folded);
        let mut k = Vec::with_capacity(leaf.len() + 1 + folded.len());
        k.extend_from_slice(leaf.as_bytes());
        k.push(0);
        k.extend_from_slice(folded.as_bytes());
        Some(k.into_boxed_slice())
    }
    fn compare(&self, a: &[u8], b: &[u8]) -> Ordering {
        natural_cmp(a, b)
    }
}

/// A tab's running or finished search.
pub(super) struct SearchTab {
    scope: SearchScope,
    query: SearchQuery,
    folder: PathBuf,
    root: PathBuf,
    handle: SearchHandle,
    summary: Option<Summary>,
    stopped: bool,
    denied: Vec<String>,
    denied_count: u64,
    index_error: Option<String>,
    /// Renames made from this tab: (old relative name, new relative name).
    renames: Vec<(String, String)>,
    /// An operation finished while results were still arriving.
    recheck: bool,
}

impl SearchTab {
    pub(super) fn cancel(&self) {
        self.handle.cancel();
    }
    pub(super) fn folder(&self) -> &Path {
        &self.folder
    }
}

// ─────────────── the tabs to re-check after an operation ───────────────

fn live() -> &'static Mutex<Vec<Weak<Tab>>> {
    static LIVE: std::sync::OnceLock<Mutex<Vec<Weak<Tab>>>> = std::sync::OnceLock::new();
    LIVE.get_or_init(Default::default)
}

/// An operation finished: every search tab re-checks its results on disk
/// (in the background).
pub(crate) fn ops_finished() {
    let tabs: Vec<Arc<Tab>> = {
        let mut l = live().lock();
        l.retain(|w| w.strong_count() > 0);
        l.iter().filter_map(Weak::upgrade).collect()
    };
    for t in tabs {
        let _ = std::thread::Builder::new()
            .name("fstudio-search-recheck".into())
            .spawn(move || t.search_recheck());
    }
}

/// The Spotlight source for searches.
fn index() -> Option<Arc<dyn IndexSource>> {
    #[cfg(target_os = "macos")]
    {
        Some(Arc::new(fstudio_search::Mdfind))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

// ─────────────── formatting ───────────────

fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn secs(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms} ms")
    } else {
        format!("{:.1} s", ms as f64 / 1_000.0)
    }
}

/// The status line for a search.
pub(crate) fn status_text(state: SearchState, found: u64, elapsed_ms: u64, denied: u64) -> String {
    let n = match found {
        0 => "No results".to_string(),
        1 => "1 found".to_string(),
        n => format!("{} found", group(n)),
    };
    let mut s = match state {
        SearchState::Searching if found == 0 => "Searching…".to_string(),
        SearchState::Searching => format!("Searching… {n}"),
        SearchState::Done => format!("{n} in {}", secs(elapsed_ms)),
        SearchState::Stopped => format!("Stopped · {n}"),
        SearchState::Truncated => format!("Showing the first {}", group(found)),
    };
    match denied {
        0 => {}
        1 => s.push_str(" · 1 folder couldn’t be read"),
        d => s.push_str(&format!(" · {} folders couldn’t be read", group(d))),
    }
    s
}

/// `~/Documents/Work` for a folder below home, else the path.
fn home_relative(p: &Path, home: &Path) -> String {
    match p.strip_prefix(home) {
        Ok(r) if r.as_os_str().is_empty() => "~".into(),
        Ok(r) => format!("~/{}", r.to_string_lossy()),
        Err(_) => p.to_string_lossy().into_owned(),
    }
}

/// The folder a result is in, as rows show it: home-relative below home
/// (`~/Documents/Work`), else below the search root's name
/// (`fixture/Work/2026`), else the full path (This Mac).
fn location_text(root: &Path, rel: &Path, home: &Path) -> String {
    let parent = rel
        .parent()
        .map(|p| root.join(p))
        .unwrap_or(root.to_path_buf());
    if parent.starts_with(home) || root == Path::new("/") {
        return home_relative(&parent, home);
    }
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().into_owned());
    match rel.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(p) => format!("{name}/{}", p.to_string_lossy()),
        None => name,
    }
}

// ─────────────── the tab side ───────────────

impl Tab {
    /// Runs a search in this tab, replacing its rows with the results as
    /// they are found (`ListingChanged` `Partial`, then `Sorted`). "This
    /// folder" searches below the folder shown (or the folder a running
    /// search started from). Fails only on a bad pattern.
    pub fn search(&self, req: SearchRequest) -> Result<()> {
        let mut q = req.query;
        {
            let mut st = self.st.lock();
            if st.closed {
                return Ok(());
            }
            let folder = st
                .search
                .as_ref()
                .map(|s| s.folder.clone())
                .unwrap_or_else(|| st.location.clone());
            match req.scope {
                SearchScope::ThisFolder => {
                    q.roots = vec![folder.clone()];
                    q.skip.clear();
                }
                SearchScope::ThisMac => {
                    let m = SearchQuery::this_mac();
                    q.roots = m.roots;
                    q.skip = m.skip;
                }
            }
            let root = q.roots[0].clone();
            let (handle, rx) = fstudio_search::start(q.clone(), index())
                .map_err(|e| CoreError::InvalidName(e.to_string()))?;
            if let Some(old) = st.search.take() {
                old.cancel();
            }
            st.generation += 1;
            st.location = root.clone();
            // Not watched: results come from many folders (see the module docs).
            st.canonical = PathBuf::new();
            st.queued.clear();
            st.type_ahead = None;
            st.pending = None;
            st.pending_until = None;
            st.pending_found.clear();
            st.pending_gen = None;
            st.filter.clear();
            st.scroll = None;
            if st.view.mode == fstudio_keymap::ViewMode::Columns {
                st.view.mode = fstudio_keymap::ViewMode::List;
            }
            st.selection = Selection::new();
            st.version += 1;
            let opts = st.view.options(&st.filter);
            st.listing = Arc::new(listing::empty(&root, opts, st.version));
            st.search = Some(SearchTab {
                scope: req.scope,
                query: q,
                folder: folder.clone(),
                root: root.clone(),
                handle: handle.clone(),
                summary: None,
                stopped: false,
                denied: Vec::new(),
                denied_count: 0,
                index_error: None,
                renames: Vec::new(),
                recheck: false,
            });
            let gen_ = st.generation;
            drop(st);
            if !self.detached {
                self.shared.watch.unregister(self.id);
            }
            let tab = self.arc();
            std::thread::Builder::new()
                .name("fstudio-search-tab".into())
                .spawn(move || tab.search_job(gen_, root, handle, rx))
                .map_err(|e| CoreError::NotAvailable(e.to_string()))?;
        }
        {
            let mut l = live().lock();
            l.retain(|w| w.strong_count() > 0);
            if !l.iter().any(|w| std::ptr::eq(w.as_ptr(), self)) {
                l.push(self.me.clone());
            }
        }
        if !self.detached {
            self.shared.emit(CoreEvent::TabsChanged);
            self.shared.mark_dirty();
        }
        self.shared.emit(CoreEvent::ListingChanged {
            tab: self.id,
            version: self.listing().version,
            kind: ChangeKind::Partial,
        });
        self.shared.emit(CoreEvent::SelectionChanged {
            tab: self.id,
            count: 0,
            cursor: None,
        });
        Ok(())
    }

    /// Stops a running search, keeping the results so far. False when the
    /// tab isn't searching (or already stopped).
    pub fn stop_search(&self) -> bool {
        let mut st = self.st.lock();
        match st.search.as_mut() {
            Some(s) if s.summary.is_none() && !s.stopped => {
                s.stopped = true;
                s.cancel();
                true
            }
            _ => false,
        }
    }

    /// Ends the search and shows the folder it started from. False when the
    /// tab isn't a search tab.
    pub fn end_search(&self) -> bool {
        let Some(folder) = self.st.lock().search.as_ref().map(|s| s.folder.clone()) else {
            return false;
        };
        self.start(folder, Hist::Keep, None, true, false);
        true
    }

    /// True while the tab shows search results (running or not).
    pub fn is_search(&self) -> bool {
        self.st.lock().search.is_some()
    }

    /// The search's status, or `None` for a folder tab.
    pub fn search_status(&self) -> Option<SearchStatus> {
        let st = self.st.lock();
        let s = st.search.as_ref()?;
        let p = s.handle.progress();
        let (state, found, elapsed, first, dirs, cloud, index_used) = match &s.summary {
            Some(sum) => (
                if sum.truncated {
                    SearchState::Truncated
                } else if sum.cancelled || s.stopped {
                    SearchState::Stopped
                } else {
                    SearchState::Done
                },
                sum.found,
                sum.elapsed.as_millis() as u64,
                sum.first_hit.map(|d| d.as_millis() as u64),
                sum.dirs_scanned,
                sum.cloud_skipped,
                sum.index_used,
            ),
            None => (
                if s.stopped {
                    SearchState::Stopped
                } else {
                    SearchState::Searching
                },
                p.found,
                s.handle.elapsed().as_millis() as u64,
                None,
                p.dirs_scanned,
                0,
                false,
            ),
        };
        // Once done, the listing is the truth (re-checks remove results).
        let found = if s.summary.is_some() {
            u64::from(st.listing.total_len())
        } else {
            found
        };
        Some(SearchStatus {
            scope: s.scope,
            query: s.query.clone(),
            folder: s.folder.to_string_lossy().into_owned(),
            root: s.root.to_string_lossy().into_owned(),
            state,
            found,
            dirs_scanned: dirs,
            denied: s.denied.clone(),
            denied_count: s.denied_count,
            cloud_skipped: cloud,
            index_used,
            index_error: s.index_error.clone(),
            elapsed_ms: elapsed,
            first_hit_ms: first,
            text: status_text(state, found, elapsed, s.denied_count),
        })
    }

    /// For `refresh`: runs the search again. False for a folder tab.
    pub(super) fn search_rerun(&self) -> bool {
        let req = {
            let st = self.st.lock();
            let Some(s) = st.search.as_ref() else {
                return false;
            };
            SearchRequest {
                scope: s.scope,
                query: s.query.clone(),
            }
        };
        let _ = self.search(req);
        true
    }

    /// For renames from a search tab: the new row name (relative path) for
    /// `new_name` given the old row name, and remembers the rename so the
    /// row follows it. For a folder tab, `new_name` itself.
    pub(crate) fn renamed_row_name(&self, old_name: &str, new_name: &str) -> String {
        let mut st = self.st.lock();
        let Some(s) = st.search.as_mut() else {
            return new_name.to_string();
        };
        let new_rel = match Path::new(old_name).parent() {
            Some(p) if !p.as_os_str().is_empty() => p.join(new_name).to_string_lossy().into_owned(),
            _ => new_name.to_string(),
        };
        s.renames.push((old_name.to_string(), new_rel.clone()));
        new_rel
    }

    /// Fills the search columns of rows: the item's own name, the folder it
    /// is in, and kind, icon and extension from its own name.
    pub(super) fn search_rows(&self, rows: &mut [RowInfo]) {
        let root = {
            let st = self.st.lock();
            match st.search.as_ref() {
                Some(s) => s.root.clone(),
                None => return,
            }
        };
        let home = self.shared.home.clone();
        let listing = self.listing();
        for r in rows {
            let rel = Path::new(&r.name);
            let leaf = rel
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| r.name.clone());
            if let Some(mut e) = listing.inner.entry(r.index as usize).cloned() {
                e.name = leaf.clone();
                r.icon_key = listing::icon_key(&e);
                r.kind_text = listing::kind_text(&e);
                if !matches!(r.kind, listing::RowKind::Folder) {
                    r.ext_text = e.extension().to_string();
                }
                r.is_hidden = e.is_hidden();
            }
            r.display_name = leaf;
            r.location_text = location_text(&root, rel, &home);
        }
    }

    /// The background half of a search: turns hits into entries named by
    /// relative path and publishes them like a folder listing.
    fn search_job(
        self: Arc<Self>,
        gen_: u64,
        root: PathBuf,
        handle: SearchHandle,
        rx: crossbeam_channel::Receiver<SearchEvent>,
    ) {
        let opts = self.st.lock().view.options("");
        let mut b = ListingBuilder::with_collation(opts, Arc::new(LeafCollation));
        let mut first = true;
        let mut last = Instant::now();
        let mut dirty = false;
        loop {
            let ev = match rx.recv_timeout(PARTIAL_EVERY) {
                Ok(ev) => Some(ev),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => None,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
            };
            if !self.is_current(gen_) {
                handle.cancel(); // this job's own search, never a newer one
                return;
            }
            match ev {
                Some(SearchEvent::Hits(hits)) => {
                    let entries = hits
                        .into_iter()
                        .filter_map(|h| {
                            let rel = h
                                .path
                                .strip_prefix(&root)
                                .ok()?
                                .to_string_lossy()
                                .into_owned();
                            let mut e = h.entry;
                            e.name = rel;
                            Some(e)
                        })
                        .collect();
                    b.push_chunk(entries);
                    dirty = true;
                }
                Some(SearchEvent::Denied { path, .. }) => {
                    let mut st = self.st.lock();
                    if let Some(s) = st.search.as_mut() {
                        s.denied_count += 1;
                        if s.denied.len() < DENIED_KEPT {
                            s.denied.push(path.to_string_lossy().into_owned());
                        }
                    }
                }
                Some(SearchEvent::IndexUnavailable(why)) => {
                    if let Some(s) = self.st.lock().search.as_mut() {
                        s.index_error = Some(why);
                    }
                }
                Some(SearchEvent::Finished(sum)) => {
                    let recheck = {
                        let mut st = self.st.lock();
                        if st.generation != gen_ {
                            return;
                        }
                        match st.search.as_mut() {
                            Some(s) => {
                                s.summary = Some(sum);
                                std::mem::take(&mut s.recheck)
                            }
                            None => false,
                        }
                    };
                    let l = b.finish();
                    self.publish(gen_, None, l, ListingState::Complete, ChangeKind::Sorted);
                    if recheck {
                        self.search_recheck();
                    }
                    return;
                }
                None => {}
            }
            if (dirty && (first || last.elapsed() >= PARTIAL_EVERY))
                || (!dirty && last.elapsed() >= Duration::from_millis(500))
            {
                // A snapshot for new rows; without new rows, a heartbeat so
                // the status (folders scanned, time) moves on.
                first = false;
                last = Instant::now();
                dirty = false;
                let snap = b.snapshot();
                self.publish(gen_, None, snap, ListingState::Loading, ChangeKind::Partial);
            }
        }
    }

    /// Re-checks every result on disk: gone ones are removed, changed ones
    /// updated, renames made from this tab followed.
    pub(super) fn search_recheck(&self) {
        let (gen_, root, renames, base) = {
            let mut st = self.st.lock();
            let gen_ = st.generation;
            let base = st.listing.clone();
            let Some(s) = st.search.as_mut() else {
                return;
            };
            if s.summary.is_none() {
                s.recheck = true; // after the last results arrive
                return;
            }
            (gen_, s.root.clone(), std::mem::take(&mut s.renames), base)
        };
        let mut changes = Vec::new();
        for (old, new) in &renames {
            if std::fs::symlink_metadata(root.join(old)).is_err()
                && let Ok(mut e) = fstudio_fs::stat_entry(root.join(new))
            {
                changes.push(Change::Remove(old.clone()));
                e.name = new.clone();
                changes.push(Change::Upsert(e));
            }
        }
        if (base.inner.len()) <= REVERIFY_MAX {
            for e in base.inner.iter() {
                if renames.iter().any(|(o, _)| *o == e.name) {
                    continue;
                }
                let p = root.join(&e.name);
                match fstudio_fs::stat_entry(&p) {
                    Ok(mut now) => {
                        let leaf = p.file_name().map(|n| n.to_string_lossy().into_owned());
                        if leaf.as_deref() != Some(now.name.as_str()) {
                            // Renamed by case or normalisation only: keep the
                            // row, under the name now on disk.
                            changes.push(Change::Remove(e.name.clone()));
                            let parent = Path::new(&e.name).parent().unwrap_or(Path::new(""));
                            now.name = parent.join(&now.name).to_string_lossy().into_owned();
                            changes.push(Change::Upsert(now));
                        } else if now.size != e.size
                            || now.mtime_ns != e.mtime_ns
                            || now.flags != e.flags
                            || now.kind != e.kind
                        {
                            now.name = e.name.clone();
                            changes.push(Change::Upsert(now));
                        }
                    }
                    Err(err) if err.kind() == Some(std::io::ErrorKind::NotFound) => {
                        changes.push(Change::Remove(e.name.clone()))
                    }
                    Err(_) => {}
                }
            }
        }
        if changes.is_empty() {
            return;
        }
        let next = base.inner.apply(changes);
        self.publish(
            gen_,
            Some(base.version),
            next,
            base.state,
            ChangeKind::Patched,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_lines() {
        assert_eq!(status_text(SearchState::Searching, 0, 10, 0), "Searching…");
        assert_eq!(
            status_text(SearchState::Searching, 1234, 10, 2),
            "Searching… 1,234 found · 2 folders couldn’t be read"
        );
        assert_eq!(
            status_text(SearchState::Done, 1, 2_100, 0),
            "1 found in 2.1 s"
        );
        assert_eq!(
            status_text(SearchState::Done, 0, 80, 1),
            "No results in 80 ms · 1 folder couldn’t be read"
        );
        assert_eq!(
            status_text(SearchState::Stopped, 12, 80, 0),
            "Stopped · 12 found"
        );
        assert_eq!(
            status_text(SearchState::Truncated, 50_000, 80, 0),
            "Showing the first 50,000"
        );
    }

    #[test]
    fn leaf_collation_orders_by_own_name() {
        let c = LeafCollation;
        let k = |s: &str| c.key(s, s).unwrap();
        let mut v = ["zeta/a10.txt", "a/b2.txt", "m/a2.txt", "a2.txt"];
        v.sort_by(|a, b| c.compare(&k(a), &k(b)));
        assert_eq!(v, ["a2.txt", "m/a2.txt", "zeta/a10.txt", "a/b2.txt"]);
    }

    #[test]
    fn location_texts() {
        let h = Path::new("/Users/me");
        let t = |root: &str, rel: &str| location_text(Path::new(root), Path::new(rel), h);
        assert_eq!(t("/Users/me/Docs", "a/b.txt"), "~/Docs/a");
        assert_eq!(t("/Users/me", "b.txt"), "~");
        assert_eq!(t("/tmp/fixture", "Work/2026/x.txt"), "fixture/Work/2026");
        assert_eq!(t("/tmp/fixture", "x.txt"), "fixture");
        assert_eq!(t("/", "Users/me/x"), "~");
        assert_eq!(t("/", "Library/x"), "/Library");
    }

    #[test]
    fn home_relative_paths() {
        let h = Path::new("/Users/me");
        assert_eq!(home_relative(Path::new("/Users/me"), h), "~");
        assert_eq!(home_relative(Path::new("/Users/me/Docs"), h), "~/Docs");
        assert_eq!(home_relative(Path::new("/tmp"), h), "/tmp");
    }
}

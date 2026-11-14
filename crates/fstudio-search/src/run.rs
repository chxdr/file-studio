//! Running a search: both sources on their own threads, merged and
//! de-duplicated on a third, streamed out in batches.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, unbounded};
use fstudio_model::Entry;
use parking_lot::Mutex;

use crate::cancel::CancelToken;
use crate::matcher::Matcher;
use crate::query::{QueryError, SearchQuery};
use crate::spotlight::{IndexQuery, IndexSource, spotlight_query};
use crate::walk::{WalkOptions, WalkStats, walk};

/// Where a result came from first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    /// The filesystem walk.
    Walk,
    /// The Spotlight index (re-checked on disk).
    Index,
}

/// One result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// Its full path, below one of the query's roots as spelled there.
    pub path: PathBuf,
    /// What it is, as listed (the name exactly as stored).
    pub entry: Entry,
    /// The source that found it first.
    pub source: Source,
}

/// What a running search reports.
#[derive(Debug, Clone, PartialEq)]
pub enum SearchEvent {
    /// New results (never one already sent).
    Hits(Vec<Hit>),
    /// A folder couldn't be read; the search goes on.
    Denied {
        /// The folder.
        path: PathBuf,
        /// Why.
        error: String,
    },
    /// The index couldn't be asked; the walk still runs.
    IndexUnavailable(String),
    /// The search is over (done, cut short or cancelled). Always last.
    Finished(Summary),
}

/// How a search went.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    /// Results sent.
    pub found: u64,
    /// Results the index found before the walk did.
    pub from_index: u64,
    /// Folders listed by the walk.
    pub dirs_scanned: u64,
    /// Entries the walk looked at.
    pub entries_seen: u64,
    /// Folders that couldn't be read.
    pub denied: u64,
    /// iCloud placeholder folders not entered.
    pub cloud_skipped: u64,
    /// Stopped at `max_results`.
    pub truncated: bool,
    /// Stopped by [`SearchHandle::cancel`].
    pub cancelled: bool,
    /// The index was asked and answered.
    pub index_used: bool,
    /// Wall-clock time.
    pub elapsed: Duration,
    /// Time to the first result.
    pub first_hit: Option<Duration>,
}

/// A snapshot of a running search's counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    /// Results so far.
    pub found: u64,
    /// Folders listed so far.
    pub dirs_scanned: u64,
    /// Folders that couldn't be read so far.
    pub denied: u64,
    /// The search is over.
    pub finished: bool,
}

#[derive(Default)]
struct Stats {
    walk: WalkStats,
    found: AtomicU64,
    finished: AtomicBool,
}

/// Controls a running search. Dropping it does not stop the search; drop
/// the event receiver or call [`SearchHandle::cancel`].
#[derive(Clone)]
pub struct SearchHandle {
    cancel: CancelToken,
    stats: Arc<Stats>,
    started: Instant,
}

impl std::fmt::Debug for SearchHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchHandle")
            .field("progress", &self.progress())
            .finish()
    }
}

impl SearchHandle {
    /// Stops both sources. A `Finished` event with `cancelled` follows.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
    /// True once cancelled (or cut short at `max_results`).
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
    /// The counters so far.
    pub fn progress(&self) -> Progress {
        Progress {
            found: self.stats.found.load(Ordering::Relaxed),
            dirs_scanned: self.stats.walk.dirs.load(Ordering::Relaxed),
            denied: self.stats.walk.denied.load(Ordering::Relaxed),
            finished: self.stats.finished.load(Ordering::Relaxed),
        }
    }
    /// Time since the search started.
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

enum Msg {
    Hits(Vec<Hit>),
    Denied(PathBuf, String),
    IndexErr(String),
    IndexUsed,
    Done,
}

/// True when an index result at `rel` (relative to its root) is in a place
/// the walk wouldn't enter: below a hidden folder (unless hidden items are
/// included) or inside a package (unless packages are included). The last
/// component (the item itself) is judged by the matcher.
pub fn excluded_by_path(rel: &Path, q: &SearchQuery) -> bool {
    let parts: Vec<_> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy()),
            _ => None,
        })
        .collect();
    let Some((_, parents)) = parts.split_last() else {
        return true;
    };
    parents.iter().any(|p| {
        (!q.include_hidden && p.starts_with('.'))
            || (!q.include_packages && fstudio_fs::is_package_name(p))
    })
}

/// True when a folder between the root and `path` has the Finder-hidden
/// flag (`UF_HIDDEN`, as `~/Library` does); the walk doesn't enter those.
fn under_hidden_folder(
    path: &Path,
    rel: &Path,
    cache: &mut std::collections::HashMap<PathBuf, bool>,
) -> bool {
    let depth = rel.components().count();
    path.ancestors()
        .skip(1)
        .take(depth.saturating_sub(1))
        .any(|dir| {
            *cache
                .entry(dir.to_path_buf())
                .or_insert_with(|| fstudio_fs::stat_entry(dir).is_ok_and(|e| e.is_hidden()))
        })
}

/// Maps an index path to the spelling of the root it is under: the root as
/// given, or its canonical form (`/tmp` → `/private/tmp`), or the APFS data
/// volume's firmlinked spelling (`/System/Volumes/Data/Users/…` →
/// `/Users/…`). Returns the path and its part below the root.
fn under_root(p: &Path, roots: &[(PathBuf, PathBuf)]) -> Option<(PathBuf, PathBuf)> {
    let firm = p
        .strip_prefix("/System/Volumes/Data")
        .ok()
        .map(|r| Path::new("/").join(r))
        .filter(|f| std::fs::symlink_metadata(f).is_ok());
    for cand in firm.into_iter().chain(std::iter::once(p.to_path_buf())) {
        for (given, canon) in roots {
            for base in [given, canon] {
                if let Ok(rel) = cand.strip_prefix(base)
                    && rel.components().next().is_some()
                {
                    return Some((given.join(rel), rel.to_path_buf()));
                }
            }
        }
    }
    None
}

/// Starts a search. Results and progress come on the returned receiver,
/// ending with exactly one [`SearchEvent::Finished`]. Fails only on a bad
/// pattern or when there are no roots.
pub fn start(
    q: SearchQuery,
    index: Option<Arc<dyn IndexSource>>,
) -> Result<(SearchHandle, Receiver<SearchEvent>), QueryError> {
    if q.roots.is_empty() {
        return Err(QueryError::NoRoots);
    }
    let matcher = Arc::new(Matcher::new(&q)?);
    let cancel = CancelToken::new();
    let stats = Arc::new(Stats::default());
    let started = Instant::now();
    let handle = SearchHandle {
        cancel: cancel.clone(),
        stats: stats.clone(),
        started,
    };
    let (tx_out, rx_out) = unbounded();
    let (tx_in, rx_in) = unbounded::<Msg>();
    let q = Arc::new(q);

    let mut sources = 0;
    if q.walk {
        sources += 1;
        let (q, m, c, st, tx) = (
            q.clone(),
            matcher.clone(),
            cancel.clone(),
            stats.clone(),
            tx_in.clone(),
        );
        spawn("fstudio-search-walk", move || {
            run_walk(&q, &m, &c, &st, &tx);
            let _ = tx.send(Msg::Done);
        });
    }
    let index_query = spotlight_query(&q);
    if let (true, Some(index), Some(text)) = (q.use_index, index, index_query) {
        sources += 1;
        let (q, m, c, tx) = (q.clone(), matcher.clone(), cancel.clone(), tx_in.clone());
        spawn("fstudio-search-index", move || {
            run_index(&q, &m, &c, &*index, text, &tx);
            let _ = tx.send(Msg::Done);
        });
    }
    drop(tx_in);
    let max = q.max_results.max(1) as u64;
    spawn("fstudio-search-merge", move || {
        merge(rx_in, tx_out, sources, max, cancel, stats, started);
    });
    Ok((handle, rx_out))
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(f)
        .expect("spawn a search thread");
}

fn run_walk(q: &SearchQuery, m: &Matcher, cancel: &CancelToken, st: &Stats, tx: &Sender<Msg>) {
    let opts = WalkOptions {
        skip: q.skip.clone(),
        include_hidden: q.include_hidden,
        include_packages: q.include_packages,
        threads: q.thread_count(),
    };
    let visit = |dir: &Path, entries: &[Entry]| {
        let hits: Vec<Hit> = entries
            .iter()
            .filter(|e| m.matches_meta(e))
            .filter_map(|e| {
                let path = dir.join(&e.name);
                (!m.needs_io() || m.matches(&path, e)).then(|| Hit {
                    path,
                    entry: e.clone(),
                    source: Source::Walk,
                })
            })
            .collect();
        if !hits.is_empty() && tx.send(Msg::Hits(hits)).is_err() {
            cancel.cancel();
        }
    };
    let denied = |dir: &Path, why: String| {
        let _ = tx.send(Msg::Denied(dir.to_path_buf(), why));
    };
    walk(&q.roots, &opts, cancel, &st.walk, &visit, &denied);
}

fn run_index(
    q: &SearchQuery,
    m: &Matcher,
    cancel: &CancelToken,
    index: &dyn IndexSource,
    text: String,
    tx: &Sender<Msg>,
) {
    let roots: Vec<(PathBuf, PathBuf)> = q
        .roots
        .iter()
        .map(|r| {
            (
                r.clone(),
                std::fs::canonicalize(r).unwrap_or_else(|_| r.clone()),
            )
        })
        .collect();
    let this_mac = q.roots.iter().any(|r| r == Path::new("/"));
    let iq = IndexQuery {
        roots: if this_mac {
            Vec::new()
        } else {
            roots.iter().map(|r| r.1.clone()).collect()
        },
        query: text,
    };
    let batch = Mutex::new((Vec::<Hit>::new(), Instant::now()));
    let flush = |force: bool| {
        let mut b = batch.lock();
        if !b.0.is_empty()
            && (force || b.0.len() >= 256 || b.1.elapsed() >= Duration::from_millis(30))
        {
            let v = std::mem::take(&mut b.0);
            b.1 = Instant::now();
            if tx.send(Msg::Hits(v)).is_err() {
                cancel.cancel();
            }
        }
    };
    let mut first = true;
    // Folders already checked for the Finder-hidden flag (`~/Library`).
    let mut hidden_dirs: std::collections::HashMap<PathBuf, bool> = Default::default();
    let r = index.candidates(&iq, cancel, &mut |p| {
        let Some((path, rel)) = under_root(&p, &roots) else {
            return;
        };
        if excluded_by_path(&rel, q) {
            return;
        }
        if !q.include_hidden && under_hidden_folder(&path, &rel, &mut hidden_dirs) {
            return;
        }
        let Ok(e) = fstudio_fs::stat_entry(&path) else {
            return; // stale: gone since it was indexed
        };
        if !m.matches(&path, &e) {
            return;
        }
        batch.lock().0.push(Hit {
            path,
            entry: e,
            source: Source::Index,
        });
        flush(std::mem::take(&mut first));
    });
    flush(true);
    let _ = tx.send(match r {
        Ok(()) => Msg::IndexUsed,
        Err(e) => Msg::IndexErr(e),
    });
}

fn merge(
    rx: Receiver<Msg>,
    tx: Sender<SearchEvent>,
    mut sources: usize,
    max: u64,
    cancel: CancelToken,
    stats: Arc<Stats>,
    started: Instant,
) {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut sum = Summary::default();
    let mut gone = false; // the receiver was dropped
    while sources > 0 {
        let Ok(msg) = rx.recv() else { break };
        match msg {
            Msg::Hits(v) => {
                let mut out = Vec::with_capacity(v.len());
                for h in v {
                    if sum.found >= max {
                        sum.truncated = true;
                        cancel.cancel();
                        break;
                    }
                    if seen.insert(h.path.clone()) {
                        sum.found += 1;
                        if h.source == Source::Index {
                            sum.from_index += 1;
                        }
                        out.push(h);
                    }
                }
                if out.is_empty() {
                    continue;
                }
                sum.first_hit.get_or_insert_with(|| started.elapsed());
                stats.found.store(sum.found, Ordering::Relaxed);
                if !gone && tx.send(SearchEvent::Hits(out)).is_err() {
                    gone = true;
                    cancel.cancel();
                }
            }
            Msg::Denied(path, error) => {
                if !gone {
                    let _ = tx.send(SearchEvent::Denied { path, error });
                }
            }
            Msg::IndexErr(e) => {
                if !gone {
                    let _ = tx.send(SearchEvent::IndexUnavailable(e));
                }
            }
            Msg::IndexUsed => sum.index_used = true,
            Msg::Done => sources -= 1,
        }
    }
    sum.cancelled = cancel.is_cancelled() && !sum.truncated;
    sum.dirs_scanned = stats.walk.dirs.load(Ordering::Relaxed);
    sum.entries_seen = stats.walk.entries.load(Ordering::Relaxed);
    sum.denied = stats.walk.denied.load(Ordering::Relaxed);
    sum.cloud_skipped = stats.walk.cloud_skipped.load(Ordering::Relaxed);
    sum.elapsed = started.elapsed();
    stats.finished.store(true, Ordering::Relaxed);
    let _ = tx.send(SearchEvent::Finished(sum));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_exclusion() {
        let q = SearchQuery::in_folder("/r");
        assert!(!excluded_by_path(Path::new("a/b.txt"), &q));
        assert!(excluded_by_path(Path::new(".git/x"), &q));
        assert!(excluded_by_path(Path::new("Tool.app/Contents/x"), &q));
        assert!(!excluded_by_path(Path::new("Tool.app"), &q));
        assert!(excluded_by_path(Path::new(""), &q));
        let all = SearchQuery {
            include_hidden: true,
            include_packages: true,
            ..q
        };
        assert!(!excluded_by_path(Path::new(".git/Tool.app/x"), &all));
    }

    #[test]
    fn index_paths_map_to_the_root_spelling() {
        let roots = vec![(PathBuf::from("/tmp/x"), PathBuf::from("/private/tmp/x"))];
        assert_eq!(
            under_root(Path::new("/private/tmp/x/a/b"), &roots),
            Some((PathBuf::from("/tmp/x/a/b"), PathBuf::from("a/b")))
        );
        assert_eq!(under_root(Path::new("/private/tmp/x"), &roots), None);
        assert_eq!(under_root(Path::new("/elsewhere"), &roots), None);
        let mac = vec![(PathBuf::from("/"), PathBuf::from("/"))];
        assert_eq!(
            under_root(Path::new("/System/Volumes/Data/Users/a"), &mac).map(|x| x.0),
            Some(PathBuf::from("/System/Volumes/Data/Users/a"))
        );
    }
}

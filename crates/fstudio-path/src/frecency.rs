//! Frecency: zoxide's algorithm, kept in memory for queries and written to the
//! `path_frecency` table in the background.
//!
//! - Every explicit visit adds 1 to the folder's rank and stamps the time.
//! - When the total rank passes `max_age` (default 10 000) every rank is scaled
//!   by `0.9 * max_age / total` and entries under `min_rank` (1.0) are dropped.
//! - The score used for ranking is the rank times a recency weight: ×4 within an
//!   hour, ×2 within a day, ×0.5 within a week, ×0.25 older.
//!
//! Queries read the in-memory index only. Writes update memory at once and are
//! batched to SQLite by a worker thread, so nothing here waits on the disk.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender};
use fstudio_db::{Db, Migrations};
use parking_lot::RwLock;

use crate::error::PathError;
use crate::keys::fold;
use crate::probe::FsProbe;

const HOUR: u64 = 3600;
const DAY: u64 = 24 * HOUR;
const WEEK: u64 = 7 * DAY;

/// The `path` module's schema. Called by [`Frecency::open`].
pub const MIGRATIONS: Migrations = Migrations {
    module: "path",
    steps: &[(
        1,
        "CREATE TABLE path_frecency (
            path          TEXT PRIMARY KEY,
            rank          REAL NOT NULL,
            last_accessed INTEGER NOT NULL
        ) WITHOUT ROWID;",
    )],
};

/// Settings for [`Frecency`].
#[derive(Debug, Clone)]
pub struct FrecencyConfig {
    /// Total rank above which everything is aged (zoxide `_ZO_MAXAGE`).
    pub max_age: f64,
    /// Entries whose rank falls under this when aging are dropped.
    pub min_rank: f64,
    /// Glob patterns of paths never recorded (`*` matches anything, `?` one
    /// character). Default: none. `/Volumes/*` is a sensible option.
    pub excluded: Vec<String>,
}

impl Default for FrecencyConfig {
    fn default() -> Self {
        Self {
            max_age: 10_000.0,
            min_rank: 1.0,
            excluded: Vec::new(),
        }
    }
}

/// One remembered folder.
#[derive(Debug, Clone, PartialEq)]
pub struct FrecencyEntry {
    /// The folder, as visited.
    pub path: String,
    /// Folded full path, for matching.
    pub key_path: String,
    /// Folded last segment, for matching.
    pub key_name: String,
    /// Accumulated visits (aged).
    pub rank: f64,
    /// Seconds since the epoch of the last visit.
    pub last_accessed: u64,
}

impl FrecencyEntry {
    fn new(path: &str, rank: f64, last_accessed: u64) -> Self {
        let trimmed = path.trim_end_matches('/');
        let name = trimmed.rsplit('/').next().unwrap_or(trimmed);
        Self {
            path: path.to_owned(),
            key_path: fold(path),
            key_name: fold(name),
            rank,
            last_accessed,
        }
    }

    /// The zoxide score at time `now` (seconds).
    pub fn score(&self, now: u64) -> f64 {
        let age = now.saturating_sub(self.last_accessed);
        if age < HOUR {
            self.rank * 4.0
        } else if age < DAY {
            self.rank * 2.0
        } else if age < WEEK {
            self.rank * 0.5
        } else {
            self.rank * 0.25
        }
    }
}

#[derive(Default)]
struct State {
    entries: Vec<FrecencyEntry>,
    index: HashMap<String, usize>,
    total: f64,
}

impl State {
    fn remove(&mut self, path: &str) -> bool {
        let Some(i) = self.index.remove(path) else {
            return false;
        };
        let e = self.entries.swap_remove(i);
        self.total -= e.rank;
        if let Some(moved) = self.entries.get(i) {
            self.index.insert(moved.path.clone(), i);
        }
        true
    }

    fn upsert(&mut self, path: &str, add: f64, last: u64, merge_time: bool) -> usize {
        self.total += add;
        if let Some(&i) = self.index.get(path) {
            let e = &mut self.entries[i];
            e.rank += add;
            e.last_accessed = if merge_time {
                e.last_accessed.max(last)
            } else {
                last
            };
            return i;
        }
        self.entries.push(FrecencyEntry::new(path, add, last));
        let i = self.entries.len() - 1;
        self.index.insert(path.to_owned(), i);
        i
    }

    /// zoxide's aging. Returns true if anything changed.
    fn age(&mut self, max_age: f64, min_rank: f64) -> bool {
        if self.total <= max_age {
            return false;
        }
        let factor = 0.9 * max_age / self.total;
        for e in &mut self.entries {
            e.rank *= factor;
        }
        self.entries.retain(|e| e.rank >= min_rank);
        self.index = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.path.clone(), i))
            .collect();
        self.total = self.entries.iter().map(|e| e.rank).sum();
        true
    }
}

enum Op {
    Upsert(String, f64, u64),
    Delete(String),
    ReplaceAll(Vec<(String, f64, u64)>),
    Flush(Sender<()>),
}

struct Inner {
    state: RwLock<State>,
    cfg: RwLock<FrecencyConfig>,
    clock: RwLock<Arc<dyn Fn() -> u64 + Send + Sync>>,
    tx: Option<Sender<Op>>,
    worker: parking_lot::Mutex<Option<JoinHandle<()>>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Closing the channel lets the worker finish its queue; wait for it so a
        // clean exit never loses visits.
        self.tx.take();
        if let Some(h) = self.worker.lock().take() {
            let _ = h.join();
        }
    }
}

/// The frecency index. Cheap to clone (shared); all methods take `&self`.
#[derive(Clone)]
pub struct Frecency {
    inner: Arc<Inner>,
}

fn system_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Frecency {
    /// An index with no database behind it (tests, benchmarks).
    pub fn in_memory(cfg: FrecencyConfig) -> Self {
        Self::build(State::default(), cfg, None)
    }

    /// Opens the index on `db`, creating the `path_frecency` table if needed and
    /// loading every row into memory.
    pub fn open(db: Db, cfg: FrecencyConfig) -> Result<Self, PathError> {
        db.migrate(&MIGRATIONS)?;
        let rows: Vec<(String, f64, i64)> = db.with(|c| {
            let mut st = c.prepare("SELECT path, rank, last_accessed FROM path_frecency")?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect()
        })?;
        let mut state = State::default();
        for (p, r, t) in rows {
            state.upsert(&p, r, t.max(0) as u64, true);
        }
        Ok(Self::build(state, cfg, Some(db)))
    }

    fn build(state: State, cfg: FrecencyConfig, db: Option<Db>) -> Self {
        let (tx, worker) = match db {
            Some(db) => {
                let (tx, rx) = crossbeam_channel::unbounded();
                let h = std::thread::Builder::new()
                    .name("path-frecency-writer".into())
                    .spawn(move || writer(db, rx))
                    .ok();
                (Some(tx), h)
            }
            None => (None, None),
        };
        Self {
            inner: Arc::new(Inner {
                state: RwLock::new(state),
                cfg: RwLock::new(cfg),
                clock: RwLock::new(Arc::new(system_now)),
                tx,
                worker: parking_lot::Mutex::new(worker),
            }),
        }
    }

    /// Replaces the clock (seconds since the epoch). For tests.
    pub fn set_clock(&self, f: impl Fn() -> u64 + Send + Sync + 'static) {
        *self.inner.clock.write() = Arc::new(f);
    }

    /// The current time according to this index's clock.
    pub fn now(&self) -> u64 {
        (self.inner.clock.read())()
    }

    fn send(&self, op: Op) {
        if let Some(tx) = &self.inner.tx {
            let _ = tx.send(op);
        }
    }

    /// True if `path` matches an excluded glob.
    pub fn is_excluded(&self, path: &str) -> bool {
        self.inner
            .cfg
            .read()
            .excluded
            .iter()
            .any(|g| glob_match(g, path))
    }

    /// Replaces the excluded globs and forgets entries that now match.
    pub fn set_excluded(&self, globs: Vec<String>) {
        self.inner.cfg.write().excluded = globs;
        let doomed: Vec<String> = self
            .inner
            .state
            .read()
            .entries
            .iter()
            .filter(|e| self.is_excluded(&e.path))
            .map(|e| e.path.clone())
            .collect();
        for p in doomed {
            self.remove(Path::new(&p));
        }
    }

    /// Records an explicit visit now. Returns false if the path is excluded.
    pub fn visit(&self, path: &Path) -> bool {
        self.visit_at(path, self.now())
    }

    /// Records a visit at a given time (seconds).
    pub fn visit_at(&self, path: &Path, now: u64) -> bool {
        let p = path.to_string_lossy();
        let p = if p.len() > 1 {
            p.trim_end_matches('/')
        } else {
            &p
        };
        if p.is_empty() || self.is_excluded(p) {
            return false;
        }
        let (max_age, min_rank) = {
            let c = self.inner.cfg.read();
            (c.max_age, c.min_rank)
        };
        let mut st = self.inner.state.write();
        let i = st.upsert(p, 1.0, now, false);
        let (rank, last) = (st.entries[i].rank, st.entries[i].last_accessed);
        if st.age(max_age, min_rank) {
            let all = st
                .entries
                .iter()
                .map(|e| (e.path.clone(), e.rank, e.last_accessed))
                .collect();
            drop(st);
            self.send(Op::ReplaceAll(all));
        } else {
            drop(st);
            self.send(Op::Upsert(p.to_owned(), rank, last));
        }
        true
    }

    /// Forgets a path. Returns true if it was known.
    pub fn remove(&self, path: &Path) -> bool {
        let p = path.to_string_lossy();
        let p = if p.len() > 1 {
            p.trim_end_matches('/')
        } else {
            &p
        };
        let existed = self.inner.state.write().remove(p);
        if existed {
            self.send(Op::Delete(p.to_owned()));
        }
        existed
    }

    /// Forgets `path` if it no longer exists on disk. Call this when the user picks
    /// a remembered folder (lazy clean-up, never on the keystroke path).
    /// Returns true if it was removed.
    pub fn forget_if_missing(&self, path: &Path, probe: &dyn FsProbe) -> bool {
        if probe.kind(path).is_none() {
            self.remove(path)
        } else {
            false
        }
    }

    /// Checks up to `max_checks` of the lowest-scoring entries and forgets the ones
    /// that are gone. Meant for idle time. Returns how many were removed.
    pub fn prune_missing(&self, probe: &dyn FsProbe, max_checks: usize) -> usize {
        let now = self.now();
        let mut cand: Vec<(f64, String)> = self
            .inner
            .state
            .read()
            .entries
            .iter()
            .map(|e| (e.score(now), e.path.clone()))
            .collect();
        cand.sort_by(|a, b| a.0.total_cmp(&b.0));
        cand.truncate(max_checks);
        cand.into_iter()
            .filter(|(_, p)| self.forget_if_missing(Path::new(p), probe))
            .count()
    }

    /// The current zoxide score of a path, if known.
    pub fn score(&self, path: &Path) -> Option<f64> {
        let now = self.now();
        let st = self.inner.state.read();
        let p = path.to_string_lossy();
        let p = if p.len() > 1 {
            p.trim_end_matches('/')
        } else {
            &p
        };
        st.index.get(p).map(|&i| st.entries[i].score(now))
    }

    /// The raw rank of a path, if known.
    pub fn rank(&self, path: &Path) -> Option<f64> {
        let st = self.inner.state.read();
        let p = path.to_string_lossy();
        let p = if p.len() > 1 {
            p.trim_end_matches('/')
        } else {
            &p
        };
        st.index.get(p).map(|&i| st.entries[i].rank)
    }

    /// How many folders are remembered.
    pub fn len(&self) -> usize {
        self.inner.state.read().entries.len()
    }

    /// True when nothing is remembered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The sum of all ranks (what aging compares with `max_age`).
    pub fn total_rank(&self) -> f64 {
        self.inner.state.read().total
    }

    /// Runs `f` over the entries under a read lock. Used by the completer; keep `f` short.
    pub fn with_entries<T>(&self, f: impl FnOnce(&[FrecencyEntry]) -> T) -> T {
        f(&self.inner.state.read().entries)
    }

    /// A copy of all entries, best score first.
    pub fn ranked(&self) -> Vec<FrecencyEntry> {
        let now = self.now();
        let mut v = self.inner.state.read().entries.clone();
        v.sort_by(|a, b| b.score(now).total_cmp(&a.score(now)));
        v
    }

    /// Adds `rank` to a path with a given last-access time (import). Skips
    /// excluded paths. Ages afterwards like a normal visit.
    pub fn add_imported(&self, path: &str, rank: f64, last_accessed: u64) -> bool {
        if path.is_empty() || !path.starts_with('/') || self.is_excluded(path) || rank <= 0.0 {
            return false;
        }
        let p = if path.len() > 1 {
            path.trim_end_matches('/')
        } else {
            path
        };
        let mut st = self.inner.state.write();
        let i = st.upsert(p, rank, last_accessed, true);
        let (r, l) = (st.entries[i].rank, st.entries[i].last_accessed);
        drop(st);
        self.send(Op::Upsert(p.to_owned(), r, l));
        true
    }

    /// Ages if the total rank is over the cap (call once after a bulk import).
    pub fn age_now(&self) {
        let (max_age, min_rank) = {
            let c = self.inner.cfg.read();
            (c.max_age, c.min_rank)
        };
        let mut st = self.inner.state.write();
        if st.age(max_age, min_rank) {
            let all = st
                .entries
                .iter()
                .map(|e| (e.path.clone(), e.rank, e.last_accessed))
                .collect();
            drop(st);
            self.send(Op::ReplaceAll(all));
        }
    }

    /// Waits until every queued write has reached the database.
    pub fn flush(&self) {
        if let Some(tx) = &self.inner.tx {
            let (ack_tx, ack_rx) = crossbeam_channel::bounded(1);
            if tx.send(Op::Flush(ack_tx)).is_ok() {
                let _ = ack_rx.recv();
            }
        }
    }
}

fn writer(db: Db, rx: Receiver<Op>) {
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        while batch.len() < 512 {
            match rx.try_recv() {
                Ok(op) => batch.push(op),
                Err(_) => break,
            }
        }
        let mut acks = Vec::new();
        let _ = db.with(|c| {
            let tx = c.transaction()?;
            for op in &batch {
                match op {
                    Op::Upsert(p, r, t) => {
                        tx.execute(
                            "INSERT INTO path_frecency(path, rank, last_accessed) VALUES (?1, ?2, ?3)
                             ON CONFLICT(path) DO UPDATE SET rank = excluded.rank,
                                last_accessed = excluded.last_accessed",
                            (p, r, *t as i64),
                        )?;
                    }
                    Op::Delete(p) => {
                        tx.execute("DELETE FROM path_frecency WHERE path = ?1", [p])?;
                    }
                    Op::ReplaceAll(all) => {
                        tx.execute("DELETE FROM path_frecency", [])?;
                        let mut st = tx.prepare(
                            "INSERT INTO path_frecency(path, rank, last_accessed) VALUES (?1, ?2, ?3)",
                        )?;
                        for (p, r, t) in all {
                            st.execute((p, r, *t as i64))?;
                        }
                    }
                    Op::Flush(_) => {}
                }
            }
            tx.commit()
        });
        for op in batch {
            if let Op::Flush(a) = op {
                acks.push(a);
            }
        }
        for a in acks {
            let _ = a.send(());
        }
    }
}

/// Minimal glob: `*` matches any run of characters (including `/`), `?` one character.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) && p[pi] != '*' {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

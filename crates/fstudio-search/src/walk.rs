//! The parallel filesystem walk.
//!
//! Plain threads share a breadth-first queue of folders, so shallow results
//! come first. Each folder is listed with [`fstudio_fs::Lister`]
//! (`getattrlistbulk` on macOS), which reports symlinks as links and never
//! follows them. A set of visited `(device, inode)` pairs stops any other
//! cycle (firmlinks, hard-linked folders, bind mounts). The walk never enters:
//! hidden folders (unless asked), packages (unless asked), iCloud
//! placeholder folders (listing one may download it, F-016), or the `skip`
//! list. A folder that can't be read is reported and the walk goes on.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use fstudio_fs::Lister;
use fstudio_model::{Entry, EntryKind};
use parking_lot::{Condvar, Mutex};

use crate::cancel::CancelToken;

/// What the walk enters.
#[derive(Debug, Clone, Default)]
pub struct WalkOptions {
    /// Folders never entered (absolute; compared exactly as given).
    pub skip: Vec<PathBuf>,
    /// Enter dot-folders and `UF_HIDDEN` folders.
    pub include_hidden: bool,
    /// Enter packages (`.app`, `.photoslibrary`, …).
    pub include_packages: bool,
    /// Worker threads (at least 1).
    pub threads: usize,
}

/// Counters, readable while the walk runs.
#[derive(Debug, Default)]
pub struct WalkStats {
    /// Folders listed.
    pub dirs: AtomicU64,
    /// Entries seen.
    pub entries: AtomicU64,
    /// Folders that couldn't be read.
    pub denied: AtomicU64,
    /// iCloud placeholder folders not entered.
    pub cloud_skipped: AtomicU64,
}

struct Queue {
    dirs: VecDeque<PathBuf>,
    active: usize,
    done: bool,
}

struct Shared<'a> {
    q: Mutex<Queue>,
    cv: Condvar,
    visited: Mutex<HashSet<(u64, u64)>>,
    opts: &'a WalkOptions,
    cancel: &'a CancelToken,
}

impl Shared<'_> {
    fn next(&self) -> Option<PathBuf> {
        let mut q = self.q.lock();
        loop {
            if self.cancel.is_cancelled() {
                q.done = true;
                self.cv.notify_all();
                return None;
            }
            if let Some(d) = q.dirs.pop_front() {
                q.active += 1;
                return Some(d);
            }
            if q.active == 0 || q.done {
                q.done = true;
                self.cv.notify_all();
                return None;
            }
            // A timeout as well, so a cancel is seen within 50 ms even if
            // its wake-up raced the wait.
            self.cv.wait_for(&mut q, Duration::from_millis(50));
        }
    }

    fn finish(&self, more: Vec<PathBuf>) {
        let mut q = self.q.lock();
        q.active -= 1;
        let n = more.len();
        q.dirs.extend(more);
        if n > 0 || q.active == 0 {
            self.cv.notify_all();
        }
    }

    fn first_visit(&self, dev: u64, ino: u64) -> bool {
        self.visited.lock().insert((dev, ino))
    }
}

/// Should the walk enter `e` (a child of a folder being listed)?
pub fn enters(e: &Entry, opts: &WalkOptions) -> bool {
    e.kind == EntryKind::Dir
        && (opts.include_hidden || !e.is_hidden())
        && (opts.include_packages || !(e.is_package || fstudio_fs::is_package_name(&e.name)))
        && !e.is_dataless()
}

/// Walks below `roots` on `opts.threads` threads and returns when every
/// folder is done or `cancel` fires (within about 50 ms).
///
/// `visit(dir, entries)` is called for each chunk of each folder's entries
/// (from any worker thread). `denied(dir, message)` is called for each
/// folder that can't be listed (not for ones that vanished meanwhile).
pub fn walk(
    roots: &[PathBuf],
    opts: &WalkOptions,
    cancel: &CancelToken,
    stats: &WalkStats,
    visit: &(dyn Fn(&Path, &[Entry]) + Sync),
    denied: &(dyn Fn(&Path, String) + Sync),
) {
    let shared = Shared {
        q: Mutex::new(Queue {
            dirs: VecDeque::new(),
            active: 0,
            done: false,
        }),
        cv: Condvar::new(),
        visited: Mutex::new(HashSet::new()),
        opts,
        cancel,
    };
    for r in roots {
        // A root may itself be a symlink (`/tmp`): follow it, once.
        match std::fs::metadata(r) {
            Ok(m) if m.is_dir() => {
                use std::os::unix::fs::MetadataExt;
                if shared.first_visit(m.dev(), m.ino()) {
                    shared.q.lock().dirs.push_back(r.clone());
                }
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                stats.denied.fetch_add(1, Ordering::Relaxed);
                denied(r, fstudio_fs::plain_error(&e));
            }
        }
    }
    let threads = opts.threads.max(1);
    std::thread::scope(|s| {
        for i in 0..threads {
            let shared = &shared;
            let _ = std::thread::Builder::new()
                .name(format!("fstudio-search-{i}"))
                .spawn_scoped(s, move || {
                    while let Some(dir) = shared.next() {
                        let more = scan(&dir, shared, stats, visit, denied);
                        shared.finish(more);
                    }
                });
        }
    });
}

fn scan(
    dir: &Path,
    sh: &Shared<'_>,
    stats: &WalkStats,
    visit: &(dyn Fn(&Path, &[Entry]) + Sync),
    denied: &(dyn Fn(&Path, String) + Sync),
) -> Vec<PathBuf> {
    let mut more = Vec::new();
    let mut lister = match Lister::open(dir) {
        Ok(l) => l,
        Err(e) => {
            if e.kind() != Some(std::io::ErrorKind::NotFound) {
                stats.denied.fetch_add(1, Ordering::Relaxed);
                denied(dir, e.to_string());
            }
            return more;
        }
    };
    stats.dirs.fetch_add(1, Ordering::Relaxed);
    while let Some(chunk) = lister.next_chunk() {
        if sh.cancel.is_cancelled() {
            break;
        }
        let entries = match chunk {
            Ok(v) => v,
            Err(e) => {
                stats.denied.fetch_add(1, Ordering::Relaxed);
                denied(dir, e.to_string());
                break;
            }
        };
        stats
            .entries
            .fetch_add(entries.len() as u64, Ordering::Relaxed);
        visit(dir, &entries);
        for e in &entries {
            if e.kind != EntryKind::Dir {
                continue;
            }
            if e.is_dataless() {
                stats.cloud_skipped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            if !enters(e, sh.opts) {
                continue;
            }
            let p = dir.join(&e.name);
            if sh.opts.skip.iter().any(|s| s == &p) {
                continue;
            }
            if sh.first_visit(e.dev, e.inode) {
                more.push(p);
            }
        }
    }
    more
}

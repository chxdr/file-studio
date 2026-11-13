//! Folder watching behind a [`Watcher`] trait.
//!
//! * macOS: [`FsEventsWatcher`], direct FSEvents with `FileEvents | NoDefer` (F-014).
//! * Everywhere: [`PollWatcher`], a slow, simple directory differ. It is the fallback
//!   on platforms without a native backend yet (an inotify backend can slot in behind the trait).
//!
//! Events come in batches over a crossbeam channel. An event's `kind` is a hint: when
//! FSEvents coalesces several changes to one path the consumer should `stat` the path to
//! learn what is true now. When `must_rescan` is set, the consumer must re-list the folder.

use crate::error::Result;
use crossbeam_channel::{Receiver, Sender, unbounded};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

/// What happened to a path (a hint; see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FsEventKind {
    /// A new file or folder.
    Created,
    /// A file or folder is gone.
    Removed,
    /// Content, attributes, tags or other metadata changed.
    Modified,
    /// Moved or renamed (the old and new paths each get an event).
    Renamed,
    /// Anything else.
    Other,
}

/// One change under a watched folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsEvent {
    /// The path that changed. On macOS this is the real path (`/tmp` is reported as `/private/tmp`).
    pub path: PathBuf,
    /// What happened.
    pub kind: FsEventKind,
    /// Events were coalesced, dropped, or the watched root moved: `stat`-ing paths is not
    /// enough and the folder must be listed again.
    pub must_rescan: bool,
}

/// Settings for a watcher.
#[derive(Debug, Clone)]
pub struct WatchConfig {
    /// How long events may be gathered before delivery. Default 30 ms.
    pub latency: Duration,
    /// Also report when a watched folder itself is moved or deleted (`must_rescan`).
    pub watch_root: bool,
    /// Report events for the whole subtree. By default only the folder itself and its
    /// direct children are reported, which is what a listing needs.
    pub recursive: bool,
}

impl Default for WatchConfig {
    fn default() -> Self {
        WatchConfig {
            latency: Duration::from_millis(30),
            watch_root: true,
            recursive: false,
        }
    }
}

/// A source of folder-change events.
pub trait Watcher: Send {
    /// Makes `paths` the set of watched folders. Folders already watched keep their stream;
    /// others are added or dropped.
    fn set_paths(&mut self, paths: &[PathBuf]) -> Result<()>;
    /// The channel that batches arrive on. Every call returns a handle to the same channel.
    fn events(&self) -> Receiver<Vec<FsEvent>>;
}

/// The best watcher for this platform.
pub fn default_watcher(config: WatchConfig) -> Result<Box<dyn Watcher>> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(FsEventsWatcher::new(config)))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(Box::new(PollWatcher::new(config)))
    }
}

// FSEvents item/stream flag bits (see <CoreServices/FSEvents.h>).
const EV_MUST_SCAN_SUBDIRS: u32 = 0x1;
const EV_USER_DROPPED: u32 = 0x2;
const EV_KERNEL_DROPPED: u32 = 0x4;
const EV_IDS_WRAPPED: u32 = 0x8;
const EV_ROOT_CHANGED: u32 = 0x20;
const EV_CREATED: u32 = 0x100;
const EV_REMOVED: u32 = 0x200;
const EV_INODE_META_MOD: u32 = 0x400;
const EV_RENAMED: u32 = 0x800;
const EV_MODIFIED: u32 = 0x1000;
const EV_FINDER_INFO_MOD: u32 = 0x2000;
const EV_CHANGE_OWNER: u32 = 0x4000;
const EV_XATTR_MOD: u32 = 0x8000;
const EV_HISTORY_DONE: u32 = 0x10;

/// Maps one FSEvents `(path, flags)` pair to an [`FsEvent`]. `None` for bookkeeping events.
///
/// FSEvents flags are cumulative: a file that was created and later deleted reports both
/// `Created` and `Removed`. For that ambiguous case `exists` is called to see what is true now.
/// Otherwise a rename wins, then removal, then creation, then any metadata change.
pub fn event_from_flags(
    path: &str,
    flags: u32,
    exists: impl FnOnce(&Path) -> bool,
) -> Option<FsEvent> {
    if flags == EV_HISTORY_DONE {
        return None;
    }
    let must_rescan = flags
        & (EV_MUST_SCAN_SUBDIRS
            | EV_USER_DROPPED
            | EV_KERNEL_DROPPED
            | EV_IDS_WRAPPED
            | EV_ROOT_CHANGED)
        != 0;
    let meta =
        EV_MODIFIED | EV_INODE_META_MOD | EV_FINDER_INFO_MOD | EV_CHANGE_OWNER | EV_XATTR_MOD;
    let path = PathBuf::from(if path.len() > 1 {
        path.trim_end_matches('/')
    } else {
        path
    });
    let kind = if flags & EV_RENAMED != 0 {
        FsEventKind::Renamed
    } else if flags & EV_REMOVED != 0 && flags & EV_CREATED == 0 {
        FsEventKind::Removed
    } else if flags & EV_REMOVED != 0 {
        if exists(&path) {
            FsEventKind::Created
        } else {
            FsEventKind::Removed
        }
    } else if flags & EV_CREATED != 0 {
        FsEventKind::Created
    } else if flags & meta != 0 {
        FsEventKind::Modified
    } else {
        FsEventKind::Other
    };
    Some(FsEvent {
        path,
        kind,
        must_rescan,
    })
}

/// Keeps events for the root itself and its direct children, plus every rescan request.
#[cfg(any(target_os = "macos", test))]
fn is_shallow_match(root: &Path, ev: &FsEvent) -> bool {
    ev.must_rescan || ev.path == root || ev.path.parent() == Some(root)
}

// ---------------------------------------------------------------------------------------------
// macOS: FSEvents
// ---------------------------------------------------------------------------------------------

/// FSEvents-backed watcher (macOS), one stream per watched folder.
#[cfg(target_os = "macos")]
pub struct FsEventsWatcher {
    config: WatchConfig,
    streams: HashMap<PathBuf, crate::sys::fsevents::Stream>,
    tx: Sender<Vec<FsEvent>>,
    rx: Receiver<Vec<FsEvent>>,
}

#[cfg(target_os = "macos")]
impl FsEventsWatcher {
    /// Creates a watcher that watches nothing yet.
    pub fn new(config: WatchConfig) -> Self {
        let (tx, rx) = unbounded();
        FsEventsWatcher {
            config,
            streams: HashMap::new(),
            tx,
            rx,
        }
    }
}

#[cfg(target_os = "macos")]
impl Watcher for FsEventsWatcher {
    fn set_paths(&mut self, paths: &[PathBuf]) -> Result<()> {
        use crate::sys::fsevents::{CF_FILE_EVENTS, CF_NO_DEFER, CF_WATCH_ROOT, Stream};
        let wanted: std::collections::HashSet<&PathBuf> = paths.iter().collect();
        self.streams.retain(|p, _| wanted.contains(p));
        for p in paths {
            if self.streams.contains_key(p) {
                continue;
            }
            let root = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
            let tx = self.tx.clone();
            let recursive = self.config.recursive;
            let cb = Box::new(move |batch: Vec<(String, u32)>| {
                let events: Vec<FsEvent> = batch
                    .iter()
                    .filter_map(|(path, flags)| {
                        event_from_flags(path, *flags, |p| std::fs::symlink_metadata(p).is_ok())
                    })
                    .filter(|e| recursive || is_shallow_match(&root, e))
                    .collect();
                if !events.is_empty() {
                    let _ = tx.send(events);
                }
            });
            let mut flags = CF_FILE_EVENTS | CF_NO_DEFER;
            if self.config.watch_root {
                flags |= CF_WATCH_ROOT;
            }
            let stream = Stream::start(p, self.config.latency.as_secs_f64(), flags, cb)
                .map_err(|e| crate::error::Error::io(p, e))?;
            self.streams.insert(p.clone(), stream);
        }
        Ok(())
    }

    fn events(&self) -> Receiver<Vec<FsEvent>> {
        self.rx.clone()
    }
}

// ---------------------------------------------------------------------------------------------
// Portable fallback: polling
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct Sig {
    mtime: Option<SystemTime>,
    len: u64,
    is_dir: bool,
}

type Snapshot = HashMap<OsString, Sig>;

fn snapshot(dir: &Path) -> std::io::Result<Snapshot> {
    let mut m = HashMap::new();
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        if let Ok(md) = e.metadata() {
            m.insert(
                e.file_name(),
                Sig {
                    mtime: md.modified().ok(),
                    len: md.len(),
                    is_dir: md.is_dir(),
                },
            );
        }
    }
    Ok(m)
}

/// Compares two snapshots of `dir`. Pure; used by [`PollWatcher`] and tested directly.
fn diff_snapshots(dir: &Path, old: &Snapshot, new: &Snapshot) -> Vec<FsEvent> {
    let mut out = Vec::new();
    let mut push = |name: &OsString, kind| {
        out.push(FsEvent {
            path: dir.join(name),
            kind,
            must_rescan: false,
        })
    };
    for (n, s) in new {
        match old.get(n) {
            None => push(n, FsEventKind::Created),
            Some(o) if o != s => push(n, FsEventKind::Modified),
            _ => {}
        }
    }
    for n in old.keys() {
        if !new.contains_key(n) {
            push(n, FsEventKind::Removed);
        }
    }
    out
}

/// A simple polling watcher: re-reads every watched folder at a fixed interval and reports
/// differences. Slower and heavier than native backends; used where none exists.
pub struct PollWatcher {
    paths: Arc<Mutex<Vec<PathBuf>>>,
    stop: Arc<AtomicBool>,
    rx: Receiver<Vec<FsEvent>>,
    handle: Option<JoinHandle<()>>,
}

impl PollWatcher {
    /// Starts the polling thread. The interval is `config.latency` but never below 20 ms.
    pub fn new(config: WatchConfig) -> Self {
        let (tx, rx): (Sender<Vec<FsEvent>>, _) = unbounded();
        let paths = Arc::new(Mutex::new(Vec::<PathBuf>::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (p2, s2) = (paths.clone(), stop.clone());
        let interval = config.latency.max(Duration::from_millis(20));
        let handle = std::thread::Builder::new()
            .name("fstudio-poll-watcher".into())
            .spawn(move || {
                let mut known: HashMap<PathBuf, Snapshot> = HashMap::new();
                while !s2.load(Ordering::Relaxed) {
                    let current: Vec<PathBuf> = p2.lock().map(|g| g.clone()).unwrap_or_default();
                    known.retain(|k, _| current.contains(k));
                    let mut batch = Vec::new();
                    for dir in &current {
                        match (snapshot(dir), known.get(dir)) {
                            (Ok(new), Some(old)) => {
                                batch.extend(diff_snapshots(dir, old, &new));
                                known.insert(dir.clone(), new);
                            }
                            (Ok(new), None) => {
                                known.insert(dir.clone(), new);
                            }
                            (Err(_), Some(_)) => {
                                known.remove(dir);
                                batch.push(FsEvent {
                                    path: dir.clone(),
                                    kind: FsEventKind::Removed,
                                    must_rescan: true,
                                });
                            }
                            (Err(_), None) => {}
                        }
                    }
                    if !batch.is_empty() && tx.send(batch).is_err() {
                        return;
                    }
                    std::thread::sleep(interval);
                }
            })
            .expect("spawn poll watcher thread");
        PollWatcher {
            paths,
            stop,
            rx,
            handle: Some(handle),
        }
    }
}

impl Watcher for PollWatcher {
    fn set_paths(&mut self, paths: &[PathBuf]) -> Result<()> {
        if let Ok(mut g) = self.paths.lock() {
            *g = paths.to_vec();
        }
        Ok(())
    }

    fn events(&self) -> Receiver<Vec<FsEvent>> {
        self.rx.clone()
    }
}

impl Drop for PollWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_mapping() {
        let e = event_from_flags("/a/b", EV_CREATED | 0x10000, |_| true).unwrap();
        assert_eq!(e.kind, FsEventKind::Created);
        assert!(!e.must_rescan);
        assert_eq!(
            event_from_flags("/a/b", EV_REMOVED, |_| true).unwrap().kind,
            FsEventKind::Removed
        );
        // Created and removed in one history: what exists now decides.
        let both = EV_CREATED | EV_REMOVED;
        assert_eq!(
            event_from_flags("/a/b", both, |_| true).unwrap().kind,
            FsEventKind::Created
        );
        assert_eq!(
            event_from_flags("/a/b", both, |_| false).unwrap().kind,
            FsEventKind::Removed
        );
        assert_eq!(
            event_from_flags("/a/b", EV_RENAMED | EV_CREATED, |_| true)
                .unwrap()
                .kind,
            FsEventKind::Renamed
        );
        assert_eq!(
            event_from_flags("/a/b", EV_XATTR_MOD, |_| true)
                .unwrap()
                .kind,
            FsEventKind::Modified
        );
        assert_eq!(
            event_from_flags("/a/b", 0x10000, |_| true).unwrap().kind,
            FsEventKind::Other
        );
        assert!(
            event_from_flags("/a", EV_MUST_SCAN_SUBDIRS, |_| true)
                .unwrap()
                .must_rescan
        );
        assert!(
            event_from_flags("/a", EV_KERNEL_DROPPED, |_| true)
                .unwrap()
                .must_rescan
        );
        assert!(
            event_from_flags("/a", EV_ROOT_CHANGED, |_| true)
                .unwrap()
                .must_rescan
        );
        assert!(event_from_flags("/a", EV_HISTORY_DONE, |_| true).is_none());
        assert_eq!(
            event_from_flags("/a/b/", EV_CREATED, |_| true)
                .unwrap()
                .path,
            PathBuf::from("/a/b")
        );
    }

    #[test]
    fn shallow_filter() {
        let root = Path::new("/r");
        let mk = |p: &str, r| FsEvent {
            path: p.into(),
            kind: FsEventKind::Created,
            must_rescan: r,
        };
        assert!(is_shallow_match(root, &mk("/r/a", false)));
        assert!(is_shallow_match(root, &mk("/r", false)));
        assert!(!is_shallow_match(root, &mk("/r/a/b", false)));
        assert!(is_shallow_match(root, &mk("/r/a/b", true)));
    }

    #[test]
    fn poll_diff() {
        let sig = |len| Sig {
            mtime: None,
            len,
            is_dir: false,
        };
        let old: Snapshot = [("a".into(), sig(1)), ("b".into(), sig(1))]
            .into_iter()
            .collect();
        let new: Snapshot = [("b".into(), sig(2)), ("c".into(), sig(1))]
            .into_iter()
            .collect();
        let mut ev = diff_snapshots(Path::new("/d"), &old, &new);
        ev.sort_by(|x, y| x.path.cmp(&y.path));
        let kinds: Vec<_> = ev
            .iter()
            .map(|e| (e.path.to_str().unwrap(), e.kind))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("/d/a", FsEventKind::Removed),
                ("/d/b", FsEventKind::Modified),
                ("/d/c", FsEventKind::Created)
            ]
        );
    }
}

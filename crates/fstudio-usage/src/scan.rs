//! The scanner: a parallel, fork-join walk over a folder or a volume.
//!
//! - Each folder is read in one go ([`crate::read`]); its subfolders are then
//!   scanned in parallel on a private rayon pool (plain threads, no async:
//!   F-010).
//! - Symlinks are never followed. Folders on other volumes are not entered
//!   unless [`ScanOptions::cross_volumes`] is set.
//! - A file with several hard links is counted once (the first link the walk
//!   reaches; the others are flagged [`flags::HARDLINK`] and add nothing).
//! - iCloud placeholders are never read (F-016): a placeholder file counts
//!   its logical size and is flagged; a placeholder *folder* is not listed.
//! - A folder that can't be read is flagged and counted, never fatal.
//! - Packages are sized in full but kept as one leaf.
//!
//! APFS clones: two clones of one file share their blocks, but each reports
//! the full allocated size, so a folder of clones is over-counted. There is no
//! cheap per-file way to see shared extents, so this is a known limitation.

use std::collections::HashSet;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use parking_lot::Mutex;
use rayon::prelude::*;

use crate::read::{EntryKind, Method, RawEntry, SF_DATALESS, read_dir};
use crate::tree::{NO_NODE, Node, NodeId, NodeKind, SizeMode, Tree, ext_of, flags};

/// How to scan.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Enter folders on other volumes (off: they are listed but not sized).
    pub cross_volumes: bool,
    /// How folders are read.
    pub method: Method,
    /// Worker threads (0 = a default for this machine).
    pub threads: usize,
    /// Keep every node (the usage view). Off keeps only totals, which is all
    /// Get Folder Size needs.
    pub keep_tree: bool,
    /// The size the tree is sorted by when the scan ends.
    pub mode: SizeMode,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            cross_volumes: false,
            method: Method::Auto,
            threads: 0,
            keep_tree: true,
            mode: SizeMode::Allocated,
        }
    }
}

/// The default worker count: twice the cores, between 4 and 24. Workers
/// spend most of their time waiting in the kernel, so more threads than
/// cores helps a little (home folder, 1.16 M items, 12 cores: 4 threads
/// 6.6 s, 8 threads 3.3 s, 12 threads 3.6 s, 24 threads 2.7 s, warm).
pub fn default_threads() -> usize {
    (std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        * 2)
    .clamp(4, 24)
}

/// A snapshot of a running scan's progress.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    /// Items seen so far.
    pub items: u64,
    /// Folders read so far.
    pub folders: u64,
    /// Allocated bytes seen so far.
    pub allocated: u64,
    /// Logical bytes seen so far.
    pub logical: u64,
    /// The folder being read most recently.
    pub current: String,
    /// Milliseconds since the scan started.
    pub elapsed_ms: u64,
}

/// Shared between a running scan and whoever watches or cancels it.
#[derive(Debug)]
pub struct ScanControl {
    cancel: AtomicBool,
    items: AtomicU64,
    folders: AtomicU64,
    allocated: AtomicU64,
    logical: AtomicU64,
    current: Mutex<PathBuf>,
    last_current_ms: AtomicU64,
    start: Instant,
}

impl Default for ScanControl {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanControl {
    /// A fresh control; the clock starts now.
    pub fn new() -> Self {
        ScanControl {
            cancel: AtomicBool::new(false),
            items: AtomicU64::new(0),
            folders: AtomicU64::new(0),
            allocated: AtomicU64::new(0),
            logical: AtomicU64::new(0),
            current: Mutex::new(PathBuf::new()),
            last_current_ms: AtomicU64::new(0),
            start: Instant::now(),
        }
    }
    /// Asks the scan to stop. Folders not yet read are flagged incomplete.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    /// True once [`Self::cancel`] was called.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    /// The progress so far.
    pub fn progress(&self) -> Progress {
        Progress {
            items: self.items.load(Ordering::Relaxed),
            folders: self.folders.load(Ordering::Relaxed),
            allocated: self.allocated.load(Ordering::Relaxed),
            logical: self.logical.load(Ordering::Relaxed),
            current: self.current.lock().to_string_lossy().into_owned(),
            elapsed_ms: self.start.elapsed().as_millis() as u64,
        }
    }

    fn note_folder(&self, path: &Path, entries: &[RawEntry]) {
        self.folders.fetch_add(1, Ordering::Relaxed);
        self.items
            .fetch_add(entries.len() as u64, Ordering::Relaxed);
        let (mut a, mut l) = (0u64, 0u64);
        for e in entries {
            a += e.allocated;
            l += e.logical;
        }
        self.allocated.fetch_add(a, Ordering::Relaxed);
        self.logical.fetch_add(l, Ordering::Relaxed);
        // The current folder is only for show: update it about 20 times a second.
        let now = self.start.elapsed().as_millis() as u64;
        let last = self.last_current_ms.load(Ordering::Relaxed);
        if now >= last + 50
            && self
                .last_current_ms
                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            *self.current.lock() = path.to_path_buf();
        }
    }
}

const SHARDS: usize = 64;

/// A set of `(dev, inode)` pairs split into locked shards, so threads rarely
/// wait on each other.
struct Seen {
    shards: Vec<Mutex<HashSet<(u64, u64)>>>,
}

impl Seen {
    fn new() -> Self {
        Seen {
            shards: (0..SHARDS).map(|_| Mutex::new(HashSet::new())).collect(),
        }
    }
    /// True the first time a pair is seen.
    fn first(&self, dev: u64, ino: u64) -> bool {
        let h = (ino ^ dev.rotate_left(17)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        self.shards[(h >> 58) as usize % SHARDS]
            .lock()
            .insert((dev, ino))
    }
}

struct Ctx<'a> {
    opts: &'a ScanOptions,
    ctl: &'a ScanControl,
    devs: Vec<u64>,
    skip: Vec<PathBuf>,
    links: Seen,
}

/// A scanned item before it is put into the arena.
struct ScanNode {
    name: Vec<u8>,
    kind: NodeKind,
    flags: u16,
    logical: u64,
    allocated: u64,
    cloud: u64,
    items: u64,
    denied: u32,
    mtime_ns: i64,
    descend: bool,
    children: Vec<ScanNode>,
}

impl ScanNode {
    fn weight(&self) -> u64 {
        1 + if self.kind == NodeKind::Dir {
            self.items
        } else {
            0
        }
    }
}

/// The Data volume of the startup disk. Scanning `/` reaches its contents
/// through the firmlinks (`/Users`, `/Applications`…), so the walk accepts
/// its device but skips its own mount point, which would count it twice.
const DATA_VOLUME: &str = "/System/Volumes/Data";

/// Scans the folder at `root` and returns the tree, sorted by
/// [`ScanOptions::mode`]. Blocks until done or cancelled; run it on a
/// background thread. `Err` only when `root` itself can't be used (missing,
/// not a folder); everything below it is reported per item.
pub fn scan(root: &Path, opts: &ScanOptions, ctl: &ScanControl) -> io::Result<Tree> {
    let meta = root.symlink_metadata()?;
    if !meta.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotADirectory, "not a folder"));
    }
    let mut raw = RawEntry::default();
    crate::read::fill_from_metadata(&mut raw, &meta);
    let mut devs = vec![raw.dev];
    let mut skip = Vec::new();
    if root == Path::new("/")
        && let Ok(m) = Path::new(DATA_VOLUME).symlink_metadata()
    {
        let mut d = RawEntry::default();
        crate::read::fill_from_metadata(&mut d, &m);
        devs.push(d.dev);
        skip.push(PathBuf::from(DATA_VOLUME));
    }
    let ctx = Ctx {
        opts,
        ctl,
        devs,
        skip,
        links: Seen::new(),
    };
    let mut top = ScanNode {
        name: Vec::new(),
        kind: NodeKind::Dir,
        flags: 0,
        logical: 0,
        allocated: 0,
        cloud: 0,
        items: 0,
        denied: 0,
        mtime_ns: raw.mtime_ns,
        descend: true,
        children: Vec::new(),
    };
    let threads = if opts.threads == 0 {
        default_threads()
    } else {
        opts.threads
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .stack_size(8 << 20)
        .thread_name(|i| format!("fstudio-usage-{i}"))
        .build()
        .map_err(io::Error::other)?;
    pool.install(|| scan_dir(&ctx, root, &mut top, opts.keep_tree));

    let mut tree = Tree::new(root);
    {
        let r = tree.node_mut(tree.root());
        r.flags = top.flags;
        r.logical = top.logical;
        r.allocated = top.allocated;
        r.cloud = top.cloud;
        r.items = top.items;
        r.denied = top.denied;
        r.mtime_ns = top.mtime_ns;
    }
    let root_id = tree.root();
    for c in top.children {
        attach(&mut tree, c, root_id);
    }
    tree.sort(opts.mode);
    Ok(tree)
}

fn attach(tree: &mut Tree, n: ScanNode, parent: NodeId) {
    let ext = match n.kind {
        NodeKind::File | NodeKind::Package => {
            let e = ext_of(&n.name);
            tree.intern_ext(&e)
        }
        _ => 0,
    };
    let node = Node {
        name: n.name.into_boxed_slice(),
        parent: NO_NODE,
        children: Vec::new(),
        kind: n.kind,
        flags: n.flags,
        ext,
        logical: n.logical,
        allocated: n.allocated,
        cloud: n.cloud,
        items: n.items,
        denied: n.denied,
        mtime_ns: n.mtime_ns,
    };
    let id = tree.push(node, parent);
    if !n.children.is_empty() {
        tree.node_mut(id).children.reserve_exact(n.children.len());
        for c in n.children {
            attach(tree, c, id);
        }
    }
}

fn is_denied(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(libc::EACCES) | Some(libc::EPERM))
}

impl Ctx<'_> {
    fn make_node(&self, r: RawEntry) -> ScanNode {
        let mut n = ScanNode {
            kind: NodeKind::File,
            flags: 0,
            logical: r.logical,
            allocated: r.allocated,
            cloud: 0,
            items: 0,
            denied: 0,
            mtime_ns: r.mtime_ns,
            descend: false,
            children: Vec::new(),
            name: r.name,
        };
        if r.error != 0 {
            n.flags |= flags::ERROR;
        }
        let dataless = r.flags & SF_DATALESS != 0;
        match r.kind {
            EntryKind::Dir => {
                let name = String::from_utf8_lossy(&n.name);
                n.kind = if r.has_bundle || fstudio_fs::is_package_name(&name) {
                    NodeKind::Package
                } else {
                    NodeKind::Dir
                };
                // A folder's own size is not counted (it is a few bytes on APFS).
                n.logical = 0;
                n.allocated = 0;
                if dataless {
                    n.flags |= flags::CLOUD_DIR;
                } else if !self.opts.cross_volumes && !self.devs.contains(&r.dev) {
                    n.flags |= flags::OTHER_VOLUME;
                } else if r.error == 0 {
                    n.descend = true;
                }
            }
            EntryKind::File => {
                if dataless {
                    n.flags |= flags::DATALESS;
                    n.cloud = n.logical;
                    n.allocated = 0;
                }
                if r.nlink > 1 && !self.links.first(r.dev, r.ino) {
                    n.flags |= flags::HARDLINK;
                    n.logical = 0;
                    n.allocated = 0;
                    n.cloud = 0;
                }
            }
            EntryKind::Symlink => n.kind = NodeKind::Symlink,
            EntryKind::Other => n.kind = NodeKind::Other,
        }
        n
    }
}

/// Reads `path` into `node` and scans its subfolders in parallel. With
/// `keep` off, only totals are kept (packages, Get Folder Size).
fn scan_dir(ctx: &Ctx, path: &Path, node: &mut ScanNode, keep: bool) {
    if ctx.ctl.is_cancelled() {
        node.flags |= flags::INCOMPLETE;
        return;
    }
    if ctx.skip.iter().any(|s| s == path) {
        node.flags |= flags::SKIPPED;
        return;
    }
    let mut raws = Vec::new();
    if let Err(e) = read_dir(path, ctx.opts.method, &mut raws) {
        if is_denied(&e) {
            node.flags |= flags::DENIED;
            node.denied += 1;
        } else {
            node.flags |= flags::ERROR;
        }
        return;
    }
    ctx.ctl.note_folder(path, &raws);
    let mut kids: Vec<ScanNode> = raws.into_iter().map(|r| ctx.make_node(r)).collect();
    let n_dirs = kids.iter().filter(|k| k.descend).count();
    let keep_below = |k: &ScanNode| keep && k.kind == NodeKind::Dir;
    if n_dirs == 1 {
        // No fork for a single subfolder (deep, narrow trees).
        if let Some(k) = kids.iter_mut().find(|k| k.descend) {
            let p = path.join(std::ffi::OsStr::from_bytes(&k.name));
            let kb = keep_below(k);
            scan_dir(ctx, &p, k, kb);
        }
    } else if n_dirs > 1 {
        kids.par_iter_mut().filter(|k| k.descend).for_each(|k| {
            let p = path.join(std::ffi::OsStr::from_bytes(&k.name));
            let kb = keep_below(k);
            scan_dir(ctx, &p, k, kb);
        });
    }
    for k in &kids {
        node.logical += k.logical;
        node.allocated += k.allocated;
        node.cloud += k.cloud;
        node.items += k.weight();
        node.denied += k.denied;
        node.mtime_ns = node.mtime_ns.max(k.mtime_ns);
        if k.flags & flags::INCOMPLETE != 0 {
            node.flags |= flags::INCOMPLETE;
        }
    }
    if keep {
        node.children = kids;
    }
}

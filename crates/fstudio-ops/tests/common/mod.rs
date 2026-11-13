//! Shared test helpers: a queue on a temp database with a folder trash,
//! tree builders and comparisons, and small disk images.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::Receiver;
use fstudio_db::Db;
use fstudio_ops::*;

pub const WAIT: Duration = Duration::from_secs(120);

pub struct Env {
    pub dir: tempfile::TempDir,
    pub q: OpQueue,
    pub events: Receiver<OpEvent>,
    pub trash: PathBuf,
    pub db_path: PathBuf,
}

impl Env {
    pub fn root(&self) -> &Path {
        self.dir.path()
    }
    pub fn p(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }
    /// Runs a request to the end and returns its status.
    pub fn run(&self, req: OpRequest) -> OpStatus {
        let id = self.q.enqueue(req).expect("enqueue");
        self.q.wait(id, WAIT).expect("status")
    }
    pub fn run_with(&self, req: OpRequest, opts: EnqueueOptions) -> OpStatus {
        let id = self.q.enqueue_with(req, opts).expect("enqueue");
        self.q.wait(id, WAIT).expect("status")
    }
    /// Undo and wait for the inverse ops.
    pub fn undo(&self) -> Result<()> {
        for id in self.q.undo()? {
            let st = self.q.wait(id, WAIT).unwrap();
            assert_eq!(st.state, OpState::Completed, "undo op: {st:?}");
        }
        Ok(())
    }
    pub fn drain(&self) -> Vec<OpEvent> {
        self.events.try_iter().collect()
    }
}

/// The copy backend these tests run on: `FSTUDIO_COPY_BACKEND=safe_transfer`
/// runs the whole suite on Safe Transfer (CI runs it both ways, F-026).
pub fn backend() -> CopyBackend {
    std::env::var("FSTUDIO_COPY_BACKEND")
        .map(|v| CopyBackend::parse(&v))
        .unwrap_or_default()
}

/// Whether this run is on Safe Transfer.
pub fn on_safe_transfer() -> bool {
    backend() == CopyBackend::SafeTransfer
}

/// Opens a queue on the database at `db_path` with a folder trash, on the
/// tests' backend. Safe Transfer keeps its journal next to the database, so
/// reopening (after a crash, say) finds the same jobs.
pub fn open_queue(
    db_path: &Path,
    mut cfg: OpsConfig,
    trash: &Path,
    sink: Arc<dyn EventSink>,
) -> OpQueue {
    let trasher: Arc<dyn Trasher> = Arc::new(TestTrasher {
        main: DirTrasher::new(trash).unwrap(),
    });
    cfg.copy_backend = backend();
    let transfer = on_safe_transfer().then(|| {
        let dir = db_path.parent().unwrap().join("transfers");
        Transfer::open(dir, false, trasher.clone(), cfg.faults.clone()).unwrap()
    });
    OpQueue::open_with_transfer(Db::open(db_path).unwrap(), cfg, trasher, sink, transfer).unwrap()
}

/// The tests' Trash: a folder next to the database, and for items on
/// another volume (a disk image) a `.test-trash` folder at that volume's
/// top, as the real Trash has a `.Trashes` per volume. Safe Transfer sends
/// moved originals there; ops' own engine only trashes on the main volume.
struct TestTrasher {
    main: DirTrasher,
}

impl Trasher for TestTrasher {
    fn trash(&self, path: &Path) -> std::io::Result<PathBuf> {
        match self.main.trash(path) {
            Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
                let dev = std::fs::symlink_metadata(path)?.dev();
                let mut top = path.parent().unwrap_or(path).to_path_buf();
                while let Some(up) = top.parent() {
                    match std::fs::symlink_metadata(up) {
                        Ok(m) if m.dev() == dev => top = up.to_path_buf(),
                        _ => break,
                    }
                }
                DirTrasher::new(top.join(".test-trash"))?.trash(path)
            }
            r => r,
        }
    }
}

pub fn env_with(cfg: OpsConfig) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let trash = dir.path().join(".test-trash");
    let db_path = dir.path().join(".db/fstudio.sqlite3");
    let (tx, rx) = crossbeam_channel::unbounded();
    let q = open_queue(&db_path, cfg, &trash, Arc::new(tx));
    Env {
        dir,
        q,
        events: rx,
        trash,
        db_path,
    }
}

pub fn env() -> Env {
    env_with(OpsConfig::default())
}

pub fn write(p: &Path, data: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, data).unwrap();
}

pub fn read(p: &Path) -> Vec<u8> {
    std::fs::read(p).unwrap()
}

pub fn exists(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// A tree snapshot: relative path → ("d" | "l:<target>" | "f:<hash>").
pub fn tree(root: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    fn go(root: &Path, p: &Path, out: &mut BTreeMap<String, String>) {
        for e in std::fs::read_dir(p).unwrap() {
            let e = e.unwrap();
            let path = e.path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let md = std::fs::symlink_metadata(&path).unwrap();
            if md.file_type().is_symlink() {
                out.insert(
                    rel,
                    format!("l:{}", std::fs::read_link(&path).unwrap().display()),
                );
            } else if md.is_dir() {
                out.insert(rel, "d".into());
                go(root, &path, out);
            } else {
                out.insert(rel, format!("f:{}", hash_file(&path).unwrap()));
            }
        }
    }
    go(root, root, &mut out);
    out
}

/// Files under `root` whose names are our temporary names.
pub fn partials(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    fn go(p: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        for e in rd.flatten() {
            let path = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            // Either backend's temporary names.
            if name.ends_with(".fstudio-partial") || name.ends_with(".st-partial") {
                out.push(path.clone());
            }
            if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                go(&path, out);
            }
        }
    }
    go(root, &mut out);
    out
}

pub fn mtime_ns(p: &Path) -> i64 {
    let m = std::fs::symlink_metadata(p).unwrap();
    m.mtime() * 1_000_000_000 + m.mtime_nsec()
}

/// The longest any `hdiutil` (or `plutil`, `kill`) call may take. A stuck
/// disk-image service fails the test with a message instead of hanging it.
pub const TOOL_LIMIT: Duration = Duration::from_secs(60);

/// Runs `cmd` with a time limit. `None` if it couldn't start or ran over
/// (it is then killed).
fn run_limited<S: AsRef<std::ffi::OsStr>>(cmd: &str, args: &[S]) -> Option<std::process::Output> {
    use std::io::Read;
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    let drain = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut v = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut v);
            }
            v
        })
    };
    let out = drain(child.stdout.take().map(|r| Box::new(r) as _));
    let err = drain(child.stderr.take().map(|r| Box::new(r) as _));
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if start.elapsed() < TOOL_LIMIT => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    Some(std::process::Output {
        status,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}

/// Runs a command (time-limited), panicking with its output on failure.
pub fn sh(cmd: &str, args: &[&str]) -> String {
    let out = run_limited(cmd, args)
        .unwrap_or_else(|| panic!("{cmd} {args:?} didn't finish in {} s", TOOL_LIMIT.as_secs()));
    assert!(
        out.status.success(),
        "{cmd} {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Attached images: (image path, its mount points), from `hdiutil info`.
pub fn attached_images() -> Option<Vec<(PathBuf, Vec<PathBuf>)>> {
    let plist = run_limited("hdiutil", &["info", "-plist"])?;
    let dir = tempfile::tempdir().ok()?;
    let f = dir.path().join("info.plist");
    std::fs::write(&f, &plist.stdout).ok()?;
    let json = run_limited(
        "plutil",
        &[
            std::ffi::OsStr::new("-convert"),
            std::ffi::OsStr::new("json"),
            std::ffi::OsStr::new("-o"),
            std::ffi::OsStr::new("-"),
            f.as_os_str(),
        ],
    )?;
    let v: serde_json::Value = serde_json::from_slice(&json.stdout).ok()?;
    Some(
        v.get("images")?
            .as_array()?
            .iter()
            .filter_map(|im| {
                let path = PathBuf::from(im.get("image-path")?.as_str()?);
                let mounts = im
                    .get("system-entities")
                    .and_then(|e| e.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e.get("mount-point")?.as_str().map(PathBuf::from))
                    .collect();
                Some((path, mounts))
            })
            .collect(),
    )
}

fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (std::fs::canonicalize(a), std::fs::canonicalize(b)),
            (Ok(x), Ok(y)) if x == y
        )
}

/// The file next to each test image that names the test process owning it.
const OWNER: &str = "owner.pid";

/// Detaches and deletes images that an earlier test process left attached
/// when it died without unwinding (an abort or a kill skips `Drop`). Only
/// images made here are touched: `vol.dmg` in a temporary folder, with an
/// owner file naming a process that no longer runs. Once per process.
fn sweep_orphaned_images() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Some(images) = attached_images() else {
            return;
        };
        let tmp = std::fs::canonicalize(std::env::temp_dir()).unwrap_or_default();
        for (img, mounts) in images {
            let Some(dir) = img.parent() else { continue };
            let canon = std::fs::canonicalize(dir).unwrap_or_default();
            if img.file_name() != Some(std::ffi::OsStr::new("vol.dmg")) || !canon.starts_with(&tmp)
            {
                continue;
            }
            let Some(pid) = std::fs::read_to_string(dir.join(OWNER))
                .ok()
                .and_then(|t| t.trim().parse::<u32>().ok())
            else {
                continue;
            };
            let alive =
                run_limited("kill", &["-0", &pid.to_string()]).is_none_or(|o| o.status.success());
            if alive || pid == std::process::id() {
                continue;
            }
            eprintln!(
                "removing a test image left by dead process {pid}: {}",
                img.display()
            );
            for m in &mounts {
                let _ = run_limited(
                    "hdiutil",
                    &[
                        std::ffi::OsStr::new("detach"),
                        std::ffi::OsStr::new("-force"),
                        std::ffi::OsStr::new("-quiet"),
                        m.as_os_str(),
                    ],
                );
            }
            if !is_attached(&img) {
                let _ = std::fs::remove_file(&img);
                let _ = std::fs::remove_file(dir.join(OWNER));
                let _ = std::fs::remove_dir(dir.join("mnt"));
                let _ = std::fs::remove_dir(dir);
            }
        }
    });
}

/// Whether `img` is attached (unknown counts as attached).
pub fn is_attached(img: &Path) -> bool {
    attached_images().is_none_or(|all| all.iter().any(|(p, _)| same_file(p, img)))
}

/// A small disk image, attached without Finder; detached and deleted on drop.
pub struct Image {
    /// Its folder (`None` once kept for a later sweep).
    dir: Option<tempfile::TempDir>,
    pub mount: PathBuf,
    /// The image file (to attach it again after a forced detach).
    pub img: PathBuf,
}

impl Image {
    /// `fs` is an `hdiutil` filesystem name: "APFS", "Case-sensitive APFS", "HFS+", "ExFAT".
    pub fn new(fs: &str, megabytes: u32) -> Image {
        Self::with(fs, megabytes, false)
    }
    /// A writable image. On the safe_transfer backend it gets more room:
    /// Safe Transfer refuses a job that would leave less than 100 MB free
    /// (its safety margin, proposal §10), and these tests (some of which
    /// fill most of their image) are about other things. [`Image::exact`]
    /// keeps the size (the full-disk test).
    pub fn with(fs: &str, megabytes: u32, readonly: bool) -> Image {
        let room = if on_safe_transfer() && !readonly {
            200 + megabytes * 3 / 4
        } else {
            0
        };
        Self::exact_with(fs, megabytes + room, readonly)
    }
    /// An image of exactly `megabytes`, whatever the backend.
    pub fn exact(fs: &str, megabytes: u32) -> Image {
        Self::exact_with(fs, megabytes, false)
    }
    fn exact_with(fs: &str, megabytes: u32, readonly: bool) -> Image {
        sweep_orphaned_images();
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("vol.dmg");
        let mount = dir.path().join("mnt");
        std::fs::create_dir(&mount).unwrap();
        // Before anything is attached: a later test process removes this
        // image if this one dies without detaching it.
        std::fs::write(dir.path().join(OWNER), std::process::id().to_string()).unwrap();
        sh(
            "hdiutil",
            &[
                "create",
                "-quiet",
                "-megabytes",
                &megabytes.to_string(),
                "-fs",
                fs,
                "-volname",
                "FSTEST",
                img.to_str().unwrap(),
            ],
        );
        // From here on `Drop` detaches it, also if attaching panics.
        let im = Image {
            dir: Some(dir),
            mount,
            img,
        };
        let mut args = vec![
            "attach",
            "-quiet",
            "-nobrowse",
            "-noverify",
            "-noautoopen",
            "-mountpoint",
            im.mount.to_str().unwrap(),
        ];
        if readonly {
            args.push("-readonly");
        }
        args.push(im.img.to_str().unwrap());
        sh("hdiutil", &args);
        im
    }

    /// Pulls the "card" out: `hdiutil detach -force`, as a card pulled out
    /// mid-copy would leave things.
    pub fn pull_out(&self) {
        sh(
            "hdiutil",
            &["detach", "-force", "-quiet", self.mount.to_str().unwrap()],
        );
    }

    /// Puts it back at the same mount point.
    pub fn put_back(&self) {
        sh(
            "hdiutil",
            &[
                "attach",
                "-quiet",
                "-nobrowse",
                "-noverify",
                "-noautoopen",
                "-mountpoint",
                self.mount.to_str().unwrap(),
                self.img.to_str().unwrap(),
            ],
        );
    }
}

impl Drop for Image {
    /// Detaches (a forced detach if a plain one is refused), then checks the
    /// image really is gone. Never panics, also while a failed test unwinds;
    /// every call has a time limit. An image it can't detach is named on
    /// stderr, and the next test process removes it.
    fn drop(&mut self) {
        let m = self.mount.as_os_str();
        let detach = |force: bool| {
            let mut a = vec![
                std::ffi::OsStr::new("detach"),
                std::ffi::OsStr::new("-quiet"),
            ];
            if force {
                a.push(std::ffi::OsStr::new("-force"));
            }
            a.push(m);
            run_limited("hdiutil", &a).is_some_and(|o| o.status.success())
        };
        for _ in 0..5 {
            if detach(false) || !is_attached(&self.img) {
                return;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        detach(true);
        if is_attached(&self.img) {
            eprintln!(
                "WARNING: couldn't detach test image {}; the next test run removes it",
                self.img.display()
            );
            // Keep the folder (and the owner file) for that sweep.
            if let Some(dir) = self.dir.take() {
                let _ = dir.keep();
            }
        }
    }
}

/// Waits until the op reports a pending conflict.
pub fn wait_conflict(e: &Env, id: OpId) -> ConflictInfo {
    let t = std::time::Instant::now();
    loop {
        let st = e.q.status(id).unwrap();
        if let Some(c) = st.conflicts.first() {
            return c.clone();
        }
        assert!(
            !st.state.is_finished(),
            "finished without a conflict: {st:?}"
        );
        assert!(t.elapsed() < WAIT, "no conflict");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Kind of source/destination item for conflict tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum K {
    File,
    Dir,
    Link,
}

/// Makes an item of kind `k` at `p` whose content mentions `tag`.
pub fn make(p: &Path, k: K, tag: &str) {
    match k {
        K::File => write(p, tag.as_bytes()),
        K::Dir => {
            write(&p.join(format!("{tag}.txt")), tag.as_bytes());
            write(&p.join("shared.txt"), tag.as_bytes());
        }
        K::Link => {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(format!("target-{tag}"), p).unwrap();
        }
    }
}

/// Fails `step` for any source whose path contains `needle`.
pub struct FailAt {
    pub step: FaultStep,
    pub needle: String,
}

impl FaultHook for FailAt {
    fn check(&self, step: FaultStep, path: &Path) -> std::io::Result<()> {
        if step == self.step && path.to_string_lossy().contains(&self.needle) {
            Err(std::io::Error::other(format!(
                "injected failure at {step:?}"
            )))
        } else {
            Ok(())
        }
    }
}

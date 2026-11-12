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

pub fn env_with(cfg: OpsConfig) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let trash = dir.path().join(".test-trash");
    let db_path = dir.path().join(".db/fstudio.sqlite3");
    let db = Db::open(&db_path).unwrap();
    let (tx, rx) = crossbeam_channel::unbounded();
    let q = OpQueue::open(
        db,
        cfg,
        Arc::new(DirTrasher::new(&trash).unwrap()),
        Arc::new(tx),
    )
    .unwrap();
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
            if e.file_name()
                .to_string_lossy()
                .ends_with(".fstudio-partial")
            {
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

pub fn sh(cmd: &str, args: &[&str]) -> String {
    let out = Command::new(cmd).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{cmd} {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A small disk image, attached without Finder; detached and deleted on drop.
pub struct Image {
    _dir: tempfile::TempDir,
    pub mount: PathBuf,
}

impl Image {
    /// `fs` is an `hdiutil` filesystem name: "APFS", "Case-sensitive APFS", "HFS+", "ExFAT".
    pub fn new(fs: &str, megabytes: u32) -> Image {
        Self::with(fs, megabytes, false)
    }
    pub fn with(fs: &str, megabytes: u32, readonly: bool) -> Image {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("vol.dmg");
        let mount = dir.path().join("mnt");
        std::fs::create_dir(&mount).unwrap();
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
                "fstudio-test",
                "-layout",
                "NONE",
                img.to_str().unwrap(),
            ],
        );
        let mut args = vec![
            "attach",
            "-quiet",
            "-nobrowse",
            "-noverify",
            "-noautoopen",
            "-mountpoint",
            mount.to_str().unwrap(),
        ];
        if readonly {
            args.push("-readonly");
        }
        args.push(img.to_str().unwrap());
        sh("hdiutil", &args);
        Image { _dir: dir, mount }
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        let m = self.mount.to_str().unwrap().to_string();
        for _ in 0..5 {
            let ok = Command::new("hdiutil")
                .args(["detach", "-quiet", &m])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok {
                return;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        let _ = Command::new("hdiutil")
            .args(["detach", "-force", "-quiet", &m])
            .status();
    }
}

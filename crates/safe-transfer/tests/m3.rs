//! M3: conflicts, moves and folder metadata, through the engine, on temp
//! folders of the internal disk. Cross-volume moves are simulated with a
//! volume identity that gives the destination folder its own UUID, and the
//! Trash is a test folder, so nothing touches the real Trash.
// Uses the macOS copy path (F-024: macOS-only tests are gated).
#![cfg(target_os = "macos")]

mod common;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use safe_transfer::{
    ConflictPolicy, Decision, Disposal, Engine, EngineConfig, Event, Item, JobId, JobSpec,
    JobState, Mode, Options, Platform, Trash, VolumeIdentity, VolumeInfo,
};

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// A Trash that is a folder: items are renamed into it, never removed.
struct TestTrash {
    dir: PathBuf,
    n: AtomicU32,
}

impl TestTrash {
    fn new(dir: &Path) -> Arc<TestTrash> {
        std::fs::create_dir_all(dir).unwrap();
        Arc::new(TestTrash {
            dir: dir.to_path_buf(),
            n: AtomicU32::new(0),
        })
    }
    fn names(&self) -> Vec<String> {
        names(&self.dir)
    }
}

impl Trash for TestTrash {
    fn trash(&self, path: &Path) -> io::Result<PathBuf> {
        let n = self.n.fetch_add(1, Ordering::Relaxed);
        let to = self.dir.join(format!(
            "{n}-{}",
            path.file_name().unwrap().to_string_lossy()
        ));
        std::fs::rename(path, &to)?;
        Ok(to)
    }
}

/// Gives everything under `root` a volume of its own, as if it were a card.
struct OtherVolume {
    root: PathBuf,
}

impl OtherVolume {
    fn info(&self) -> VolumeInfo {
        let mut v = Platform::native().volumes.volume_of(&self.root).unwrap();
        v.uuid = Some("00000000-0000-4000-8000-0000CA4D0000".into());
        v.name = "CARD".into();
        v.mount = self.root.clone();
        v
    }
}

impl VolumeIdentity for OtherVolume {
    fn volume_of(&self, p: &Path) -> io::Result<VolumeInfo> {
        let c = std::fs::canonicalize(p).unwrap_or(p.to_path_buf());
        if c.starts_with(&self.root) {
            Ok(self.info())
        } else {
            Platform::native().volumes.volume_of(p)
        }
    }
    fn mounted(&self) -> io::Result<Vec<VolumeInfo>> {
        let mut v = Platform::native().volumes.mounted()?;
        v.push(self.info());
        Ok(v)
    }
    fn case_sensitive(&self, dir: &Path) -> io::Result<bool> {
        Platform::native().volumes.case_sensitive(dir)
    }
}

#[derive(Default, Clone)]
struct Events(Arc<Mutex<Vec<Event>>>);
impl Events {
    fn sink(&self) -> Arc<dyn safe_transfer::EventSink> {
        let me = self.0.clone();
        Arc::new(move |e: Event| me.lock().unwrap().push(e))
    }
    fn conflicts(&self) -> usize {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, Event::Conflict { .. }))
            .count()
    }
}

fn platform(trash: &Arc<TestTrash>, other: Option<&Path>) -> Platform {
    let mut p = Platform::native();
    p.trash = trash.clone();
    if let Some(root) = other {
        p.volumes = Arc::new(OtherVolume {
            root: std::fs::canonicalize(root).unwrap(),
        });
    }
    p
}

fn engine(journal: &Path, p: Platform, ev: &Events) -> Engine {
    let mut cfg = EngineConfig::new(journal);
    cfg.journal_interval = Duration::ZERO;
    Engine::open(cfg, p, ev.sink()).unwrap()
}

fn job(src: &Path, dest: &Path, f: impl FnOnce(&mut Options)) -> JobSpec {
    let mut s = JobSpec::new(vec![Item::Path(src.to_path_buf())], dest);
    s.options = Options {
        chunk_size: 1 << 20,
        ..no_clone()
    };
    f(&mut s.options);
    s
}

fn run(e: &Engine, spec: JobSpec) -> JobId {
    let id = e.create(spec).unwrap();
    e.start(id).unwrap();
    id
}

fn finish(e: &Engine, id: JobId) -> JobState {
    e.wait(id, Duration::from_secs(120)).unwrap().state
}

/// Waits for the job to stop at its questions.
fn questions(e: &Engine, id: JobId) {
    let t = Instant::now();
    while e.job(id).unwrap().state != JobState::WaitingForAnswer {
        assert!(t.elapsed() < Duration::from_secs(60), "{:?}", e.job(id));
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// `dir/name` with `content`.
fn put(dir: &Path, name: &str, content: &[u8]) -> PathBuf {
    let p = dir.join(name);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(&p, content).unwrap();
    p
}

// ───────────────────────── conflicts ─────────────────────────

#[test]
fn asked_names_wait_while_the_rest_is_copied_then_follow_the_answers() {
    let (s, d, j, t) = (tempdir(), tempdir(), tempdir(), tempdir());
    let src = s.path().join("shoot");
    put(&src, "a.ARW", b"new a");
    put(&src, "b.ARW", b"new b");
    put(&src, "c.ARW", b"new c");
    put(&d.path().join("shoot"), "a.ARW", b"old a");
    put(&d.path().join("shoot"), "b.ARW", b"old b");
    let trash = TestTrash::new(t.path());
    let ev = Events::default();
    let e = engine(j.path(), platform(&trash, None), &ev);
    let plan = e.plan(&job(&src, d.path(), |_| {})).unwrap();
    assert_eq!(plan.conflicts.len(), 2);
    let id = run(&e, job(&src, d.path(), |_| {}));
    questions(&e, id);
    // The free name is already copied; the taken ones wait.
    assert_eq!(
        std::fs::read(d.path().join("shoot/c.ARW")).unwrap(),
        b"new c"
    );
    assert_eq!(ev.conflicts(), 2);
    let qs = e.conflicts(id);
    assert_eq!(qs.len(), 2);
    let a = qs.iter().find(|q| q.path.ends_with("a.ARW")).unwrap();
    let b = qs.iter().find(|q| q.path.ends_with("b.ARW")).unwrap();
    assert_eq!(a.existing_size, 5);
    assert!(
        e.resolve_conflict(id, a.id, Decision::Merge, false)
            .is_err(),
        "merge is for folders"
    );
    e.resolve_conflict(id, a.id, Decision::Replace, false)
        .unwrap();
    e.resolve_conflict(id, b.id, Decision::KeepBoth, false)
        .unwrap();
    assert_eq!(finish(&e, id), JobState::Finished);
    let dest = d.path().join("shoot");
    assert_eq!(std::fs::read(dest.join("a.ARW")).unwrap(), b"new a");
    assert_eq!(std::fs::read(dest.join("b.ARW")).unwrap(), b"old b");
    assert_eq!(std::fs::read(dest.join("b 2.ARW")).unwrap(), b"new b");
    // The replaced file is in the Trash, not gone.
    assert_eq!(trash.names().len(), 1);
    assert_eq!(
        std::fs::read(t.path().join(&trash.names()[0])).unwrap(),
        b"old a"
    );
    let r = e.report(id).unwrap();
    assert_eq!(r.replaced.len(), 1, "{r:?}");
    assert_eq!(r.kept_both.len(), 1, "{r:?}");
    assert!(r.kept_both[0].reason.contains("b 2.ARW"));
    assert!(names(&dest).iter().all(|n| !n.contains("st-partial")));
}

#[test]
fn apply_to_all_answers_the_rest_too() {
    let (s, d, j, t) = (tempdir(), tempdir(), tempdir(), tempdir());
    let src = s.path().join("cards");
    for i in 0..6 {
        put(&src, &format!("{i}.ARW"), format!("new {i}").as_bytes());
        put(&d.path().join("cards"), &format!("{i}.ARW"), b"old");
    }
    let e = engine(
        j.path(),
        platform(&TestTrash::new(t.path()), None),
        &Events::default(),
    );
    let id = run(&e, job(&src, d.path(), |_| {}));
    questions(&e, id);
    let first = e.conflicts(id)[0].id;
    e.resolve_conflict(id, first, Decision::Skip, true).unwrap();
    assert_eq!(finish(&e, id), JobState::Finished);
    assert_eq!(e.report(id).unwrap().skipped.len(), 6);
    for i in 0..6 {
        assert_eq!(
            std::fs::read(d.path().join(format!("cards/{i}.ARW"))).unwrap(),
            b"old"
        );
    }
}

#[test]
fn policies_decide_without_asking() {
    let (s, d, j, t) = (tempdir(), tempdir(), tempdir(), tempdir());
    let src = s.path().join("x");
    put(&src, "same.txt", b"identical");
    put(&src, "diff.txt", b"new");
    put(&d.path().join("x"), "same.txt", b"identical");
    put(&d.path().join("x"), "diff.txt", b"old");
    let trash = TestTrash::new(t.path());
    let e = engine(j.path(), platform(&trash, None), &Events::default());
    // Skip if identical, otherwise ask: one skipped, one asked.
    let id = run(
        &e,
        job(&src, d.path(), |o| {
            o.on_conflict.files = Decision::SkipIfIdentical
        }),
    );
    questions(&e, id);
    assert_eq!(e.conflicts(id).len(), 1);
    assert!(e.conflicts(id)[0].path.ends_with("diff.txt"));
    let q = e.conflicts(id)[0].id;
    e.resolve_conflict(id, q, Decision::Replace, false).unwrap();
    assert_eq!(finish(&e, id), JobState::Finished);
    let r = e.report(id).unwrap();
    assert!(
        r.skipped.iter().any(|n| n.reason.contains("identical")),
        "{r:?}"
    );
    assert_eq!(std::fs::read(d.path().join("x/diff.txt")).unwrap(), b"new");

    // Keep both by policy, twice: "name 2", then "name 3".
    for want in ["diff 2.txt", "diff 3.txt"] {
        let id = run(
            &e,
            job(&src, d.path(), |o| o.on_conflict.files = Decision::KeepBoth),
        );
        assert_eq!(finish(&e, id), JobState::Finished);
        assert_eq!(
            std::fs::read(d.path().join("x").join(want)).unwrap(),
            b"new"
        );
    }
}

#[test]
fn folders_merge_keep_both_skip_or_replace() {
    let (s, d, j, t) = (tempdir(), tempdir(), tempdir(), tempdir());
    let src = s.path().join("Job");
    put(&src, "new.txt", b"new");
    put(&src, "sub/deep.txt", b"deep");
    let trash = TestTrash::new(t.path());
    let e = engine(j.path(), platform(&trash, None), &Events::default());
    let existing = d.path().join("Job");
    put(&existing, "old.txt", b"old");

    // Merge (the default): both files end up in the one folder.
    let id = run(
        &e,
        job(&src, d.path(), |o| o.on_conflict.files = Decision::Skip),
    );
    assert_eq!(finish(&e, id), JobState::Finished);
    assert_eq!(names(&existing), ["new.txt", "old.txt", "sub"]);

    // Keep both: a second folder "Job 2" with the whole tree.
    let id = run(
        &e,
        job(&src, d.path(), |o| {
            o.on_conflict.folders = Decision::KeepBoth
        }),
    );
    assert_eq!(finish(&e, id), JobState::Finished);
    assert_eq!(
        std::fs::read(d.path().join("Job 2/sub/deep.txt")).unwrap(),
        b"deep"
    );

    // Skip: nothing inside is copied.
    put(&src, "later.txt", b"later");
    let id = run(
        &e,
        job(&src, d.path(), |o| o.on_conflict.folders = Decision::Skip),
    );
    assert_eq!(finish(&e, id), JobState::Finished);
    assert!(!existing.join("later.txt").exists());
    assert!(e.report(id).unwrap().skipped.len() >= 3);

    // Replace: built under a hidden name, swapped in when complete; the old
    // folder (with old.txt) is in the Trash.
    let id = run(
        &e,
        job(&src, d.path(), |o| {
            o.on_conflict.folders = Decision::Replace
        }),
    );
    assert_eq!(finish(&e, id), JobState::Finished);
    assert_eq!(names(&existing), ["later.txt", "new.txt", "sub"]);
    assert!(names(d.path()).iter().all(|n| !n.contains("st-partial")));
    let trashed = trash.names();
    assert_eq!(trashed.len(), 1);
    assert!(t.path().join(&trashed[0]).join("old.txt").exists());
    assert_eq!(e.report(id).unwrap().replaced.len(), 1);
}

#[test]
fn a_folder_is_never_half_replaced() {
    let (s, d, j, t) = (tempdir(), tempdir(), tempdir(), tempdir());
    let src = s.path().join("Job");
    put(&src, "fine.txt", b"fine");
    let locked = put(&src, "locked.txt", b"secret");
    std::fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
    let existing = d.path().join("Job");
    put(&existing, "old.txt", b"old");
    let trash = TestTrash::new(t.path());
    let e = engine(j.path(), platform(&trash, None), &Events::default());
    let id = run(
        &e,
        job(&src, d.path(), |o| {
            o.on_conflict.folders = Decision::Replace
        }),
    );
    finish(&e, id);
    std::fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    // The old folder is untouched; the incomplete new one went to the Trash.
    assert_eq!(names(&existing), ["old.txt"]);
    assert!(names(d.path()).iter().all(|n| !n.contains("st-partial")));
    let r = e.report(id).unwrap();
    assert!(
        r.failed
            .iter()
            .any(|n| n.reason.contains("wasn't replaced")),
        "{r:?}"
    );
}

// ───────────────────────── moves ─────────────────────────

#[test]
fn a_move_on_one_volume_is_a_rename() {
    let (s, d, j, t) = (tempdir(), tempdir(), tempdir(), tempdir());
    let src = s.path().join("Smith");
    put(&src, "a.ARW", &bytes(100_000, 1));
    put(&src, "sub/b.ARW", &bytes(50_000, 2));
    let ino = std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(src.join("a.ARW")).unwrap());
    let trash = TestTrash::new(t.path());
    let e = engine(j.path(), platform(&trash, None), &Events::default());
    let id = run(&e, job(&src, d.path(), |o| o.mode = Mode::Move));
    assert_eq!(finish(&e, id), JobState::Finished);
    assert!(!src.exists(), "the original folder moved");
    let moved = d.path().join("Smith/a.ARW");
    assert_eq!(std::fs::read(&moved).unwrap(), bytes(100_000, 1));
    // Same file, not a copy.
    assert_eq!(
        std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&moved).unwrap()),
        ino
    );
    assert!(trash.names().is_empty());
    assert_eq!(
        e.report(id).unwrap().moved,
        1,
        "one rename moved the whole folder"
    );
}

#[test]
fn a_move_into_an_existing_folder_moves_each_item_and_removes_the_emptied_one() {
    let (s, d, j, t) = (tempdir(), tempdir(), tempdir(), tempdir());
    let src = s.path().join("Smith");
    put(&src, "a.ARW", b"a");
    put(&src, "b.ARW", b"b");
    put(&d.path().join("Smith"), "old.ARW", b"old");
    let e = engine(
        j.path(),
        platform(&TestTrash::new(t.path()), None),
        &Events::default(),
    );
    let id = run(&e, job(&src, d.path(), |o| o.mode = Mode::Move));
    assert_eq!(finish(&e, id), JobState::Finished);
    assert_eq!(
        names(&d.path().join("Smith")),
        ["a.ARW", "b.ARW", "old.ARW"]
    );
    assert!(!src.exists(), "the emptied original folder was removed");
    assert_eq!(e.report(id).unwrap().moved, 2);
}

#[test]
fn a_move_across_volumes_copies_verifies_then_trashes_each_original() {
    let (s, d, j, t) = (tempdir(), tempdir(), tempdir(), tempdir());
    let src = s.path().join("Card");
    put(&src, "DSC1.ARW", &bytes(300_000, 1));
    put(&src, "DCIM/DSC2.ARW", &bytes(9 << 20, 2));
    put(&src, "DCIM/DSC3.XMP", b"sidecar");
    let trash = TestTrash::new(t.path());
    let e = engine(
        j.path(),
        platform(&trash, Some(d.path())),
        &Events::default(),
    );
    let id = run(&e, job(&src, d.path(), |o| o.mode = Mode::Move));
    assert_eq!(finish(&e, id), JobState::Finished);
    let dest = d.path().join("Card");
    assert_eq!(
        std::fs::read(dest.join("DCIM/DSC2.ARW")).unwrap(),
        bytes(9 << 20, 2)
    );
    assert_eq!(
        std::fs::read(dest.join("DSC1.ARW")).unwrap(),
        bytes(300_000, 1)
    );
    // Every original is in the (test) Trash; the emptied folders are gone.
    let mut t_names = trash.names();
    t_names.sort();
    assert_eq!(t_names.len(), 3, "{t_names:?}");
    assert!(!src.exists(), "{:?}", names(s.path()));
    let r = e.report(id).unwrap();
    assert_eq!(r.sources_trashed.len(), 3, "{r:?}");
    assert_eq!(r.copied_verified, 3);
    assert_eq!(r.moved, 0);
}

#[test]
fn a_move_that_keeps_originals_only_copies() {
    let (s, d, j, t) = (tempdir(), tempdir(), tempdir(), tempdir());
    let src = s.path().join("Card");
    put(&src, "a.ARW", b"a");
    let trash = TestTrash::new(t.path());
    let e = engine(
        j.path(),
        platform(&trash, Some(d.path())),
        &Events::default(),
    );
    let id = run(
        &e,
        job(&src, d.path(), |o| {
            o.mode = Mode::Move;
            o.disposal = Disposal::Keep;
        }),
    );
    assert_eq!(finish(&e, id), JobState::Finished);
    assert!(src.join("a.ARW").exists());
    assert!(trash.names().is_empty());
}

#[test]
fn moves_follow_conflict_answers_too() {
    let (s, d, j, t) = (tempdir(), tempdir(), tempdir(), tempdir());
    let src = s.path().join("Card");
    put(&src, "a.ARW", b"new");
    put(&d.path().join("Card"), "a.ARW", b"old");
    let trash = TestTrash::new(t.path());
    let e = engine(j.path(), platform(&trash, None), &Events::default());
    let id = run(
        &e,
        job(&src, d.path(), |o| {
            o.mode = Mode::Move;
            o.on_conflict = ConflictPolicy {
                files: Decision::Replace,
                folders: Decision::Merge,
            };
        }),
    );
    assert_eq!(finish(&e, id), JobState::Finished);
    assert_eq!(std::fs::read(d.path().join("Card/a.ARW")).unwrap(), b"new");
    assert_eq!(trash.names().len(), 1, "the old file is in the Trash");
    assert!(!src.exists());
}

// ───────────────────────── metadata ─────────────────────────

#[test]
fn folders_get_their_dates_and_tags_after_their_contents() {
    let (s, d, j, t) = (tempdir(), tempdir(), tempdir(), tempdir());
    let src = s.path().join("2019 Wedding");
    put(&src, "a.ARW", b"a");
    sh(
        "xattr",
        &["-w", "com.example.note", "client", src.to_str().unwrap()],
    );
    sh("touch", &["-t", "201906151200", src.to_str().unwrap()]);
    let e = engine(
        j.path(),
        platform(&TestTrash::new(t.path()), None),
        &Events::default(),
    );
    let id = run(&e, job(&src, d.path(), |_| {}));
    assert_eq!(finish(&e, id), JobState::Finished);
    let (a, b) = (
        std::fs::metadata(&src).unwrap().modified().unwrap(),
        std::fs::metadata(d.path().join("2019 Wedding"))
            .unwrap()
            .modified()
            .unwrap(),
    );
    assert_eq!(a, b, "the folder's date is the original's, not today's");
    let x = std::process::Command::new("xattr")
        .arg(d.path().join("2019 Wedding"))
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&x.stdout).contains("com.example.note"));
    assert!(e.report(id).unwrap().metadata_lost.is_empty());
}

#[test]
fn one_worker_and_many_give_the_same_result() {
    let (s, j, t) = (tempdir(), tempdir(), tempdir());
    let src = s.path().join("mix");
    for i in 0..120 {
        put(
            &src,
            &format!("small/{i}.xmp"),
            &bytes(500 + i * 7, i as u64),
        );
    }
    for i in 0..5 {
        put(&src, &format!("big/{i}.mov"), &bytes(9 << 20, 100 + i));
    }
    for workers in [Some(1), None, Some(8)] {
        let d = tempdir();
        let e = engine(
            j.path(),
            platform(&TestTrash::new(t.path()), None),
            &Events::default(),
        );
        let id = run(&e, job(&src, d.path(), |o| o.workers = workers));
        assert_eq!(finish(&e, id), JobState::Finished, "{workers:?}");
        let diffs =
            safe_transfer::compare::compare(&Platform::native(), &src, &d.path().join("mix"))
                .unwrap();
        assert!(diffs.is_empty(), "{workers:?}: {diffs:?}");
    }
}

#[test]
fn free_space_counts_purgeable_space_like_finder() {
    let v = Platform::native()
        .volumes
        .volume_of(Path::new("/"))
        .unwrap();
    let statfs_free = {
        let out = std::process::Command::new("df")
            .args(["-k", "/"])
            .output()
            .unwrap();
        let line = String::from_utf8_lossy(&out.stdout)
            .lines()
            .nth(1)
            .unwrap()
            .to_string();
        line.split_whitespace()
            .nth(3)
            .unwrap()
            .parse::<u64>()
            .unwrap()
            * 1024
    };
    // At least what statfs says (purgeable space can only add to it).
    assert!(
        v.free + (1 << 30) >= statfs_free,
        "{} < {statfs_free}",
        v.free
    );
}

//! Multi-file jobs through the engine, on temp folders of the internal disk
//! (disk-image tests are in `faults.rs`, `#[ignore]`d).
// Uses the macOS copy path (F-024: macOS-only tests are gated).
#![cfg(target_os = "macos")]

mod common;

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use safe_transfer::{
    Cancel, Decision, Engine, EngineConfig, EngineError, Event, Flush, FlushLevel, Item, JobSpec,
    JobState, Options, Platform, VolumeIdentity, VolumeInfo,
};

/// Collects events.
#[derive(Default, Clone)]
struct Events(Arc<Mutex<Vec<Event>>>);

impl Events {
    fn sink(&self) -> Arc<dyn safe_transfer::EventSink> {
        let me = self.0.clone();
        Arc::new(move |e: Event| me.lock().unwrap().push(e))
    }
    fn count(&self, f: impl Fn(&Event) -> bool) -> usize {
        self.0.lock().unwrap().iter().filter(|e| f(e)).count()
    }
}

fn engine(journal: &Path, p: Platform, ev: &Events) -> Engine {
    let mut cfg = EngineConfig::new(journal);
    cfg.journal_interval = Duration::ZERO;
    Engine::open(cfg, p, ev.sink()).unwrap()
}

/// A shoot: nested folders, sidecars, raws, a big file, an empty file, a link.
fn shoot(root: &Path) -> PathBuf {
    let top = root.join("Smith wedding");
    for (d, n) in [("100MSDCF", 40), ("101MSDCF", 25)] {
        let dir = top.join("DCIM").join(d);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..n {
            std::fs::write(
                dir.join(format!("DSC{i:05}.XMP")),
                bytes(500 + i * 91, i as u64),
            )
            .unwrap();
        }
        std::fs::write(dir.join("DSC09000.ARW"), bytes(3 << 20, 7)).unwrap();
    }
    std::fs::write(top.join("video.MP4"), bytes(9 << 20, 8)).unwrap();
    std::fs::write(top.join("empty"), b"").unwrap();
    std::fs::write(top.join("café ☕.txt"), b"unicode").unwrap();
    std::os::unix::fs::symlink("DCIM/100MSDCF", top.join("latest")).unwrap();
    top
}

fn spec(src: &Path, dest: &Path) -> JobSpec {
    let mut s = JobSpec::new(vec![Item::Path(src.to_path_buf())], dest);
    s.options = Options {
        chunk_size: 1 << 20,
        ..no_clone()
    };
    s
}

fn all_partials(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && !p.is_symlink() {
                out.extend(all_partials(&p));
            } else if p.to_string_lossy().ends_with(".st-partial") {
                out.push(p);
            }
        }
    }
    out
}

fn same_tree(a: &Path, b: &Path) {
    let diffs = safe_transfer::compare::compare(&Platform::native(), a, b).unwrap();
    assert!(diffs.is_empty(), "{diffs:?}");
}

#[test]
fn a_folder_job_copies_everything_and_reports_it() {
    let (s, d, j) = (tempdir(), tempdir(), tempdir());
    let top = shoot(s.path());
    let ev = Events::default();
    let e = engine(j.path(), Platform::native(), &ev);
    let id = e.create(spec(&top, d.path())).unwrap();
    assert_eq!(e.job(id).unwrap().state, JobState::Planned);
    e.start(id).unwrap();
    let info = e.wait(id, Duration::from_secs(120)).unwrap();
    assert_eq!(info.state, JobState::Finished);
    same_tree(&top, &d.path().join("Smith wedding"));
    assert!(all_partials(d.path()).is_empty());
    let r = e.report(id).unwrap();
    assert_eq!(r.copied_verified, 40 + 25 + 2 + 3, "{r:?}");
    assert_eq!(r.links, 1);
    assert_eq!(r.folders, 4);
    assert!(r.failed.is_empty() && r.skipped.is_empty(), "{r:?}");
    assert_eq!(ev.count(|e| matches!(e, Event::FileDone { .. })), 71);
    assert_eq!(ev.count(|e| matches!(e, Event::JobFinished { .. })), 1);
    assert!(
        j.path()
            .join("jobs")
            .join(id.as_str())
            .join("report.json")
            .exists()
    );
    // Progress ends at 100 %.
    let last =
        ev.0.lock()
            .unwrap()
            .iter()
            .rev()
            .find_map(|e| match e {
                Event::Progress(p) => Some(p.clone()),
                _ => None,
            })
            .unwrap();
    assert_eq!(last.done, last.total);
}

/// Counts drive flushes of files (the group barriers, plus journal writes).
struct CountingFlush(AtomicU64);
impl Flush for CountingFlush {
    fn flush_file(&self, f: &File) -> io::Result<FlushLevel> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Platform::native().flush.flush_file(f)
    }
    fn flush_dir(&self, dir: &Path) -> io::Result<FlushLevel> {
        Platform::native().flush.flush_dir(dir)
    }
    fn os_flush(&self, f: &File) -> io::Result<()> {
        Platform::native().flush.os_flush(f)
    }
}

#[test]
fn small_files_share_one_drive_flush_per_group() {
    let (s, d, j) = (tempdir(), tempdir(), tempdir());
    let src = s.path().join("sidecars");
    std::fs::create_dir(&src).unwrap();
    for i in 0..600 {
        std::fs::write(
            src.join(format!("f{i:04}.xmp")),
            bytes(1024 + i * 13, i as u64),
        )
        .unwrap();
    }
    let counter = Arc::new(CountingFlush(AtomicU64::new(0)));
    let mut p = Platform::native();
    p.flush = counter.clone();
    let ev = Events::default();
    let mut cfg = EngineConfig::new(j.path());
    cfg.journal_interval = Duration::from_secs(3600); // only forced journal writes
    let e = Engine::open(cfg, p, ev.sink()).unwrap();
    let id = e.create(spec(&src, d.path())).unwrap();
    e.start(id).unwrap();
    assert_eq!(
        e.wait(id, Duration::from_secs(120)).unwrap().state,
        JobState::Finished
    );
    same_tree(&src, &d.path().join("sidecars"));
    let flushes = counter.0.load(Ordering::Relaxed);
    // 600 files in groups of at most 256: 3 group barriers, plus a handful of
    // journal writes. One flush per file would be at least 600.
    assert!(flushes < 40, "{flushes} drive flushes for 600 small files");
    let r = e.report(id).unwrap();
    assert_eq!(r.copied_verified, 600);
    assert_eq!(r.os_flush_only, 0);
}

#[test]
fn existing_files_and_same_name_files_are_skipped_never_replaced() {
    let (s, d, j) = (tempdir(), tempdir(), tempdir());
    let src = s.path().join("job");
    std::fs::create_dir_all(src.join("sub")).unwrap();
    std::fs::write(src.join("keep.txt"), b"new").unwrap();
    std::fs::write(src.join("fresh.txt"), b"fresh").unwrap();
    std::fs::create_dir_all(d.path().join("job")).unwrap();
    std::fs::write(d.path().join("job/keep.txt"), b"old").unwrap();
    // Two items whose names differ only by case, onto a case-insensitive disk.
    let other = s.path().join("other");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("FRESH.TXT"), b"shouting").unwrap();
    let ev = Events::default();
    let e = engine(j.path(), Platform::native(), &ev);
    let mut sp = spec(&src, d.path());
    sp.options.on_conflict.files = Decision::Skip;
    sp.items.push(Item::Exact {
        source: other.join("FRESH.TXT"),
        dest_rel: "job/FRESH.TXT".into(),
    });
    let plan = e.plan(&sp).unwrap();
    assert_eq!(plan.skipped.len(), 1, "{:?}", plan.skipped);
    assert_eq!(plan.conflicts, vec![PathBuf::from("job/keep.txt")]);
    let id = e.create(sp).unwrap();
    e.start(id).unwrap();
    e.wait(id, Duration::from_secs(60)).unwrap();
    assert_eq!(
        std::fs::read(d.path().join("job/keep.txt")).unwrap(),
        b"old"
    );
    assert_eq!(
        std::fs::read(d.path().join("job/fresh.txt")).unwrap(),
        b"fresh"
    );
    let r = e.report(id).unwrap();
    assert_eq!(r.skipped.len(), 2, "{r:?}");
    assert!(
        r.skipped
            .iter()
            .any(|n| n.reason.contains("already exists"))
    );
    assert!(r.skipped.iter().any(|n| n.reason.contains("same name")));
}

#[test]
fn exact_items_land_where_asked_with_their_folders() {
    let (s, d, j) = (tempdir(), tempdir(), tempdir());
    let a = s.path().join("DSC01234.ARW");
    std::fs::write(&a, bytes(70_000, 1)).unwrap();
    let ev = Events::default();
    let e = engine(j.path(), Platform::native(), &ev);
    let mut sp = JobSpec::new(
        vec![Item::Exact {
            source: a.clone(),
            dest_rel: "2026/2026-11-13 Smith/Smith_0001.ARW".into(),
        }],
        d.path(),
    );
    sp.options = no_clone();
    let id = e.create(sp).unwrap();
    e.start(id).unwrap();
    assert_eq!(
        e.wait(id, Duration::from_secs(60)).unwrap().state,
        JobState::Finished
    );
    assert_eq!(
        std::fs::read(d.path().join("2026/2026-11-13 Smith/Smith_0001.ARW")).unwrap(),
        bytes(70_000, 1)
    );
}

/// A destination that says it is nearly full.
struct Tiny;
impl VolumeIdentity for Tiny {
    fn volume_of(&self, p: &Path) -> io::Result<VolumeInfo> {
        let mut v = Platform::native().volumes.volume_of(p)?;
        v.free = 50_000_000;
        Ok(v)
    }
    fn mounted(&self) -> io::Result<Vec<VolumeInfo>> {
        Platform::native().volumes.mounted()
    }
    fn case_sensitive(&self, dir: &Path) -> io::Result<bool> {
        Platform::native().volumes.case_sensitive(dir)
    }
}

#[test]
fn a_job_that_wont_fit_is_refused_up_front() {
    let (s, d, j) = (tempdir(), tempdir(), tempdir());
    let src = s.path().join("big");
    std::fs::create_dir(&src).unwrap();
    let f = File::create(src.join("huge.bin")).unwrap();
    f.set_len(2_000_000_000).unwrap(); // sparse: costs nothing here
    let mut p = Platform::native();
    p.volumes = Arc::new(Tiny);
    let e = engine(j.path(), p, &Events::default());
    let err = e.create(spec(&src, d.path())).unwrap_err();
    assert!(matches!(err, EngineError::NoSpace { .. }), "{err}");
    assert!(err.to_string().contains("free"), "{err}");
    assert!(e.jobs().is_empty());
    assert!(std::fs::read_dir(d.path()).unwrap().next().is_none());
}

#[test]
fn a_folder_is_never_copied_into_itself() {
    let (s, j) = (tempdir(), tempdir());
    let inner = s.path().join("inside");
    std::fs::create_dir(&inner).unwrap();
    let e = engine(j.path(), Platform::native(), &Events::default());
    let err = e.create(spec(s.path(), &inner)).unwrap_err();
    assert!(matches!(err, EngineError::IntoItself), "{err}");
}

#[test]
fn cancel_leaves_the_destination_clean_and_the_job_resumable() {
    let (s, d, j) = (tempdir(), tempdir(), tempdir());
    let src = s.path().join("cards");
    std::fs::create_dir(&src).unwrap();
    for i in 0..6 {
        std::fs::write(src.join(format!("clip{i}.MP4")), bytes(12 << 20, i)).unwrap();
    }
    let e = engine(j.path(), Platform::native(), &Events::default());
    let id = e.create(spec(&src, d.path())).unwrap();
    e.start(id).unwrap();
    std::thread::sleep(Duration::from_millis(60));
    let _ = e.cancel(id, Cancel::KeepResumable);
    let info = e.job(id).unwrap();
    if info.state == JobState::Cancelled {
        assert!(
            all_partials(d.path()).is_empty(),
            "{:?}",
            all_partials(d.path())
        );
        // What is there under a real name is complete.
        for f in std::fs::read_dir(d.path().join("cards")).unwrap().flatten() {
            let name = f.file_name();
            assert_eq!(
                std::fs::read(f.path()).unwrap(),
                std::fs::read(src.join(&name)).unwrap()
            );
        }
        e.resume(id).unwrap();
    }
    assert_eq!(
        e.wait(id, Duration::from_secs(120)).unwrap().state,
        JobState::Finished
    );
    same_tree(&src, &d.path().join("cards"));
    // A finished job can't be cancelled or started again.
    assert!(e.cancel(id, Cancel::Close).is_err());
    assert!(e.start(id).is_err());
}

#[test]
fn pause_holds_the_job_and_resume_continues_it() {
    let (s, d, j) = (tempdir(), tempdir(), tempdir());
    let src = s.path().join("clips");
    std::fs::create_dir(&src).unwrap();
    for i in 0..4 {
        std::fs::write(src.join(format!("c{i}.MOV")), bytes(10 << 20, i)).unwrap();
    }
    let e = engine(j.path(), Platform::native(), &Events::default());
    let id = e.create(spec(&src, d.path())).unwrap();
    e.start(id).unwrap();
    e.pause(id).unwrap();
    assert_eq!(e.job(id).unwrap().state, JobState::Paused);
    let before = e.job(id).unwrap().files_done;
    std::thread::sleep(Duration::from_millis(300));
    assert!(e.job(id).unwrap().files_done <= before + 1);
    e.resume(id).unwrap();
    assert_eq!(
        e.wait(id, Duration::from_secs(120)).unwrap().state,
        JobState::Finished
    );
    same_tree(&src, &d.path().join("clips"));
}

#[test]
fn jobs_survive_reopening_the_engine() {
    let (s, d, j) = (tempdir(), tempdir(), tempdir());
    let src = s.path().join("a");
    std::fs::create_dir(&src).unwrap();
    std::fs::write(src.join("x.ARW"), bytes(2 << 20, 1)).unwrap();
    let id = {
        let e = engine(j.path(), Platform::native(), &Events::default());
        e.create(spec(&src, d.path())).unwrap()
    };
    let ev = Events::default();
    let e = engine(j.path(), Platform::native(), &ev);
    assert_eq!(e.jobs().len(), 1);
    assert_eq!(e.job(id).unwrap().state, JobState::Planned);
    e.start(id).unwrap();
    assert_eq!(
        e.wait(id, Duration::from_secs(60)).unwrap().state,
        JobState::Finished
    );
    // Reopened again, the finished job is still listed with its report.
    let e = engine(j.path(), Platform::native(), &Events::default());
    assert_eq!(e.job(id).unwrap().state, JobState::Finished);
    assert_eq!(e.report(id).unwrap().copied_verified, 1);
}

#[test]
fn a_damaged_journal_is_left_alone_and_listed() {
    let j = tempdir();
    let dir = j.path().join("jobs").join("k3f9q2xm");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("manifest.json"), b"{\"torn").unwrap();
    let e = engine(j.path(), Platform::native(), &Events::default());
    assert!(e.jobs().is_empty());
    assert_eq!(e.damaged(), vec![dir.clone()]);
    assert!(dir.join("manifest.json").exists());
}

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// Hides one volume, as if that card were out of its reader.
struct Without(Option<String>);
impl VolumeIdentity for Without {
    fn volume_of(&self, p: &Path) -> io::Result<VolumeInfo> {
        Platform::native().volumes.volume_of(p)
    }
    fn mounted(&self) -> io::Result<Vec<VolumeInfo>> {
        Ok(Platform::native()
            .volumes
            .mounted()?
            .into_iter()
            .filter(|v| v.uuid != self.0)
            .collect())
    }
    fn case_sensitive(&self, dir: &Path) -> io::Result<bool> {
        Platform::native().volumes.case_sensitive(dir)
    }
}

#[test]
fn a_stopped_job_stays_listed_and_can_be_resumed_by_hand() {
    use safe_transfer::Resumability;
    let (s, d, j) = (tempdir(), tempdir(), tempdir());
    let src = s.path().join("job");
    std::fs::create_dir(&src).unwrap();
    for i in 0..5 {
        std::fs::write(src.join(format!("{i}.ARW")), bytes(200_000, i)).unwrap();
    }
    let dest_uuid = Platform::native().volumes.volume_of(d.path()).unwrap().uuid;

    // The destination "out of its reader": listed, says what it needs, and
    // Discard is refused until the disk is back.
    let id = {
        let mut p = Platform::native();
        p.volumes = Arc::new(Without(dest_uuid.clone()));
        let e = engine(j.path(), p, &Events::default());
        let id = e.create(spec(&src, d.path())).unwrap();
        assert_eq!(e.job(id).unwrap().resume, Resumability::NotNeeded);
        e.cancel(id, Cancel::KeepResumable).unwrap();
        let info = e.job(id).unwrap();
        assert_eq!(info.state, JobState::Cancelled);
        assert!(
            matches!(&info.resume, Resumability::NeedsVolumes(v) if v.len() == 1),
            "{:?}",
            info.resume
        );
        assert_eq!(e.resumable().len(), 1);
        assert!(e.resume_all().is_empty(), "nothing is ready");
        let err = e.cancel(id, Cancel::Close).unwrap_err();
        assert!(matches!(err, EngineError::NeedsVolume(_)), "{err}");
        id
    };

    // The disk is back: ready, found from the sidebar by its UUID, resumed.
    let e = engine(j.path(), Platform::native(), &Events::default());
    let info = e.job(id).unwrap();
    assert!(
        matches!(info.resume, Resumability::Ready { .. }),
        "{:?}",
        info.resume
    );
    assert_eq!(e.jobs_on_volume(dest_uuid.as_deref().unwrap()).len(), 1);
    assert_eq!(e.resume_all(), vec![id]);
    assert_eq!(
        e.wait(id, Duration::from_secs(60)).unwrap().state,
        JobState::Finished
    );
    assert!(e.resumable().is_empty());
    assert_eq!(e.report(id).unwrap().copied_verified, 5);
    same_tree(&src, &d.path().join("job"));
}

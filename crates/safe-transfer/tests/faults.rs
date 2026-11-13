//! Failures at each named point of the pipeline, and pulling a disk-image
//! "card" out at those points. Needs `--features fault-injection`; the
//! disk-image tests also need `hdiutil` and run with `--ignored`.
#![cfg(all(feature = "fault-injection", target_os = "macos"))]

mod common;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use common::*;
use safe_transfer::fault::{FaultHook, FaultPoint, SpecHook};
use safe_transfer::{Control, CopyError, CopyRequest, NoProgress, Options, Outcome, Platform};

fn with_hook(h: Arc<dyn FaultHook>) -> Options {
    Options {
        faults: Some(h),
        chunk_size: 1 << 20,
        ..no_clone()
    }
}

fn run(
    src: &Path,
    dir: &Path,
    opts: &Options,
) -> Result<safe_transfer::FileReport, safe_transfer::Failure> {
    let req = CopyRequest::into_dir(src, dir).unwrap();
    safe_transfer::copy_file(
        &Platform::native(),
        &req,
        opts,
        &Control::new(),
        &NoProgress,
    )
}

#[test]
fn a_failure_before_the_rename_never_leaves_the_real_name() {
    let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let src = s.path().join("DSC0001.ARW");
    std::fs::write(&src, bytes(5 << 20, 1)).unwrap();
    for spec in [
        "fail@before_copy",
        "fail@mid_chunk:2097152",
        "fail@before_flush",
        "fail@after_flush",
        "fail@mid_verify:1048576",
        "fail@before_rename",
    ] {
        let opts = with_hook(Arc::new(SpecHook::parse(spec).unwrap()));
        let e = run(&src, d.path(), &opts).unwrap_err();
        assert!(e.partial_left.is_none(), "{spec}");
        assert!(names(d.path()).is_empty(), "{spec}: {:?}", names(d.path()));
    }
    // The original is untouched by all of that.
    assert_eq!(std::fs::read(&src).unwrap(), bytes(5 << 20, 1));
}

#[test]
fn a_failure_after_the_rename_leaves_a_complete_file() {
    let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let src = s.path().join("a.bin");
    std::fs::write(&src, bytes(3 << 20, 2)).unwrap();
    let opts = with_hook(Arc::new(SpecHook::parse("fail@after_rename").unwrap()));
    assert!(run(&src, d.path(), &opts).is_err());
    assert_eq!(
        std::fs::read(d.path().join("a.bin")).unwrap(),
        bytes(3 << 20, 2)
    );
    assert!(no_partials(d.path()));
}

/// Flips a byte of the partial after its flush, `times` times.
#[derive(Debug)]
struct Corrupt {
    partial: PathBuf,
    times: AtomicU32,
}
impl FaultHook for Corrupt {
    fn at(&self, point: FaultPoint, _src: &Path) -> io::Result<()> {
        if point == (FaultPoint::AfterFlush { chunk: None })
            && self
                .times
                .try_update(Ordering::AcqRel, Ordering::Acquire, |t| t.checked_sub(1))
                .is_ok()
        {
            use std::os::unix::fs::FileExt;
            let f = std::fs::OpenOptions::new()
                .write(true)
                .open(&self.partial)?;
            f.write_all_at(&[0xFF, 0x00, 0xFF], 1000)?;
            f.sync_all()?;
        }
        Ok(())
    }
}

#[test]
fn a_bad_copy_is_caught_redone_once_then_refused() {
    let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let src = s.path().join("photo.ARW");
    std::fs::write(&src, bytes(2 << 20, 3)).unwrap();
    for (times, ok) in [(1, true), (2, false)] {
        let req = CopyRequest::into_dir(&src, d.path()).unwrap();
        let hook = Arc::new(Corrupt {
            partial: req.partial_path(),
            times: AtomicU32::new(times),
        });
        let opts = with_hook(hook);
        let r = safe_transfer::copy_file(
            &Platform::native(),
            &req,
            &opts,
            &Control::new(),
            &NoProgress,
        );
        if ok {
            let r = r.unwrap();
            assert_eq!(r.retries, 1);
            assert_eq!(r.outcome, Outcome::Verified);
            assert_eq!(std::fs::read(&r.dest).unwrap(), bytes(2 << 20, 3));
            std::fs::remove_file(&r.dest).unwrap();
        } else {
            let e = r.unwrap_err();
            assert!(matches!(e.error, CopyError::VerifyFailed), "{e}");
            assert!(names(d.path()).is_empty());
        }
    }
}

/// Before the second read, changes one byte of the original but keeps its
/// size and date: what a flaky card reader would look like.
#[derive(Debug)]
struct FlakySource;
impl FaultHook for FlakySource {
    fn at(&self, point: FaultPoint, src: &Path) -> io::Result<()> {
        if point == FaultPoint::BeforeReread {
            use std::os::unix::fs::FileExt;
            let mtime = std::fs::metadata(src)?.modified()?;
            let f = std::fs::OpenOptions::new().write(true).open(src)?;
            f.write_all_at(&[0x42], 5)?;
            f.set_modified(mtime)?;
            f.sync_all()?;
        }
        Ok(())
    }
}

#[test]
fn reread_mode_catches_an_original_that_reads_differently() {
    let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let src = s.path().join("card.ARW");
    std::fs::write(&src, bytes(2 << 20, 4)).unwrap();
    let opts = Options {
        reread_source: true,
        ..with_hook(Arc::new(FlakySource))
    };
    let e = run(&src, d.path(), &opts).unwrap_err();
    assert!(matches!(e.error, CopyError::SourceUnstable), "{e}");
    assert!(names(d.path()).is_empty());
}

#[test]
fn the_harness_stops_a_tool_that_hangs() {
    let t = std::time::Instant::now();
    let e = run_limited("sleep", &["30"], None, Duration::from_secs(1)).unwrap_err();
    assert!(e.contains("didn't finish in 1 s"), "{e}");
    assert!(t.elapsed() < Duration::from_secs(10));
}

#[test]
fn the_harness_never_picks_a_helper_for_an_image_that_isnt_attached() {
    let d = tempfile::tempdir().unwrap();
    let img = d.path().join("vol.dmg");
    std::fs::write(&img, b"not an image").unwrap();
    assert_eq!(attachments(&img).unwrap(), vec![]);
    let e = helper_pid(&img).unwrap_err();
    assert!(e.contains("isn't attached"), "{e}");
    // A failed yank is reported, not silently ignored.
    let e = try_yank(&img, &d.path().join("mnt")).unwrap_err();
    assert!(e.contains("isn't attached"), "{e}");
}

/// Pulls the image out the first time `point` is reached.
#[derive(Debug)]
struct Yank {
    point: &'static str,
    offset: u64,
    img: PathBuf,
    mount: PathBuf,
    done: std::sync::atomic::AtomicBool,
}
impl FaultHook for Yank {
    fn at(&self, point: FaultPoint, _src: &Path) -> io::Result<()> {
        if point.name() == self.point
            && point.offset() >= self.offset
            && !self.done.swap(true, Ordering::AcqRel)
        {
            yank(&self.img, &self.mount);
        }
        Ok(())
    }
}

fn yank_suite(fs: &str) {
    let s = tempfile::tempdir().unwrap();
    let src = s.path().join("DSC09999.ARW");
    let data = bytes(24 << 20, 5);
    std::fs::write(&src, &data).unwrap();
    let img = Image::new(fs, 128);
    for (point, offset) in [
        ("mid_chunk", 9 << 20),
        ("before_flush", 0),
        ("mid_verify", 12 << 20),
        ("before_rename", 0),
        ("after_rename", 0),
    ] {
        let dir = img.mount.join(point);
        std::fs::create_dir(&dir).unwrap();
        let hook = Arc::new(Yank {
            point,
            offset,
            img: img.img.clone(),
            mount: img.mount.clone(),
            done: Default::default(),
        });
        let r = run(&src, &dir, &with_hook(hook));
        assert!(
            r.is_err(),
            "{fs} {point}: the copy should notice the card is gone"
        );
        img.put_back();
        // The invariant: the real name holds the complete file, or nothing.
        let dest = dir.join("DSC09999.ARW");
        if let Ok(got) = std::fs::read(&dest) {
            assert_eq!(
                got.len(),
                data.len(),
                "{fs} {point}: incomplete file under the real name"
            );
            assert!(got == data, "{fs} {point}: wrong bytes under the real name");
        }
        // Only before the rename may a hidden partial be left; it is reported.
        if let Err(e) = &r
            && let Some(p) = &e.partial_left
        {
            assert!(safe_transfer::names::is_partial_name(p), "{p:?}");
        }
    }
    assert_eq!(
        std::fs::read(&src).unwrap(),
        data,
        "the original is untouched"
    );
}

#[test]
#[ignore = "needs hdiutil; run with --ignored"]
fn pulling_an_exfat_card_never_leaves_an_incomplete_real_file() {
    yank_suite("ExFAT");
}

#[test]
#[ignore = "needs hdiutil; run with --ignored"]
fn pulling_an_apfs_drive_never_leaves_an_incomplete_real_file() {
    yank_suite("APFS");
}

#[test]
#[ignore = "needs hdiutil; run with --ignored"]
fn an_exfat_card_gets_a_flushed_verified_copy_and_honest_metadata() {
    let s = tempfile::tempdir().unwrap();
    let src = s.path().join("DSC00001.ARW");
    std::fs::write(&src, bytes(3 << 20, 6)).unwrap();
    sh(
        "xattr",
        &["-w", "com.example.note", "keep", src.to_str().unwrap()],
    );
    let img = Image::new("ExFAT", 64);
    let r = run(&src, &img.mount, &no_clone()).unwrap();
    assert_eq!(r.outcome, Outcome::Verified);
    assert_eq!(r.rename, safe_transfer::RenameMode::CheckThenRename);
    assert_eq!(r.flush_level, safe_transfer::FlushLevel::Device);
    assert_eq!(std::fs::read(&r.dest).unwrap(), bytes(3 << 20, 6));
    // exFAT keeps xattrs in a ._ file (so none are lost) but not permissions.
    assert!(
        !r.metadata_lost
            .iter()
            .any(|m| matches!(m, safe_transfer::MetaLoss::Xattrs(_))),
        "{:?}",
        r.metadata_lost
    );
    let v = Platform::native().volumes.volume_of(&img.mount).unwrap();
    assert_eq!(v.fs_type, "exfat");
    assert!(v.uuid.is_some());
    assert!(no_partials(&img.mount));
    // AppleDouble placement (proposal §9, decided in M3): the xattrs are
    // written on the partial and travel with it in the rename, so the card
    // ends up with the final name's `._` companion and never an orphaned
    // `._.<name>…st-partial` (resume and cancel remove those with the partial).
    assert!(
        names(&img.mount).contains(&"._DSC00001.ARW".to_string()),
        "{:?}",
        names(&img.mount)
    );
    assert!(
        !names(&img.mount).iter().any(|n| n.starts_with("._.")),
        "{:?}",
        names(&img.mount)
    );
}

/// The negative control: the yank harness must lose writes that were never
/// flushed, or the yank tests above would prove nothing.
#[test]
#[ignore = "needs hdiutil; run with --ignored"]
fn the_yank_harness_loses_unflushed_writes() {
    let img = Image::new("APFS", 64);
    let dir = img.mount.join("unflushed");
    std::fs::create_dir(&dir).unwrap();
    for i in 0..200 {
        std::fs::write(dir.join(format!("f{i}")), bytes(10_000, i)).unwrap();
    }
    img.pull_out();
    img.put_back();
    let intact = (0..200)
        .filter(|i| std::fs::read(dir.join(format!("f{i}"))).is_ok_and(|b| b == bytes(10_000, *i)))
        .count();
    assert!(
        intact < 200,
        "nothing was lost: this harness can't detect missing flushes"
    );
}

// ───────────── M2: jobs through the engine, yanked at every point ─────────────

use safe_transfer::{Engine, EngineConfig, Event, Item, JobSpec, JobState};
use std::sync::Mutex;
use std::time::Duration;

/// Yanks an image the `nth` time `point` is reached at or past `offset`.
#[derive(Debug)]
struct YankAt {
    point: &'static str,
    offset: u64,
    nth: u32,
    hits: AtomicU32,
    img: PathBuf,
    mount: Mutex<PathBuf>,
}

impl YankAt {
    fn new(point: &'static str, offset: u64, nth: u32, img: &Image) -> Arc<YankAt> {
        Arc::new(YankAt {
            point,
            offset,
            nth,
            hits: AtomicU32::new(0),
            img: img.img.clone(),
            mount: Mutex::new(img.mount.clone()),
        })
    }
    fn fired(&self) -> bool {
        self.hits.load(Ordering::Acquire) >= self.nth
    }
}

impl FaultHook for YankAt {
    fn at(&self, point: FaultPoint, _p: &Path) -> io::Result<()> {
        if point.name() == self.point
            && point.offset() >= self.offset
            && self.hits.fetch_add(1, Ordering::AcqRel) + 1 == self.nth
        {
            yank(&self.img, &self.mount.lock().unwrap());
        }
        Ok(())
    }
}

#[derive(Default, Clone)]
struct Events(Arc<Mutex<Vec<Event>>>);
impl Events {
    fn sink(&self) -> Arc<dyn safe_transfer::EventSink> {
        let me = self.0.clone();
        Arc::new(move |e: Event| me.lock().unwrap().push(e))
    }
    fn has(&self, f: impl Fn(&Event) -> bool) -> bool {
        self.0.lock().unwrap().iter().any(f)
    }
}

/// 40 sidecars (grouped), two 3 MiB raws and a 9 MiB clip (per chunk).
fn shoot(root: &Path) -> PathBuf {
    let top = root.join("shoot");
    let d = top.join("100MSDCF");
    std::fs::create_dir_all(&d).unwrap();
    for i in 0..40 {
        std::fs::write(
            d.join(format!("DSC{i:05}.XMP")),
            bytes(700 + i * 113, i as u64),
        )
        .unwrap();
    }
    std::fs::write(d.join("DSC09001.ARW"), bytes(3 << 20, 41)).unwrap();
    std::fs::write(d.join("DSC09002.ARW"), bytes(3 << 20, 42)).unwrap();
    std::fs::write(top.join("clip.MP4"), bytes(9 << 20, 43)).unwrap();
    top
}

fn job_engine(journal: &Path, hook: Arc<dyn FaultHook>, ev: &Events) -> Engine {
    let mut cfg = EngineConfig::new(journal);
    cfg.journal_interval = Duration::ZERO;
    cfg.faults = Some(hook);
    Engine::open(cfg, Platform::native(), ev.sink()).unwrap()
}

fn job_spec(src: &Path, dest: &Path) -> JobSpec {
    let mut s = JobSpec::new(vec![Item::Path(src.to_path_buf())], dest);
    s.options = Options {
        chunk_size: 1 << 20,
        ..no_clone()
    };
    s
}

/// Every file under `dest` with a real name equals its original under `src`.
fn real_names_are_complete(src: &Path, dest: &Path, what: &str) {
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(dest, &mut files);
    for f in files {
        let rel = f.strip_prefix(dest).unwrap();
        let name = rel.file_name().unwrap().to_string_lossy();
        if name.ends_with(".st-partial") || name.starts_with("._") {
            continue;
        }
        let got = std::fs::read(&f).unwrap();
        let want = std::fs::read(src.join(rel)).unwrap();
        assert!(
            got == want,
            "{what}: {rel:?} has its real name but isn't complete ({} of {} bytes)",
            got.len(),
            want.len()
        );
    }
}

fn no_partials_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(no_partials_under(&p));
        } else if p.to_string_lossy().contains(".st-partial") {
            out.push(p);
        }
    }
    out
}

fn job_yank_suite(fs: &str) {
    let s = tempfile::tempdir().unwrap();
    let src = shoot(s.path());
    let img = Image::new(fs, 256);
    for (point, offset, nth) in [
        ("mid_chunk", 5 << 20, 1),
        ("after_flush", 0, 3),
        ("mid_verify", 2 << 20, 1),
        ("before_rename", 0, 43),
        ("after_rename", 0, 43),
        ("before_group_barrier", 0, 1),
        ("after_group_barrier", 0, 1),
        ("mid_group_rename", 5, 1),
        ("before_folder_barrier", 0, 1),
    ] {
        let what = format!("{fs} {point}#{nth}");
        let dest = img.mount.join(point);
        std::fs::create_dir(&dest).unwrap();
        let j = tempfile::tempdir().unwrap();
        let hook = YankAt::new(point, offset, nth, &img);
        let ev = Events::default();
        let e = job_engine(j.path(), hook.clone(), &ev);
        let id = e.create(job_spec(&src, &dest)).unwrap();
        e.start(id).unwrap();
        let info = e.wait(id, Duration::from_secs(300)).unwrap();
        assert!(hook.fired(), "{what}: the point was never reached");
        assert_eq!(info.state, JobState::WaitingForDestination, "{what}");
        img.put_back();
        real_names_are_complete(&src, &dest.join("shoot"), &what);
        e.volumes_changed();
        assert!(
            ev.has(|e| matches!(e, Event::ResumeAvailable { .. })),
            "{what}: no resume offer"
        );
        e.resume(id).unwrap();
        let info = e.wait(id, Duration::from_secs(300)).unwrap();
        assert_eq!(info.state, JobState::Finished, "{what}");
        let r = e.report(id).unwrap();
        assert!(r.failed.is_empty() && r.skipped.is_empty(), "{what}: {r:?}");
        let diffs = safe_transfer::compare::compare(&Platform::native(), &src, &dest.join("shoot"))
            .unwrap();
        assert!(diffs.is_empty(), "{what}: {diffs:?}");
        assert!(
            no_partials_under(&dest).is_empty(),
            "{what}: {:?}",
            no_partials_under(&dest)
        );
    }
}

#[test]
#[ignore = "needs hdiutil; run with --ignored"]
fn jobs_survive_an_exfat_card_pulled_at_every_point() {
    job_yank_suite("ExFAT");
}

#[test]
#[ignore = "needs hdiutil; run with --ignored"]
fn jobs_survive_an_apfs_drive_pulled_at_every_point() {
    job_yank_suite("APFS");
}

#[test]
#[ignore = "needs hdiutil; run with --ignored"]
fn a_card_back_under_another_mount_point_is_recognised_by_uuid() {
    let s = tempfile::tempdir().unwrap();
    let src = shoot(s.path());
    let img = Image::new("ExFAT", 128);
    let j = tempfile::tempdir().unwrap();
    let hook = YankAt::new("mid_group_rename", 3, 1, &img);
    let ev = Events::default();
    let e = job_engine(j.path(), hook.clone(), &ev);
    let id = e.create(job_spec(&src, &img.mount)).unwrap();
    e.start(id).unwrap();
    assert_eq!(
        e.wait(id, Duration::from_secs(300)).unwrap().state,
        JobState::WaitingForDestination
    );
    let elsewhere = img.mount.with_file_name("Untitled 1");
    img.put_back_at(&elsewhere);
    e.volumes_changed();
    e.resume(id).unwrap();
    assert_eq!(
        e.wait(id, Duration::from_secs(300)).unwrap().state,
        JobState::Finished
    );
    let diffs =
        safe_transfer::compare::compare(&Platform::native(), &src, &elsewhere.join("shoot"))
            .unwrap();
    assert!(diffs.is_empty(), "{diffs:?}");
}

#[test]
#[ignore = "needs hdiutil; run with --ignored"]
fn a_pulled_source_card_makes_files_wait_then_resume() {
    let card = Image::new("ExFAT", 128);
    let src = shoot(&card.mount);
    let (d, j) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let hook = YankAt::new("mid_chunk", 1 << 20, 1, &card);
    let ev = Events::default();
    let e = job_engine(j.path(), hook.clone(), &ev);
    let id = e.create(job_spec(&src, d.path())).unwrap();
    e.start(id).unwrap();
    let info = e.wait(id, Duration::from_secs(300)).unwrap();
    assert_eq!(info.state, JobState::WaitingForSource);
    assert_eq!(info.missing.len(), 1, "{:?}", info.missing);
    assert_eq!(info.missing[0].volume, "mnt");
    assert!(!info.missing[0].files.is_empty());
    assert!(ev.has(|e| matches!(e, Event::WaitingForSource { .. })));
    card.put_back();
    e.volumes_changed();
    assert!(ev.has(|e| matches!(e, Event::ResumeAvailable { .. })));
    e.resume(id).unwrap();
    assert_eq!(
        e.wait(id, Duration::from_secs(300)).unwrap().state,
        JobState::Finished
    );
    let diffs = safe_transfer::compare::compare(&Platform::native(), &src, &d.path().join("shoot"))
        .unwrap();
    assert!(diffs.is_empty(), "{diffs:?}");
}

#[test]
#[ignore = "needs hdiutil; run with --ignored"]
fn an_original_changed_while_interrupted_is_copied_again_and_reported() {
    let s = tempfile::tempdir().unwrap();
    let src = shoot(s.path());
    let img = Image::new("APFS", 128);
    let j = tempfile::tempdir().unwrap();
    let hook = YankAt::new("mid_chunk", 5 << 20, 1, &img);
    let ev = Events::default();
    let e = job_engine(j.path(), hook.clone(), &ev);
    let id = e.create(job_spec(&src, &img.mount)).unwrap();
    e.start(id).unwrap();
    assert_eq!(
        e.wait(id, Duration::from_secs(300)).unwrap().state,
        JobState::WaitingForDestination
    );
    img.put_back();
    // The clip is edited while the card is out.
    std::fs::write(src.join("clip.MP4"), bytes(10 << 20, 99)).unwrap();
    e.resume(id).unwrap();
    assert_eq!(
        e.wait(id, Duration::from_secs(300)).unwrap().state,
        JobState::Finished
    );
    let r = e.report(id).unwrap();
    assert_eq!(r.restarted.len(), 1, "{r:?}");
    assert!(r.restarted[0].reason.contains("changed"), "{r:?}");
    let diffs =
        safe_transfer::compare::compare(&Platform::native(), &src, &img.mount.join("shoot"))
            .unwrap();
    assert!(diffs.is_empty(), "{diffs:?}");
}

// ───────────── M3: moves and replaces, yanked ─────────────

fn m3_engine(journal: &Path, hook: Arc<dyn FaultHook>, ev: &Events, trash: &Path) -> Engine {
    let mut cfg = EngineConfig::new(journal);
    cfg.journal_interval = Duration::ZERO;
    cfg.faults = Some(hook);
    let mut p = Platform::native();
    p.trash = Arc::new(safe_transfer::DirTrash::new(trash).unwrap());
    Engine::open(cfg, p, ev.sink()).unwrap()
}

fn all_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

fn contents(dir: &Path) -> Vec<Vec<u8>> {
    all_files(dir)
        .into_iter()
        .filter(|p| !p.file_name().unwrap().to_string_lossy().starts_with("._"))
        .filter_map(|p| std::fs::read(p).ok())
        .collect()
}

#[test]
#[ignore = "needs hdiutil; run with --ignored"]
fn a_move_onto_a_card_never_loses_a_file_when_the_card_is_pulled() {
    let img = Image::new("ExFAT", 256);
    for (point, offset, nth) in [
        ("mid_chunk", 5 << 20, 1),
        ("before_source_trash", 0, 3),
        ("before_group_barrier", 0, 1),
        ("mid_group_rename", 5, 1),
        ("before_folder_barrier", 0, 1),
    ] {
        let what = format!("move {point}#{nth}");
        let (s, keep, j, t) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        let src = shoot(s.path());
        let pristine = shoot(keep.path());
        let trash = t.path().join("Trash");
        let dest = img.mount.join(point);
        std::fs::create_dir(&dest).unwrap();
        let hook = YankAt::new(point, offset, nth, &img);
        let ev = Events::default();
        let e = m3_engine(j.path(), hook.clone(), &ev, &trash);
        let mut spec = job_spec(&src, &dest);
        spec.options.mode = safe_transfer::Mode::Move;
        let id = e.create(spec).unwrap();
        e.start(id).unwrap();
        assert_eq!(
            e.wait(id, Duration::from_secs(300)).unwrap().state,
            JobState::WaitingForDestination,
            "{what}"
        );
        assert!(hook.fired(), "{what}");
        img.put_back();
        // Every original is somewhere whole: still in place, in the Trash, or
        // complete on the card. Nothing is missing from all three.
        let (here, trashed, there) = (contents(&src), contents(&trash), contents(&dest));
        real_names_are_complete(&pristine, &dest.join("shoot"), &what);
        for want in contents(&pristine) {
            assert!(
                here.contains(&want) || trashed.contains(&want) || there.contains(&want),
                "{what}: a file of {} bytes is nowhere",
                want.len()
            );
        }
        e.volumes_changed();
        e.resume(id).unwrap();
        assert_eq!(
            e.wait(id, Duration::from_secs(300)).unwrap().state,
            JobState::Finished,
            "{what}"
        );
        let diffs =
            safe_transfer::compare::compare(&Platform::native(), &pristine, &dest.join("shoot"))
                .unwrap();
        assert!(diffs.is_empty(), "{what}: {diffs:?}");
        assert!(
            all_files(&src).is_empty(),
            "{what}: originals left: {:?}",
            all_files(&src)
        );
        assert!(no_partials_under(&dest).is_empty(), "{what}");
    }
}

#[test]
#[ignore = "needs hdiutil; run with --ignored"]
fn replacing_on_a_card_never_loses_the_old_or_shows_a_half_new() {
    let img = Image::new("ExFAT", 256);
    for (point, nth) in [
        ("before_replace_trash", 3),
        ("after_rename", 4),
        ("before_folder_swap", 1),
    ] {
        let what = format!("replace {point}#{nth}");
        let (s, j) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let src = shoot(s.path());
        let dest = img.mount.join(point);
        // Older versions of everything are already on the card.
        for f in all_files(&src) {
            let rel = f.strip_prefix(s.path()).unwrap();
            let q = dest.join(rel);
            std::fs::create_dir_all(q.parent().unwrap()).unwrap();
            std::fs::write(&q, format!("old {}", rel.display())).unwrap();
        }
        let trash = img.mount.join(format!("Trash-{point}"));
        let hook = YankAt::new(point, 0, nth, &img);
        let ev = Events::default();
        let e = m3_engine(j.path(), hook.clone(), &ev, &trash);
        let mut spec = job_spec(&src, &dest);
        spec.options.on_conflict.files = safe_transfer::Decision::Replace;
        if point == "before_folder_swap" {
            spec.options.on_conflict.folders = safe_transfer::Decision::Replace;
        }
        let id = e.create(spec).unwrap();
        e.start(id).unwrap();
        assert_eq!(
            e.wait(id, Duration::from_secs(300)).unwrap().state,
            JobState::WaitingForDestination,
            "{what}"
        );
        img.put_back();
        let trashed = contents(&trash);
        for f in all_files(&src) {
            let rel = f.strip_prefix(s.path()).unwrap();
            let old = format!("old {}", rel.display()).into_bytes();
            let new = std::fs::read(&f).unwrap();
            let now = std::fs::read(dest.join(rel)).ok();
            if point != "before_folder_swap" {
                match &now {
                    None => {}
                    Some(b) => assert!(
                        *b == old || *b == new,
                        "{what}: {rel:?} is neither old nor new"
                    ),
                }
            }
            assert!(
                now.as_deref() == Some(&old[..])
                    || trashed.contains(&old)
                    || contents(&dest).contains(&old),
                "{what}: the old {rel:?} is lost"
            );
        }
        e.volumes_changed();
        e.resume(id).unwrap();
        assert_eq!(
            e.wait(id, Duration::from_secs(300)).unwrap().state,
            JobState::Finished,
            "{what}"
        );
        let diffs = safe_transfer::compare::compare(&Platform::native(), &src, &dest.join("shoot"))
            .unwrap();
        assert!(diffs.is_empty(), "{what}: {diffs:?}");
        assert!(
            no_partials_under(&dest).is_empty(),
            "{what}: {:?}",
            no_partials_under(&dest)
        );
    }
}

/// Three big files in flight on three workers when the card goes, as in the
/// coordinator's demo (two raws marked "the disk is no longer connected"
/// instead of kept for resume): `a` pulls the card mid-chunk, `b` is held
/// mid-verify and `c` mid-chunk until it is gone.
///
/// A card mounted under /Volumes stays listed for a moment after it is
/// pulled, while I/O on it already fails (ENXIO, then EIO). Our private
/// test mounts are cleared at once, so [`ListedAfterPull`] keeps the dead
/// card listed by volume lookups for 5 s, as the demo's was.
#[derive(Debug)]
struct InFlightYank {
    listed: Arc<ListedAfterPull>,
    img: PathBuf,
    mount: PathBuf,
    b_verifying: std::sync::atomic::AtomicBool,
    c_writing: std::sync::atomic::AtomicBool,
    yanked: Arc<std::sync::atomic::AtomicBool>,
    fired: std::sync::atomic::AtomicBool,
    problems: Mutex<Vec<String>>,
}

impl InFlightYank {
    fn wait_for(&self, what: &str, flag: &std::sync::atomic::AtomicBool) -> bool {
        let t = std::time::Instant::now();
        while !flag.load(Ordering::Acquire) {
            if t.elapsed() > Duration::from_secs(60) {
                self.problems
                    .lock()
                    .unwrap()
                    .push(format!("timed out waiting: {what}"));
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        true
    }
}

impl FaultHook for InFlightYank {
    fn at(&self, point: FaultPoint, src: &Path) -> io::Result<()> {
        let name = src.file_name().map(|n| n.to_string_lossy().into_owned());
        let yanked = self.yanked.load(Ordering::Acquire);
        match (point.name(), name.as_deref()) {
            ("mid_verify", Some("b.bin")) if !yanked => {
                self.b_verifying.store(true, Ordering::Release);
                self.wait_for("the yank (b)", &self.yanked);
            }
            ("mid_chunk", Some("c.bin")) if !yanked && point.offset() >= 4 << 20 => {
                self.c_writing.store(true, Ordering::Release);
                self.wait_for("the yank (c)", &self.yanked);
            }
            ("mid_chunk", Some("a.bin"))
                if point.offset() >= 12 << 20 && !self.fired.swap(true, Ordering::AcqRel) =>
            {
                if self.wait_for("b mid-verify", &self.b_verifying)
                    && self.wait_for("c mid-chunk", &self.c_writing)
                {
                    self.listed
                        .keep_listing(&self.mount, Duration::from_secs(5));
                    let flag = self.yanked.clone();
                    if let Err(e) =
                        try_yank_stale(&self.img, &self.mount, Duration::ZERO, move || {
                            flag.store(true, Ordering::Release)
                        })
                    {
                        self.problems.lock().unwrap().push(e);
                    }
                }
                // Never leave b and c waiting.
                self.yanked.store(true, Ordering::Release);
            }
            _ => {}
        }
        Ok(())
    }
}

/// Volume lookups that keep a pulled card listed for a while, the way a
/// card mounted under /Volumes stays listed after it is pulled.
#[derive(Debug, Default)]
struct ListedAfterPull {
    dead: Mutex<Option<(PathBuf, safe_transfer::VolumeInfo, std::time::Instant)>>,
}

impl ListedAfterPull {
    fn keep_listing(&self, mount: &Path, for_how_long: Duration) {
        let v = Platform::native().volumes.volume_of(mount).unwrap();
        let mount = std::fs::canonicalize(mount).unwrap();
        *self.dead.lock().unwrap() = Some((mount, v, std::time::Instant::now() + for_how_long));
    }
    fn listed(&self) -> Option<(PathBuf, safe_transfer::VolumeInfo)> {
        let d = self.dead.lock().unwrap();
        d.as_ref()
            .filter(|(_, _, until)| std::time::Instant::now() < *until)
            .map(|(m, v, _)| (m.clone(), v.clone()))
    }
}

/// Shared with the hook, so it can start the window at the pull.
#[derive(Debug)]
struct Listing(Arc<ListedAfterPull>);

impl safe_transfer::VolumeIdentity for Listing {
    fn volume_of(&self, p: &Path) -> io::Result<safe_transfer::VolumeInfo> {
        if let Some((m, v)) = self.0.listed() {
            let c = std::fs::canonicalize(p).unwrap_or(p.to_path_buf());
            if c.starts_with(&m) || p.starts_with(&m) {
                return Ok(v);
            }
        }
        Platform::native().volumes.volume_of(p)
    }
    fn mounted(&self) -> io::Result<Vec<safe_transfer::VolumeInfo>> {
        let mut all = Platform::native().volumes.mounted()?;
        if let Some((_, v)) = self.0.listed()
            && !all.iter().any(|x| x.uuid == v.uuid)
        {
            all.push(v);
        }
        Ok(all)
    }
    fn case_sensitive(&self, dir: &Path) -> io::Result<bool> {
        Platform::native().volumes.case_sensitive(dir)
    }
}

#[test]
#[ignore = "needs hdiutil; run with --ignored"]
fn files_in_flight_when_the_card_goes_are_kept_for_resume_never_failed() {
    let s = tempfile::tempdir().unwrap();
    let src = s.path().join("shoot");
    std::fs::create_dir_all(&src).unwrap();
    for (name, mb, seed) in [("a.bin", 40, 1), ("b.bin", 10, 2), ("c.bin", 24, 3)] {
        std::fs::write(src.join(name), bytes(mb << 20, seed)).unwrap();
    }
    let card = Image::new("ExFAT", 256);
    let j = tempfile::tempdir().unwrap();
    let listed = Arc::new(ListedAfterPull::default());
    let hook = Arc::new(InFlightYank {
        listed: listed.clone(),
        img: card.img.clone(),
        mount: card.mount.clone(),
        b_verifying: Default::default(),
        c_writing: Default::default(),
        yanked: Default::default(),
        fired: Default::default(),
        problems: Mutex::new(Vec::new()),
    });
    let ev = Events::default();
    let mut cfg = EngineConfig::new(j.path());
    cfg.journal_interval = Duration::ZERO;
    cfg.faults = Some(hook.clone());
    let mut p = Platform::native();
    p.volumes = Arc::new(Listing(listed.clone()));
    let e = Engine::open(cfg, p, ev.sink()).unwrap();
    let mut spec = job_spec(&src, &card.mount);
    spec.options.workers = Some(3);
    let id = e.create(spec).unwrap();
    e.start(id).unwrap();
    let info = e.wait(id, Duration::from_secs(300)).unwrap();
    assert!(
        hook.problems.lock().unwrap().is_empty(),
        "{:?}",
        hook.problems.lock().unwrap()
    );
    assert!(
        hook.fired.load(Ordering::Acquire),
        "the card was never pulled"
    );
    assert_eq!(info.state, JobState::WaitingForDestination, "{info:?}");
    // The point: none of the three in-flight files is failed or skipped.
    let entries = e.entries(id).unwrap();
    let lost: Vec<_> = entries
        .iter()
        .filter(|x| x.state.is_terminal() && x.state != safe_transfer::journal::FileSt::Done)
        .collect();
    assert!(
        lost.is_empty(),
        "failed instead of kept for resume: {lost:?}"
    );
    assert!(
        e.report(id).unwrap().failed.is_empty(),
        "{:?}",
        e.report(id).unwrap().failed
    );

    card.put_back();
    real_names_are_complete(&src, &card.mount.join("shoot"), "after the yank");
    e.volumes_changed();
    e.resume(id).unwrap();
    let info = e.wait(id, Duration::from_secs(300)).unwrap();
    assert_eq!(info.state, JobState::Finished, "{info:?}");
    let r = e.report(id).unwrap();
    assert!(r.failed.is_empty() && r.skipped.is_empty(), "{r:?}");
    let diffs =
        safe_transfer::compare::compare(&Platform::native(), &src, &card.mount.join("shoot"))
            .unwrap();
    assert!(diffs.is_empty(), "{diffs:?}");
    assert!(no_partials_under(&card.mount).is_empty());
}

/// exFAT renames a file and its `._` AppleDouble companion (its extended
/// attributes) in two steps. A card pulled between them leaves the copy
/// under its real name and the companion still named after the partial
/// (seen in `jobs_survive_an_exfat_card_pulled_at_every_point`,
/// mid_group_rename: `._.DSC00028.XMP.<job>-30.st-partial` left behind).
/// This hook makes exactly that state for the first partial of a group,
/// then pulls the card.
#[derive(Debug)]
struct HalfRenamed {
    img: PathBuf,
    mount: PathBuf,
    dir: PathBuf,
    victim: Mutex<Option<String>>,
    fired: std::sync::atomic::AtomicBool,
    problems: Mutex<Vec<String>>,
}

impl FaultHook for HalfRenamed {
    fn at(&self, point: FaultPoint, _p: &Path) -> io::Result<()> {
        if point.name() != "mid_group_rename" || self.fired.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let problem = |m: String| self.problems.lock().unwrap().push(m);
        let partial = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with("._") && n.ends_with(".st-partial"))
            .find(|n| self.dir.join(format!("._{n}")).exists());
        let Some(partial) = partial else {
            problem("no partial with a `._` companion at mid_group_rename".into());
            return Ok(());
        };
        // ".DSC0001.XMP.<job>-<n>.st-partial" → "DSC0001.XMP"
        let real = partial
            .strip_suffix(".st-partial")
            .and_then(|s| s.rsplit_once('.'))
            .map(|(head, _)| head[1..].to_string())
            .unwrap();
        // The data file gets its real name (macOS moves the companion too)…
        if let Err(e) = std::fs::rename(self.dir.join(&partial), self.dir.join(&real)) {
            problem(format!("rename {partial}: {e}"));
        }
        // …and the companion goes back to the partial's name: the second
        // step never happened.
        if let Err(e) = std::fs::rename(
            self.dir.join(format!("._{real}")),
            self.dir.join(format!("._{partial}")),
        ) {
            problem(format!("rename ._{real}: {e}"));
        }
        *self.victim.lock().unwrap() = Some(real);
        if let Err(e) = try_yank(&self.img, &self.mount) {
            problem(e);
        }
        Ok(())
    }
}

/// The xattr the test puts on every original.
fn tag(p: &Path) -> Option<String> {
    let out = std::process::Command::new("xattr")
        .args(["-p", "com.filestudio.test"])
        .arg(p)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[test]
#[ignore = "needs hdiutil; run with --ignored"]
fn a_card_pulled_between_a_file_and_its_appledouble_rename_leaves_nothing_behind() {
    let s = tempfile::tempdir().unwrap();
    let src = s.path().join("sidecars");
    std::fs::create_dir_all(&src).unwrap();
    for i in 0..12 {
        let f = src.join(format!("DSC{i:04}.XMP"));
        std::fs::write(&f, format!("xmp {i}")).unwrap();
        sh(
            "xattr",
            &[
                "-w",
                "com.filestudio.test",
                &format!("tag {i}"),
                f.to_str().unwrap(),
            ],
        );
    }
    let card = Image::new("ExFAT", 128);
    let j = tempfile::tempdir().unwrap();
    let hook = Arc::new(HalfRenamed {
        img: card.img.clone(),
        mount: card.mount.clone(),
        dir: card.mount.join("sidecars"),
        victim: Mutex::new(None),
        fired: Default::default(),
        problems: Mutex::new(Vec::new()),
    });
    let ev = Events::default();
    let e = job_engine(j.path(), hook.clone(), &ev);
    let id = e.create(job_spec(&src, &card.mount)).unwrap();
    e.start(id).unwrap();
    let info = e.wait(id, Duration::from_secs(120)).unwrap();
    let problems = hook.problems.lock().unwrap().clone();
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(info.state, JobState::WaitingForDestination, "{info:?}");
    card.put_back();
    e.volumes_changed();
    e.resume(id).unwrap();
    let info = e.wait(id, Duration::from_secs(120)).unwrap();
    assert_eq!(info.state, JobState::Finished, "{info:?}");
    let r = e.report(id).unwrap();
    assert!(r.failed.is_empty() && r.skipped.is_empty(), "{r:?}");
    let dest = card.mount.join("sidecars");
    // Nothing of ours is left: no partial, and no partial's `._` companion.
    let left = no_partials_under(&card.mount);
    assert!(left.is_empty(), "left behind: {left:?}");
    // The half-renamed file has its extended attributes, like every other.
    let victim = hook.victim.lock().unwrap().clone().unwrap();
    for i in 0..12 {
        let name = format!("DSC{i:04}.XMP");
        assert_eq!(
            tag(&dest.join(&name)).as_deref(),
            Some(format!("tag {i}").as_str()),
            "{name} (the half-renamed one was {victim}) lost its extended attributes"
        );
    }
    let diffs = safe_transfer::compare::compare(&Platform::native(), &src, &dest).unwrap();
    assert!(diffs.is_empty(), "{diffs:?}");
}

/// Attaches and detaches small disk images until `stop`, so the system's
/// mount list keeps changing size (what other tests, cards and drives do).
fn churn_mounts(stop: &std::sync::atomic::AtomicBool, problems: &Mutex<Vec<String>>) {
    while !stop.load(Ordering::Acquire) {
        let img = Image::new("APFS", 16);
        std::thread::sleep(Duration::from_millis(50));
        drop(img);
        if stop.load(Ordering::Acquire) {
            break;
        }
    }
    let _ = problems;
}

/// The mount list is read from many threads at once (job threads, the
/// host's checks, several engines in one process) while disks come and
/// go. `getmntinfo` shares one buffer across the whole process: under
/// that load it freed a buffer another thread was reading (the ops
/// tests' SIGABRT) and returned garbled lists (a destination "gone" that
/// was still there).
#[test]
#[ignore = "needs hdiutil; run with --ignored"]
fn the_mount_list_is_safe_to_read_while_disks_come_and_go() {
    let stop = std::sync::atomic::AtomicBool::new(false);
    let problems = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| churn_mounts(&stop, &problems));
        }
        let readers: Vec<_> = (0..16)
            .map(|_| {
                s.spawn(|| {
                    let t = std::time::Instant::now();
                    while t.elapsed() < Duration::from_secs(30) {
                        match Platform::native().volumes.mounted() {
                            Ok(v) if v.iter().any(|x| x.mount == Path::new("/")) => {}
                            other => problems.lock().unwrap().push(format!(
                                "the startup disk is missing: {:?}",
                                other.map(|v| v.len())
                            )),
                        }
                    }
                })
            })
            .collect();
        for r in readers {
            r.join().unwrap();
        }
        stop.store(true, Ordering::Release);
    });
    let p = problems.into_inner().unwrap();
    assert!(
        p.is_empty(),
        "{} bad lists, first: {:?}",
        p.len(),
        p.first()
    );
}

/// Fails `victim` with an error that has nothing to do with any disk.
#[derive(Debug)]
struct FailOne;
impl FaultHook for FailOne {
    fn at(&self, point: FaultPoint, src: &Path) -> io::Result<()> {
        if point.name() == "before_copy" && src.ends_with("victim.bin") {
            return Err(io::Error::other("injected failure"));
        }
        Ok(())
    }
}

/// A file's own error fails that file only, also while other disks are
/// attached and detached and the mount list is read from many threads: the
/// job never stops to wait for a destination that is still there (seen in
/// the ops tests on the safe_transfer backend: "Interrupted" instead of
/// "completed with errors").
#[test]
#[ignore = "needs hdiutil; run with --ignored"]
fn a_files_own_error_never_makes_the_job_wait_while_disks_come_and_go() {
    let s = tempfile::tempdir().unwrap();
    let src = s.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("victim.bin"), bytes(1 << 20, 1)).unwrap();
    for i in 0..4 {
        std::fs::write(src.join(format!("ok{i}.txt")), format!("ok {i}")).unwrap();
    }
    let dest = Image::new("APFS", 256);
    let stop = std::sync::atomic::AtomicBool::new(false);
    let problems = Mutex::new(Vec::new());
    std::thread::scope(|sc| {
        for _ in 0..4 {
            sc.spawn(|| churn_mounts(&stop, &problems));
        }
        for _ in 0..16 {
            sc.spawn(|| {
                while !stop.load(Ordering::Acquire) {
                    let _ = Platform::native().volumes.mounted();
                }
            });
        }
        for round in 0..150 {
            let j = tempfile::tempdir().unwrap();
            let d = dest.mount.join(format!("r{round}"));
            std::fs::create_dir_all(&d).unwrap();
            let e = job_engine(j.path(), Arc::new(FailOne), &Events::default());
            let id = e.create(job_spec(&src, &d)).unwrap();
            e.start(id).unwrap();
            let info = e.wait(id, Duration::from_secs(120)).unwrap();
            let r = e.report(id).unwrap();
            if info.state != JobState::Finished || r.failed.len() != 1 {
                problems.lock().unwrap().push(format!(
                    "round {round}: {:?}, failed {:?}",
                    info.state, r.failed
                ));
            }
        }
        stop.store(true, Ordering::Release);
    });
    let p = problems.into_inner().unwrap();
    assert!(p.is_empty(), "{p:#?}");
}

/// Volume lookups whose mount list can't be read (what a garbled or failed
/// `getmntinfo` gave), while each path's own volume still answers.
#[derive(Debug)]
struct UnreadableList;
impl safe_transfer::VolumeIdentity for UnreadableList {
    fn volume_of(&self, p: &Path) -> io::Result<safe_transfer::VolumeInfo> {
        Platform::native().volumes.volume_of(p)
    }
    fn mounted(&self) -> io::Result<Vec<safe_transfer::VolumeInfo>> {
        Err(io::Error::other("the mount list couldn't be read"))
    }
    fn case_sensitive(&self, dir: &Path) -> io::Result<bool> {
        Platform::native().volumes.case_sensitive(dir)
    }
}

/// A mount list that can't be read is "unknown", never "not connected": a
/// file's own error still fails just that file, and the job finishes
/// instead of waiting for a destination that is right there.
#[test]
fn an_unreadable_mount_list_never_makes_a_job_wait_for_a_disk_that_is_there() {
    let (s, d, j) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let src = s.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("victim.bin"), bytes(1 << 20, 1)).unwrap();
    std::fs::write(src.join("ok.txt"), b"ok").unwrap();
    let mut cfg = EngineConfig::new(j.path());
    cfg.journal_interval = Duration::ZERO;
    cfg.faults = Some(Arc::new(FailOne));
    let mut p = Platform::native();
    p.volumes = Arc::new(UnreadableList);
    let e = Engine::open(cfg, p, Events::default().sink()).unwrap();
    let id = e.create(job_spec(&src, d.path())).unwrap();
    e.start(id).unwrap();
    let info = e.wait(id, Duration::from_secs(60)).unwrap();
    assert_eq!(info.state, JobState::Finished, "{info:?}");
    let r = e.report(id).unwrap();
    assert_eq!(r.failed.len(), 1, "{r:?}");
    assert!(r.failed[0].path.ends_with("victim.bin"), "{r:?}");
    assert_eq!(std::fs::read(d.path().join("src/ok.txt")).unwrap(), b"ok");
}

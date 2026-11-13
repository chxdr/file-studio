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

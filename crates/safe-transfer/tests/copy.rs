//! The single-file pipeline end to end, on the internal disk (temp folders).

mod common;

use std::fs::File;
use std::io;
use std::os::unix::fs::{FileExt, MetadataExt, PermissionsExt};
use std::path::Path;
use std::sync::Arc;

use common::*;
use safe_transfer::{
    Control, CopyError, CopyRequest, FileProgress, Flush, FlushLevel, MetaLoss, NoProgress,
    Options, Outcome, Phase, Platform, ProgressSink, Residency, Symlinks, Uncached,
};

fn xxh(p: &Path) -> u128 {
    let mut h = safe_transfer::chunk::ChunkHasher::new(64 << 20);
    h.update(&std::fs::read(p).unwrap());
    h.finish().file
}

#[test]
fn a_verified_copy_matches_and_leaves_nothing_behind() {
    let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let src = s.path().join("DSC01234.ARW");
    std::fs::write(&src, bytes(3 << 20, 1)).unwrap();
    let r = copy(&src, d.path(), &no_clone()).unwrap();
    let dest = d.path().join("DSC01234.ARW");
    assert_eq!(r.outcome, Outcome::Verified);
    assert_eq!(r.dest, dest);
    assert_eq!(r.bytes, 3 << 20);
    assert_eq!(r.hashes.file, xxh(&src));
    assert_eq!(xxh(&dest), xxh(&src));
    assert_eq!(r.flush_level, FlushLevel::Device);
    assert_eq!(r.retries, 0);
    assert!(!r.source_reread);
    assert_eq!(names(d.path()), ["DSC01234.ARW"]);
    // Same volume, so dates and mode survive; nothing reported lost.
    assert!(r.metadata_lost.is_empty(), "{:?}", r.metadata_lost);
    let (a, b) = (src.metadata().unwrap(), dest.metadata().unwrap());
    assert_eq!(a.mtime(), b.mtime());
    assert_eq!(a.mode(), b.mode());
}

#[test]
fn the_same_apfs_volume_clones_and_says_so() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("big.bin");
    std::fs::write(&src, bytes(8 << 20, 2)).unwrap();
    std::fs::create_dir(d.path().join("out")).unwrap();
    let r = copy(&src, &d.path().join("out"), &Options::default()).unwrap();
    assert_eq!(r.outcome, Outcome::Cloned);
    assert!(r.hashes.chunks.is_empty(), "a clone claims no hashes");
    assert_eq!(xxh(&r.dest), xxh(&src));
}

#[test]
fn an_existing_file_is_never_replaced() {
    let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let src = s.path().join("a.ARW");
    std::fs::write(&src, b"new").unwrap();
    std::fs::write(d.path().join("a.ARW"), b"old").unwrap();
    for opts in [no_clone(), Options::default()] {
        let e = copy(&src, d.path(), &opts).unwrap_err();
        assert!(matches!(e.error, CopyError::Conflict(_)), "{e}");
        assert_eq!(std::fs::read(d.path().join("a.ARW")).unwrap(), b"old");
        assert_eq!(names(d.path()), ["a.ARW"]);
    }
}

#[test]
fn multi_chunk_files_flush_per_chunk_and_hash_every_chunk() {
    let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let src = s.path().join("video.mov");
    std::fs::write(&src, bytes((5 << 20) + 12345, 3)).unwrap();
    let opts = Options {
        chunk_size: 1 << 20,
        ..no_clone()
    };
    let r = copy(&src, d.path(), &opts).unwrap();
    assert_eq!(r.hashes.chunks.len(), 6);
    assert_eq!(xxh(&r.dest), xxh(&src));
}

#[test]
fn zero_byte_files() {
    let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let src = s.path().join("empty");
    std::fs::write(&src, b"").unwrap();
    let r = copy(&src, d.path(), &no_clone()).unwrap();
    assert_eq!(r.outcome, Outcome::Verified);
    assert_eq!(std::fs::metadata(&r.dest).unwrap().len(), 0);
}

#[test]
fn sparse_files_stay_sparse_on_apfs() {
    let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let src = s.path().join("disk.img");
    let f = File::create(&src).unwrap();
    f.set_len(64 << 20).unwrap();
    f.write_at(&[7u8; 4096], 32 << 20).unwrap();
    f.sync_all().unwrap();
    drop(f);
    let r = copy(&src, d.path(), &no_clone()).unwrap();
    assert_eq!(xxh(&r.dest), xxh(&src));
    let on_disk = std::fs::metadata(&r.dest).unwrap().blocks() * 512;
    assert!(on_disk < 4 << 20, "{on_disk} bytes on disk");
    assert!(!r.metadata_lost.contains(&MetaLoss::Sparseness));
}

#[test]
fn symlinks_are_copied_as_links_unless_asked() {
    let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let target = s.path().join("real.txt");
    std::fs::write(&target, b"content").unwrap();
    let link = s.path().join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let dangling = s.path().join("dangling");
    std::os::unix::fs::symlink("/nowhere/at/all", &dangling).unwrap();

    let r = copy(&link, d.path(), &no_clone()).unwrap();
    assert_eq!(r.outcome, Outcome::Link);
    assert_eq!(std::fs::read_link(d.path().join("link")).unwrap(), target);
    let r = copy(&dangling, d.path(), &no_clone()).unwrap();
    assert_eq!(r.outcome, Outcome::Link);

    let e = tempfile::tempdir().unwrap();
    let follow = Options {
        symlinks: Symlinks::Follow,
        ..no_clone()
    };
    let req = CopyRequest::new(&link, e.path().join("link")).unwrap();
    let r = safe_transfer::copy_file(
        &Platform::native(),
        &req,
        &follow,
        &Control::new(),
        &NoProgress,
    )
    .unwrap();
    assert_eq!(r.outcome, Outcome::Verified);
    assert_eq!(std::fs::read(e.path().join("link")).unwrap(), b"content");
    assert!(
        !std::fs::symlink_metadata(e.path().join("link"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn awkward_names_are_kept_exactly() {
    let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    for (i, name) in [
        "📷 Smith wedding.ARW",
        "cafe\u{301} (NFD).jpg",
        "caf\u{e9} (NFC).jpg",
        "say \"cheese\".txt",
        "trailing space ",
        ".hidden",
        &"é".repeat(127), // 254 bytes
    ]
    .iter()
    .enumerate()
    {
        let src = s.path().join(name);
        std::fs::write(&src, bytes(1000 + i, i as u64)).unwrap();
        let r = copy(&src, d.path(), &no_clone()).unwrap();
        assert_eq!(r.dest.file_name().unwrap(), std::ffi::OsStr::new(name));
        assert_eq!(xxh(&r.dest), xxh(&src));
    }
    assert!(no_partials(d.path()));
}

#[test]
fn names_differing_only_by_case_conflict_on_a_case_insensitive_disk() {
    let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = s.path().join("upper");
    std::fs::create_dir(&a).unwrap();
    std::fs::write(a.join("File.txt"), b"one").unwrap();
    std::fs::write(s.path().join("file.txt"), b"two").unwrap();
    copy(&a.join("File.txt"), d.path(), &no_clone()).unwrap();
    let e = copy(&s.path().join("file.txt"), d.path(), &no_clone()).unwrap_err();
    assert!(matches!(e.error, CopyError::Conflict(_)), "{e}");
    assert_eq!(std::fs::read(d.path().join("File.txt")).unwrap(), b"one");
    assert!(no_partials(d.path()));
}

#[test]
fn unreadable_sources_and_read_only_folders_fail_cleanly() {
    let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let src = s.path().join("secret");
    std::fs::write(&src, b"x").unwrap();
    std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o000)).unwrap();
    let e = copy(&src, d.path(), &no_clone()).unwrap_err();
    assert!(e.to_string().contains("permission denied"), "{e}");
    assert!(names(d.path()).is_empty());
    std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o644)).unwrap();

    let ro = d.path().join("ro");
    std::fs::create_dir(&ro).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    let e = copy(&src, &ro, &no_clone()).unwrap_err();
    assert!(e.to_string().contains("permission denied"), "{e}");
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(names(&ro).is_empty());
}

#[test]
fn folders_and_devices_are_refused() {
    let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let e = copy(s.path(), d.path(), &no_clone()).unwrap_err();
    assert!(matches!(e.error, CopyError::Unsupported(_)));
    let e = copy(Path::new("/dev/null"), d.path(), &no_clone()).unwrap_err();
    assert!(matches!(e.error, CopyError::Unsupported(_)));
}

/// Cancels on the first report of the given phase.
struct CancelAt(Phase, Arc<Control>);
impl ProgressSink for CancelAt {
    fn progress(&self, p: &FileProgress) {
        if p.phase == self.0 && p.done > 0 {
            self.1.cancel();
        }
    }
}

#[test]
fn cancelling_leaves_the_destination_as_it_was() {
    let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let src = s.path().join("big.bin");
    std::fs::write(&src, bytes(24 << 20, 4)).unwrap();
    for phase in [Phase::Copying, Phase::Verifying] {
        let ctl = Arc::new(Control::new());
        let opts = Options {
            progress_interval: std::time::Duration::ZERO,
            ..no_clone()
        };
        let req = CopyRequest::into_dir(&src, d.path()).unwrap();
        let e = safe_transfer::copy_file(
            &Platform::native(),
            &req,
            &opts,
            &ctl,
            &CancelAt(phase, ctl.clone()),
        )
        .unwrap_err();
        assert!(matches!(e.error, CopyError::Cancelled), "{phase:?}: {e}");
        assert!(e.partial_left.is_none());
        assert!(names(d.path()).is_empty(), "{:?}", names(d.path()));
    }
}

/// Appends to the original while it is being copied.
struct Grow(std::path::PathBuf);
impl ProgressSink for Grow {
    fn progress(&self, p: &FileProgress) {
        if p.phase == Phase::Copying && p.done > 0 && p.done < p.total {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&self.0)
                .unwrap();
            f.write_all(b"more").unwrap();
        }
    }
}

#[test]
fn a_changing_original_is_never_named() {
    let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let src = s.path().join("growing.log");
    std::fs::write(&src, bytes(12 << 20, 5)).unwrap();
    let opts = Options {
        progress_interval: std::time::Duration::ZERO,
        ..no_clone()
    };
    let req = CopyRequest::into_dir(&src, d.path()).unwrap();
    let e = safe_transfer::copy_file(
        &Platform::native(),
        &req,
        &opts,
        &Control::new(),
        &Grow(src.clone()),
    )
    .unwrap_err();
    assert!(matches!(e.error, CopyError::SourceChanged), "{e}");
    assert!(names(d.path()).is_empty());
}

#[test]
fn reread_mode_reads_the_original_twice() {
    let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let src = s.path().join("client.ARW");
    std::fs::write(&src, bytes(6 << 20, 6)).unwrap();
    let opts = Options {
        reread_source: true,
        ..no_clone()
    };
    let r = copy(&src, d.path(), &opts).unwrap();
    assert!(r.source_reread);
    assert!(r.reread_time > std::time::Duration::ZERO);
    assert_eq!(xxh(&r.dest), xxh(&src));
}

#[test]
fn tags_acls_and_locks_survive() {
    let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let src = s.path().join("tagged.ARW");
    std::fs::write(&src, bytes(100_000, 7)).unwrap();
    sh(
        "xattr",
        &["-w", "com.example.note", "keep me", src.to_str().unwrap()],
    );
    sh(
        "xattr",
        &[
            "-w",
            "com.apple.metadata:_kMDItemUserTags",
            "(\"Red\n6\")",
            src.to_str().unwrap(),
        ],
    );
    sh(
        "chmod",
        &["+a", "everyone deny delete", src.to_str().unwrap()],
    );
    sh("chflags", &["uchg", src.to_str().unwrap()]);
    let r = copy(&src, d.path(), &no_clone());
    // Unlock both before asserting, so the temp folders can be cleaned up.
    let unlock = |p: &Path| {
        let _ = std::process::Command::new("chflags")
            .arg("nouchg")
            .arg(p)
            .status();
        let _ = std::process::Command::new("chmod")
            .arg("-N")
            .arg(p)
            .status();
    };
    unlock(&src);
    let dest = d.path().join("tagged.ARW");
    let flags = std::fs::symlink_metadata(&dest).map(|m| {
        use std::os::macos::fs::MetadataExt;
        m.st_flags()
    });
    let acl_out = std::process::Command::new("ls")
        .arg("-le")
        .arg(&dest)
        .output();
    unlock(&dest);
    let r = r.unwrap();
    assert!(r.metadata_lost.is_empty(), "{:?}", r.metadata_lost);
    assert_eq!(flags.unwrap() & 0x2, 0x2, "uchg kept");
    assert!(String::from_utf8_lossy(&acl_out.unwrap().stdout).contains("deny delete"));
    let x = std::process::Command::new("xattr")
        .arg(&dest)
        .output()
        .unwrap();
    let x = String::from_utf8_lossy(&x.stdout);
    assert!(
        x.contains("com.example.note") && x.contains("_kMDItemUserTags"),
        "{x}"
    );
}

#[test]
fn compressed_originals_copy_correctly() {
    let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let plain = s.path().join("plain.txt");
    std::fs::write(&plain, "compress me ".repeat(200_000)).unwrap();
    let src = s.path().join("compressed.txt");
    sh(
        "ditto",
        &[
            "--hfsCompression",
            plain.to_str().unwrap(),
            src.to_str().unwrap(),
        ],
    );
    let r = copy(&src, d.path(), &no_clone()).unwrap();
    assert_eq!(r.outcome, Outcome::Verified);
    assert_eq!(
        std::fs::read(&r.dest).unwrap(),
        std::fs::read(&plain).unwrap()
    );
}

/// A disk that only manages an OS-level flush.
struct WeakFlush;
impl Flush for WeakFlush {
    fn flush_file(&self, f: &File) -> io::Result<FlushLevel> {
        f.sync_all()?;
        Ok(FlushLevel::OsOnly)
    }
    fn flush_dir(&self, _dir: &Path) -> io::Result<FlushLevel> {
        Ok(FlushLevel::OsOnly)
    }
}

/// A cache that can't be emptied.
struct StickyCache;
impl Uncached for StickyCache {
    fn set_uncached(&self, _f: &File) -> io::Result<()> {
        Ok(())
    }
    fn evict(&self, _p: &Path) -> io::Result<Residency> {
        Ok(Residency {
            resident_pages: 1,
            total_pages: 1,
        })
    }
    fn residency(&self, _p: &Path) -> io::Result<Residency> {
        self.evict(_p)
    }
}

#[test]
fn reports_never_claim_more_than_happened() {
    let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let src = s.path().join("a.bin");
    std::fs::write(&src, bytes(1 << 20, 8)).unwrap();
    let mut p = Platform::native();
    p.flush = Arc::new(WeakFlush);
    p.uncached = Arc::new(StickyCache);
    let req = CopyRequest::into_dir(&src, d.path()).unwrap();
    let r = safe_transfer::copy_file(&p, &req, &no_clone(), &Control::new(), &NoProgress).unwrap();
    assert_eq!(r.flush_level, FlushLevel::OsOnly);
    assert_eq!(r.outcome, Outcome::VerifiedFromMemory);
}

#[test]
fn planning_reads_no_data_and_counts_space() {
    let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let src = s.path().join("a.ARW");
    std::fs::write(&src, bytes(50_000, 9)).unwrap();
    let req = CopyRequest::into_dir(&src, d.path()).unwrap();
    let plan = safe_transfer::plan(&Platform::native(), &req).unwrap();
    assert!(!plan.conflict && plan.same_volume && plan.fits);
    assert_eq!(plan.bytes, 50_000);
    assert_eq!(plan.dest_volume.fs_type, "apfs");
}

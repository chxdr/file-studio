//! Metadata survives every copy path: clone (same APFS volume), copyfile
//! (cross volume), the verifying loop (cross volume + verify), and a
//! cross-volume move. Checks xattrs, Finder tags, the resource fork, ACL,
//! mode, mtime, birth time, immutable flag, and sparse files.

mod common;
use common::*;
use fstudio_ops::*;
use std::os::macos::fs::MetadataExt as _;
use std::path::Path;

const TAGS_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><array><string>Red
6</string><string>Project</string></array></plist>"#;

fn tags_hex(scratch: &Path) -> String {
    let p = scratch.join("tags.plist");
    std::fs::write(&p, TAGS_PLIST).unwrap();
    sh("plutil", &["-convert", "binary1", p.to_str().unwrap()]);
    read(&p).iter().map(|b| format!("{b:02x}")).collect()
}

/// A file with everything we care about, plus a sparse file.
fn make_rich(dir: &Path, scratch: &Path) {
    let f = dir.join("rich.txt");
    write(&f, b"rich content");
    let fs = f.to_str().unwrap();
    sh("xattr", &["-w", "com.example.note", "hello", fs]);
    sh(
        "xattr",
        &[
            "-wx",
            "com.apple.metadata:_kMDItemUserTags",
            &tags_hex(scratch),
            fs,
        ],
    );
    std::fs::write(f.join("..namedfork/rsrc"), vec![0xAB; 5000]).unwrap();
    std::fs::set_permissions(&f, std::os::unix::fs::PermissionsExt::from_mode(0o640)).unwrap();
    // An old mtime also pulls the birth time back (macOS keeps btime ≤ mtime).
    let t = std::time::UNIX_EPOCH + std::time::Duration::new(1_000_000_000, 123_456_789);
    std::fs::File::options()
        .write(true)
        .open(&f)
        .unwrap()
        .set_modified(t)
        .unwrap();
    sh("chmod", &["+a", "everyone deny delete", fs]);

    let locked = dir.join("locked.txt");
    write(&locked, b"locked");
    sh("chflags", &["uchg", locked.to_str().unwrap()]);

    let sparse = dir.join("sparse.bin");
    let s = std::fs::File::create(&sparse).unwrap();
    s.set_len(1 << 30).unwrap();
    use std::os::unix::fs::FileExt;
    s.write_all_at(&[1u8; 4096], 0).unwrap();
    s.write_all_at(&[2u8; 4096], 512 << 20).unwrap();
}

fn unlock(dir: &Path) {
    let _ = std::process::Command::new("chflags")
        .args(["-R", "nouchg"])
        .arg(dir)
        .status();
    let _ = std::process::Command::new("chmod")
        .args(["-R", "-N"])
        .arg(dir)
        .status();
}

fn xattrs(p: &Path) -> Vec<(String, String)> {
    let names = sh("xattr", &[p.to_str().unwrap()]);
    let mut out: Vec<(String, String)> = names
        .lines()
        .filter(|n| !n.is_empty() && *n != "com.apple.provenance" && *n != "com.apple.quarantine")
        .map(|n| (n.to_string(), sh("xattr", &["-px", n, p.to_str().unwrap()])))
        .collect();
    out.sort();
    out
}

fn acl(p: &Path) -> String {
    sh("ls", &["-led", p.to_str().unwrap()])
        .lines()
        .skip(1)
        .collect::<Vec<_>>()
        .join("\n")
}

fn check_rich(src: &Path, dst: &Path, what: &str) {
    let (a, b) = (src.join("rich.txt"), dst.join("rich.txt"));
    assert_eq!(read(&a), read(&b), "{what}: data");
    let xa = xattrs(&a);
    assert!(
        xa.iter().any(|(n, _)| n == "com.example.note"),
        "{what}: setup"
    );
    assert!(
        xa.iter().any(|(n, _)| n == "com.apple.ResourceFork"),
        "{what}: setup rsrc"
    );
    assert_eq!(xa, xattrs(&b), "{what}: xattrs, tags and resource fork");
    let acl_a = acl(&a);
    assert!(acl_a.contains("deny delete"), "{what}: setup acl");
    assert_eq!(acl_a, acl(&b), "{what}: ACL");
    let (ma, mb) = (
        std::fs::metadata(&a).unwrap(),
        std::fs::metadata(&b).unwrap(),
    );
    assert_eq!(ma.st_mode() & 0o7777, mb.st_mode() & 0o7777, "{what}: mode");
    assert_eq!(mtime_ns(&a), mtime_ns(&b), "{what}: mtime");
    assert_eq!(ma.st_birthtime(), mb.st_birthtime(), "{what}: birth time");

    let lb = std::fs::metadata(dst.join("locked.txt")).unwrap();
    assert!(lb.st_flags() & 0x2 != 0, "{what}: uchg kept");
    assert_eq!(read(&dst.join("locked.txt")), b"locked");

    let sb = std::fs::metadata(dst.join("sparse.bin")).unwrap();
    assert_eq!(sb.len(), 1 << 30, "{what}: sparse length");
    assert!(
        sb.st_blocks() * 512 < 4 << 20,
        "{what}: sparse file filled in: {} bytes on disk",
        sb.st_blocks() * 512
    );
    assert_eq!(
        hash_file(&src.join("sparse.bin")).unwrap(),
        hash_file(&dst.join("sparse.bin")).unwrap(),
        "{what}: sparse content"
    );
    assert!(partials(dst).is_empty(), "{what}: partials left");
}

fn copy_and_check(e: &Env, dest_root: &Path, what: &str) {
    let src = e.p("src");
    make_rich(&src, e.root());
    std::fs::create_dir_all(dest_root).unwrap();
    let st = e.run(OpRequest::Copy {
        sources: vec![src.clone()],
        dest_dir: dest_root.to_path_buf(),
    });
    assert_eq!(st.state, OpState::Completed, "{what}: {st:?}");
    check_rich(&src, &dest_root.join("src"), what);
    unlock(&src);
    unlock(dest_root);
}

#[test]
fn same_volume_clone_keeps_metadata() {
    let e = env();
    copy_and_check(&e, &e.p("dst"), "clone");
}

#[test]
fn cross_volume_copyfile_keeps_metadata() {
    let img = Image::new("APFS", 256);
    let e = env();
    copy_and_check(&e, &img.mount.join("dst"), "copyfile cross-volume");
}

#[test]
fn verified_loop_keeps_metadata() {
    let img = Image::new("APFS", 256);
    let e = env_with(OpsConfig {
        verify: true,
        ..Default::default()
    });
    copy_and_check(&e, &img.mount.join("dst"), "verify loop cross-volume");
}

#[test]
fn verified_same_volume_without_clone_is_not_needed_but_works() {
    // Same volume with verify: clones skip verify, result is identical.
    let e = env_with(OpsConfig {
        verify: true,
        ..Default::default()
    });
    copy_and_check(&e, &e.p("dst"), "clone with verify on");
}

#[test]
fn cross_volume_move_keeps_metadata_and_removes_source() {
    let img = Image::new("APFS", 256);
    let e = env_with(OpsConfig {
        verify: true,
        ..Default::default()
    });
    let src = e.p("src");
    make_rich(&src, e.root());
    // Keep a reference copy to compare against (same volume clone).
    std::fs::create_dir_all(e.p("ref")).unwrap();
    assert_eq!(
        e.run(OpRequest::Copy {
            sources: vec![src.clone()],
            dest_dir: e.p("ref")
        })
        .state,
        OpState::Completed
    );
    // The ACL denies delete on rich.txt and locked.txt is uchg, so removing
    // those two sources fails per file and keeps them.
    let st = e.run(OpRequest::Move {
        sources: vec![src.clone()],
        dest_dir: img.mount.clone(),
    });
    check_rich(&e.p("ref/src"), &img.mount.join("src"), "cross-volume move");
    assert_eq!(st.state, OpState::CompletedWithErrors, "{st:?}");
    assert_eq!(st.errors.len(), 2, "{st:?}");
    assert!(st.errors.iter().all(|e| {
        e.message
            .starts_with("copied, but the original couldn't be removed")
    }));
    assert!(
        exists(&src.join("rich.txt")) && exists(&src.join("locked.txt")),
        "sources we couldn't remove are kept"
    );
    assert!(exists(&src), "the source folder isn't empty, so it is kept");
    assert_eq!(st.bytes_done, st.bytes_total, "progress counts holes too");
    assert!(
        !exists(&src.join("sparse.bin")),
        "moved sources are removed"
    );
    unlock(&src);
    unlock(&e.p("ref"));
    unlock(&img.mount);
}

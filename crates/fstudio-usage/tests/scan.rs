//! The scanner against real temporary folders: sizes, hard links, names
//! that differ only by Unicode form or case, symlink loops, unreadable
//! folders, cancellation, packages, rescans and the session API.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fstudio_usage::*;

fn write(p: &Path, bytes: usize) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, vec![7u8; bytes]).unwrap();
}

fn opts() -> ScanOptions {
    ScanOptions::default()
}

fn scan_ok(root: &Path, method: Method) -> Tree {
    let o = ScanOptions { method, ..opts() };
    scan(root, &o, &ScanControl::new()).unwrap()
}

fn child(t: &Tree, parent: NodeId, name: &str) -> NodeId {
    *t.children(parent)
        .iter()
        .find(|c| t.node(**c).name_lossy() == name)
        .unwrap_or_else(|| panic!("no child {name}"))
}

#[test]
fn sizes_counts_and_order() {
    for method in [Method::Auto, Method::Portable] {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(&r.join("a/big.bin"), 100_000);
        write(&r.join("a/small.txt"), 10);
        write(&r.join("b/c/d/deep.jpg"), 50_000);
        write(&r.join("top.mov"), 1);
        fs::create_dir(r.join("empty")).unwrap();
        let t = scan_ok(r, method);
        let root = t.node(t.root());
        assert_eq!(root.logical, 150_011, "{method:?}");
        // a, a/big, a/small, b, b/c, b/c/d, deep, top, empty
        assert_eq!(root.items, 9);
        assert!(root.allocated >= root.logical - 11);
        // Largest first.
        let names: Vec<String> = t
            .children(t.root())
            .iter()
            .map(|c| t.node(*c).name_lossy())
            .collect();
        assert_eq!(names[0], "a");
        assert_eq!(names[1], "b");
        let a = child(&t, t.root(), "a");
        assert_eq!(t.node(a).items, 2);
        assert_eq!(t.node(a).logical, 100_010);
        assert_eq!(t.path(a), r.join("a"));
        assert_eq!(
            t.find(&r.join("b/c/d/deep.jpg")).map(|i| t.path(i)),
            Some(r.join("b/c/d/deep.jpg"))
        );
        let big = t.find(&r.join("a/big.bin")).unwrap();
        assert_eq!(t.ext_name(t.node(big).ext), "bin");
    }
}

#[test]
fn bulk_and_portable_agree() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    for i in 0..300 {
        write(&r.join(format!("d{}/f{i}.dat", i % 7)), i * 13);
    }
    let a = scan_ok(r, Method::Auto);
    let b = scan_ok(r, Method::Portable);
    let (ra, rb) = (a.node(a.root()), b.node(b.root()));
    assert_eq!(ra.logical, rb.logical);
    assert_eq!(ra.allocated, rb.allocated);
    assert_eq!(ra.items, rb.items);
}

#[test]
fn hard_links_count_once() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(&r.join("x/original.raw"), 64_000);
    fs::create_dir_all(r.join("y")).unwrap();
    fs::hard_link(r.join("x/original.raw"), r.join("y/link1.raw")).unwrap();
    fs::hard_link(r.join("x/original.raw"), r.join("y/link2.raw")).unwrap();
    for method in [Method::Auto, Method::Portable] {
        let t = scan_ok(r, method);
        let root = t.node(t.root());
        assert_eq!(root.logical, 64_000, "{method:?}");
        assert_eq!(root.items, 5);
        let dup = [
            r.join("x/original.raw"),
            r.join("y/link1.raw"),
            r.join("y/link2.raw"),
        ]
        .iter()
        .filter(|p| t.node(t.find(p).unwrap()).has(flags::HARDLINK))
        .count();
        assert_eq!(dup, 2);
    }
}

#[test]
fn unicode_forms_and_case_are_kept_apart() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    let nfc = "caf\u{e9}.txt";
    let nfd = "cafe\u{301}.txt";
    write(&r.join(nfc), 100);
    // APFS looks names up normalisation-insensitively, so the NFD name may
    // land on the same file; whatever the disk keeps is what we show.
    let nfd_separate =
        fs::write(r.join(nfd), vec![1u8; 200]).is_ok() && fs::read_dir(r).unwrap().count() == 2;
    write(&r.join("Photo.JPG"), 10);
    let case_separate = fs::write(r.join("photo.jpg"), vec![1u8; 20]).is_ok()
        && fs::read_dir(r).unwrap().count() >= if nfd_separate { 4 } else { 3 };
    let t = scan_ok(r, Method::Auto);
    let on_disk: Vec<Vec<u8>> = {
        use std::os::unix::ffi::OsStrExt;
        let mut v: Vec<Vec<u8>> = fs::read_dir(r)
            .unwrap()
            .map(|e| e.unwrap().file_name().as_bytes().to_vec())
            .collect();
        v.sort();
        v
    };
    let mut ours: Vec<Vec<u8>> = t
        .children(t.root())
        .iter()
        .map(|c| t.node(*c).name.to_vec())
        .collect();
    ours.sort();
    assert_eq!(ours, on_disk, "names exactly as stored");
    let total: u64 = fs::read_dir(r)
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum();
    assert_eq!(t.node(t.root()).logical, total);
    // Type keys are case-folded: JPG and jpg are one type.
    let stats = types::breakdown(&t, t.root(), SizeMode::Logical);
    let jpg: Vec<_> = stats.iter().filter(|s| s.ext_name == "jpg").collect();
    assert_eq!(jpg.len(), 1);
    assert_eq!(jpg[0].count, if case_separate { 2 } else { 1 });
    let _ = nfd_separate;
}

#[test]
fn symlinks_are_never_followed() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(&r.join("real/data.bin"), 10_000);
    std::os::unix::fs::symlink(r, r.join("real/loop")).unwrap();
    std::os::unix::fs::symlink("..", r.join("real/up")).unwrap();
    std::os::unix::fs::symlink(r.join("real/data.bin"), r.join("alias.bin")).unwrap();
    std::os::unix::fs::symlink("/", r.join("root")).unwrap();
    let t = scan_ok(r, Method::Auto);
    let root = t.node(t.root());
    // real, data.bin, loop, up, alias.bin, root
    assert_eq!(root.items, 6);
    assert!(
        root.logical < 10_000 + 4096,
        "links add only their own size"
    );
    let l = t.find(&r.join("real/loop")).unwrap();
    assert_eq!(t.node(l).kind, NodeKind::Symlink);
    assert!(t.children(l).is_empty());
}

#[test]
fn unreadable_folders_are_reported_not_fatal() {
    if running_as_root() {
        return; // root reads everything
    }
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(&r.join("ok/file.txt"), 500);
    write(&r.join("locked/secret.txt"), 500);
    fs::set_permissions(r.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let t = scan(r, &opts(), &ScanControl::new());
    fs::set_permissions(r.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
    let t = t.unwrap();
    let locked = t.find(&r.join("locked")).unwrap();
    assert!(t.node(locked).has(flags::DENIED));
    assert_eq!(t.node(t.root()).denied, 1);
    assert_eq!(t.node(t.root()).logical, 500);
}

fn running_as_root() -> bool {
    std::env::var("USER").is_ok_and(|u| u == "root")
}

#[test]
fn a_read_only_tree_scans_fine() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(&r.join("ro/a.txt"), 30);
    fs::set_permissions(r.join("ro/a.txt"), fs::Permissions::from_mode(0o444)).unwrap();
    fs::set_permissions(r.join("ro"), fs::Permissions::from_mode(0o555)).unwrap();
    let t = scan_ok(r, Method::Auto);
    fs::set_permissions(r.join("ro"), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(t.node(t.root()).logical, 30);
}

#[test]
fn packages_are_one_item_but_sized() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(&r.join("Tool.app/Contents/MacOS/tool"), 70_000);
    write(&r.join("Tool.app/Contents/Info.plist"), 1_000);
    write(&r.join("Shoot.photoslibrary/originals/a.heic"), 9_000);
    write(&r.join("plain/x.txt"), 1);
    let t = scan_ok(r, Method::Auto);
    let app = t.find(&r.join("Tool.app")).unwrap();
    let n = t.node(app);
    assert_eq!(n.kind, NodeKind::Package);
    assert_eq!(n.logical, 71_000);
    assert!(t.children(app).is_empty(), "packages are leaves");
    assert_eq!(n.weight(), 1);
    // Tool.app, Shoot.photoslibrary, plain, x.txt
    assert_eq!(t.node(t.root()).items, 4);
    let stats = types::breakdown(&t, t.root(), SizeMode::Logical);
    assert_eq!(stats[0].ext_name, "app");
    assert_eq!(stats[0].category, Category::Package);
}

#[test]
fn cancelling_stops_early_and_flags_incomplete() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    for i in 0..400 {
        write(&r.join(format!("d{}/e{}/f.txt", i % 20, i)), 10);
    }
    let ctl = ScanControl::new();
    ctl.cancel();
    let t = scan(r, &opts(), &ctl).unwrap();
    assert!(t.node(t.root()).has(flags::INCOMPLETE));
    assert_eq!(t.node(t.root()).items, 0);

    // Through a session: cancel right away, the state says so.
    let s = UsageSession::start(r, opts(), Arc::new(NullObserver));
    s.cancel();
    wait(&s);
    assert!(matches!(s.state(), ScanState::Cancelled | ScanState::Done));
}

#[test]
fn a_missing_or_file_root_fails() {
    let d = tempfile::tempdir().unwrap();
    assert!(scan(&d.path().join("nope"), &opts(), &ScanControl::new()).is_err());
    write(&d.path().join("f"), 1);
    assert!(scan(&d.path().join("f"), &opts(), &ScanControl::new()).is_err());
    // A symlink to a folder is not entered either.
    std::os::unix::fs::symlink(d.path(), d.path().join("l")).unwrap();
    assert!(scan(&d.path().join("l"), &opts(), &ScanControl::new()).is_err());
}

#[test]
fn folder_totals_match_the_tree() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(&r.join("a/b/c.bin"), 12_345);
    write(&r.join("a/d.bin"), 1);
    let tot = folder_totals(r, &ScanControl::new()).unwrap();
    assert_eq!(tot.logical, 12_346);
    assert_eq!(tot.items, 4);
    assert!(!tot.incomplete);
}

fn wait(s: &UsageSession) {
    let t0 = Instant::now();
    while s.state() == ScanState::Scanning {
        assert!(t0.elapsed() < Duration::from_secs(30), "scan never ended");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for(f: impl Fn() -> bool) {
    let t0 = Instant::now();
    while !f() {
        assert!(t0.elapsed() < Duration::from_secs(30), "timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn session_rows_selection_and_treemap() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(&r.join("Photos/2024/a.cr3"), 300_000);
    write(&r.join("Photos/2024/b.cr3"), 200_000);
    write(&r.join("Photos/2025/c.jpg"), 50_000);
    write(&r.join("Video/clip.mov"), 400_000);
    write(&r.join("notes.txt"), 100);
    let s = UsageSession::start(r, opts(), Arc::new(NullObserver));
    wait(&s);
    assert_eq!(s.state(), ScanState::Done);
    // Root expanded: root + Photos, Video, notes.txt.
    assert_eq!(s.row_count(), 4);
    let rows = s.rows(0, 10);
    assert_eq!(rows[0].depth, 0);
    assert!(rows[0].expanded);
    let photos = s.find(&r.join("Photos")).unwrap();
    assert_eq!(rows[1].id, photos, "largest first");

    // Selecting a deep file expands its folders.
    let b = s.find(&r.join("Photos/2024/b.cr3")).unwrap();
    let row = s.select(Some(b)).unwrap();
    assert_eq!(s.rows(row, 1)[0].id, b);
    assert_eq!(s.rows(row, 1)[0].depth, 3);
    // ← goes to the parent, ← again collapses it.
    s.move_selection(Move::Left);
    let y2024 = s.find(&r.join("Photos/2024")).unwrap();
    assert_eq!(s.selected(), Some(y2024));
    s.move_selection(Move::Left);
    assert_eq!(s.selected(), Some(y2024));
    assert!(!s.rows(s.row_of(y2024).unwrap(), 1)[0].expanded);
    s.move_selection(Move::Right);
    assert!(s.rows(s.row_of(y2024).unwrap(), 1)[0].expanded);
    s.move_selection(Move::Home);
    assert_eq!(s.selected(), Some(s.with_tree(|t| t.root()).unwrap()));

    // The treemap covers the view and every leaf is inside it.
    let tiles = s.layout(400.0, 300.0);
    assert!(!tiles.is_empty());
    let leaf_area: f64 = tiles.iter().filter(|t| t.leaf).map(|t| t.rect.area()).sum();
    assert!(leaf_area > 400.0 * 300.0 * 0.9, "{leaf_area}");
    for t in tiles.iter() {
        assert!(t.rect.x >= -1e-6 && t.rect.y >= -1e-6);
        assert!(t.rect.x + t.rect.w <= 400.0 + 1e-6);
        assert!(t.rect.y + t.rect.h <= 300.0 + 1e-6);
    }
    let clip = s.find(&r.join("Video/clip.mov")).unwrap();
    let ct = tiles.iter().find(|t| t.id == clip).unwrap();
    let (cx, cy) = (ct.rect.x + ct.rect.w / 2.0, ct.rect.y + ct.rect.h / 2.0);
    assert_eq!(s.hit(cx, cy), Some(clip));
    // cr3 (500 KB) is the biggest type, mov (400 KB) the second.
    assert_eq!(ct.color, 2);
    let pal = s.palette();
    assert_eq!(
        s.with_tree(|t| t.ext_name(pal[0]).to_string()).unwrap(),
        "cr3"
    );

    // Zoom in to Photos: tiles only from below it.
    s.set_zoom(photos);
    let tiles = s.layout(400.0, 300.0);
    s.with_tree(|t| assert!(tiles.iter().all(|x| t.is_within(x.id, photos))));
    let types = s.types();
    assert_eq!(types[0].ext_name, "cr3");
    assert_eq!(types[0].count, 2);
    assert!(s.zoom_out());
    assert!(!s.zoom_out());

    // Logical vs allocated mode both work.
    s.set_mode(SizeMode::Logical);
    assert_eq!(
        s.with_tree(|t| t.node(t.root()).size(SizeMode::Logical))
            .unwrap(),
        950_100
    );
}

#[test]
fn trash_and_undo_keep_sizes_right() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(&r.join("keep/a.bin"), 1_000);
    write(&r.join("gone/b.bin"), 5_000);
    write(&r.join("gone/c.bin"), 5_000);
    let s = UsageSession::start(r, opts(), Arc::new(NullObserver));
    wait(&s);
    let b = s.find(&r.join("gone/b.bin")).unwrap();
    s.select(Some(b));
    // "Trash" it (a rename out of the tree stands in for the Trash here).
    let away = tempfile::tempdir().unwrap();
    fs::rename(r.join("gone/b.bin"), away.path().join("b.bin")).unwrap();
    s.sync_paths(&[r.join("gone/b.bin")]);
    let logical = |s: &UsageSession| s.with_tree(|t| t.node(t.root()).logical).unwrap();
    assert_eq!(logical(&s), 6_000);
    assert_eq!(s.find(&r.join("gone/b.bin")), None);
    // Selection moved to the nearest remaining folder.
    assert_eq!(s.selected(), s.find(&r.join("gone")));
    // "Undo": it comes back, and its folder is rescanned.
    fs::rename(away.path().join("b.bin"), r.join("gone/b.bin")).unwrap();
    s.sync_paths(&[r.join("gone/b.bin")]);
    wait_for(|| s.rescans() >= 1);
    assert_eq!(logical(&s), 11_000);
    assert!(s.find(&r.join("gone/b.bin")).is_some());
    // Removing a whole folder.
    let gone = s.find(&r.join("gone")).unwrap();
    assert!(s.remove(gone));
    assert_eq!(logical(&s), 1_000);
    assert_eq!(s.with_tree(|t| t.node(t.root()).items).unwrap(), 2);
}

#[test]
fn refresh_rescans_one_folder() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(&r.join("sub/a.bin"), 100);
    write(&r.join("other/z.bin"), 7);
    let s = UsageSession::start(r, opts(), Arc::new(NullObserver));
    wait(&s);
    write(&r.join("sub/new/b.bin"), 900);
    let sub = s.find(&r.join("sub")).unwrap();
    s.refresh(sub);
    wait_for(|| s.rescans() >= 1);
    s.with_tree(|t| {
        assert_eq!(t.node(t.root()).logical, 1_007);
        assert_eq!(t.node(t.root()).items, 6);
        let sub = t.find(&r.join("sub")).unwrap();
        assert_eq!(t.node(sub).logical, 1_000);
        // Largest first after the rescan.
        assert_eq!(t.node(t.children(t.root())[0]).name_lossy(), "sub");
    })
    .unwrap();
}

/// A big synthetic tree. Small by default; set `FSTUDIO_USAGE_BIG=1` for
/// 120k files.
#[test]
fn big_tree_timing() {
    let big = std::env::var("FSTUDIO_USAGE_BIG").is_ok();
    let (dirs, per) = if big { (1_200, 100) } else { (100, 50) };
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    for i in 0..dirs {
        let dir: PathBuf = r.join(format!("g{}/d{i}", i % 30));
        fs::create_dir_all(&dir).unwrap();
        for j in 0..per {
            fs::write(dir.join(format!("f{j}.dat")), [1u8; 3]).unwrap();
        }
    }
    for method in [Method::Auto, Method::Portable] {
        let o = ScanOptions { method, ..opts() };
        let t0 = Instant::now();
        let t = scan(r, &o, &ScanControl::new()).unwrap();
        let el = t0.elapsed();
        let n = t.node(t.root()).items;
        assert_eq!(n as usize, dirs * per + dirs + 30);
        eprintln!("{method:?}: {n} items in {el:?}");
    }
}

use fstudio_fs::*;
use fstudio_model::{Entry, EntryKind, file_flags};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn by_name(v: Vec<Entry>) -> BTreeMap<String, Entry> {
    v.into_iter().map(|e| (e.name.clone(), e)).collect()
}

#[test]
fn lists_basic_entries_and_matches_stat() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path();
    fs::write(d.join("a.txt"), b"hello").unwrap();
    fs::create_dir(d.join("sub")).unwrap();
    fs::write(d.join(".hidden"), b"").unwrap();
    symlink("a.txt", d.join("link")).unwrap();
    let all = by_name(list_all(d).unwrap());
    assert_eq!(all.len(), 4);
    assert_eq!(all["a.txt"].kind, EntryKind::File);
    assert_eq!(all["a.txt"].size, 5);
    assert_eq!(all["sub"].kind, EntryKind::Dir);
    assert_eq!(all["link"].kind, EntryKind::Symlink);
    assert!(all[".hidden"].is_hidden());
    // The bulk lister, the readdir fallback and lstat must agree on every field.
    let fb = by_name({
        let mut l = Lister::open_readdir(d).unwrap();
        let mut v = Vec::new();
        while let Some(c) = l.next_chunk() {
            v.extend(c.unwrap());
        }
        v
    });
    for (name, e) in &all {
        let s = stat_entry(d.join(name)).unwrap();
        assert_eq!(e, &s, "bulk vs lstat for {name}");
        assert_eq!(e, &fb[name], "bulk vs fallback for {name}");
        assert!(e.btime_ns.is_some());
        assert!(e.mtime_ns > 1_600_000_000_000_000_000);
    }
    assert!(
        all["a.txt"].alloc_size.unwrap() >= 4096
            || all["a.txt"].alloc_size == Some(0)
            || all["a.txt"].alloc_size.is_some()
    );
    assert_eq!(
        all["a.txt"].mode & 0o777,
        fs::metadata(d.join("a.txt")).unwrap().permissions().mode() & 0o777
    );
}

#[test]
fn streams_in_chunks_and_never_repeats() {
    let t = tempfile::tempdir().unwrap();
    for i in 0..3000 {
        fs::write(
            t.path().join(format!("some-longer-file-name-{i:05}.dat")),
            b"",
        )
        .unwrap();
    }
    let mut l = Lister::open(t.path()).unwrap();
    let mut chunks = 0;
    let mut names = std::collections::HashSet::new();
    while let Some(c) = l.next_chunk() {
        chunks += 1;
        for e in c.unwrap() {
            assert!(names.insert(e.name));
        }
    }
    assert_eq!(names.len(), 3000);
    // The chunk-count check below is macOS-only (getattrlistbulk); this keeps Linux lint-clean.
    #[cfg(not(target_os = "macos"))]
    let _ = chunks;
    #[cfg(target_os = "macos")]
    assert!(
        chunks > 1,
        "first call uses a small buffer so there are several chunks"
    );
    assert!(l.next_chunk().is_none());
    assert!(l.skipped().is_empty());
}

#[test]
fn names_are_returned_exactly_as_stored() {
    let t = tempfile::tempdir().unwrap();
    let nfd = "Cafe\u{301} NFD.txt";
    let nfc = "Caf\u{e9} NFC.txt";
    fs::write(t.path().join(nfd), b"").unwrap();
    fs::write(t.path().join(nfc), b"").unwrap();
    fs::write(t.path().join("MiXeD Case.TXT"), b"").unwrap();
    let all = by_name(list_all(t.path()).unwrap());
    assert!(all.contains_key(nfd), "NFD name kept as NFD");
    assert!(all.contains_key(nfc), "NFC name kept as NFC");
    assert!(all.contains_key("MiXeD Case.TXT"));
    assert_eq!(all.len(), 3);
}

/// Needs a case-sensitive volume, so it makes a small disk image. Run with `--ignored`.
#[test]
#[ignore = "creates and attaches a disk image with hdiutil"]
fn case_variants_on_a_case_sensitive_volume() {
    let t = tempfile::tempdir().unwrap();
    let img = t.path().join("cs.dmg");
    let mnt = t.path().join("mnt");
    fs::create_dir(&mnt).unwrap();
    let ok = Command::new("hdiutil")
        .args([
            "create",
            "-size",
            "10m",
            "-fs",
            "Case-sensitive APFS",
            "-volname",
            "FSTCase",
        ])
        .arg(&img)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let ok = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-mountpoint"])
        .arg(&mnt)
        .arg(&img)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok);
    struct Detach(std::path::PathBuf);
    impl Drop for Detach {
        fn drop(&mut self) {
            let _ = Command::new("hdiutil")
                .args(["detach", "-force"])
                .arg(&self.0)
                .output();
        }
    }
    let _g = Detach(mnt.clone());
    fs::write(mnt.join("file"), b"1").unwrap();
    fs::write(mnt.join("File"), b"2").unwrap();
    fs::write(mnt.join("FILE"), b"3").unwrap();
    let all = by_name(list_all(&mnt).unwrap());
    assert_eq!(all.len(), 3);
    let v = volume_for(&mnt).unwrap();
    assert!(v.case_sensitive);
    assert_eq!(v.fs_type, "apfs");
    assert_eq!(v.mount_path, fs::canonicalize(&mnt).unwrap());
}

#[test]
fn symlinks_are_never_followed() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path();
    fs::create_dir(d.join("real")).unwrap();
    fs::write(d.join("real/inner.txt"), b"x").unwrap();
    symlink(".", d.join("self")).unwrap();
    symlink("..", d.join("real/up")).unwrap();
    symlink("real", d.join("to_real")).unwrap();
    symlink("loop_b", d.join("loop_a")).unwrap();
    symlink("loop_a", d.join("loop_b")).unwrap();
    symlink("missing", d.join("broken")).unwrap();
    let root = by_name(list_all(d).unwrap());
    for n in ["self", "to_real", "loop_a", "loop_b", "broken"] {
        assert_eq!(root[n].kind, EntryKind::Symlink, "{n}");
        assert!(!root[n].is_package);
    }
    assert_eq!(root["real"].kind, EntryKind::Dir);
    let inner = by_name(list_all(d.join("real")).unwrap());
    assert_eq!(inner["up"].kind, EntryKind::Symlink);
    assert_eq!(inner.len(), 2);
    // lstat semantics for single entries too.
    assert_eq!(
        stat_entry(d.join("loop_a")).unwrap().kind,
        EntryKind::Symlink
    );
    assert_eq!(
        stat_entry(d.join("broken")).unwrap().kind,
        EntryKind::Symlink
    );
    // Listing a symlink *to* a directory as the target lists that directory's contents.
    assert_eq!(list_all(d.join("to_real")).unwrap().len(), 2);
}

#[test]
fn permission_denied_and_missing() {
    let t = tempfile::tempdir().unwrap();
    let locked = t.path().join("locked");
    fs::create_dir(&locked).unwrap();
    fs::write(locked.join("x"), b"").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let r = Lister::open(&locked).and_then(|mut l| {
        let mut n = 0;
        while let Some(c) = l.next_chunk() {
            n += c?.len();
        }
        Ok(n)
    });
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if unsafe_is_root() {
        return;
    }
    let e = r.unwrap_err();
    assert_eq!(e.kind(), Some(std::io::ErrorKind::PermissionDenied), "{e}");
    assert!(e.to_string().contains("locked"));
    let e = Lister::open(t.path().join("nope")).err().unwrap();
    assert_eq!(e.kind(), Some(std::io::ErrorKind::NotFound));
    let f = t.path().join("plain");
    fs::write(&f, b"").unwrap();
    assert!(Lister::open(&f).is_err(), "a file is not a directory");
}

fn unsafe_is_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .map(|o| o.stdout == b"0\n")
        .unwrap_or(false)
}

#[test]
fn flags_pass_through_and_dataless_is_a_flag_check() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("h.txt");
    fs::write(&p, b"x").unwrap();
    fs::write(t.path().join("v.txt"), b"x").unwrap();
    if Command::new("chflags")
        .arg("hidden")
        .arg(&p)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        let e = stat_entry(&p).unwrap();
        assert_ne!(e.flags & file_flags::HIDDEN, 0);
        assert!(e.is_hidden());
        let l = by_name(list_all(t.path()).unwrap());
        assert_eq!(l["h.txt"].flags, e.flags);
        assert!(!l["v.txt"].is_hidden());
    }
    let mut e = stat_entry(t.path().join("v.txt")).unwrap();
    assert!(!e.is_dataless());
    e.flags |= file_flags::DATALESS;
    assert!(e.is_dataless());
}

#[test]
fn package_detection() {
    let t = tempfile::tempdir().unwrap();
    for n in [
        "Safari.app",
        "Lib.photoslibrary",
        "Thing.FRAMEWORK",
        "plain",
        "a.d",
        "x.xcodeproj",
    ] {
        fs::create_dir(t.path().join(n)).unwrap();
    }
    fs::write(t.path().join("file.app"), b"").unwrap();
    let l = by_name(list_all(t.path()).unwrap());
    for n in [
        "Safari.app",
        "Lib.photoslibrary",
        "Thing.FRAMEWORK",
        "x.xcodeproj",
    ] {
        assert!(l[n].is_package, "{n}");
    }
    for n in ["plain", "a.d", "file.app"] {
        assert!(!l[n].is_package, "{n}");
    }
    assert!(stat_entry(t.path().join("Safari.app")).unwrap().is_package);
    // The Finder "has bundle" bit marks an oddly named folder as a package.
    // macOS-only: `com.apple.FinderInfo` is a macOS attribute (Linux needs a `user.` name).
    #[cfg(target_os = "macos")]
    {
        fs::create_dir(t.path().join("Odd")).unwrap();
        let mut info = [0u8; 32];
        info[8] = 0x20; // kHasBundle
        xattr_set(t.path().join("Odd"), "com.apple.FinderInfo", &info).unwrap();
        let l = by_name(list_all(t.path()).unwrap());
        assert!(l["Odd"].is_package);
    }
}

// macOS-only as written: the `com.example.*` names are not valid on Linux, which only allows
// the `user.` namespace. TODO: add a Linux variant of this test using `user.` names.
#[cfg(target_os = "macos")]
#[test]
fn xattr_round_trip() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("f");
    fs::write(&p, b"").unwrap();
    // macOS may add its own attributes (e.g. com.apple.provenance) to new files.
    let base = xattr_list(&p).unwrap();
    assert_eq!(xattr_get(&p, "user.test").unwrap(), None);
    xattr_set(&p, "com.example.a", b"one").unwrap();
    xattr_set(&p, "com.example.b", b"").unwrap();
    let mut l = xattr_list(&p).unwrap();
    l.retain(|n| !base.contains(n));
    l.sort();
    assert_eq!(l, ["com.example.a", "com.example.b"]);
    assert_eq!(xattr_get(&p, "com.example.a").unwrap().unwrap(), b"one");
    assert_eq!(xattr_get(&p, "com.example.b").unwrap().unwrap(), b"");
    assert!(xattr_remove(&p, "com.example.a").unwrap());
    assert!(!xattr_remove(&p, "com.example.a").unwrap());
    assert!(
        !xattr_list(&p)
            .unwrap()
            .contains(&"com.example.a".to_string())
    );
    // Attributes on a symlink belong to the link, not its target.
    let ln = t.path().join("ln");
    symlink("f", &ln).unwrap();
    assert!(
        !xattr_list(&ln)
            .unwrap()
            .contains(&"com.example.b".to_string())
    );
}

#[cfg(target_os = "macos")]
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(target_os = "macos")]
#[test]
fn finder_tags_round_trip_with_system_tools() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("tagged.txt");
    fs::write(&p, b"").unwrap();
    assert!(finder_tags(&p).unwrap().is_empty());
    let tags = vec![
        FinderTag::new("Work", TagColor::None),
        FinderTag::new("Urgent", TagColor::Red),
        FinderTag::new("Café", TagColor::Blue),
    ];
    set_finder_tags(&p, &tags).unwrap();
    assert_eq!(finder_tags(&p).unwrap(), tags);

    // The system's own xattr tool sees a binary plist Finder can read: convert it with plutil.
    let out = Command::new("xattr")
        .args(["-px", "com.apple.metadata:_kMDItemUserTags"])
        .arg(&p)
        .output()
        .unwrap();
    assert!(out.status.success());
    let hexs: String = String::from_utf8(out.stdout)
        .unwrap()
        .split_whitespace()
        .collect();
    let bytes: Vec<u8> = (0..hexs.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hexs[i..i + 2], 16).unwrap())
        .collect();
    assert!(bytes.starts_with(b"bplist00"));
    let raw = p.with_extension("bplist");
    fs::write(&raw, &bytes).unwrap();
    let json = Command::new("plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(&raw)
        .output()
        .unwrap();
    assert!(json.status.success(), "plutil accepts our plist");
    assert_eq!(
        String::from_utf8(json.stdout).unwrap(),
        "[\"Work\",\"Urgent\\n6\",\"Café\\n4\"]"
    );

    // Finder's label colour in FinderInfo is the last coloured tag (blue = 4).
    let fi = xattr_get(&p, "com.apple.FinderInfo").unwrap().unwrap();
    assert_eq!(fi.len(), 32);
    assert_eq!((fi[9] >> 1) & 7, 4);

    // And the reverse: a plist written by system tools is read correctly.
    let src = t.path().join("src.json");
    fs::write(&src, "[\"Green\\n2\",\"Plain\"]").unwrap();
    let q = t.path().join("q.txt");
    fs::write(&q, b"").unwrap();
    let bp = t.path().join("src.bplist");
    assert!(
        Command::new("plutil")
            .args(["-convert", "binary1", "-o"])
            .arg(&bp)
            .arg(&src)
            .status()
            .unwrap()
            .success()
    );
    let h = hex(&fs::read(&bp).unwrap());
    assert!(
        Command::new("xattr")
            .args(["-wx", "com.apple.metadata:_kMDItemUserTags", &h])
            .arg(&q)
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        finder_tags(&q).unwrap(),
        vec![
            FinderTag::new("Green", TagColor::Green),
            FinderTag::new("Plain", TagColor::None)
        ]
    );

    // Clearing removes both attributes.
    set_finder_tags(&p, &[]).unwrap();
    assert!(finder_tags(&p).unwrap().is_empty());
    assert_eq!(xattr_get(&p, "com.apple.FinderInfo").unwrap(), None);
    assert!(
        !xattr_list(&p)
            .unwrap()
            .iter()
            .any(|n| n.contains("UserTags"))
    );
}

#[cfg(target_os = "macos")]
#[test]
fn finder_tags_keep_other_finder_info() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("f");
    fs::write(&p, b"").unwrap();
    let mut info = [0u8; 32];
    info[8] = 0x40; // some other Finder flag
    xattr_set(&p, "com.apple.FinderInfo", &info).unwrap();
    set_finder_tags(&p, &[FinderTag::new("R", TagColor::Red)]).unwrap();
    let fi = xattr_get(&p, "com.apple.FinderInfo").unwrap().unwrap();
    assert_eq!(fi[8], 0x40);
    assert_eq!((fi[9] >> 1) & 7, 6);
    set_finder_tags(&p, &[]).unwrap();
    let fi = xattr_get(&p, "com.apple.FinderInfo").unwrap().unwrap();
    assert_eq!(fi[8], 0x40);
    assert_eq!(fi[9] & 0x0E, 0);
}

#[test]
fn volumes_describe_the_filesystem() {
    let t = tempfile::tempdir().unwrap();
    let v = volume_for(t.path()).unwrap();
    assert!(v.mount_path.is_absolute());
    assert!(v.total_bytes > 0 && v.free_bytes <= v.total_bytes);
    assert!(!v.is_network);
    let e = stat_entry(t.path()).unwrap();
    assert_eq!(
        v.device_id, e.dev,
        "device id matches the st_dev of its files"
    );
    let all = volumes().unwrap();
    assert!(all.iter().any(|x| x.mount_path == Path::new("/")));
    assert!(all.iter().any(|x| x.device_id == v.device_id) || v.mount_path.starts_with("/System"));
    #[cfg(target_os = "macos")]
    {
        let root = all.iter().find(|x| x.mount_path == Path::new("/")).unwrap();
        eprintln!("{root:#?}");
        assert_eq!(root.fs_type, "apfs");
        assert!(root.supports_clone);
        assert!(!root.name.is_empty());
        assert_ne!(
            root.name, "/",
            "the root volume has a real name (Macintosh HD)"
        );
        assert!(root.is_internal);
    }
}

fn wait_for(
    rx: &crossbeam_channel::Receiver<Vec<FsEvent>>,
    within: Duration,
    f: impl Fn(&FsEvent) -> bool,
) -> Option<Duration> {
    let start = Instant::now();
    while let Some(left) = within.checked_sub(start.elapsed()) {
        if let Ok(batch) = rx.recv_timeout(left) {
            if batch.iter().any(&f) {
                return Some(start.elapsed());
            }
        } else {
            break;
        }
    }
    None
}

#[cfg(target_os = "macos")]
#[test]
fn fsevents_latency_under_100ms() {
    let t = tempfile::tempdir().unwrap();
    let dir = fs::canonicalize(t.path()).unwrap();
    let mut w = FsEventsWatcher::new(WatchConfig::default());
    w.set_paths(std::slice::from_ref(&dir)).unwrap();
    let rx = w.events();
    std::thread::sleep(Duration::from_millis(400)); // let the stream settle
    let mut times = Vec::new();
    for i in 0..7 {
        let p = dir.join(format!("f{i}.txt"));
        let start = Instant::now();
        fs::write(&p, b"x").unwrap();
        let got = wait_for(&rx, Duration::from_secs(2), |e| e.path == p).expect("event arrives");
        times.push(got.max(start.elapsed().min(got)));
        std::thread::sleep(Duration::from_millis(60));
    }
    times.sort();
    eprintln!("write -> event: {times:?}");
    assert!(
        times[times.len() / 2] < Duration::from_millis(100),
        "median {:?}",
        times[times.len() / 2]
    );
}

#[cfg(target_os = "macos")]
#[test]
fn fsevents_kinds_filter_and_paths() {
    let t = tempfile::tempdir().unwrap();
    let dir = fs::canonicalize(t.path()).unwrap();
    fs::create_dir(dir.join("deep")).unwrap();
    let mut w = FsEventsWatcher::new(WatchConfig::default());
    w.set_paths(std::slice::from_ref(&dir)).unwrap();
    let rx = w.events();
    std::thread::sleep(Duration::from_millis(400));
    // A change two levels down is not reported for a non-recursive watch.
    fs::write(dir.join("deep/inner.txt"), b"x").unwrap();
    let p = dir.join("top.txt");
    fs::write(&p, b"x").unwrap();
    let mut seen = Vec::new();
    let end = Instant::now() + Duration::from_millis(600);
    while Instant::now() < end {
        if let Ok(b) = rx.recv_timeout(Duration::from_millis(50)) {
            seen.extend(b);
        }
    }
    assert!(
        seen.iter()
            .any(|e| e.path == p && e.kind == FsEventKind::Created),
        "{seen:?}"
    );
    assert!(
        !seen.iter().any(|e| e.path.ends_with("inner.txt")),
        "{seen:?}"
    );
    // Removal.
    fs::remove_file(&p).unwrap();
    assert!(
        wait_for(&rx, Duration::from_secs(1), |e| e.path == p
            && e.kind == FsEventKind::Removed)
        .is_some()
    );
    // Dropping the folder from the watched set stops its events.
    w.set_paths(&[]).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    while rx.try_recv().is_ok() {}
    fs::write(dir.join("later.txt"), b"x").unwrap();
    assert!(wait_for(&rx, Duration::from_millis(300), |_| true).is_none());
}

#[cfg(target_os = "macos")]
#[test]
fn fsevents_root_change_requests_rescan() {
    let t = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(t.path()).unwrap();
    let dir = base.join("watched");
    fs::create_dir(&dir).unwrap();
    let mut w = FsEventsWatcher::new(WatchConfig::default());
    w.set_paths(std::slice::from_ref(&dir)).unwrap();
    let rx = w.events();
    std::thread::sleep(Duration::from_millis(400));
    fs::rename(&dir, base.join("moved")).unwrap();
    assert!(
        wait_for(&rx, Duration::from_secs(2), |e| e.must_rescan).is_some(),
        "root move asks for a rescan"
    );
}

#[test]
fn poll_watcher_reports_changes() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().to_path_buf();
    fs::write(dir.join("old.txt"), b"1").unwrap();
    let mut w = PollWatcher::new(WatchConfig {
        latency: Duration::from_millis(30),
        ..Default::default()
    });
    w.set_paths(std::slice::from_ref(&dir)).unwrap();
    let rx = w.events();
    std::thread::sleep(Duration::from_millis(150));
    fs::write(dir.join("new.txt"), b"1").unwrap();
    assert!(
        wait_for(&rx, Duration::from_secs(2), |e| e.path.ends_with("new.txt")
            && e.kind == FsEventKind::Created)
        .is_some()
    );
    fs::remove_file(dir.join("old.txt")).unwrap();
    assert!(
        wait_for(&rx, Duration::from_secs(2), |e| e.path.ends_with("old.txt")
            && e.kind == FsEventKind::Removed)
        .is_some()
    );
    let _ = default_watcher(WatchConfig::default()).unwrap();
}

#[test]
#[ignore = "creates 100k files; run with --ignored"]
fn hundred_thousand_entries() {
    let t = tempfile::tempdir().unwrap();
    for i in 0..100_000 {
        fs::write(t.path().join(format!("file_{i:06}.dat")), b"").unwrap();
    }
    let start = Instant::now();
    let mut l = Lister::open(t.path()).unwrap();
    let first = l.next_chunk().unwrap().unwrap();
    eprintln!(
        "first chunk: {} entries in {:?}",
        first.len(),
        start.elapsed()
    );
    let mut n = first.len();
    while let Some(c) = l.next_chunk() {
        n += c.unwrap().len();
    }
    eprintln!("all {n} in {:?}", start.elapsed());
    assert_eq!(n, 100_000);
}

/// S5 (safety audit): after a case-only or NFD → NFC rename, the old
/// spelling still `lstat`s on an insensitive volume, but the entry is named
/// as the disk stores it, so a watcher patch can't bring the old name back.
#[test]
fn stat_entry_names_the_item_as_stored_on_disk() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    std::fs::write(dir.join("Notes.txt"), b"x").unwrap();
    std::fs::rename(dir.join("Notes.txt"), dir.join("NOTES.txt")).unwrap();
    let sensitive = std::fs::symlink_metadata(dir.join("Notes.txt")).is_err();
    if !sensitive {
        let e = fstudio_fs::stat_entry(dir.join("Notes.txt")).unwrap();
        assert_eq!(e.name, "NOTES.txt");
        assert_eq!(
            fstudio_fs::on_disk_name(dir.join("notes.TXT")).unwrap(),
            "NOTES.txt"
        );
    }
    std::fs::write(dir.join("Cafe\u{301}.txt"), b"x").unwrap();
    assert_eq!(
        fstudio_fs::on_disk_name(dir.join("Cafe\u{301}.txt")).unwrap(),
        "Cafe\u{301}.txt"
    );
    std::fs::rename(dir.join("Cafe\u{301}.txt"), dir.join("Caf\u{e9}.txt")).unwrap();
    if !sensitive {
        let e = fstudio_fs::stat_entry(dir.join("Cafe\u{301}.txt")).unwrap();
        assert_eq!(e.name, "Caf\u{e9}.txt");
    }
    // A symlink is named, not followed.
    std::os::unix::fs::symlink("NOTES.txt", dir.join("link")).unwrap();
    assert_eq!(fstudio_fs::on_disk_name(dir.join("link")).unwrap(), "link");
    assert!(fstudio_fs::on_disk_name(dir.join("missing")).is_err());
}

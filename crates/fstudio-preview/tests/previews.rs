//! End-to-end previews over temporary files.

use std::os::unix::fs::PermissionsExt;

use fstudio_preview::*;

fn opts() -> PreviewOptions {
    PreviewOptions::default()
}

fn summarize(p: &Previewer, path: &std::path::Path) -> std::sync::Arc<Preview> {
    p.summarize(path, &opts(), &CancelToken::new())
        .expect("preview")
}

#[test]
fn text_file_has_basics_and_lines() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("notes.md");
    std::fs::write(&f, "# Title\nbody\n").unwrap();
    let pr = summarize(&Previewer::new(), &f);
    assert_eq!(pr.category, Category::Text);
    assert_eq!(pr.size, 13);
    assert_eq!(pr.name, "notes.md");
    assert!(pr.modified_ns > 0);
    assert_eq!(pr.path, f.to_string_lossy());
    assert_eq!(pr.text.as_ref().unwrap().line_count, 2);
}

#[test]
fn nfd_names_are_kept_exactly() {
    let d = tempfile::tempdir().unwrap();
    let name = "Cafe\u{301}.txt"; // decomposed é
    let f = d.path().join(name);
    std::fs::write(&f, "x").unwrap();
    let pr = summarize(&Previewer::new(), &f);
    assert_eq!(pr.name.as_bytes(), name.as_bytes());
}

#[test]
fn folder_counts_visible_items_and_stops_at_the_limit() {
    let d = tempfile::tempdir().unwrap();
    for i in 0..5 {
        std::fs::write(d.path().join(format!("f{i}")), "").unwrap();
    }
    std::fs::write(d.path().join(".hidden"), "").unwrap();
    let pr = summarize(&Previewer::new(), d.path());
    assert_eq!(pr.category, Category::Folder);
    assert_eq!(pr.folder.unwrap().items, 5);
    let small = PreviewOptions {
        folder_count_limit: 3,
        ..opts()
    };
    let pr = Previewer::new()
        .summarize(d.path(), &small, &CancelToken::new())
        .unwrap();
    let info = pr.folder.unwrap();
    assert!(info.at_least && info.items == 3);
}

#[test]
fn placeholder_is_never_opened() {
    // The file holds a corrupt JPEG that would be read if the flag were ignored;
    // the stat is rigged to say SF_DATALESS.
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("cloud.jpg");
    std::fs::write(&f, b"\xff\xd8garbage").unwrap();
    let mut st = FileStat::of(&f).unwrap();
    st.dataless = true;
    let pr = Previewer::new()
        .summarize_with(&f, st, &opts(), &CancelToken::new())
        .unwrap();
    assert!(pr.dataless);
    assert!(pr.photo.is_none() && pr.text.is_none() && pr.video.is_none());
    assert!(pr.note.as_deref().unwrap().contains("iCloud"));
    // A placeholder folder isn't listed either.
    let mut st = FileStat::of(d.path()).unwrap();
    st.dataless = true;
    let pr = Previewer::new()
        .summarize_with(d.path(), st, &opts(), &CancelToken::new())
        .unwrap();
    assert!(pr.folder.is_none());
}

#[test]
fn dataless_flag_is_read_from_st_flags() {
    // A normal file is not a placeholder (the flag itself can't be set in a test).
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("a.txt");
    std::fs::write(&f, "x").unwrap();
    assert!(!FileStat::of(&f).unwrap().dataless);
}

#[test]
fn unreadable_file_and_folder_report_instead_of_failing() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("secret.txt");
    std::fs::write(&f, "hidden").unwrap();
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o000)).unwrap();
    let sub = d.path().join("locked");
    std::fs::create_dir(&sub).unwrap();
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o000)).unwrap();
    let p = Previewer::new();
    let pr = summarize(&p, &f);
    // (Running as root can read it; then there is simply text.)
    assert!(pr.text.is_some() || pr.note.is_some());
    let pr = summarize(&p, &sub);
    assert!(pr.folder.map(|f| f.unreadable).unwrap_or(true) || pr.folder.unwrap().items == 0);
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn cache_hits_until_the_file_changes() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("a.txt");
    std::fs::write(&f, "one\n").unwrap();
    let p = Previewer::new();
    assert!(p.cached(&f, &opts()).is_none());
    let a = summarize(&p, &f);
    let b = summarize(&p, &f);
    assert!(std::sync::Arc::ptr_eq(&a, &b));
    assert!(p.cached(&f, &opts()).is_some());
    std::fs::write(&f, "one\ntwo\n").unwrap();
    let c = summarize(&p, &f);
    assert_eq!(c.text.as_ref().unwrap().line_count, 2);
    // Different options are a different entry.
    let few = PreviewOptions {
        max_text_lines: 1,
        ..opts()
    };
    assert!(p.cached(&f, &few).is_none());
}

#[test]
fn cancelled_previews_return_nothing_and_are_not_cached() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("a.txt");
    std::fs::write(&f, "x\n").unwrap();
    let p = Previewer::new();
    let c = CancelToken::new();
    c.cancel();
    assert!(p.summarize(&f, &opts(), &c).is_none());
    assert!(p.cached(&f, &opts()).is_none());
}

#[test]
fn symlinks_describe_their_target_and_loops_survive() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("real.txt");
    std::fs::write(&f, "a\nb\n").unwrap();
    let l = d.path().join("link.txt");
    std::os::unix::fs::symlink(&f, &l).unwrap();
    let pr = summarize(&Previewer::new(), &l);
    assert!(pr.is_symlink);
    assert_eq!(pr.text.as_ref().unwrap().line_count, 2);
    let a = d.path().join("a");
    let b = d.path().join("b");
    std::os::unix::fs::symlink(&b, &a).unwrap();
    std::os::unix::fs::symlink(&a, &b).unwrap();
    let _ = Previewer::new().summarize(&a, &opts(), &CancelToken::new());
}

#[test]
fn corrupt_media_with_media_names_is_harmless() {
    let d = tempfile::tempdir().unwrap();
    let p = Previewer::new();
    for name in [
        "x.jpg", "x.CR3", "x.arw", "x.heic", "x.mov", "x.mp4", "x.dng", "x.png",
    ] {
        let f = d.path().join(name);
        std::fs::write(&f, [0u8, 0, 0, 20, b'f', b't', b'y', b'p', 1, 2, 3]).unwrap();
        let pr = summarize(&p, &f);
        assert!(pr.photo.is_none() && pr.video.is_none(), "{name}");
    }
}

#[test]
#[ignore = "set FSTUDIO_PREVIEW_SAMPLES to a folder of raws/photos/videos to time them"]
fn time_real_samples() {
    let Ok(dir) = std::env::var("FSTUDIO_PREVIEW_SAMPLES") else {
        return;
    };
    let p = Previewer::new();
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    names.sort();
    for path in names.into_iter().filter(|p| p.is_file()) {
        let t = std::time::Instant::now();
        let pr = p.summarize(&path, &opts(), &CancelToken::new()).unwrap();
        println!(
            "{:>7.1} ms  {}  {:?}\n    photo={:?}\n    video={:?}\n    note={:?}",
            t.elapsed().as_secs_f64() * 1000.0,
            pr.name,
            pr.category,
            pr.photo,
            pr.video,
            pr.note
        );
    }
}

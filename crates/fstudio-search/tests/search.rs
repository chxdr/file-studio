//! End-to-end searches over trees built in temporary folders.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fstudio_search::*;

fn write(p: &Path, body: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// Runs to the end; returns relative paths found, the denied folders and the summary.
fn run_with(
    q: SearchQuery,
    index: Option<Arc<dyn IndexSource>>,
) -> (BTreeSet<String>, Vec<PathBuf>, Summary, Vec<Hit>) {
    let root = q.roots[0].clone();
    let (_h, rx) = start(q, index).unwrap();
    let (mut found, mut denied, mut hits) = (BTreeSet::new(), Vec::new(), Vec::new());
    for e in rx.iter() {
        match e {
            SearchEvent::Hits(v) => {
                for h in v {
                    let rel = h
                        .path
                        .strip_prefix(&root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned();
                    assert!(found.insert(rel.clone()), "duplicate result {rel}");
                    hits.push(h);
                }
            }
            SearchEvent::Denied { path, .. } => denied.push(path),
            SearchEvent::IndexUnavailable(_) => {}
            SearchEvent::Finished(s) => return (found, denied, s, hits),
        }
    }
    panic!("no Finished event");
}

fn run(q: SearchQuery) -> BTreeSet<String> {
    run_with(
        SearchQuery {
            use_index: false,
            ..q
        },
        None,
    )
    .0
}

fn set(v: &[&str]) -> BTreeSet<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn tree() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(
        &r.join("Report 2026.txt"),
        "quarterly numbers: Café revenue",
    );
    write(&r.join("docs/report-draft.md"), "draft");
    write(&r.join("docs/deep/er/notes.txt"), "nothing here");
    write(&r.join("photos/IMG_0001.JPG"), "jpeg");
    write(&r.join("photos/IMG_0002.png"), "png");
    write(&r.join("photos/big.png"), &"x".repeat(10_000));
    write(&r.join(".hidden/report-secret.txt"), "s");
    write(&r.join(".report-dot.txt"), "s");
    write(&r.join("Tool.app/Contents/report-inside.txt"), "s");
    write(&r.join("bin/report.bin"), "report\0binary");
    d
}

#[test]
fn name_contains_skips_hidden_and_packages() {
    let d = tree();
    let q = SearchQuery {
        name: "REPORT".into(),
        ..SearchQuery::in_folder(d.path())
    };
    assert_eq!(
        run(q.clone()),
        set(&["Report 2026.txt", "docs/report-draft.md", "bin/report.bin"])
    );
    let all = SearchQuery {
        include_hidden: true,
        include_packages: true,
        ..q
    };
    assert_eq!(
        run(all),
        set(&[
            "Report 2026.txt",
            "docs/report-draft.md",
            "bin/report.bin",
            ".hidden/report-secret.txt",
            ".report-dot.txt",
            "Tool.app/Contents/report-inside.txt",
        ])
    );
}

#[test]
fn glob_and_regex_modes() {
    let d = tree();
    let g = SearchQuery {
        name: "img_????.*".into(),
        name_mode: NameMode::Glob,
        ..SearchQuery::in_folder(d.path())
    };
    assert_eq!(run(g), set(&["photos/IMG_0001.JPG", "photos/IMG_0002.png"]));
    let r = SearchQuery {
        name: r"\.(png|jpg)$".into(),
        name_mode: NameMode::Regex,
        ..SearchQuery::in_folder(d.path())
    };
    assert_eq!(
        run(r),
        set(&[
            "photos/IMG_0001.JPG",
            "photos/IMG_0002.png",
            "photos/big.png"
        ])
    );
    let bad = SearchQuery {
        name: "([".into(),
        name_mode: NameMode::Regex,
        ..SearchQuery::in_folder(d.path())
    };
    assert!(matches!(start(bad, None), Err(QueryError::BadPattern(_))));
}

#[test]
fn kind_size_and_date_filters() {
    let d = tree();
    let k = SearchQuery {
        kinds: vec![Kind::Image],
        ..SearchQuery::in_folder(d.path())
    };
    assert_eq!(
        run(k),
        set(&[
            "photos/IMG_0001.JPG",
            "photos/IMG_0002.png",
            "photos/big.png"
        ])
    );
    let folders = SearchQuery {
        kinds: vec![Kind::Folder],
        name: "e".into(),
        ..SearchQuery::in_folder(d.path())
    };
    assert_eq!(run(folders), set(&["docs/deep", "docs/deep/er"]));
    let apps = SearchQuery {
        kinds: vec![Kind::Application],
        ..SearchQuery::in_folder(d.path())
    };
    assert_eq!(run(apps), set(&["Tool.app"]));

    let s = SearchQuery {
        min_size: Some(5_000),
        ..SearchQuery::in_folder(d.path())
    };
    assert_eq!(run(s), set(&["photos/big.png"]));
    let small = SearchQuery {
        max_size: Some(4),
        name: "img".into(),
        ..SearchQuery::in_folder(d.path())
    };
    assert_eq!(
        run(small),
        set(&["photos/IMG_0001.JPG", "photos/IMG_0002.png"])
    );

    // Back-date one file and search a date range around it.
    let old = d.path().join("docs/report-draft.md");
    let t = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000); // 2001
    std::fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(t)
        .unwrap();
    let ns = 1_000_000_000i64 * 1_000_000_000;
    let range = SearchQuery {
        date_from_ns: Some(ns - 1_000_000_000),
        date_to_ns: Some(ns + 1_000_000_000),
        ..SearchQuery::in_folder(d.path())
    };
    assert_eq!(run(range), set(&["docs/report-draft.md"]));
    let recent = SearchQuery {
        date_from_ns: Some(ns + 1_000_000_000),
        name: "report".into(),
        ..SearchQuery::in_folder(d.path())
    };
    assert_eq!(run(recent), set(&["Report 2026.txt", "bin/report.bin"]));
}

#[test]
fn content_filter_reads_text_only() {
    let d = tree();
    let c = SearchQuery {
        content: Some("CAFÉ REV".into()),
        ..SearchQuery::in_folder(d.path())
    };
    assert_eq!(run(c), set(&["Report 2026.txt"]));
    let bin = SearchQuery {
        content: Some("binary".into()),
        ..SearchQuery::in_folder(d.path())
    };
    assert_eq!(run(bin), set(&[]));
    let capped = SearchQuery {
        content: Some("xxxx".into()),
        content_max_bytes: 100,
        ..SearchQuery::in_folder(d.path())
    };
    assert_eq!(run(capped), set(&[]));
}

#[test]
fn finder_tags_filter() {
    let d = tree();
    let p = d.path().join("photos/IMG_0002.png");
    fstudio_fs::set_finder_tags(
        &p,
        &[
            fstudio_fs::FinderTag::new("Red", fstudio_fs::TagColor::Red),
            fstudio_fs::FinderTag::new("Client Ä", fstudio_fs::TagColor::None),
        ],
    )
    .unwrap();
    let q = SearchQuery {
        tags: vec!["red".into(), "client a\u{308}".into()],
        ..SearchQuery::in_folder(d.path())
    };
    assert_eq!(run(q), set(&["photos/IMG_0002.png"]));
    let none = SearchQuery {
        tags: vec!["Red".into(), "Blue".into()],
        ..SearchQuery::in_folder(d.path())
    };
    assert_eq!(run(none), set(&[]));
}

#[test]
fn nfc_nfd_and_case_only_names() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(&r.join("a/Cafe\u{301} NFD.txt"), "");
    write(&r.join("b/Caf\u{e9} NFC.txt"), "");
    // Names that differ only by case, in two folders (a case-insensitive
    // volume can't hold both in one).
    write(&r.join("c/Notes.TXT"), "");
    write(&r.join("d/notes.txt"), "");
    let q = SearchQuery {
        name: "café".into(),
        ..SearchQuery::in_folder(r)
    };
    let (found, _, _, hits) = run_with(
        SearchQuery {
            use_index: false,
            ..q
        },
        None,
    );
    assert_eq!(found.len(), 2);
    // Names come back exactly as stored.
    let names: BTreeSet<_> = hits.iter().map(|h| h.entry.name.clone()).collect();
    assert!(names.contains("Cafe\u{301} NFD.txt"));
    assert!(names.contains("Caf\u{e9} NFC.txt"));
    let q2 = SearchQuery {
        name: "NOTES.txt".into(),
        ..SearchQuery::in_folder(r)
    };
    assert_eq!(run(q2), set(&["c/Notes.TXT", "d/notes.txt"]));
}

#[test]
fn symlink_loops_end_and_links_are_not_followed() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(&r.join("a/b/target.txt"), "");
    std::os::unix::fs::symlink(r, r.join("a/b/loop-up")).unwrap();
    std::os::unix::fs::symlink(r.join("a"), r.join("a/b/loop-a")).unwrap();
    std::os::unix::fs::symlink("self", r.join("self")).unwrap();
    let start_t = Instant::now();
    let found = run(SearchQuery::in_folder(r));
    assert!(start_t.elapsed() < Duration::from_secs(5));
    assert_eq!(
        found,
        set(&[
            "a",
            "a/b",
            "a/b/loop-up",
            "a/b/loop-a",
            "a/b/target.txt",
            "self"
        ])
    );
}

#[test]
fn permission_denied_folders_are_reported_and_the_walk_goes_on() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(&r.join("locked/inside.txt"), "");
    write(&r.join("open/z/match.txt"), "");
    write(&r.join("zz/match.txt"), "");
    std::fs::set_permissions(r.join("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();
    let q = SearchQuery {
        name: "match".into(),
        use_index: false,
        ..SearchQuery::in_folder(r)
    };
    let (found, denied, sum, _) = run_with(q, None);
    std::fs::set_permissions(r.join("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
    if unsafe_is_root() {
        return; // root can read anything
    }
    assert_eq!(found, set(&["open/z/match.txt", "zz/match.txt"]));
    assert_eq!(denied, vec![r.join("locked")]);
    assert_eq!(sum.denied, 1);
}

fn unsafe_is_root() -> bool {
    std::env::var("USER").as_deref() == Ok("root")
}

#[test]
fn read_only_tree_is_searchable() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(&r.join("ro/x.txt"), "");
    std::fs::set_permissions(r.join("ro"), std::fs::Permissions::from_mode(0o555)).unwrap();
    let found = run(SearchQuery {
        name: "x".into(),
        ..SearchQuery::in_folder(r)
    });
    std::fs::set_permissions(r.join("ro"), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(found, set(&["ro/x.txt"]));
}

fn make_tree(root: &Path, dirs: usize, files_per_dir: usize) -> usize {
    let mut n = 0;
    for i in 0..dirs {
        let dir = root.join(format!("d{:03}/s{}", i, i % 7));
        std::fs::create_dir_all(&dir).unwrap();
        for j in 0..files_per_dir {
            std::fs::write(dir.join(format!("file {i} {j}.txt")), "").unwrap();
            n += 1;
        }
    }
    n
}

#[test]
fn cancel_stops_quickly() {
    let d = tempfile::tempdir().unwrap();
    make_tree(d.path(), 400, 20);
    let q = SearchQuery {
        use_index: false,
        threads: 2,
        ..SearchQuery::in_folder(d.path())
    };
    let (h, rx) = start(q, None).unwrap();
    // Wait for the first batch, then cancel.
    let mut found = 0u64;
    for e in rx.iter() {
        if let SearchEvent::Hits(v) = e {
            found += v.len() as u64;
            break;
        }
    }
    let t = Instant::now();
    h.cancel();
    let mut sum = None;
    for e in rx.iter() {
        match e {
            SearchEvent::Hits(v) => found += v.len() as u64,
            SearchEvent::Finished(s) => {
                sum = Some(s);
                break;
            }
            _ => {}
        }
    }
    let s = sum.expect("finished");
    assert!(
        t.elapsed() < Duration::from_millis(500),
        "{:?}",
        t.elapsed()
    );
    assert!(s.cancelled);
    assert_eq!(s.found, found);
    assert!(
        found < 8_400,
        "cancel came too late to test anything ({found})"
    );
}

#[test]
fn dropping_the_receiver_stops_the_search() {
    let d = tempfile::tempdir().unwrap();
    make_tree(d.path(), 50, 10);
    let (h, rx) = start(
        SearchQuery {
            use_index: false,
            ..SearchQuery::in_folder(d.path())
        },
        None,
    )
    .unwrap();
    drop(rx);
    let t = Instant::now();
    while !h.progress().finished {
        assert!(t.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn max_results_cuts_short() {
    let d = tempfile::tempdir().unwrap();
    make_tree(d.path(), 20, 20);
    let (found, _, sum, _) = run_with(
        SearchQuery {
            use_index: false,
            max_results: 50,
            name: "file".into(),
            ..SearchQuery::in_folder(d.path())
        },
        None,
    );
    assert_eq!(found.len(), 50);
    assert!(sum.truncated);
    assert!(!sum.cancelled);
}

/// An index that returns a fixed list, as Spotlight would (stale entries,
/// duplicates of walk results, paths outside the root, hidden and package
/// insides).
struct FakeIndex(Vec<PathBuf>);

impl IndexSource for FakeIndex {
    fn candidates(
        &self,
        _q: &IndexQuery,
        _c: &CancelToken,
        out: &mut dyn FnMut(PathBuf),
    ) -> Result<(), String> {
        for p in &self.0 {
            out(p.clone());
        }
        Ok(())
    }
}

#[test]
fn index_results_are_rechecked_and_deduplicated() {
    let d = tree();
    let r = d.path().to_path_buf();
    let idx = FakeIndex(vec![
        r.join("Report 2026.txt"),           // real, also found by the walk
        r.join("Report 2026.txt"),           // twice
        r.join("gone-report.txt"),           // stale
        r.join("photos/IMG_0001.JPG"),       // doesn't match the name
        r.join(".hidden/report-secret.txt"), // inside a hidden folder
        r.join("Tool.app/Contents/report-inside.txt"), // inside a package
        PathBuf::from("/elsewhere/report.txt"), // outside the root
        r.clone(),                           // the root itself
    ]);
    let q = SearchQuery {
        name: "report".into(),
        ..SearchQuery::in_folder(&r)
    };
    let (found, _, sum, _) = run_with(q.clone(), Some(Arc::new(idx)));
    assert_eq!(
        found,
        set(&["Report 2026.txt", "docs/report-draft.md", "bin/report.bin"])
    );
    assert!(sum.index_used);

    // Index alone (no walk): only the verified candidates.
    let idx = FakeIndex(vec![r.join("Report 2026.txt"), r.join("gone.txt")]);
    let (found, _, sum, hits) = run_with(SearchQuery { walk: false, ..q }, Some(Arc::new(idx)));
    assert_eq!(found, set(&["Report 2026.txt"]));
    assert_eq!(sum.from_index, 1);
    assert_eq!(hits[0].source, Source::Index);
}

#[test]
fn a_symlinked_root_is_searched_through_its_link() {
    let d = tempfile::tempdir().unwrap();
    write(&d.path().join("real/x.txt"), "");
    let link = d.path().join("link");
    std::os::unix::fs::symlink(d.path().join("real"), &link).unwrap();
    let (found, _, _, hits) = run_with(
        SearchQuery {
            use_index: false,
            ..SearchQuery::in_folder(&link)
        },
        None,
    );
    assert_eq!(found, set(&["x.txt"]));
    assert_eq!(hits[0].path, link.join("x.txt"));
}

/// A big tree (20k files): first result and full walk timings. Run with
/// `cargo test -p fstudio-search --release -- --ignored --nocapture big_tree`.
#[test]
#[ignore]
fn big_tree() {
    let n_dirs: usize = std::env::var("FSTUDIO_SEARCH_DIRS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_000);
    let d = tempfile::tempdir().unwrap();
    let files = make_tree(d.path(), n_dirs, 20);
    for name in ["", "file 999 7", "zzz-nothing"] {
        let t = Instant::now();
        let (found, _, sum, _) = run_with(
            SearchQuery {
                use_index: false,
                name: name.into(),
                max_results: usize::MAX,
                ..SearchQuery::in_folder(d.path())
            },
            None,
        );
        println!(
            "big tree: {files} files, query {name:?}: {} found, first {:?}, all {:?} ({} dirs)",
            found.len(),
            sum.first_hit,
            t.elapsed(),
            sum.dirs_scanned
        );
        if name.is_empty() {
            assert_eq!(found.len(), files + n_dirs * 2);
        }
    }
}

/// A real Spotlight search of the user's home folder, for timings. Never
/// asserts on content (it depends on the index). Run by hand with
/// `--ignored --nocapture spotlight_home`.
#[test]
#[ignore]
fn spotlight_home() {
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    for (walk, index) in [(false, true), (true, false), (true, true)] {
        let t = Instant::now();
        let (found, denied, sum, _) = run_with(
            SearchQuery {
                name: std::env::var("FSTUDIO_SEARCH_NAME").unwrap_or("invoice".into()),
                walk,
                use_index: index,
                ..SearchQuery::in_folder(&home)
            },
            Some(Arc::new(Mdfind)),
        );
        println!(
            "home walk={walk} index={index}: {} found ({} from index), first {:?}, all {:?}, {} dirs, {} denied",
            found.len(),
            sum.from_index,
            sum.first_hit,
            t.elapsed(),
            sum.dirs_scanned,
            denied.len()
        );
    }
}

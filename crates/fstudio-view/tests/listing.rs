use fstudio_model::{Entry, EntryKind};
use fstudio_view::*;

fn entry(name: &str, kind: EntryKind, size: u64, mtime: i64) -> Entry {
    Entry {
        name: name.into(),
        kind,
        size,
        alloc_size: None,
        mtime_ns: mtime,
        btime_ns: None,
        mode: 0o644,
        flags: 0,
        inode: 1,
        dev: 1,
        is_package: false,
    }
}
fn file(name: &str) -> Entry {
    entry(name, EntryKind::File, 10, 0)
}
fn dir(name: &str) -> Entry {
    entry(name, EntryKind::Dir, 0, 0)
}
fn names(l: &Listing) -> Vec<String> {
    l.iter().map(|e| e.name.clone()).collect()
}
fn opts() -> ViewOptions {
    ViewOptions::default()
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[test]
fn natural_name_sort_with_folders_first() {
    let l = Listing::build(
        vec![
            file("file10"),
            file("File2"),
            dir("zeta"),
            file("file1"),
            dir("Alpha"),
        ],
        opts(),
    );
    assert_eq!(names(&l), ["Alpha", "zeta", "file1", "File2", "file10"]);
    let l = l.with_options(ViewOptions {
        folders_first: false,
        ascending: false,
        ..opts()
    });
    assert_eq!(names(&l), ["zeta", "file10", "File2", "file1", "Alpha"]);
}

#[test]
fn nfc_and_nfd_names_and_case_variants() {
    // The same visible name stored two ways, plus names differing only by case.
    let l = Listing::build(
        vec![
            file("Caf\u{e9}"),
            file("Cafe\u{301}x"),
            file("readme"),
            file("README"),
            file("Readme"),
        ],
        opts(),
    );
    assert_eq!(l.len(), 5);
    // Exact-name lookup distinguishes them; display names are untouched.
    assert!(l.index_of("README").is_some());
    assert!(l.index_of("readme").is_some());
    assert_ne!(l.index_of("README"), l.index_of("readme"));
    assert!(l.get("Cafe\u{301}x").is_some());
    // The filter and type-select are normalisation- and case-insensitive.
    let f = l.with_options(ViewOptions {
        filter: "CAFE\u{301}".into(),
        ..opts()
    });
    assert_eq!(names(&f), ["Caf\u{e9}", "Cafe\u{301}x"]);
    assert_eq!(
        type_select(&l, "cafe\u{301}", 0)
            .map(|i| l.entry(i).unwrap().name.clone())
            .as_deref(),
        Some("Caf\u{e9}")
    );
    assert_eq!(
        l.with_options(ViewOptions {
            filter: "readme".into(),
            ..opts()
        })
        .len(),
        3
    );
}

#[test]
fn hidden_filter_and_glob() {
    let mut h = file("secret");
    h.flags = fstudio_model::file_flags::HIDDEN;
    let l = Listing::build(
        vec![file(".git"), h, file("a.png"), file("b.PNG"), file("c.txt")],
        opts(),
    );
    assert_eq!(names(&l), ["a.png", "b.PNG", "c.txt"]);
    let all = l.with_options(ViewOptions {
        show_hidden: true,
        ..opts()
    });
    assert_eq!(all.len(), 5);
    assert_eq!(all.total_len(), 5);
    let g = l.with_options(ViewOptions {
        filter: "*.png".into(),
        ..opts()
    });
    assert_eq!(names(&g), ["a.png", "b.PNG"]);
    assert_eq!(g.total_len(), 5);
    assert_eq!(g.index_of("c.txt"), None);
    assert!(g.get("c.txt").is_some());
}

#[test]
fn sort_keys() {
    let es = vec![
        entry("a.zip", EntryKind::File, 300, 5),
        entry("b.txt", EntryKind::File, 100, 9),
        entry("c.abc", EntryKind::File, 200, 1),
        entry("d", EntryKind::File, 200, 3),
    ];
    let by = |k, asc| {
        names(&Listing::build(
            es.clone(),
            ViewOptions {
                sort_key: k,
                ascending: asc,
                ..opts()
            },
        ))
    };
    assert_eq!(by(SortKey::Size, true), ["b.txt", "c.abc", "d", "a.zip"]);
    assert_eq!(by(SortKey::Size, false), ["a.zip", "c.abc", "d", "b.txt"]);
    assert_eq!(
        by(SortKey::Modified, true),
        ["c.abc", "d", "a.zip", "b.txt"]
    );
    assert_eq!(
        by(SortKey::Extension, true),
        ["d", "c.abc", "b.txt", "a.zip"]
    );
    assert_eq!(by(SortKey::Kind, true), ["d", "c.abc", "b.txt", "a.zip"]);
    let mut c = es.clone();
    c[0].btime_ns = Some(0);
    assert_eq!(
        names(&Listing::build(
            c,
            ViewOptions {
                sort_key: SortKey::Created,
                ..opts()
            }
        ))[0],
        "a.zip"
    );
}

#[test]
fn packages_as_files() {
    let mut app = dir("Safari.app");
    app.is_package = true;
    let es = vec![app, dir("Zed"), file("b.txt")];
    let files = Listing::build(es.clone(), opts());
    assert_eq!(names(&files), ["Zed", "b.txt", "Safari.app"]);
    let folders = Listing::build(
        es,
        ViewOptions {
            packages_as_files: false,
            ..opts()
        },
    );
    assert_eq!(names(&folders), ["Safari.app", "Zed", "b.txt"]);
}

#[test]
fn rows_and_lookup() {
    let l = Listing::build((0..10).map(|i| file(&format!("f{i}"))).collect(), opts());
    assert_eq!(
        l.rows(2, 3).map(|e| e.name.as_str()).collect::<Vec<_>>(),
        ["f2", "f3", "f4"]
    );
    assert_eq!(l.rows(8, 99).count(), 2);
    assert_eq!(l.rows(50, 3).count(), 0);
    assert_eq!(l.index_of("f7"), Some(7));
    assert_eq!(l.index_of("nope"), None);
    assert!(l.entry(10).is_none());
}

#[test]
fn streaming_equals_full_build() {
    let mut rng = Rng(7);
    let es: Vec<Entry> = (0..500)
        .map(|i| {
            if rng.below(5) == 0 {
                dir(&format!("d{}", i))
            } else {
                file(&format!("File {} {}", rng.below(300), i))
            }
        })
        .collect();
    let full = Listing::build(es.clone(), opts());
    let mut b = ListingBuilder::new(opts());
    let mut last = 0;
    for chunk in es.chunks(37) {
        b.push_chunk(chunk.to_vec());
        let s = b.snapshot();
        assert!(s.version() > last);
        last = s.version();
        let part = Listing::build(es[..s.total_len()].to_vec(), opts());
        assert_eq!(names(&s), names(&part), "partial snapshot sorted");
    }
    let streamed = b.finish();
    assert_eq!(names(&streamed), names(&full));
    assert_eq!(streamed.total_len(), full.total_len());
}

#[test]
fn duplicate_names_in_stream_are_replaced() {
    let mut b = ListingBuilder::new(opts());
    b.push_chunk(vec![file("a"), file("b")]);
    let mut a2 = file("a");
    a2.size = 99;
    b.push_chunk(vec![a2]);
    let l = b.finish();
    assert_eq!(l.len(), 2);
    assert_eq!(l.get("a").unwrap().size, 99);
}

#[test]
fn apply_basic() {
    let l = Listing::build(vec![file("a"), file("c")], opts());
    let l2 = l.apply([
        Change::Upsert(file("b")),
        Change::Remove("a".into()),
        Change::Remove("zzz".into()),
    ]);
    assert_eq!(names(&l2), ["b", "c"]);
    assert_eq!(l2.version(), l.version() + 1);
    assert_eq!(names(&l), ["a", "c"], "old version is unchanged");
    let mut c2 = file("c");
    c2.size = 5;
    assert_eq!(l2.apply([Change::Upsert(c2)]).get("c").unwrap().size, 5);
}

/// Random operations applied incrementally must always match a full rebuild.
#[test]
fn incremental_apply_matches_rebuild() {
    let configs = [
        ViewOptions::default(),
        ViewOptions {
            sort_key: SortKey::Size,
            ascending: false,
            ..opts()
        },
        ViewOptions {
            sort_key: SortKey::Modified,
            folders_first: false,
            show_hidden: true,
            ..opts()
        },
        ViewOptions {
            sort_key: SortKey::Kind,
            filter: "*a*".into(),
            ..opts()
        },
        ViewOptions {
            sort_key: SortKey::Extension,
            filter: "B".into(),
            packages_as_files: false,
            ..opts()
        },
    ];
    for (ci, cfg) in configs.iter().enumerate() {
        let mut rng = Rng(0x9E37_79B9 + ci as u64);
        let pool: Vec<String> = [
            "a",
            "A",
            "b1",
            "b10",
            "b2",
            ".hid",
            "Caf\u{e9}",
            "Cafe\u{301}",
            "x.txt",
            "y.TXT",
            "z.app",
            "q",
        ]
        .iter()
        .map(|s| s.to_string())
        .chain((0..40).map(|i| format!("n{}{}", i, if i % 3 == 0 { "a" } else { "b" })))
        .collect();
        let mut model: std::collections::BTreeMap<String, Entry> = Default::default();
        let mut live = Listing::empty(cfg.clone());
        for step in 0..400 {
            let mut changes = Vec::new();
            for _ in 0..(1 + rng.below(4)) {
                let name = pool[rng.below(pool.len() as u64) as usize].clone();
                if rng.below(3) == 0 {
                    model.remove(&name);
                    changes.push(Change::Remove(name));
                } else {
                    let kind = if rng.below(4) == 0 {
                        EntryKind::Dir
                    } else {
                        EntryKind::File
                    };
                    let mut e = entry(&name, kind, rng.below(5), rng.below(5) as i64);
                    e.is_package = kind == EntryKind::Dir && name.ends_with(".app");
                    model.insert(name, e.clone());
                    changes.push(Change::Upsert(e));
                }
            }
            live = live.apply(changes);
            let rebuilt = Listing::build(model.values().cloned().collect(), cfg.clone());
            assert_eq!(names(&live), names(&rebuilt), "config {ci} step {step}");
            assert_eq!(live.total_len(), rebuilt.total_len());
            for (i, e) in rebuilt.iter().enumerate() {
                assert_eq!(live.index_of(&e.name), Some(i));
                assert_eq!(live.entry(i), Some(e));
            }
        }
    }
}

#[test]
fn custom_collation_hook() {
    struct Reverse;
    impl Collation for Reverse {
        fn key(&self, _n: &str, folded: &str) -> Option<Box<[u8]>> {
            Some(folded.as_bytes().to_vec().into_boxed_slice())
        }
        fn compare(&self, a: &[u8], b: &[u8]) -> std::cmp::Ordering {
            b.cmp(a)
        }
    }
    let mut b = ListingBuilder::with_collation(opts(), std::sync::Arc::new(Reverse));
    b.push_chunk(vec![file("a"), file("c"), file("b")]);
    assert_eq!(names(&b.finish()), ["c", "b", "a"]);
}

#[test]
fn type_select_rules() {
    let l = Listing::build(
        vec![
            file("Alpha"),
            file("apple"),
            file("Banana"),
            file("band"),
            file("Zed"),
        ],
        opts(),
    );
    assert_eq!(names(&l), ["Alpha", "apple", "Banana", "band", "Zed"]);
    assert_eq!(type_select(&l, "a", 0), Some(0));
    assert_eq!(type_select(&l, "ap", 0), Some(1));
    assert_eq!(type_select(&l, "B", 0), Some(2));
    assert_eq!(type_select(&l, "ban", 0), Some(2));
    assert_eq!(type_select(&l, "band", 0), Some(3));
    // Searching starts at `from` and wraps around.
    assert_eq!(type_select(&l, "a", 1), Some(1));
    assert_eq!(type_select(&l, "a", 2), Some(0));
    assert_eq!(type_select(&l, "ban", 3), Some(3));
    assert_eq!(type_select(&l, "q", 0), None);
    assert_eq!(type_select(&l, "", 0), None);
    assert_eq!(type_select(&l, "z", 99), Some(4), "from is clamped");
    assert_eq!(type_select(&Listing::empty(opts()), "a", 0), None);
}

#[test]
fn selection_moves() {
    let mut s = Selection::new();
    s.move_cursor(10, Move::Down, 4, false);
    assert_eq!((s.cursor(), s.count()), (Some(0), 1));
    s.move_cursor(10, Move::Down, 4, false);
    s.move_cursor(10, Move::PageDown, 4, false);
    assert_eq!(s.cursor(), Some(5));
    s.move_cursor(10, Move::End, 4, false);
    assert_eq!(s.cursor(), Some(9));
    s.move_cursor(10, Move::Down, 4, false);
    assert_eq!(s.cursor(), Some(9), "clamps at the end");
    s.move_cursor(10, Move::Home, 4, false);
    assert_eq!(s.cursor(), Some(0));
    s.move_cursor(10, Move::Up, 4, false);
    assert_eq!(s.cursor(), Some(0));
    let mut e = Selection::new();
    e.move_cursor(10, Move::Up, 4, false);
    assert_eq!(
        e.cursor(),
        Some(9),
        "Up with nothing focused goes to the last row"
    );
}

#[test]
fn selection_shift_extend() {
    let mut s = Selection::new();
    s.select_only(3);
    s.move_cursor(10, Move::Down, 4, true);
    s.move_cursor(10, Move::Down, 4, true);
    assert_eq!(s.selected().collect::<Vec<_>>(), [3, 4, 5]);
    assert_eq!((s.anchor(), s.cursor()), (Some(3), Some(5)));
    s.move_cursor(10, Move::Up, 4, true);
    s.move_cursor(10, Move::Up, 4, true);
    s.move_cursor(10, Move::Up, 4, true);
    assert_eq!(s.selected().collect::<Vec<_>>(), [2, 3]);
    s.move_cursor(10, Move::End, 4, true);
    assert_eq!(
        s.selected().collect::<Vec<_>>(),
        (3..=9).collect::<Vec<_>>()
    );
    s.move_cursor(10, Move::Down, 4, false);
    assert_eq!(s.count(), 1);
    s.select_only(2);
    s.extend_to(5);
    assert_eq!(s.count(), 4);
    s.toggle(3);
    assert!(!s.is_selected(3));
    assert_eq!(s.count(), 3);
}

#[test]
fn selection_all_none_invert_glob() {
    let l = Listing::build(
        vec![file("a.png"), file("b.txt"), file("c.PNG"), file("d.md")],
        opts(),
    );
    let mut s = Selection::new();
    s.select_all(l.len());
    assert_eq!(s.count(), 4);
    s.select_none();
    assert_eq!(s.count(), 0);
    s.select_glob(&l, "*.png");
    assert_eq!(s.selected().collect::<Vec<_>>(), [0, 2]);
    s.invert(l.len());
    assert_eq!(s.selected().collect::<Vec<_>>(), [1, 3]);
    s.select_glob(&l, "*.png");
    s.deselect_glob(&l, "*.PNG");
    assert_eq!(s.selected().collect::<Vec<_>>(), [1, 3]);
    assert_eq!(s.names(&l), ["b.txt", "d.md"]);
}

#[test]
fn after_remove_rules() {
    let mk = |sel: &[usize]| {
        let mut s = Selection::new();
        for &i in sel {
            s.toggle(i);
        }
        s
    };
    // Next item after the removed one; its index shifts down by the number removed before it.
    let mut s = mk(&[3]);
    assert_eq!(s.after_remove(&[3], 10), Some(3));
    assert_eq!(s.selected().collect::<Vec<_>>(), [3]);
    // Several removed: the item after the last one.
    let mut s = mk(&[2, 3, 6]);
    assert_eq!(s.after_remove(&[2, 3, 6], 10), Some(4)); // old row 7 -> 7 - 3
    // Last row removed: the previous one.
    let mut s = mk(&[9]);
    assert_eq!(s.after_remove(&[9], 10), Some(8));
    // The tail removed: previous surviving item.
    let mut s = mk(&[8, 9]);
    assert_eq!(s.after_remove(&[8, 9], 10), Some(7));
    // Everything removed.
    let mut s = mk(&[0, 1]);
    assert_eq!(s.after_remove(&[0, 1], 2), None);
    assert!(s.is_empty() && s.cursor().is_none());
}

#[test]
fn remap_keeps_selection_by_name() {
    let old = Listing::build(vec![file("a"), file("b"), file("c"), file("d")], opts());
    let mut s = Selection::new();
    s.select_only(1);
    s.toggle(3);
    // "a" is deleted, "bb" is added, "b" and "d" stay.
    let new = old.apply([Change::Remove("a".into()), Change::Upsert(file("bb"))]);
    let r = s.remap(&old, &new);
    assert_eq!(r.names(&new), ["b", "d"]);
    assert_eq!(r.cursor(), new.index_of("d"));
    // A selected entry that disappeared is dropped and the cursor falls back to its row.
    let new2 = new.apply([Change::Remove("d".into())]);
    let r2 = r.remap(&new, &new2);
    assert_eq!(r2.names(&new2), ["b"]);
    assert_eq!(r2.cursor(), Some(new2.len() - 1));
    let empty = new2.apply([
        Change::Remove("b".into()),
        Change::Remove("bb".into()),
        Change::Remove("c".into()),
    ]);
    let r3 = r2.remap(&new2, &empty);
    assert_eq!((r3.cursor(), r3.count()), (None, 0));
}

#[test]
#[ignore = "builds 100k entries; run with --ignored"]
fn hundred_thousand_entries_sort_and_update() {
    let mut rng = Rng(42);
    let es: Vec<Entry> = (0..100_000)
        .map(|i| file(&format!("IMG_{:05}_{}.jpg", rng.below(100_000), i)))
        .collect();
    let t = std::time::Instant::now();
    let l = Listing::build(es, opts());
    eprintln!("build 100k: {:?}", t.elapsed());
    let t = std::time::Instant::now();
    let l2 = l.apply([
        Change::Upsert(file("IMG_00500.jpg")),
        Change::Remove("nothing".into()),
    ]);
    eprintln!("apply 1 change at 100k: {:?}", t.elapsed());
    assert_eq!(l2.total_len(), 100_001);
    assert_eq!(l.len(), 100_000);
}

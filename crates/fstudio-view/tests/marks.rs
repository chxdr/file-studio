//! Culling marks in listings: sorting, presets and hidden sidecars.

use fstudio_model::{Entry, EntryKind};
use fstudio_view::*;

fn entry(name: &str, kind: EntryKind) -> Entry {
    Entry {
        name: name.into(),
        kind,
        size: 10,
        alloc_size: None,
        mtime_ns: 0,
        btime_ns: None,
        mode: 0o644,
        flags: 0,
        inode: 1,
        dev: 1,
        is_package: false,
    }
}
fn file(name: &str) -> Entry {
    entry(name, EntryKind::File)
}
fn dir(name: &str) -> Entry {
    entry(name, EntryKind::Dir)
}
fn names(l: &Listing) -> Vec<String> {
    l.iter().map(|e| e.name.clone()).collect()
}
fn mk(rating: u8, label: Option<Label>, flag: Flag) -> Marks {
    Marks {
        rating,
        label,
        flag,
    }
}

/// A shoot: three raws with sidecars (one in upper case, one NFD), a JPEG
/// with no sidecar, a lone sidecar and a folder.
fn shoot() -> Vec<Entry> {
    vec![
        file("A.ARW"),
        file("A.xmp"),
        file("B.ARW"),
        file("B.XMP"),
        file("Cafe\u{301}.ARW"),
        file("Caf\u{e9}.xmp"),
        file("D.JPG"),
        file("lonely.xmp"),
        dir("Exports"),
    ]
}

fn table() -> MarkTable {
    MarkTable::from_sidecars([
        ("A.xmp".into(), mk(2, Some(Label::Green), Flag::None)),
        ("B.XMP".into(), mk(4, None, Flag::Reject)),
        ("Caf\u{e9}.xmp".into(), mk(5, Some(Label::Red), Flag::Pick)),
        ("lonely.xmp".into(), mk(1, None, Flag::None)),
    ])
}

fn with(marks: MarkTable, f: impl FnOnce(&mut ViewOptions)) -> ViewOptions {
    let mut o = ViewOptions {
        marks,
        ..ViewOptions::default()
    };
    f(&mut o);
    o
}

#[test]
fn paired_sidecars_are_hidden_lone_ones_stay() {
    let l = Listing::build(shoot(), ViewOptions::default());
    assert_eq!(
        names(&l),
        [
            "Exports",
            "A.ARW",
            "B.ARW",
            "Cafe\u{301}.ARW",
            "D.JPG",
            "lonely.xmp"
        ]
    );
    let i = l.index_of("A.ARW").unwrap();
    assert!(l.has_hidden_sidecar(i));
    assert!(!l.has_hidden_sidecar(l.index_of("D.JPG").unwrap()));
    assert_eq!(l.index_of("A.xmp"), None);
    assert_eq!(l.get("A.xmp").map(|e| e.name.as_str()), Some("A.xmp"));
    assert_eq!(l.sidecar_entries().count(), 4);

    let shown = l.with_options(ViewOptions {
        hide_sidecars: false,
        ..ViewOptions::default()
    });
    assert_eq!(shown.len(), 9);
}

#[test]
fn marks_reach_image_rows_across_case_and_normalisation() {
    let l = Listing::build(shoot(), with(table(), |_| {}));
    let at = |n: &str| l.marks_at(l.index_of(n).unwrap());
    assert_eq!(at("A.ARW"), Some(mk(2, Some(Label::Green), Flag::None)));
    assert_eq!(at("B.ARW"), Some(mk(4, None, Flag::Reject)));
    assert_eq!(
        at("Cafe\u{301}.ARW"),
        Some(mk(5, Some(Label::Red), Flag::Pick))
    );
    assert_eq!(at("D.JPG"), None);
    assert_eq!(at("Exports"), None);
    assert_eq!(at("lonely.xmp"), Some(mk(1, None, Flag::None)));
}

#[test]
fn sort_by_rating_label_flag() {
    let base = Listing::build(shoot(), with(table(), |_| {}));
    let by = |k: SortKey, asc: bool| {
        names(&base.with_options(with(table(), |o| {
            o.sort_key = k;
            o.ascending = asc;
            o.folders_first = false;
        })))
    };
    // Highest first; unrated D ties at 0 and falls back to name; reject last.
    assert_eq!(
        by(SortKey::Rating, false),
        [
            "Cafe\u{301}.ARW",
            "A.ARW",
            "lonely.xmp",
            "D.JPG",
            "Exports",
            "B.ARW"
        ]
    );
    assert_eq!(
        by(SortKey::Label, true),
        [
            "Cafe\u{301}.ARW",
            "A.ARW",
            "B.ARW",
            "D.JPG",
            "Exports",
            "lonely.xmp"
        ]
    );
    assert_eq!(
        by(SortKey::Flag, true),
        [
            "Cafe\u{301}.ARW",
            "A.ARW",
            "D.JPG",
            "Exports",
            "lonely.xmp",
            "B.ARW"
        ]
    );
}

#[test]
fn presets() {
    let base = Listing::build(shoot(), with(table(), |_| {}));
    let f = |p: MarkFilter| names(&base.with_options(with(table(), |o| o.mark_filter = p)));
    assert_eq!(f(MarkFilter::MinRating(3)), ["Exports", "Cafe\u{301}.ARW"]);
    assert_eq!(f(MarkFilter::Picks), ["Exports", "Cafe\u{301}.ARW"]);
    assert_eq!(f(MarkFilter::Rejects), ["Exports", "B.ARW"]);
    assert_eq!(
        f(MarkFilter::HideRejects),
        ["Exports", "A.ARW", "Cafe\u{301}.ARW", "D.JPG", "lonely.xmp"]
    );
    // Before any sidecar is read, presets that need marks show only folders.
    let unread = base.with_options(with(MarkTable::default(), |o| {
        o.mark_filter = MarkFilter::Picks
    }));
    assert_eq!(names(&unread), ["Exports"]);
}

#[test]
fn image_arriving_after_its_sidecar_hides_it() {
    // Streaming: the sidecar comes first, alone; the image a chunk later.
    let mut b = ListingBuilder::new(ViewOptions::default());
    b.push_chunk(vec![file("X.xmp"), file("Y.txt")]);
    assert_eq!(names(&b.snapshot()), ["X.xmp", "Y.txt"]);
    b.push_chunk(vec![file("x.NEF")]);
    let l = b.finish();
    assert_eq!(names(&l), ["x.NEF", "Y.txt"]);

    // Watcher patches: removing the image shows the sidecar again, and a
    // new sidecar for an existing image hides at once.
    let l = l.apply([Change::Remove("x.NEF".into())]);
    assert_eq!(names(&l), ["X.xmp", "Y.txt"]);
    let l = l.apply([Change::Upsert(file("x.NEF")), Change::Upsert(file("Y.xmp"))]);
    assert_eq!(names(&l), ["x.NEF", "Y.txt"]);
}

#[test]
fn incremental_equals_rebuild_with_sidecars() {
    // A deterministic shuffle of adds and removes, compared with a rebuild.
    let pool: Vec<String> = (0..30)
        .flat_map(|i| [format!("IMG_{i}.CR3"), format!("img_{i}.xmp")])
        .chain((0..5).map(|i| format!("note{i}.txt")))
        .collect();
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for (hide, filter) in [
        (true, MarkFilter::All),
        (true, MarkFilter::HideRejects),
        (false, MarkFilter::MinRating(3)),
    ] {
        let marks = MarkTable::from_sidecars((0..30).map(|i| {
            let flag = [Flag::None, Flag::Pick, Flag::Reject][i % 3];
            (format!("img_{i}.xmp"), mk((i % 6) as u8, None, flag))
        }));
        let o = with(marks, |o| {
            o.hide_sidecars = hide;
            o.mark_filter = filter;
            o.sort_key = SortKey::Rating;
        });
        let mut present: std::collections::BTreeSet<String> = Default::default();
        let mut l = Listing::build(vec![], o.clone());
        for _ in 0..300 {
            let name = &pool[(next() % pool.len() as u64) as usize];
            let change = if present.remove(name) {
                Change::Remove(name.clone())
            } else {
                present.insert(name.clone());
                Change::Upsert(file(name))
            };
            l = l.apply([change]);
            let full = Listing::build(present.iter().map(|n| file(n)).collect(), o.clone());
            assert_eq!(names(&l), names(&full));
        }
    }
}

#[test]
fn folders_named_like_sidecars_are_not_sidecars() {
    let l = Listing::build(vec![file("A.ARW"), dir("A.xmp")], ViewOptions::default());
    assert_eq!(names(&l), ["A.xmp", "A.ARW"]);
    assert_eq!(l.sidecar_entries().count(), 0);
}

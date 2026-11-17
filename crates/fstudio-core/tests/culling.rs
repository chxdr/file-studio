//! Culling marks from XMP sidecars: rows, sorting, presets, hidden sidecars,
//! live updates and the preview pane's lookup. Sidecars are only ever read.

mod common;

use common::*;
use fstudio_core::*;

fn xmp(attrs: &str) -> String {
    format!(
        "<?xpacket begin=\"\u{FEFF}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Import Studio\">\n \
<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
<rdf:Description rdf:about=\"\"\n    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n    \
xmlns:istudio=\"https://importstudio.app/ns/xmp/1.0/\" {attrs}/>\n \
</rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>\n"
    )
}

fn write(p: &std::path::Path, s: &str) {
    std::fs::write(p, s).unwrap();
}

/// A shoot folder: four fake raws, sidecars in mixed case and normalisation,
/// one broken sidecar, one raw with none and one sidecar with no raw.
fn shoot(h: &H) {
    mkdirs(&h.p("shoot"));
    for n in ["A.ARW", "B.ARW", "Cafe\u{301}.ARW", "D.ARW", "E.ARW"] {
        write(&h.p(&format!("shoot/{n}")), "fake raw");
    }
    write(
        &h.p("shoot/A.xmp"),
        &xmp(r#"xmp:Rating="3" xmp:Label="Red" istudio:Flag="pick""#),
    );
    write(
        &h.p("shoot/B.XMP"),
        &xmp(r#"xmp:Rating="-1" istudio:Stars="4" istudio:Flag="reject""#),
    );
    write(&h.p("shoot/Caf\u{e9}.xmp"), &xmp(r#"xmp:Rating="5""#));
    write(&h.p("shoot/E.xmp"), "<x:xmpmeta><rdf:RDF>"); // malformed
    write(&h.p("shoot/orphan.xmp"), &xmp(r#"xmp:Rating="1""#));
}

fn row<'a>(rows: &'a [RowInfo], name: &str) -> &'a RowInfo {
    rows.iter().find(|r| r.name == name).unwrap()
}

fn marks_loaded(t: &Tab) -> bool {
    t.rows(0, 100).iter().any(|r| r.marks.is_some())
}

#[test]
fn rows_show_marks_and_paired_sidecars_hide() {
    let h = harness();
    shoot(&h);
    let t = h.window_at("shoot");
    wait_complete(&t);
    eventually("marks read", || marks_loaded(&t));
    assert_eq!(
        names(&t),
        [
            "A.ARW",
            "B.ARW",
            "Cafe\u{301}.ARW",
            "D.ARW",
            "E.ARW",
            "orphan.xmp"
        ]
    );
    let rows = t.rows(0, 100);
    let a = row(&rows, "A.ARW");
    assert_eq!(
        a.marks,
        Some(CullMarks {
            rating: 3,
            label: Some(CullLabel::Red),
            flag: CullFlag::Pick
        })
    );
    assert!(a.has_hidden_sidecar);
    assert_eq!(a.marks_text, "3 stars, Red label, picked");
    let b = row(&rows, "B.ARW");
    assert_eq!(
        b.marks.map(|m| (m.rating, m.flag)),
        Some((4, CullFlag::Reject))
    );
    assert_eq!(b.marks_text, "4 stars, rejected");
    assert_eq!(
        row(&rows, "Cafe\u{301}.ARW").marks.map(|m| m.rating),
        Some(5)
    );
    assert_eq!(row(&rows, "D.ARW").marks, None);
    assert!(!row(&rows, "D.ARW").has_hidden_sidecar);
    // Malformed sidecar: no marks, no error, and it still hides behind E.
    assert_eq!(row(&rows, "E.ARW").marks, None);
    assert!(row(&rows, "E.ARW").has_hidden_sidecar);
    assert_eq!(row(&rows, "orphan.xmp").marks.map(|m| m.rating), Some(1));

    // Showing sidecars again.
    h.engine
        .perform(CommandInvocation::new("view.hide_sidecars"), Via::Keyboard)
        .unwrap();
    eventually("sidecars shown", || names(&t).len() == 10);
    assert!(names(&t).contains(&"B.XMP".to_string()));

    // Nothing was written: the folder still has exactly its ten files.
    assert_eq!(std::fs::read_dir(h.p("shoot")).unwrap().count(), 10);
}

#[test]
fn sort_by_rating_and_presets() {
    let h = harness();
    shoot(&h);
    let t = h.window_at("shoot");
    wait_complete(&t);
    eventually("marks read", || marks_loaded(&t));

    h.engine.sort_by_column(t.id(), ColumnId::Rating).unwrap();
    eventually("sorted by rating", || {
        names(&t)
            == [
                "Cafe\u{301}.ARW",
                "A.ARW",
                "orphan.xmp",
                "D.ARW",
                "E.ARW",
                "B.ARW",
            ]
    });
    assert_eq!(t.view().sort, SortBy::Rating);
    assert!(!t.view().ascending, "best first on the first click");

    let run = |id: &str| {
        h.engine
            .perform(CommandInvocation::new(id), Via::Keyboard)
            .unwrap();
    };
    run("cull.three_stars");
    eventually("3+", || names(&t) == ["Cafe\u{301}.ARW", "A.ARW"]);
    run("cull.picks");
    eventually("picks", || names(&t) == ["A.ARW"]);
    run("cull.rejects");
    eventually("rejects", || names(&t) == ["B.ARW"]);
    run("cull.hide_rejects");
    eventually("hide rejects", || names(&t).len() == 5);
    assert_eq!(t.view().cull, CullFilter::HideRejects);
    run("cull.all");
    eventually("all", || names(&t).len() == 6);

    // A preset doesn't follow you into another folder.
    run("cull.picks");
    eventually("picks again", || names(&t) == ["A.ARW"]);
    mkdirs(&h.p("other"));
    t.go(&h.s("other")).unwrap();
    wait_complete(&t);
    assert_eq!(t.view().cull, CullFilter::All);
}

#[test]
fn an_edited_sidecar_updates_the_row() {
    let h = harness();
    shoot(&h);
    let t = h.window_at("shoot");
    wait_complete(&t);
    eventually("marks read", || marks_loaded(&t));
    // Import Studio re-rates D: a new sidecar appears.
    write(&h.p("shoot/D.xmp"), &xmp(r#"xmp:Rating="2""#));
    eventually("new sidecar read", || {
        row(&t.rows(0, 100), "D.ARW").marks.map(|m| m.rating) == Some(2)
    });
    assert!(!names(&t).contains(&"D.xmp".to_string()));
    // And re-rates A (a write changes the mtime).
    std::thread::sleep(std::time::Duration::from_millis(20));
    write(&h.p("shoot/A.xmp"), &xmp(r#"xmp:Rating="1""#));
    eventually("edited sidecar re-read", || {
        row(&t.rows(0, 100), "A.ARW")
            .marks
            .map(|m| (m.rating, m.flag))
            == Some((1, CullFlag::None))
    });
    // The sidecar goes away: its marks go too.
    std::fs::remove_file(h.p("shoot/A.xmp")).unwrap();
    eventually("removed sidecar", || {
        row(&t.rows(0, 100), "A.ARW").marks.is_none()
    });
}

#[test]
fn preview_lookup_by_path() {
    let h = harness();
    shoot(&h);
    let m = h.engine.cull_marks(&h.s("shoot/B.ARW")).unwrap();
    assert_eq!((m.rating, m.flag), (4, CullFlag::Reject));
    // NFC path for an NFD-named raw, and the sidecar itself.
    assert_eq!(
        h.engine
            .cull_marks(&h.s("shoot/Caf\u{e9}.ARW"))
            .map(|m| m.rating),
        Some(5)
    );
    assert_eq!(
        h.engine.cull_marks(&h.s("shoot/B.XMP")).map(|m| m.rating),
        Some(4)
    );
    assert_eq!(h.engine.cull_marks(&h.s("shoot/D.ARW")), None);
    assert_eq!(h.engine.cull_marks(&h.s("shoot/E.ARW")), None);
    assert_eq!(h.engine.cull_marks(&h.s("missing/X.ARW")), None);
}

#[test]
fn folders_without_sidecars_are_untouched() {
    let h = harness();
    mkdirs(&h.p("docs"));
    touch(&h.p("docs/a.txt"));
    touch(&h.p("docs/b.xmp.txt"));
    let t = h.window_at("docs");
    wait_complete(&t);
    assert_eq!(names(&t), ["a.txt", "b.xmp.txt"]);
    assert!(t.rows(0, 10).iter().all(|r| r.marks.is_none()));
}

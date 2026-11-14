//! M3 search as a tab: results stream into the tab's listing, and the
//! listing machinery (rows, sorting, selection, paths, operations) works
//! on them as on a folder.

mod common;

use common::*;
use fstudio_core::*;

fn req(name: &str) -> SearchRequest {
    SearchRequest {
        scope: SearchScope::ThisFolder,
        query: SearchQuery {
            name: name.into(),
            use_index: false, // temporary folders aren't indexed
            ..Default::default()
        },
    }
}

fn tree(h: &H) {
    mkdirs(&h.p("docs/work"));
    mkdirs(&h.p("pics"));
    touch(&h.p("Report.txt"));
    touch(&h.p("docs/work/report b.md"));
    touch(&h.p("docs/a report.pdf"));
    touch(&h.p("pics/unrelated.png"));
    touch(&h.p("Cafe\u{301} report.txt"));
}

#[test]
fn results_are_rows_named_by_relative_path() {
    let h = harness();
    tree(&h);
    let t = h.window_at("");
    t.search(req("REPORT")).unwrap();
    assert!(t.is_search());
    wait_complete(&t);
    // Sorted by the item's own name; identity is the path below the root.
    assert_eq!(
        names(&t),
        [
            "docs/a report.pdf",
            "Cafe\u{301} report.txt",
            "docs/work/report b.md",
            "Report.txt"
        ]
    );
    let rows = t.rows(0, 10);
    assert_eq!(rows[0].display_name, "a report.pdf");
    assert_eq!(rows[0].location_text, "~/docs");
    assert_eq!(rows[0].kind_text, "PDF File");
    assert_eq!(rows[0].ext_text, "pdf");
    assert_eq!(rows[0].icon_key, "file.pdf");
    assert_eq!(rows[3].location_text, "~");
    assert_eq!(rows[2].display_name, "report b.md");
    // Paths of selected rows are real paths, for Quick Look, drags and ops.
    t.set_selection(SelectionChange::Names(vec!["docs/work/report b.md".into()]));
    assert_eq!(t.selected_paths(), [h.s("docs/work/report b.md")]);
    let st = t.search_status().unwrap();
    assert_eq!(st.state, SearchState::Done);
    assert_eq!(st.found, 4);
    assert_eq!(st.folder, h.s(""));
    assert!(st.text.starts_with("4 found in"), "{}", st.text);
}

#[test]
fn back_and_navigation_end_the_search_and_restore_keeps_the_folder() {
    let h = harness();
    tree(&h);
    let t = h.window_at("docs");
    t.search(req("report")).unwrap();
    wait_complete(&t);
    assert_eq!(names(&t), ["a report.pdf", "work/report b.md"]);
    assert_eq!(t.state().location, h.s("docs"));
    t.back().unwrap();
    assert!(!t.is_search());
    wait_complete(&t);
    assert_eq!(names(&t), ["work", "a report.pdf"]);
    assert_eq!(t.location(), h.s("docs"));

    t.search(req("report")).unwrap();
    wait_complete(&t);
    t.go(&h.s("pics")).unwrap();
    assert!(t.search_status().is_none());
    wait_complete(&t);
    assert_eq!(names(&t), ["unrelated.png"]);
}

#[test]
fn trash_from_results_drops_the_row_and_rename_follows_it() {
    let h = harness();
    tree(&h);
    let t = h.window_at("");
    t.search(req("report")).unwrap();
    wait_complete(&t);

    t.set_selection(SelectionChange::Names(vec!["docs/a report.pdf".into()]));
    let out = h
        .engine
        .perform(CommandInvocation::new("file.trash"), Via::Keyboard)
        .unwrap();
    h.engine.wait_op(out.ops[0], T).unwrap();
    assert!(!h.p("docs/a report.pdf").exists());
    eventually("trashed result gone", || {
        !names(&t).contains(&"docs/a report.pdf".to_string())
    });

    let out = h
        .engine
        .rename_item(t.id(), "docs/work/report b.md", "report c.md")
        .unwrap();
    h.engine.wait_op(out.ops[0], T).unwrap();
    assert!(h.p("docs/work/report c.md").exists());
    eventually("renamed row follows", || {
        names(&t).contains(&"docs/work/report c.md".to_string())
            && !names(&t).contains(&"docs/work/report b.md".to_string())
    });
    eventually("renamed row selected", || {
        t.selection().names == ["docs/work/report c.md"]
    });
}

#[test]
fn this_mac_scope_and_stop() {
    let h = harness();
    tree(&h);
    let t = h.window_at("");
    let mut r = req("fstudio-unlikely-name-zq");
    r.scope = SearchScope::ThisMac;
    t.search(r).unwrap();
    let st = t.search_status().unwrap();
    assert_eq!(st.root, "/");
    assert!(st.query.skip.iter().any(|p| p.as_os_str() == "/Volumes"));
    // Esc from the list (filter.clear with no filter text) stops it.
    let out = h
        .engine
        .perform(CommandInvocation::new("filter.clear"), Via::Keyboard)
        .unwrap();
    assert_eq!(out.message.as_deref(), Some("Search stopped"));
    wait_complete(&t);
    let st = t.search_status().unwrap();
    assert_eq!(st.state, SearchState::Stopped);
    assert!(st.text.starts_with("Stopped"));
    assert!(!t.stop_search());
    t.end_search();
    wait_complete(&t);
    assert_eq!(t.location(), h.s(""));
}

#[test]
fn search_global_carries_the_filter_text() {
    let h = harness();
    tree(&h);
    let t = h.window_at("");
    t.set_filter("rep");
    let out = h
        .engine
        .perform(CommandInvocation::new("search.global"), Via::Keyboard)
        .unwrap();
    assert_eq!(
        out.shell,
        Some(ShellRequest::ShowSearch { text: "rep".into() })
    );
}

#[test]
fn bad_regex_is_an_error_and_keeps_the_folder() {
    let h = harness();
    tree(&h);
    let t = h.window_at("");
    let mut r = req("([");
    r.query.name_mode = NameMode::Regex;
    assert!(t.search(r).is_err());
    assert!(!t.is_search());
    assert!(names(&t).contains(&"Report.txt".to_string()));
}

#[test]
fn refresh_reruns_and_finds_new_items() {
    let h = harness();
    tree(&h);
    let t = h.window_at("");
    t.search(req("report")).unwrap();
    wait_complete(&t);
    touch(&h.p("pics/new report.txt"));
    t.refresh();
    eventually("rerun finds the new file", || {
        t.listing().state() == ListingState::Complete
            && names(&t).contains(&"pics/new report.txt".to_string())
    });
}

#[test]
fn a_new_search_replaces_a_running_one_without_stopping_itself() {
    let h = harness();
    tree(&h);
    for i in 0..200 {
        mkdirs(&h.p(&format!("bulk/{i}")));
        touch(&h.p(&format!("bulk/{i}/report {i}.txt")));
    }
    let t = h.window_at("");
    t.search(req("report")).unwrap();
    t.search(req("rep")).unwrap();
    t.search(req("report b")).unwrap();
    wait_complete(&t);
    let st = t.search_status().unwrap();
    assert_eq!(st.state, SearchState::Done, "{}", st.text);
    assert_eq!(names(&t), ["docs/work/report b.md"]);
}

//! The list and column views' core pieces: columns in the view state (with
//! per-folder memory), created/extension row fields, drops, tab-less listings
//! and column-view keys.

mod common;

use common::*;
use fstudio_core::*;

fn key(k: Key) -> KeyChord {
    KeyChord {
        key: k,
        mods: Mods::NONE,
    }
}

fn list_ctx(t: &Tab) -> InputContext {
    InputContext {
        focus: Focus::List,
        tab: Some(t.id()),
    }
}

#[test]
fn columns_default_from_settings_and_are_remembered_per_folder() {
    let h = harness();
    mkdirs(&h.p("a"));
    mkdirs(&h.p("b"));
    let t = h.window_at("a");
    assert_eq!(t.view().columns, default_columns());
    // Change the columns in "a": shown, reordered, resized.
    let mut v = t.view();
    v.columns = vec![
        ColumnSpec {
            id: ColumnId::Name,
            width: 410.0,
        },
        ColumnSpec {
            id: ColumnId::Extension,
            width: 60.0,
        },
        ColumnSpec {
            id: ColumnId::Size,
            width: 90.0,
        },
    ];
    t.set_view(v.clone());
    t.go(&h.s("b")).unwrap();
    wait_complete(&t);
    assert_eq!(t.view().columns, default_columns(), "b uses the defaults");
    t.go(&h.s("a")).unwrap();
    wait_complete(&t);
    assert_eq!(
        t.view().columns,
        v.columns,
        "a remembers its own (smart memory)"
    );
    // New defaults in settings apply to folders without memory.
    let mut s = h.engine.settings();
    s.view.columns = vec![ColumnSpec {
        id: ColumnId::Created,
        width: 150.0,
    }];
    h.engine.update_settings(s).unwrap();
    t.go(&h.s("b")).unwrap();
    wait_complete(&t);
    let ids: Vec<ColumnId> = t.view().columns.iter().map(|c| c.id).collect();
    assert_eq!(
        ids,
        [ColumnId::Name, ColumnId::Created],
        "Name is always there"
    );
}

#[test]
fn rows_carry_created_date_and_extension() {
    let h = harness();
    touch(&h.p("photo.JPG"));
    touch(&h.p("README"));
    touch(&h.p(".zshrc"));
    mkdirs(&h.p("Tool.app"));
    mkdirs(&h.p("folder.d"));
    let t = h.window_at("");
    let mut v = t.view();
    v.show_hidden = true;
    t.set_view(v);
    eventually("hidden shown", || t.listing().index_of(".zshrc").is_some());
    let rows = t.rows(0, 10);
    let by = |n: &str| rows.iter().find(|r| r.name == n).unwrap().clone();
    assert_eq!(by("photo.JPG").ext_text, "JPG", "as on disk");
    assert_eq!(by("README").ext_text, "");
    assert_eq!(by(".zshrc").ext_text, "");
    assert_eq!(by("Tool.app").ext_text, "app");
    assert_eq!(by("folder.d").ext_text, "", "folders show none");
    let r = by("photo.JPG");
    assert!(r.created_ns > 0);
    assert!(!r.created_text.is_empty());
    assert_eq!(r.created_text, r.modified_text, "a fresh file: same minute");
}

#[test]
fn drop_enqueues_a_move_or_copy_and_refuses_bad_drops() {
    let h = harness();
    mkdirs(&h.p("src"));
    mkdirs(&h.p("dst"));
    std::fs::write(h.p("src/Café.txt"), b"hello").unwrap();
    std::fs::write(h.p("src/keep.txt"), b"keep").unwrap();
    let a = vec![h.s("src/Café.txt")];
    assert_eq!(
        h.engine.drop_effect(&a, &h.s("dst"), Mods::NONE),
        DropEffect::Move
    );
    let alt = Mods {
        alt: true,
        ..Mods::NONE
    };
    assert_eq!(h.engine.drop_effect(&a, &h.s("dst"), alt), DropEffect::Copy);
    // Copy with ⌥: source stays.
    h.engine
        .drop_items(&[h.s("src/keep.txt")], &h.s("dst"), alt)
        .unwrap();
    eventually("copied", || h.p("dst/keep.txt").exists());
    assert!(h.p("src/keep.txt").exists());
    // Plain drop on the same volume: moved.
    h.engine.drop_items(&a, &h.s("dst"), Mods::NONE).unwrap();
    eventually("moved", || {
        h.p("dst/Café.txt").exists() && !h.p("src/Café.txt").exists()
    });
    // Into itself: refused, nothing queued.
    let e = h
        .engine
        .drop_items(&[h.s("src")], &h.s("src"), Mods::NONE)
        .unwrap_err();
    assert!(matches!(e, CoreError::NotAvailable(_)));
}

#[test]
fn listing_handle_lists_selects_patches_and_unwatches_on_drop() {
    let h = harness();
    mkdirs(&h.p("col/inner"));
    touch(&h.p("col/a.txt"));
    touch(&h.p("col/B.txt"));
    touch(&h.p("col/b.txt"));
    let t = h.window_at("");
    h.drain();
    let col = h.s("col");
    let canon = std::fs::canonicalize(h.p("col"))
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let handle = h.engine.open_listing(&col, false);
    assert_ne!(handle.id(), t.id());
    let id = handle.id();
    h.expect_event("handle sorted", |e| {
        matches!(e, CoreEvent::ListingChanged { tab, kind: ChangeKind::Sorted, .. } if *tab == id)
    });
    let l = handle.listing();
    assert_eq!(l.names()[0], "inner", "folders first");
    // "B.txt" and "b.txt" are one file on a case-insensitive volume.
    let case_sensitive =
        l.names().iter().any(|n| n == "B.txt") && l.names().iter().any(|n| n == "b.txt");
    assert_eq!(l.len(), if case_sensitive { 4 } else { 3 });
    // Its own selection, separate from the tab's.
    handle.set_selection(SelectionChange::Names(vec!["a.txt".into()]));
    assert_eq!(handle.selection().names, ["a.txt"]);
    assert!(t.selection().names.is_empty());
    assert!(handle.rows(0, 4).iter().any(|r| r.is_selected));
    // Menu target: commands act through the owning tab, paths are the handle's.
    let target = handle.menu_target_at(t.id(), Some(0));
    assert_eq!(
        target,
        MenuTarget::Folder {
            tab: t.id(),
            path: h.s("col/inner")
        }
    );
    assert_eq!(
        handle.selection().names,
        ["inner"],
        "right-click selected it"
    );
    // Watched, and patched by the watcher.
    assert!(h.engine.watched_folders().contains(&canon));
    touch(&h.p("col/new file.txt"));
    eventually("patched", || {
        handle.listing().index_of("new file.txt").is_some()
    });
    // Never in the workspace.
    let ws = h.engine.workspace_state();
    assert!(
        ws.windows
            .iter()
            .flat_map(|w| &w.panes)
            .flat_map(|p| &p.tabs)
            .all(|s| s.id != id)
    );
    // Dropped: no longer watched.
    drop(handle);
    eventually("unwatched", || !h.engine.watched_folders().contains(&canon));
}

#[test]
fn handle_on_missing_or_unreadable_folder_reports_an_error() {
    let h = harness();
    let handle = h.engine.open_listing(&h.s("nope"), false);
    eventually("error", || handle.listing().state() == ListingState::Error);
    assert!(handle.listing().error().is_some());
}

#[test]
fn column_view_arrows_go_in_and_out() {
    let h = harness();
    mkdirs(&h.p("one/two"));
    touch(&h.p("one/file.txt"));
    mkdirs(&h.p("one/Pkg.app"));
    let t = h.window_at("one");
    let ctx = list_ctx(&t);
    // In list view ← and → aren't column keys.
    assert!(
        h.engine
            .handle_key(key(Key::Left), ctx)
            .is_none_or(|o| o.command.is_some())
    );
    assert_eq!(t.location(), h.s("one"));
    let mut v = t.view();
    v.mode = ViewMode::Columns;
    t.set_view(v);
    // → on a file or package does nothing.
    t.set_selection(SelectionChange::Names(vec!["file.txt".into()]));
    h.engine.handle_key(key(Key::Right), ctx).unwrap();
    assert_eq!(t.location(), h.s("one"));
    t.set_selection(SelectionChange::Names(vec!["Pkg.app".into()]));
    h.engine.handle_key(key(Key::Right), ctx).unwrap();
    assert_eq!(t.location(), h.s("one"));
    // → on a folder goes in.
    t.set_selection(SelectionChange::Names(vec!["two".into()]));
    h.engine.handle_key(key(Key::Right), ctx).unwrap();
    assert_eq!(t.location(), h.s("one/two"));
    wait_complete(&t);
    // ← comes back out with "two" selected.
    h.engine.handle_key(key(Key::Left), ctx).unwrap();
    assert_eq!(t.location(), h.s("one"));
    eventually("selected", || t.selection().names == ["two"]);
    assert_eq!(
        t.view().mode,
        ViewMode::Columns,
        "browsing keeps column view"
    );
    // A click in another column.
    h.engine
        .column_select(t.id(), &h.s(""), Some("one"))
        .unwrap();
    assert_eq!(t.location(), h.s(""));
    eventually("one selected", || t.selection().names == ["one"]);
    h.engine.column_select(t.id(), &h.s(""), None).unwrap();
    assert!(t.selection().names.is_empty());
}

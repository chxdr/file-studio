//! Integration tests of the engine on temporary folders.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use fstudio_core::*;

#[test]
fn navigate_back_forward_up() {
    let h = harness();
    mkdirs(&h.p("a/inner"));
    mkdirs(&h.p("b"));
    touch(&h.p("file 10.txt"));
    touch(&h.p("file 9.txt"));
    touch(&h.p(".hidden"));
    let t = h.window_at("");
    // Folders first, natural order, hidden files hidden.
    assert_eq!(names(&t), ["a", "b", "file 9.txt", "file 10.txt"]);

    t.go("a").unwrap();
    wait_complete(&t);
    assert_eq!(t.location(), h.s("a"));
    assert_eq!(names(&t), ["inner"]);
    t.go("inner").unwrap();
    wait_complete(&t);
    assert_eq!(t.location(), h.s("a/inner"));

    t.back().unwrap();
    wait_complete(&t);
    assert_eq!(t.location(), h.s("a"));
    // Coming back selects the folder we came from.
    eventually("inner selected", || t.selection().names == ["inner"]);
    t.forward().unwrap();
    wait_complete(&t);
    assert_eq!(t.location(), h.s("a/inner"));
    assert_eq!(t.can_go(), (true, false));

    t.up().unwrap();
    wait_complete(&t);
    assert_eq!(t.location(), h.s("a"));
    eventually("inner selected after up", || {
        t.selection().names == ["inner"]
    });

    // A file location shows its folder and selects the file.
    t.go(&h.s("file 9.txt")).unwrap();
    wait_complete(&t);
    assert_eq!(t.location(), h.s(""));
    eventually("file selected", || t.selection().names == ["file 9.txt"]);

    // A missing path offers Create Folder and does not navigate.
    let out = t.go("nope/deeper").unwrap();
    assert!(matches!(
        out.shell,
        Some(ShellRequest::OfferCreateFolder { ref path, .. }) if path.ends_with("nope/deeper")
    ));
    assert_eq!(t.location(), h.s(""));

    // Visits were recorded for frecency.
    assert!(
        h.engine
            .frecent("inner", 5)
            .iter()
            .any(|f| f.path.ends_with("a/inner"))
    );
}

#[test]
fn partial_then_sorted_events_and_rows() {
    let h = harness();
    mkdirs(&h.p("big"));
    for i in 0..3000 {
        touch(&h.p(&format!("big/f{i:05}")));
    }
    let t = h.window_at("");
    h.drain();
    t.go("big").unwrap();
    let id = t.id();
    h.expect_event("sorted", |e| {
        matches!(e, CoreEvent::ListingChanged { tab, kind: ChangeKind::Sorted, .. } if *tab == id)
    });
    let l = t.listing();
    assert_eq!(l.len(), 3000);
    assert_eq!(l.state(), ListingState::Complete);
    let rows = t.rows(10, 3);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].index, 10);
    assert_eq!(rows[0].name, "f00010");
    assert_eq!(rows[0].size_text, "1 byte");
    assert_eq!(rows[0].icon_key, "file");
    assert!(rows[0].modified_text.starts_with("Today"));
    assert!(t.rows(2999, 10).len() == 1);
}

#[test]
fn unicode_case_twins_and_type_select() {
    let h = harness();
    touch(&h.p("Cafe\u{301}.txt")); // NFD
    touch(&h.p("apple"));
    touch(&h.p("Banana"));
    touch(&h.p("banana2"));
    let t = h.window_at("");
    // Names are shown exactly as stored.
    assert!(names(&t).contains(&"Cafe\u{301}.txt".to_string()));
    // Type-select matches folded names.
    let r = t.type_select("caf\u{e9}").unwrap();
    assert_eq!(t.listing().name_at(r).unwrap(), "Cafe\u{301}.txt");
    let r = t.type_select("BAN").unwrap();
    assert_eq!(t.listing().name_at(r).unwrap(), "Banana");
    // Keys nobody binds type-select too.
    let out = h
        .engine
        .handle_key(KeyChord::plain(Key::Char('a')), InputContext::default())
        .unwrap();
    assert_eq!(out.tab, Some(t.id()));
    std::thread::sleep(Duration::from_millis(1100));
    h.engine
        .handle_key(KeyChord::plain(Key::Char('a')), InputContext::default())
        .unwrap();
    assert_eq!(t.selection().names, ["apple"]);
    // ⌘↑ is "Enclosing Folder".
    t.go("/").unwrap();
    wait_complete(&t);
    h.engine
        .handle_key(KeyChord::cmd(Key::Up), InputContext::default());
    assert_eq!(t.location(), "/");
}

#[test]
fn case_twins_on_case_sensitive_names_are_both_listed() {
    let h = harness();
    touch(&h.p("Report.txt"));
    // On a case-insensitive volume this is the same file; either way the
    // listing must hold exactly what the filesystem says.
    let _ = std::fs::write(h.p("report.txt"), b"y");
    let t = h.window_at("");
    let n = names(&t);
    let on_disk = std::fs::read_dir(&h.root).unwrap().count();
    assert_eq!(n.len(), on_disk);
}

#[test]
fn watcher_patches_the_listing() {
    let h = harness();
    touch(&h.p("b"));
    let t = h.window_at("");
    h.drain();
    let id = t.id();
    touch(&h.p("a"));
    h.expect_event("patched", |e| {
        matches!(e, CoreEvent::ListingChanged { tab, kind: ChangeKind::Patched, .. } if *tab == id)
    });
    eventually("a listed", || names(&t) == ["a", "b"]);
    std::fs::remove_file(h.p("b")).unwrap();
    eventually("b gone", || names(&t) == ["a"]);
    std::fs::rename(h.p("a"), h.p("c")).unwrap();
    eventually("renamed", || names(&t) == ["c"]);
}

#[test]
fn selection_survives_changes_by_name() {
    let h = harness();
    for n in ["b", "d", "f"] {
        touch(&h.p(n));
    }
    let t = h.window_at("");
    t.set_selection(SelectionChange::Names(vec!["d".into(), "f".into()]));
    assert_eq!(t.selection().rows, [1, 2]);
    // New entries before the selection shift the rows, not the names.
    touch(&h.p("a"));
    touch(&h.p("c"));
    eventually("patched", || names(&t).len() == 5);
    let sel = t.selection();
    assert_eq!(sel.names, ["d", "f"]);
    assert_eq!(sel.rows, [3, 4]);
    // Re-sorting keeps it too.
    let mut v = t.view();
    v.ascending = false;
    t.set_view(v);
    eventually("resorted", || names(&t)[0] == "f");
    assert_eq!(t.selection().names, ["f", "d"]);
    // Filtering hides one; clearing the filter does not bring a lost row back
    // but keeps the rest.
    t.set_filter("f");
    eventually("filtered", || names(&t) == ["f"]);
    assert_eq!(t.selection().names, ["f"]);
    t.set_filter("");
    eventually("unfiltered", || names(&t).len() == 5);
    assert_eq!(t.selection().names, ["f"]);
    // A refresh keeps it.
    t.refresh();
    std::thread::sleep(Duration::from_millis(50));
    wait_complete(&t);
    assert_eq!(t.selection().names, ["f"]);
}

#[test]
fn copy_to_other_pane_then_undo() {
    let h = harness();
    mkdirs(&h.p("left"));
    mkdirs(&h.p("right"));
    std::fs::write(h.p("left/doc.txt"), b"hello").unwrap();
    let left = h.window_at("left");
    let w = h.engine.workspace_state().windows[0].id;
    assert!(h.engine.toggle_dual(w).unwrap());
    let right = h.engine.other_pane_tab(left.id()).unwrap();
    right.go("../right").unwrap();
    wait_complete(&right);
    assert_eq!(right.location(), h.s("right"));

    left.set_selection(SelectionChange::Names(vec!["doc.txt".into()]));
    let out = h
        .engine
        .perform(
            CommandInvocation::new("file.copy_to_other_pane").for_tab(left.id()),
            Via::Keyboard,
        )
        .unwrap();
    let op = out.ops[0];
    let st = h.engine.wait_op(op, T).unwrap();
    assert_eq!(st.state, OpState::Completed);
    assert_eq!(std::fs::read(h.p("right/doc.txt")).unwrap(), b"hello");
    eventually("right pane shows the copy", || names(&right) == ["doc.txt"]);
    eventually("copy selected in the other pane", || {
        right.selection().names == ["doc.txt"]
    });
    assert!(h.engine.undo_state().undo.unwrap().contains("Copy"));

    let out = h.engine.undo().unwrap();
    for id in out.ops {
        h.engine.wait_op(id, T);
    }
    assert!(!h.p("right/doc.txt").exists());
    assert!(h.p("left/doc.txt").exists());
    eventually("copy gone from the listing", || names(&right).is_empty());
    // The undone copy went to the (folder) Trash, never unlinked.
    assert_eq!(std::fs::read_dir(&h.trash).unwrap().count(), 1);
}

#[test]
fn cut_marks_then_paste_moves() {
    let h = harness();
    mkdirs(&h.p("src"));
    mkdirs(&h.p("dst"));
    touch(&h.p("src/a"));
    touch(&h.p("src/b"));
    let t = h.window_at("src");
    t.set_selection(SelectionChange::Names(vec!["a".into()]));
    h.engine
        .perform(CommandInvocation::new("edit.cut"), Via::Menu)
        .unwrap();
    // Cut deletes nothing; the row is marked.
    assert!(h.p("src/a").exists());
    let rows = t.rows(0, 2);
    assert!(rows[0].is_cut_pending && !rows[1].is_cut_pending);
    assert!(h.engine.clipboard().unwrap().1);

    t.go("../dst").unwrap();
    wait_complete(&t);
    let out = h
        .engine
        .perform(CommandInvocation::new("edit.paste"), Via::Keyboard)
        .unwrap();
    h.engine.wait_op(out.ops[0], T).unwrap();
    assert!(!h.p("src/a").exists());
    assert!(h.p("dst/a").exists());
    assert!(h.engine.clipboard().is_none());
    eventually("pasted item selected", || t.selection().names == ["a"]);

    // Copy + paste leaves the source and keeps the clipboard.
    t.set_selection(SelectionChange::Names(vec!["a".into()]));
    h.engine
        .perform(CommandInvocation::new("edit.copy"), Via::Keyboard)
        .unwrap();
    t.go("../src").unwrap();
    wait_complete(&t);
    let out = h
        .engine
        .perform(CommandInvocation::new("edit.paste"), Via::Keyboard)
        .unwrap();
    h.engine.wait_op(out.ops[0], T).unwrap();
    assert!(h.p("dst/a").exists() && h.p("src/a").exists());
    assert!(h.engine.clipboard().is_some());
}

#[test]
fn trash_selects_the_next_item_and_new_folder_starts_rename() {
    let h = harness();
    for n in ["a", "b", "c"] {
        touch(&h.p(n));
    }
    let t = h.window_at("");
    t.set_selection(SelectionChange::Names(vec!["b".into()]));
    let out = h
        .engine
        .perform(CommandInvocation::new("file.trash"), Via::Keyboard)
        .unwrap();
    assert_eq!(t.selection().names, ["c"]);
    h.engine.wait_op(out.ops[0], T).unwrap();
    assert!(!h.p("b").exists());
    eventually("b gone", || names(&t) == ["a", "c"]);
    assert_eq!(t.selection().names, ["c"]);

    let out = h
        .engine
        .perform(CommandInvocation::new("file.new_folder"), Via::Keyboard)
        .unwrap();
    assert_eq!(
        out.shell,
        Some(ShellRequest::BeginRename {
            tab: t.id(),
            name: "untitled folder".into()
        })
    );
    h.engine.wait_op(out.ops[0], T).unwrap();
    eventually("new folder selected", || {
        t.selection().names == ["untitled folder"]
    });
    // The shell edits the name; the core validates and renames.
    assert!(
        h.engine
            .rename_item(t.id(), "untitled folder", "a/b")
            .is_err()
    );
    let out = h
        .engine
        .rename_item(t.id(), "untitled folder", "Photos")
        .unwrap();
    h.engine.wait_op(out.ops[0], T).unwrap();
    assert!(h.p("Photos").is_dir());
    eventually("renamed folder selected", || {
        t.selection().names == ["Photos"]
    });
    // A second new folder gets the next free name.
    let out = h
        .engine
        .perform(CommandInvocation::new("file.new_folder"), Via::Keyboard)
        .unwrap();
    h.engine.wait_op(out.ops[0], T).unwrap();
    let out = h
        .engine
        .perform(CommandInvocation::new("file.new_folder"), Via::Keyboard)
        .unwrap();
    h.engine.wait_op(out.ops[0], T).unwrap();
    eventually("two untitled folders", || {
        names(&t)
            .iter()
            .filter(|n| n.starts_with("untitled folder"))
            .count()
            == 2
    });
}

#[test]
fn session_persists_and_restores() {
    let mut h = harness();
    mkdirs(&h.p("one"));
    mkdirs(&h.p("two"));
    touch(&h.p("two/x"));
    touch(&h.p("two/y"));
    let t1 = h.window_at("one");
    let t2 = h.engine.open_tab(Some(h.s("two"))).unwrap();
    wait_complete(&t2);
    t2.set_selection(SelectionChange::Names(vec!["y".into()]));
    let mut v = t2.view();
    v.sort = SortBy::Size;
    t2.set_view(v);
    t2.set_scroll_anchor(Some(ScrollAnchor {
        name: "y".into(),
        offset: 4.0,
    }));
    let w = h.engine.workspace_state().windows[0].id;
    h.engine.toggle_dual(w).unwrap();
    let before = h.engine.workspace_state();
    assert_eq!(before.windows[0].panes.len(), 2);
    assert_eq!(before.tab_count(), 3);

    h.relaunch();
    let after = h.engine.restore_session();
    assert_eq!(after.windows.len(), 1);
    assert_eq!(after.windows[0].panes.len(), 2);
    let ids = |ws: &WorkspaceState| -> Vec<TabId> {
        ws.windows[0]
            .panes
            .iter()
            .flat_map(|p| p.tabs.iter().map(|t| t.id))
            .collect()
    };
    assert_eq!(ids(&before), ids(&after));
    let r2 = h.engine.tab(t2.id()).unwrap();
    assert_eq!(r2.location(), h.s("two"));
    assert_eq!(r2.view().sort, SortBy::Size);
    assert_eq!(r2.scroll_anchor().unwrap().name, "y");
    wait_complete(&r2);
    eventually("selection restored", || r2.selection().names == ["y"]);
    assert_eq!(h.engine.tab(t1.id()).unwrap().location(), h.s("one"));
    // New ids never collide with restored ones.
    let t3 = h.engine.open_tab(None).unwrap();
    assert!(!ids(&after).contains(&t3.id()));

    // A folder that disappeared falls back to its nearest parent.
    std::fs::remove_dir(h.p("one")).unwrap();
    h.relaunch();
    h.engine.restore_session();
    assert_eq!(h.engine.tab(t1.id()).unwrap().location(), h.s(""));
}

#[test]
fn session_is_saved_without_quitting() {
    let mut h = harness();
    mkdirs(&h.p("deb"));
    let t = h.window_at("");
    t.go("deb").unwrap();
    wait_complete(&t);
    // The debounced saver writes after a quiet period; no app_will_quit.
    std::thread::sleep(Duration::from_millis(800));
    let base = h.root.parent().unwrap().to_path_buf();
    let engine = std::mem::replace(
        &mut h.engine,
        Engine::new(config(&base.join("unused")), Arc::new(NullEventSink)).unwrap(),
    );
    drop(engine);
    let (tx, _rx) = crossbeam_channel::unbounded();
    let e2 = Engine::new(config(&base), Arc::new(tx)).unwrap();
    let ws = e2.restore_session();
    assert_eq!(ws.windows[0].panes[0].tabs[0].location, h.s("deb"));
}

#[test]
fn tabs_open_close_reorder_reopen() {
    let h = harness();
    mkdirs(&h.p("a"));
    mkdirs(&h.p("b"));
    mkdirs(&h.p("c"));
    let ta = h.window_at("a");
    let tb = h.engine.open_tab(Some(h.s("b"))).unwrap();
    let tc = h.engine.open_tab(Some(h.s("c"))).unwrap();
    let order = || -> Vec<String> {
        h.engine.workspace_state().windows[0].panes[0]
            .tabs
            .iter()
            .map(|t| t.location.rsplit('/').next().unwrap().to_string())
            .collect()
    };
    assert_eq!(order(), ["a", "b", "c"]);
    assert_eq!(h.engine.active_tab().unwrap().id(), tc.id());
    let pane = h.engine.workspace_state().windows[0].panes[0].id;
    h.engine.move_tab(tc.id(), pane, 0).unwrap();
    assert_eq!(order(), ["c", "a", "b"]);
    h.engine.close_tab(ta.id()).unwrap();
    assert_eq!(order(), ["c", "b"]);
    let back = h.engine.reopen_closed_tab().unwrap().unwrap();
    assert_eq!(back.location(), h.s("a"));
    assert_eq!(order(), ["c", "a", "b"]);
    // Tab commands.
    h.engine.activate_tab(tb.id()).unwrap();
    h.engine
        .perform(CommandInvocation::new("tab.next"), Via::Keyboard)
        .unwrap();
    assert_eq!(h.engine.active_tab().unwrap().id(), tc.id());
    h.engine
        .perform(
            CommandInvocation::new("tab.select").with_arg(CommandArg::Index(1)),
            Via::Keyboard,
        )
        .unwrap();
    assert_eq!(h.engine.active_tab().unwrap().location(), h.s("a"));
    h.engine
        .perform(CommandInvocation::new("tab.close_others"), Via::Menu)
        .unwrap();
    assert_eq!(order(), ["a"]);
    h.engine
        .perform(CommandInvocation::new("tab.close"), Via::Keyboard)
        .unwrap();
    assert!(h.engine.workspace_state().windows.is_empty());
    assert!(h.engine.reopen_closed_tab().unwrap().is_some());
    assert_eq!(h.engine.workspace_state().windows.len(), 1);
}

#[test]
fn saved_workspaces() {
    let h = harness();
    mkdirs(&h.p("w1"));
    mkdirs(&h.p("w2"));
    h.window_at("w1");
    h.engine.open_tab(Some(h.s("w2"))).unwrap();
    let id = h.engine.save_workspace("Work").unwrap();
    assert!(h.engine.save_workspace("  ").is_err());
    let list = h.engine.saved_workspaces().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].name.as_str(), list[0].tabs), ("Work", 2));
    // Saving again under the same name replaces it.
    assert_eq!(h.engine.save_workspace("Work").unwrap(), id);

    // Change the session, then open the saved workspace.
    let w = h.engine.workspace_state().windows[0].id;
    h.engine.close_window(w).unwrap();
    h.engine.new_window(Some(h.s(""))).unwrap();
    let ws = h.engine.open_workspace(id).unwrap();
    let locs: Vec<&str> = ws.windows[0].panes[0]
        .tabs
        .iter()
        .map(|t| t.location.as_str())
        .collect();
    assert_eq!(locs, [h.s("w1"), h.s("w2")]);
    h.engine.rename_workspace(id, "Home").unwrap();
    assert_eq!(h.engine.saved_workspaces().unwrap()[0].name, "Home");
    // Switching by number.
    h.engine
        .perform(
            CommandInvocation::new("workspace.switch").with_arg(CommandArg::Index(0)),
            Via::Palette,
        )
        .unwrap();
    h.engine.delete_workspace(id).unwrap();
    assert!(h.engine.saved_workspaces().unwrap().is_empty());
    assert!(h.engine.open_workspace(id).is_err());
}

#[test]
fn stale_navigation_results_are_dropped() {
    let h = harness();
    mkdirs(&h.p("big"));
    mkdirs(&h.p("small"));
    for i in 0..20_000 {
        touch(&h.p(&format!("big/{i}")));
    }
    touch(&h.p("small/only"));
    let t = h.window_at("");
    for _ in 0..5 {
        t.go("big").unwrap();
        t.go("../small").unwrap();
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(300) {
            let l = t.listing();
            // Whatever is published must belong to the newest navigation.
            if l.version() > 0 && l.state() != ListingState::Loading {
                assert_eq!(l.dir(), h.s("small"));
                assert_eq!(l.names(), ["only"]);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(t.listing().names(), ["only"]);
        t.go("..").unwrap();
        wait_complete(&t);
    }
}

#[test]
fn settings_round_trip_and_validation() {
    let mut h = harness();
    h.drain();
    assert_eq!(h.engine.setting("view.show_hidden").unwrap(), "false");
    h.engine.set_setting("view.show_hidden", "true").unwrap();
    h.expect_event("settings changed", |e| {
        matches!(e, CoreEvent::SettingsChanged)
    });
    h.engine.set_setting("keymap.profile", "Vim").unwrap();
    assert_eq!(h.engine.vim_mode(), VimMode::Normal);
    assert!(
        h.engine
            .set_setting("operations.file_workers", "99")
            .is_err()
    );
    assert!(h.engine.set_setting("no.such", "1").is_err());
    let exported = h.engine.export_settings();
    h.relaunch();
    let s = h.engine.settings();
    assert!(s.view.show_hidden);
    assert_eq!(s.keymap.profile, Profile::Vim);
    assert_eq!(h.engine.export_settings(), exported);
    // Import of an older-style document with missing keys fills defaults.
    h.engine
        .import_settings(r#"{"version":1,"settings":{"general":{"confirm_trash":true}}}"#)
        .unwrap();
    let s = h.engine.settings();
    assert!(s.general.confirm_trash && !s.view.show_hidden);
    // Hidden files follow the view setting for new tabs.
    touch(&h.p(".dot"));
    h.engine.set_setting("view.show_hidden", "true").unwrap();
    let t = h.window_at("");
    assert_eq!(names(&t), [".dot"]);
}

#[test]
fn unreadable_folder_is_an_error_listing_and_symlink_loops_list_fine() {
    let h = harness();
    mkdirs(&h.p("locked"));
    std::os::unix::fs::symlink(h.p("loop"), h.p("loop")).unwrap();
    std::os::unix::fs::symlink(h.p("locked"), h.p("link")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(h.p("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();
    let t = h.window_at("");
    assert_eq!(names(&t), ["locked", "link", "loop"]);
    let rows = t.rows(0, 3);
    assert!(rows[1].is_symlink && rows[2].is_symlink);
    t.go("locked").unwrap();
    wait_complete(&t);
    let l = t.listing();
    std::fs::set_permissions(h.p("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
    if unsafe_is_root() {
        return;
    }
    assert_eq!(l.state(), ListingState::Error);
    assert!(l.error().is_some());
}

fn unsafe_is_root() -> bool {
    std::env::var("USER").as_deref() == Ok("root")
}

#[test]
fn completion_and_breadcrumbs() {
    let h = harness();
    mkdirs(&h.p("Projects/alpha"));
    mkdirs(&h.p("Projects/beta"));
    let t = h.window_at("");
    let typed = format!("{}/Proj", h.s(""));
    let c = t.complete(&typed, typed.len() as u32);
    assert_eq!(c.items[0].display_name, "Projects");
    let c = t.complete("Projects/a", 10);
    assert_eq!(c.items[0].display_name, "alpha");
    t.go("Projects/alpha").unwrap();
    wait_complete(&t);
    let crumbs = t.breadcrumbs();
    let last = crumbs.last().unwrap();
    assert_eq!(last.name, "alpha");
    assert!(last.is_current);
    assert!(crumbs.iter().any(|c| c.kind == CrumbKind::Home));
    assert_eq!(
        h.engine
            .copy_path_as(&t.location(), PathStyle::HomeRelative),
        "~/Projects/alpha"
    );
}

#[test]
fn hints_and_held_shortcuts() {
    let h = harness();
    touch(&h.p("a"));
    h.window_at("");
    h.drain();
    // A mouse use of a command with a shortcut hints once.
    let out = h
        .engine
        .perform(CommandInvocation::new("view.show_hidden"), Via::Mouse)
        .unwrap();
    let hint = out.hint.unwrap();
    assert_eq!(hint.command, "view.show_hidden");
    h.expect_event("hint", |e| matches!(e, CoreEvent::Hint { .. }));
    // Keyboard uses count toward "learned".
    for _ in 0..3 {
        h.engine
            .perform(CommandInvocation::new("select.all"), Via::Keyboard)
            .unwrap();
    }
    assert!(h.engine.is_learned("select.all"));
    let held = h
        .engine
        .held_shortcuts(
            Mods {
                cmd: true,
                ..Mods::NONE
            },
            InputContext::default(),
        )
        .unwrap();
    assert!(held.rows.iter().any(|r| r.id.as_str() == "select.all"));
    assert!(h.engine.keymap_conflicts().is_empty());
}

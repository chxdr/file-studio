//! Regression tests for docs/SAFETY-AUDIT.md (S1–S14). Each test is one of
//! the audit's probes (pN) turned around: it failed on the old behaviour and
//! passes on the fixed one. Everything runs in a temporary home with a
//! folder Trash.

mod common;

use std::time::Duration;

use common::*;
use fstudio_core::*;

fn ok(h: &H, out: &Outcome) {
    for id in &out.ops {
        h.engine.wait_op(*id, T).unwrap();
    }
}

fn run(h: &H, t: &Tab, id: &str, arg: Option<CommandArg>) -> Result<Outcome> {
    h.engine.perform(
        CommandInvocation {
            id: id.into(),
            arg,
            tab: Some(t.id()),
        },
        Via::Keyboard,
    )
}

fn dir_names(p: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(p)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

// ───────────────────────── S1 ─────────────────────────

/// p13: the sheet says "Delete junk.tmp immediately?"; a paste lands while
/// it is up. Yes deletes junk.tmp only, never the pasted thesis.
#[test]
fn s1_p13_confirmed_delete_acts_only_on_what_the_dialog_named() {
    let h = harness();
    h.engine
        .set_setting("operations.allow_delete_immediately", "true")
        .unwrap();
    mkdirs(&h.p("work"));
    mkdirs(&h.p("other"));
    touch(&h.p("work/junk.tmp"));
    std::fs::write(h.p("other/thesis.pdf"), vec![1u8; 2 << 20]).unwrap();
    let t = h.window_at("work");
    h.engine.set_clipboard(vec![h.s("other/thesis.pdf")], false);
    let paste = run(&h, &t, "edit.paste", None).unwrap();
    t.set_selection(SelectionChange::Names(vec!["junk.tmp".into()]));
    let ask = run(&h, &t, "file.delete_immediately", None).unwrap();
    let Some(ShellRequest::ConfirmDeleteImmediately { token, paths }) = ask.shell else {
        panic!("expected the question, got {:?}", ask.shell);
    };
    assert_eq!(paths, vec![h.s("work/junk.tmp")]);
    ok(&h, &paste);
    eventually("thesis listed", || names(&t).contains(&"thesis.pdf".into()));
    // Whatever is selected now…
    t.set_selection(SelectionChange::All);
    // …the old way of answering is refused…
    assert!(
        run(
            &h,
            &t,
            "file.delete_immediately",
            Some(CommandArg::Confirmed)
        )
        .is_err()
    );
    // …and the token acts only on junk.tmp.
    let out = h.engine.confirm(token).unwrap();
    ok(&h, &out);
    assert_eq!(dir_names(&h.p("work")), vec!["thesis.pdf".to_string()]);
}

/// The same for Move to Trash with the question on (and Vim `x`, which
/// always asks): only the named items go, and a replaced item is refused.
#[test]
fn s1_confirmed_trash_refuses_a_changed_item_and_ignores_the_selection() {
    let h = harness();
    for n in ["a.txt", "b.txt", "c.txt"] {
        touch(&h.p(n));
    }
    let t = h.window_at("");
    t.set_selection(SelectionChange::Names(vec!["a.txt".into()]));
    // Vim `x`: file.trash with Confirm always asks.
    let ask = run(&h, &t, "file.trash", Some(CommandArg::Confirm)).unwrap();
    let Some(ShellRequest::ConfirmTrash { token, .. }) = ask.shell else {
        panic!("expected the question");
    };
    t.set_selection(SelectionChange::Names(vec!["b.txt".into(), "c.txt".into()]));
    let out = h.engine.confirm(token).unwrap();
    ok(&h, &out);
    assert_eq!(dir_names(&h.p("")), vec!["b.txt", "c.txt"]);
    assert_eq!(dir_names(&h.trash), vec!["a.txt"]);

    // Asked about b.txt; b.txt is replaced by another file before the yes.
    h.engine
        .set_setting("general.confirm_trash", "true")
        .unwrap();
    t.set_selection(SelectionChange::Names(vec!["b.txt".into()]));
    let Some(ShellRequest::ConfirmTrash { token, .. }) =
        run(&h, &t, "file.trash", None).unwrap().shell
    else {
        panic!("expected the question");
    };
    std::fs::rename(h.p("b.txt"), h.p("b-old.txt")).unwrap();
    std::fs::write(h.p("b.txt"), b"new work").unwrap();
    let out = h.engine.confirm(token).unwrap();
    assert!(out.ops.is_empty(), "nothing is queued");
    assert!(out.message.unwrap().contains("changed"));
    assert_eq!(std::fs::read(h.p("b.txt")).unwrap(), b"new work");
}

/// A protected move's yes moves exactly the items it named (F6 here), even
/// when the selection changed meanwhile.
#[test]
fn s1_confirmed_protected_move_uses_the_named_items() {
    let h = harness();
    mkdirs(&h.p("Downloads"));
    mkdirs(&h.p("dst"));
    touch(&h.p("other.txt"));
    let t = h.window_at("");
    let w = h.engine.active_tab().unwrap();
    h.engine
        .toggle_dual(h.engine.workspace_state().windows[0].id)
        .unwrap();
    let o = h.engine.other_pane_tab(w.id()).unwrap();
    o.go(&h.s("dst")).unwrap();
    wait_complete(&o);
    t.set_selection(SelectionChange::Names(vec!["Downloads".into()]));
    let ask = run(&h, &t, "file.move_to_other_pane", None).unwrap();
    let Some(ShellRequest::ConfirmMove { token, .. }) = ask.shell else {
        panic!("expected the protected-move question, got {:?}", ask.shell);
    };
    t.set_selection(SelectionChange::Names(vec!["other.txt".into()]));
    let out = h.engine.confirm(token).unwrap();
    ok(&h, &out);
    assert!(h.p("dst/Downloads").is_dir());
    assert!(
        h.p("other.txt").exists(),
        "the later selection is untouched"
    );
    std::thread::sleep(Duration::from_millis(50));
}

// ───────────────────────── S2 ─────────────────────────

/// p6: `drop_files` and `drop_items` (list, crumbs, and drops from Finder,
/// which arrive as plain file URLs) never move a protected folder without
/// asking, and every move they queue says so.
#[test]
fn s2_p6_drops_of_protected_folders_ask_and_moves_show_a_toast() {
    let h = harness();
    for d in ["Downloads", "Documents", "Import Studio", "plain"] {
        mkdirs(&h.p(d));
    }
    let _t = h.window_at("Import Studio");
    // A Finder drag of ~/Downloads onto a crumb.
    let out = h
        .engine
        .drop_files(vec![h.p("Downloads")], h.p("Import Studio"), Mods::NONE)
        .unwrap();
    assert!(
        matches!(out.shell, Some(ShellRequest::ConfirmMove { .. })),
        "{:?}",
        out.shell
    );
    assert!(out.ops.is_empty());
    // A Finder drag of ~/Documents onto the list.
    let out = h
        .engine
        .drop_items(&[h.s("Documents")], &h.s("Import Studio"), Mods::NONE)
        .unwrap();
    assert!(matches!(out.shell, Some(ShellRequest::ConfirmMove { .. })));
    assert!(out.ops.is_empty());
    std::thread::sleep(Duration::from_millis(100));
    assert!(h.p("Downloads").is_dir() && h.p("Documents").is_dir());
    // An ordinary folder moves, and says so.
    let out = h
        .engine
        .drop_items(&[h.s("plain")], &h.s("Import Studio"), Mods::NONE)
        .unwrap();
    ok(&h, &out);
    assert!(matches!(
        out.shell,
        Some(ShellRequest::Moved { ref text }) if text == "Moved “plain” to “Import Studio”"
    ));
    // A refused drop is an error with a sentence for the shell to show.
    let e = h
        .engine
        .drop_items(&[h.s("Import Studio")], &h.s("Import Studio"), Mods::NONE)
        .unwrap_err();
    assert!(e.to_string().contains("dropped"), "{e}");
}

// ───────────────────────── S3 ─────────────────────────

/// p1: undo the incident's move, relaunch: there is nothing to redo.
#[test]
fn s3_p1_redo_does_not_survive_a_relaunch() {
    let mut h = harness();
    mkdirs(&h.p("Downloads/keep"));
    mkdirs(&h.p("Projects"));
    let id = h
        .engine
        .enqueue(OpRequest::Move {
            sources: vec![h.p("Downloads")],
            dest_dir: h.p("Projects"),
        })
        .unwrap();
    h.engine.wait_op(id, T).unwrap();
    let out = h.engine.undo().unwrap();
    ok(&h, &out);
    assert!(h.p("Downloads/keep").exists());
    assert!(h.engine.undo_state().redo.is_some());
    h.relaunch();
    assert_eq!(h.engine.undo_state().redo, None, "the redo stack is gone");
    let out = h.engine.redo().unwrap();
    assert!(out.ops.is_empty());
    std::thread::sleep(Duration::from_millis(100));
    assert!(h.p("Downloads/keep").exists() && !h.p("Projects/Downloads").exists());
}

/// p1 without the relaunch: redoing the move of a protected folder asks
/// first (F-023); No leaves it, Yes redoes it.
#[test]
fn s3_redo_of_a_protected_move_asks_first() {
    let h = harness();
    mkdirs(&h.p("Downloads/keep"));
    mkdirs(&h.p("Projects"));
    let id = h
        .engine
        .enqueue(OpRequest::Move {
            sources: vec![h.p("Downloads")],
            dest_dir: h.p("Projects"),
        })
        .unwrap();
    h.engine.wait_op(id, T).unwrap();
    ok(&h, &h.engine.undo().unwrap());
    let out = h.engine.redo().unwrap();
    let Some(ShellRequest::ConfirmMove { token, title, .. }) = out.shell else {
        panic!("expected a question, got {:?}", out.shell);
    };
    assert_eq!(title, "Move “Downloads” into “Projects”?");
    assert!(out.ops.is_empty());
    h.engine.cancel_confirmation(token);
    assert!(h.p("Downloads/keep").exists());
    // Asked again; yes redoes it.
    let Some(ShellRequest::ConfirmMove { token, .. }) = h.engine.redo().unwrap().shell else {
        panic!("expected a question");
    };
    let out = h.engine.confirm(token).unwrap();
    ok(&h, &out);
    assert!(h.p("Projects/Downloads/keep").exists());
}

/// p2: trash, undo, then the user renames the file and saves new work
/// under the old name. Redo refuses instead of trashing the new work.
#[test]
fn s3_p2_redo_refuses_a_different_file_at_the_same_path() {
    let mut h = harness();
    std::fs::write(h.p("Report.txt"), b"OLD").unwrap();
    let id = h
        .engine
        .enqueue(OpRequest::Trash {
            paths: vec![h.p("Report.txt")],
        })
        .unwrap();
    h.engine.wait_op(id, T).unwrap();
    ok(&h, &h.engine.undo().unwrap());
    std::fs::rename(h.p("Report.txt"), h.p("Report-old.txt")).unwrap();
    std::fs::write(h.p("Report.txt"), b"NEW WORK").unwrap();
    let out = h.engine.redo().unwrap();
    assert!(out.ops.is_empty(), "nothing queued");
    assert_eq!(
        out.message.as_deref(),
        Some("Can’t redo: “Report.txt” changed since it was put back")
    );
    assert_eq!(std::fs::read(h.p("Report.txt")).unwrap(), b"NEW WORK");
    // The audit's version, across a relaunch: nothing to redo at all.
    h.relaunch();
    ok(&h, &h.engine.redo().unwrap());
    assert_eq!(std::fs::read(h.p("Report.txt")).unwrap(), b"NEW WORK");
}

/// An unchanged item is redone as before.
#[test]
fn s3_redo_of_an_unchanged_item_still_works() {
    let h = harness();
    touch(&h.p("a.txt"));
    let id = h
        .engine
        .enqueue(OpRequest::Trash {
            paths: vec![h.p("a.txt")],
        })
        .unwrap();
    h.engine.wait_op(id, T).unwrap();
    ok(&h, &h.engine.undo().unwrap());
    assert!(h.p("a.txt").exists());
    let out = h.engine.redo().unwrap();
    assert_eq!(out.ops.len(), 1);
    ok(&h, &out);
    assert!(!h.p("a.txt").exists());
}

// ───────────────────────── S4 ─────────────────────────

/// p5: ⌘V a big file, click your own file right away. When the copy lands
/// the selection is still just your file, so ⌘⌫ trashes only that.
#[test]
fn s4_p5_a_click_cancels_the_pending_paste_selection() {
    let h = harness();
    mkdirs(&h.p("work"));
    mkdirs(&h.p("other"));
    touch(&h.p("work/mine.txt"));
    std::fs::write(h.p("other/pasted.bin"), vec![7u8; 4 << 20]).unwrap();
    let t = h.window_at("work");
    h.engine.set_clipboard(vec![h.s("other/pasted.bin")], false);
    let out = run(&h, &t, "edit.paste", None).unwrap();
    t.set_selection(SelectionChange::Names(vec!["mine.txt".into()]));
    ok(&h, &out);
    eventually("pasted item listed", || {
        names(&t).contains(&"pasted.bin".to_string())
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(t.selection().names, vec!["mine.txt".to_string()]);
    ok(&h, &run(&h, &t, "file.trash", None).unwrap());
    assert_eq!(dir_names(&h.trash), vec!["mine.txt"]);
    assert!(h.p("work/pasted.bin").exists());
}

/// Without a click, the pasted items are selected when they land, and the
/// pending selection replaces the selection (it never merges).
#[test]
fn s4_pending_selection_replaces_when_untouched() {
    let h = harness();
    mkdirs(&h.p("work"));
    mkdirs(&h.p("other"));
    touch(&h.p("work/mine.txt"));
    touch(&h.p("other/a.txt"));
    touch(&h.p("other/b.txt"));
    let t = h.window_at("work");
    h.engine
        .set_clipboard(vec![h.s("other/a.txt"), h.s("other/b.txt")], false);
    t.set_selection(SelectionChange::Names(vec!["mine.txt".into()]));
    let out = run(&h, &t, "edit.paste", None).unwrap();
    ok(&h, &out);
    eventually("both pasted items selected", || {
        let mut n = t.selection().names;
        n.sort();
        n == ["a.txt", "b.txt"]
    });
    // A right-click on an already-selected row also counts as the user's
    // choice: a later pending selection is cancelled by it.
    touch(&h.p("other/c.txt"));
    h.engine.set_clipboard(vec![h.s("other/c.txt")], false);
    let out = run(&h, &t, "edit.paste", None).unwrap();
    let row = t.listing().index_of("mine.txt").unwrap();
    t.set_selection(SelectionChange::Only(row));
    h.engine.menu_target_at(t.id(), Some(row)).unwrap();
    ok(&h, &out);
    eventually("c listed", || names(&t).contains(&"c.txt".to_string()));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(t.selection().names, vec!["mine.txt".to_string()]);
}

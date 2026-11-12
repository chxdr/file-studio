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

// ───────────────────────── S5 ─────────────────────────

fn insensitive(h: &H) -> bool {
    touch(&h.p("CaseProbe"));
    let yes = h.p("caseprobe").exists();
    std::fs::remove_file(h.p("CaseProbe")).unwrap();
    yes
}

/// p12: after a case-only and an NFD → NFC rename, the list shows each item
/// once, under its new name (no ghost of the old name).
#[test]
fn s5_p12_no_ghost_rows_after_case_or_normalisation_renames() {
    let h = harness();
    touch(&h.p("Notes.txt"));
    touch(&h.p("Cafe\u{301}.txt"));
    let t = h.window_at("");
    ok(
        &h,
        &h.engine
            .rename_item(t.id(), "Notes.txt", "NOTES.txt")
            .unwrap(),
    );
    ok(
        &h,
        &h.engine
            .rename_item(t.id(), "Cafe\u{301}.txt", "Caf\u{e9}.txt")
            .unwrap(),
    );
    eventually("both renames listed", || {
        let n = names(&t);
        n.contains(&"NOTES.txt".to_string()) && n.contains(&"Caf\u{e9}.txt".to_string())
    });
    // Give the watcher time to (wrongly) bring the old names back.
    std::thread::sleep(Duration::from_millis(1500));
    let mut n = names(&t);
    n.sort();
    assert_eq!(
        n,
        vec!["Caf\u{e9}.txt".to_string(), "NOTES.txt".to_string()]
    );
}

/// p10: the user can't trash "the old one" and hit the renamed file. With
/// no ghost there is no old row; and a stale name (renamed on disk before
/// the list caught up) is refused by the on-disk name check.
#[test]
fn s5_p10_acting_on_a_stale_name_never_hits_the_renamed_file() {
    let h = harness();
    touch(&h.p("File.txt"));
    let t = h.window_at("");
    ok(
        &h,
        &h.engine
            .rename_item(t.id(), "File.txt", "file.txt")
            .unwrap(),
    );
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(names(&t), vec!["file.txt".to_string()]);
    if !insensitive(&h) {
        return;
    }
    // Rename behind the list's back and act on the old row straight away.
    touch(&h.p("Notes.txt"));
    eventually("listed", || names(&t).contains(&"Notes.txt".to_string()));
    std::fs::rename(h.p("Notes.txt"), h.p("NOTES.txt")).unwrap();
    t.set_selection(SelectionChange::Names(vec!["Notes.txt".into()]));
    if let Ok(out) = run(&h, &t, "file.trash", None) {
        ok(&h, &out);
    }
    std::thread::sleep(Duration::from_millis(200));
    assert!(h.p("NOTES.txt").exists(), "the renamed file is untouched");
    assert!(dir_names(&h.trash).is_empty());
}

// ───────────────────────── S6 ─────────────────────────

/// p15: holding ⌘⌫ (the first press, then auto-repeats) trashes one item,
/// not a1, a2, a3… Holding ↓ still moves the cursor on every repeat.
#[test]
fn s6_p15_held_keys_repeat_movement_only() {
    let h = harness();
    for n in ["a1", "a2", "a3", "a4", "a5"] {
        touch(&h.p(n));
    }
    let t = h.window_at("");
    t.set_selection(SelectionChange::Names(vec!["a1".into()]));
    let chord = fstudio_keymap::display::parse_chord("⌘⌫").unwrap();
    let ctx = |is_repeat| InputContext {
        focus: Focus::List,
        tab: Some(t.id()),
        is_repeat,
    };
    let mut ops = Vec::new();
    for k in 0..3 {
        let out = h.engine.handle_key(chord, ctx(k > 0)).unwrap();
        ops.extend(out.ops);
        std::thread::sleep(Duration::from_millis(80));
    }
    for id in &ops {
        h.engine.wait_op(*id, T).unwrap();
    }
    assert_eq!(ops.len(), 1);
    assert_eq!(dir_names(&h.trash), vec!["a1"]);
    // F6-style and creating commands don't repeat either.
    for keys in ["⌘D", "⇧⌘N"] {
        let c = fstudio_keymap::display::parse_chord(keys).unwrap();
        let out = h.engine.handle_key(c, ctx(true)).unwrap();
        assert!(out.ops.is_empty() && out.command.is_none(), "{keys}");
    }
    // ↓ repeats.
    wait_complete(&t);
    eventually("a1 gone", || !names(&t).contains(&"a1".to_string()));
    t.set_selection(SelectionChange::Names(vec!["a2".into()]));
    let down = fstudio_keymap::display::parse_chord("↓").unwrap();
    h.engine.handle_key(down, ctx(false)).unwrap();
    h.engine.handle_key(down, ctx(true)).unwrap();
    assert_eq!(t.selection().names, vec!["a4".to_string()]);
}

// ───────────────────────── S7 ─────────────────────────

/// p14: ⌘C in File Studio, later copy other files in Finder, then ⌥⌘V in
/// another folder. What is pasted is Finder's files, as a copy; the stale
/// in-app item is not moved.
#[test]
fn s7_p14_paste_uses_the_system_pasteboard_once_it_changed() {
    let h = harness();
    mkdirs(&h.p("a"));
    mkdirs(&h.p("b"));
    mkdirs(&h.p("finder"));
    touch(&h.p("a/old-copied.txt"));
    touch(&h.p("finder/from-finder.txt"));
    let t = h.window_at("a");
    h.engine.sync_pasteboard(1, vec![]);
    t.set_selection(SelectionChange::Names(vec!["old-copied.txt".into()]));
    let out = run(&h, &t, "edit.copy", None).unwrap();
    assert!(matches!(
        out.shell,
        Some(ShellRequest::WriteFilePasteboard { .. })
    ));
    h.engine.pasteboard_written(2);
    // Finder copies other files: the pasteboard changes.
    h.engine
        .sync_pasteboard(3, vec![h.s("finder/from-finder.txt")]);
    t.go(&h.s("b")).unwrap();
    wait_complete(&t);
    let out = run(&h, &t, "file.move_here", None).unwrap();
    ok(&h, &out);
    assert!(h.p("a/old-copied.txt").exists(), "the stale item stays");
    assert!(!h.p("b/old-copied.txt").exists());
    assert!(h.p("b/from-finder.txt").exists(), "Finder's file is pasted");
    assert!(
        h.p("finder/from-finder.txt").exists(),
        "…as a copy, even for Paste as Move"
    );
}

/// A cut expires when the pasteboard changes; while it doesn't, ⌘V moves.
#[test]
fn s7_a_cut_expires_when_the_pasteboard_changes() {
    let h = harness();
    mkdirs(&h.p("a"));
    mkdirs(&h.p("b"));
    touch(&h.p("a/x.txt"));
    touch(&h.p("a/y.txt"));
    let t = h.window_at("a");
    h.engine.sync_pasteboard(10, vec![]);
    t.set_selection(SelectionChange::Names(vec!["x.txt".into()]));
    let out = run(&h, &t, "edit.cut", None).unwrap();
    assert!(matches!(
        out.shell,
        Some(ShellRequest::WriteFilePasteboard { .. })
    ));
    h.engine.pasteboard_written(11);
    // Something else is copied (text in another app): the cut is over.
    h.engine.sync_pasteboard(12, vec![]);
    assert_eq!(h.engine.clipboard(), None);
    t.go(&h.s("b")).unwrap();
    wait_complete(&t);
    let out = run(&h, &t, "edit.paste", None).unwrap();
    assert!(out.ops.is_empty());
    assert!(h.p("a/x.txt").exists() && !h.p("b/x.txt").exists());
    // A cut the pasteboard still holds moves, and says so.
    t.go(&h.s("a")).unwrap();
    wait_complete(&t);
    t.set_selection(SelectionChange::Names(vec!["y.txt".into()]));
    run(&h, &t, "edit.cut", None).unwrap();
    h.engine.pasteboard_written(13);
    h.engine.sync_pasteboard(13, vec![h.s("a/y.txt")]);
    t.go(&h.s("b")).unwrap();
    wait_complete(&t);
    let out = run(&h, &t, "edit.paste", None).unwrap();
    ok(&h, &out);
    assert!(matches!(out.shell, Some(ShellRequest::Moved { .. })));
    assert!(h.p("b/y.txt").exists() && !h.p("a/y.txt").exists());
}

/// Paste as Move of a protected folder asks first, and the yes shows the toast.
#[test]
fn s7_move_here_of_a_protected_folder_asks_and_toasts() {
    let h = harness();
    mkdirs(&h.p("Documents"));
    mkdirs(&h.p("Projects"));
    let t = h.window_at("");
    h.engine.sync_pasteboard(1, vec![]);
    t.set_selection(SelectionChange::Names(vec!["Documents".into()]));
    run(&h, &t, "edit.copy", None).unwrap();
    h.engine.pasteboard_written(2);
    t.go(&h.s("Projects")).unwrap();
    wait_complete(&t);
    let out = run(&h, &t, "file.move_here", None).unwrap();
    let Some(ShellRequest::ConfirmMove { token, .. }) = out.shell else {
        panic!("expected a question, got {:?}", out.shell);
    };
    assert!(h.p("Documents").is_dir());
    let out = h.engine.confirm(token).unwrap();
    ok(&h, &out);
    assert!(matches!(
        out.shell,
        Some(ShellRequest::Moved { ref text }) if text == "Moved “Documents” to “Projects”"
    ));
    assert!(h.p("Projects/Documents").is_dir());
}

// ───────────────────────── S9 ─────────────────────────

/// p3: copy folder Site, save new work into the copy, ⌘Z. The undo is
/// refused with a message; the copy and the new work stay.
#[test]
fn s9_p3_undo_of_a_copy_refuses_when_work_was_added() {
    let h = harness();
    mkdirs(&h.p("src/Site/css"));
    touch(&h.p("src/Site/index.html"));
    touch(&h.p("src/Site/css/a.css"));
    mkdirs(&h.p("dst"));
    let id = h
        .engine
        .enqueue(OpRequest::Copy {
            sources: vec![h.p("src/Site")],
            dest_dir: h.p("dst"),
        })
        .unwrap();
    h.engine.wait_op(id, T).unwrap();
    std::fs::write(h.p("dst/Site/css/new-work.css"), b"hours of work").unwrap();
    let out = h.engine.undo().unwrap();
    assert!(out.ops.is_empty(), "nothing is queued");
    let msg = out.message.unwrap();
    assert!(msg.starts_with("Can’t undo:"), "{msg}");
    assert!(msg.contains("“Site”"), "{msg}");
    assert!(h.p("dst/Site/css/new-work.css").exists());
    assert!(dir_names(&h.trash).is_empty());
}

/// The same for Duplicate, and for a file inside the copy that was edited.
/// An untouched copy is still undone.
#[test]
fn s9_undo_of_duplicate_refuses_edited_contents_and_undoes_untouched_copies() {
    let h = harness();
    mkdirs(&h.p("Site"));
    std::fs::write(h.p("Site/index.html"), b"v1").unwrap();
    let id = h
        .engine
        .enqueue(OpRequest::Duplicate {
            paths: vec![h.p("Site")],
        })
        .unwrap();
    h.engine.wait_op(id, T).unwrap();
    assert_eq!(dir_names(&h.p("Site copy")), vec!["index.html"]);
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(h.p("Site copy/index.html"), b"edited, longer").unwrap();
    let out = h.engine.undo().unwrap();
    assert!(out.ops.is_empty());
    assert!(out.message.unwrap().contains("was changed"));
    assert!(h.p("Site copy/index.html").exists());
    // A fresh, untouched duplicate is undone.
    std::fs::remove_dir_all(h.p("Site copy")).unwrap();
    let id = h
        .engine
        .enqueue(OpRequest::Duplicate {
            paths: vec![h.p("Site")],
        })
        .unwrap();
    h.engine.wait_op(id, T).unwrap();
    let out = h.engine.undo().unwrap();
    ok(&h, &out);
    assert!(!h.p("Site copy").exists());
}

// ───────────────────────── S10 ─────────────────────────

/// p4: trash x/a.txt here, empty the Trash in Finder, trash another a.txt
/// from Finder, then ⌘Z here. The other a.txt is not put into x/.
#[test]
fn s10_p4_undo_of_trash_refuses_a_different_item_with_the_same_trash_name() {
    let h = harness();
    mkdirs(&h.p("x"));
    mkdirs(&h.p("y"));
    std::fs::write(h.p("x/a.txt"), b"v1").unwrap();
    let id = h
        .engine
        .enqueue(OpRequest::Trash {
            paths: vec![h.p("x/a.txt")],
        })
        .unwrap();
    h.engine.wait_op(id, T).unwrap();
    std::fs::remove_file(h.trash.join("a.txt")).unwrap();
    std::fs::write(h.p("y/a.txt"), b"v2 from another folder").unwrap();
    std::fs::rename(h.p("y/a.txt"), h.trash.join("a.txt")).unwrap();
    let out = h.engine.undo().unwrap();
    assert!(out.ops.is_empty());
    let msg = out.message.unwrap();
    assert!(msg.contains("no longer in the Trash"), "{msg}");
    assert!(!h.p("x/a.txt").exists());
    assert_eq!(
        std::fs::read(h.trash.join("a.txt")).unwrap(),
        b"v2 from another folder"
    );
}

/// The item itself is put back as before.
#[test]
fn s10_undo_of_trash_puts_back_the_same_item() {
    let h = harness();
    std::fs::write(h.p("a.txt"), b"mine").unwrap();
    let id = h
        .engine
        .enqueue(OpRequest::Trash {
            paths: vec![h.p("a.txt")],
        })
        .unwrap();
    h.engine.wait_op(id, T).unwrap();
    ok(&h, &h.engine.undo().unwrap());
    assert_eq!(std::fs::read(h.p("a.txt")).unwrap(), b"mine");
}

// ───────────────────────── S11 ─────────────────────────

fn toast(out: &Outcome) -> String {
    match &out.shell {
        Some(ShellRequest::Toast { text, .. }) | Some(ShellRequest::Moved { text }) => text.clone(),
        other => panic!("expected a toast, got {other:?}"),
    }
}

/// Trash, Undo and Redo always say what they did; undo and redo name the
/// folder (and "another window") when the window they ran in doesn't show it.
#[test]
fn s11_trash_undo_and_redo_show_toasts_naming_where() {
    let h = harness();
    mkdirs(&h.p("work"));
    mkdirs(&h.p("elsewhere"));
    for n in ["a.txt", "b.txt", "c.txt"] {
        touch(&h.p(&format!("work/{n}")));
    }
    let a = h.window_at("work");
    t_select_all(&a);
    let out = run(&h, &a, "file.trash", None).unwrap();
    assert!(matches!(out.shell, Some(ShellRequest::Moved { .. })));
    assert_eq!(toast(&out), "Moved 3 items to the Trash");
    ok(&h, &out);
    // Undo from the same window: no folder named.
    let out = run(&h, &a, "edit.undo", None).unwrap();
    assert_eq!(toast(&out), "Put back 3 items");
    assert!(matches!(
        out.shell,
        Some(ShellRequest::Toast { button: Some(ref b), command: Some(ref c), .. })
            if b == "Redo" && c == "edit.redo"
    ));
    ok(&h, &out);
    // Redo from a second window showing another folder.
    let b = h.window_at("elsewhere");
    let out = run(&h, &b, "edit.redo", None).unwrap();
    assert_eq!(
        toast(&out),
        "Redid: Move to Trash of 3 Items in “work” (another window)"
    );
    ok(&h, &out);
    // Undo with the folder shown nowhere.
    a.go(&h.s("elsewhere")).unwrap();
    wait_complete(&a);
    let out = run(&h, &b, "edit.undo", None).unwrap();
    assert_eq!(toast(&out), "Put back 3 items in “work”");
    ok(&h, &out);
    // A refused undo is a toast too.
    let out = run(&h, &b, "edit.undo", None).unwrap();
    assert!(out.ops.is_empty());
    assert_eq!(out.message.as_deref(), Some("Nothing to undo"));
}

fn t_select_all(t: &Tab) {
    t.set_selection(SelectionChange::All);
}

// ───────────────────────── S12 ─────────────────────────

/// p7: New Folder with Selection is one op the shell can follow, with one
/// undo entry that puts the items back and removes the folder.
#[test]
fn s12_p7_new_folder_with_selection_is_one_op_with_one_undo() {
    let h = harness();
    touch(&h.p("a"));
    touch(&h.p("b"));
    let t = h.window_at("");
    t.set_selection(SelectionChange::Names(vec!["a".into(), "b".into()]));
    let out = run(&h, &t, "file.new_folder_with_selection", None).unwrap();
    assert_eq!(out.ops.len(), 1);
    assert!(matches!(
        out.shell,
        Some(ShellRequest::BeginRename { ref name, .. }) if name == "New Folder With Items"
    ));
    ok(&h, &out);
    assert!(h.p("New Folder With Items/a").exists() && h.p("New Folder With Items/b").exists());
    assert_eq!(
        h.engine.undo_state().undo.as_deref(),
        Some("Undo New Folder with Items")
    );
    let u = h.engine.undo().unwrap();
    ok(&h, &u);
    assert!(h.p("a").exists() && h.p("b").exists());
    assert!(!h.p("New Folder With Items").exists());
    assert_eq!(h.engine.undo_state().undo, None);
}

/// It goes through move protection: a protected folder in the selection asks.
#[test]
fn s12_new_folder_with_a_protected_folder_asks_first() {
    let h = harness();
    mkdirs(&h.p("Documents"));
    touch(&h.p("x"));
    let t = h.window_at("");
    t.set_selection(SelectionChange::Names(vec!["Documents".into(), "x".into()]));
    let out = run(&h, &t, "file.new_folder_with_selection", None).unwrap();
    let Some(ShellRequest::ConfirmMove { token, .. }) = out.shell else {
        panic!("expected a question, got {:?}", out.shell);
    };
    assert!(out.ops.is_empty());
    std::thread::sleep(Duration::from_millis(100));
    assert!(!h.p("New Folder With Items").exists());
    let out = h.engine.confirm(token).unwrap();
    ok(&h, &out);
    assert!(h.p("New Folder With Items/Documents").is_dir());
}

// ───────────────────────── S13 ─────────────────────────

/// p8: "Create 3 Folders" for a mistyped path is one journaled op; one ⌘Z
/// removes the whole chain. (The shell defaults the offer to "Go to Nearest
/// Folder" when more than one folder would be made, makes ⌘Return show the
/// offer, and ignores a held Return: S6.)
#[test]
fn s13_p8_a_chain_of_new_folders_is_one_op_and_one_undo() {
    let h = harness();
    let t = h.window_at("");
    let out = h
        .engine
        .create_folder_and_go(t.id(), &h.s("Documnets/Projects/2026"))
        .unwrap();
    assert_eq!(out.ops.len(), 1);
    ok(&h, &out);
    assert!(h.p("Documnets/Projects/2026").is_dir());
    assert_eq!(
        h.engine.undo_state().undo.as_deref(),
        Some("Undo New Folders “2026”")
    );
    eventually("navigated", || {
        t.location() == h.s("Documnets/Projects/2026")
    });
    t.go(&h.s("")).unwrap();
    wait_complete(&t);
    let u = h.engine.undo().unwrap();
    ok(&h, &u);
    assert!(!h.p("Documnets").exists(), "the whole chain is gone");
    assert_eq!(h.engine.undo_state().undo, None);
    // A single missing folder is a plain New Folder.
    let out = h.engine.create_folder_and_go(t.id(), &h.s("One")).unwrap();
    ok(&h, &out);
    assert_eq!(
        h.engine.undo_state().undo.as_deref(),
        Some("Undo New Folder")
    );
}

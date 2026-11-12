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

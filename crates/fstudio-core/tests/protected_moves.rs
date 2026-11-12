//! F-023 regression tests (incident 2026-11-12: reordering sidebar favourites
//! moved `~/Applications`, `~/Downloads` and a project folder). With real
//! folders in a temporary home:
//! - reordering, pinning and unpinning favourites never touch the disk;
//! - a move of a protected folder, by drop, F6 or cut/paste, stops for
//!   confirmation; Cancel leaves everything in place, Move moves it;
//! - every queued move says what moved where (the Undo toast).

mod common;

use std::path::Path;

use common::*;
use fstudio_core::*;

const T_OP: std::time::Duration = std::time::Duration::from_secs(10);

/// A temporary home with the incident's folders (and a few plain ones).
fn home() -> (H, std::sync::Arc<Tab>) {
    let h = harness();
    for d in [
        "Applications/Tool.app/Contents",
        "Downloads/installer",
        "Import Studio/src",
        "Documents",
        "Pictures",
        "Alpha",
        "Beta",
    ] {
        mkdirs(&h.p(d));
    }
    let t = h.window_at("");
    (h, t)
}

/// Every folder and file under the home, for "nothing changed" checks.
fn snapshot(root: &Path) -> Vec<String> {
    fn walk(p: &Path, root: &Path, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        for e in rd.flatten() {
            let path = e.path();
            out.push(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            );
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                walk(&path, root, out);
            }
        }
    }
    let mut v = Vec::new();
    walk(root, root, &mut v);
    v.sort();
    v
}

fn confirm_token(out: &Outcome) -> (u64, String) {
    match &out.shell {
        Some(ShellRequest::ConfirmMove { token, title, .. }) => (*token, title.clone()),
        other => panic!("expected ConfirmMove, got {other:?}"),
    }
}

fn wait_ops(h: &H, out: &Outcome) {
    for op in &out.ops {
        assert!(
            h.engine.ops().wait(*op, T_OP).is_some(),
            "op {op:?} didn't finish"
        );
    }
}

/// The incident's gesture: drag a favourite up or down past others and let
/// go over one. Only the favourites store changes; the disk never does.
#[test]
fn reordering_favourites_never_touches_the_filesystem() {
    let (h, _t) = home();
    for f in h.engine.favourites() {
        h.engine.remove_favourite(&f.path).unwrap();
    }
    for d in [
        "Applications",
        "Downloads",
        "Import Studio",
        "Documents",
        "Pictures",
    ] {
        h.engine.add_favourite(&h.s(d), None).unwrap();
    }
    let before = snapshot(&h.root);
    // Every reorder the sidebar can ask for, onto every slot.
    for d in ["Applications", "Downloads", "Import Studio"] {
        for i in 0..6 {
            h.engine.move_favourite(&h.s(d), i).unwrap();
            assert_eq!(
                snapshot(&h.root),
                before,
                "moving favourite {d} to {i} changed the disk"
            );
        }
    }
    h.engine
        .rename_favourite(&h.s("Downloads"), "Inbox")
        .unwrap();
    h.engine.remove_favourite(&h.s("Import Studio")).unwrap();
    h.engine
        .add_favourite(&h.s("Import Studio"), Some(0))
        .unwrap();
    assert_eq!(snapshot(&h.root), before);
    assert!(
        h.engine.ops().list().is_empty(),
        "no operation may be queued"
    );
}

#[test]
fn dropping_a_standard_folder_asks_first_and_cancel_keeps_it() {
    let (h, _t) = home();
    let before = snapshot(&h.root);
    let out = h
        .engine
        .drop_files(vec![h.p("Downloads")], h.p("Import Studio"), Mods::NONE)
        .unwrap();
    let (token, title) = confirm_token(&out);
    assert_eq!(title, "Move “Downloads” into “Import Studio”?");
    assert!(out.ops.is_empty());
    if let Some(ShellRequest::ConfirmMove { detail, .. }) = &out.shell {
        assert!(
            detail.starts_with("It’s one of your standard folders."),
            "{detail}"
        );
    }
    h.engine.cancel_move(token);
    assert_eq!(snapshot(&h.root), before);
    // A cancelled token can't be confirmed later.
    assert!(h.engine.confirm_move(token).is_err());
    // The list's entry point asks too (S2a), and moves nothing until yes.
    let out = h
        .engine
        .drop_items(&[h.s("Applications")], &h.s("Documents"), Mods::NONE)
        .unwrap();
    let (token, _) = confirm_token(&out);
    h.engine.cancel_confirmation(token);
    assert_eq!(snapshot(&h.root), before);
}

#[test]
fn confirming_moves_it_and_says_so() {
    let (h, _t) = home();
    let out = h
        .engine
        .drop_files(vec![h.p("Downloads")], h.p("Import Studio"), Mods::NONE)
        .unwrap();
    let (token, _) = confirm_token(&out);
    let out = h.engine.confirm_move(token).unwrap();
    assert_eq!(
        out.shell,
        Some(ShellRequest::Moved {
            text: "Moved “Downloads” to “Import Studio”".into()
        })
    );
    wait_ops(&h, &out);
    assert!(h.p("Import Studio/Downloads/installer").is_dir());
    assert!(!h.p("Downloads").exists());
    // ⌘Z puts it back.
    let out = h.engine.undo().unwrap();
    wait_ops(&h, &out);
    assert!(h.p("Downloads/installer").is_dir());
}

#[test]
fn an_ordinary_move_goes_ahead_with_a_moved_toast() {
    let (h, _t) = home();
    let out = h
        .engine
        .drop_files(vec![h.p("Alpha")], h.p("Beta"), Mods::NONE)
        .unwrap();
    assert_eq!(
        out.shell,
        Some(ShellRequest::Moved {
            text: "Moved “Alpha” to “Beta”".into()
        })
    );
    wait_ops(&h, &out);
    assert!(h.p("Beta/Alpha").is_dir());
    // A copy (⌥) of a protected folder is not a move: no question.
    let out = h
        .engine
        .drop_files(
            vec![h.p("Downloads")],
            h.p("Beta"),
            Mods {
                alt: true,
                ..Mods::NONE
            },
        )
        .unwrap();
    assert!(!matches!(out.shell, Some(ShellRequest::ConfirmMove { .. })));
    wait_ops(&h, &out);
    assert!(h.p("Downloads").is_dir() && h.p("Beta/Downloads").is_dir());
}

#[test]
fn f6_and_cut_paste_of_a_protected_folder_ask_first() {
    let (h, t) = home();
    let w = h.engine.workspace_state().windows[0].id;
    h.engine.toggle_dual(w).unwrap();
    let o = h.engine.other_pane_tab(t.id()).unwrap();
    o.go(&h.s("Pictures")).unwrap();
    wait_complete(&o);
    h.engine.activate_tab(t.id()).unwrap();
    let before = snapshot(&h.root);

    // F6.
    t.set_selection(SelectionChange::Names(vec!["Import Studio".into()]));
    let out = h
        .engine
        .perform(
            CommandInvocation::new("file.move_to_other_pane"),
            Via::Keyboard,
        )
        .unwrap();
    // Not a standard folder, but not protected either: it moves (and says so).
    assert!(
        matches!(out.shell, Some(ShellRequest::Moved { .. })),
        "{out:?}"
    );
    wait_ops(&h, &out);
    assert!(h.p("Pictures/Import Studio").is_dir());
    let undo = h.engine.undo().unwrap();
    wait_ops(&h, &undo);
    assert_eq!(snapshot(&h.root), before);
    eventually("listing back", || {
        t.listing().index_of("Downloads").is_some()
    });

    t.set_selection(SelectionChange::Names(vec!["Downloads".into()]));
    let out = h
        .engine
        .perform(
            CommandInvocation::new("file.move_to_other_pane"),
            Via::Keyboard,
        )
        .unwrap();
    let (token, title) = confirm_token(&out);
    assert_eq!(title, "Move “Downloads” into “Pictures”?");
    assert_eq!(snapshot(&h.root), before);
    let out = h.engine.confirm_move(token).unwrap();
    wait_ops(&h, &out);
    assert!(h.p("Pictures/Downloads").is_dir());
    let undo = h.engine.undo().unwrap();
    wait_ops(&h, &undo);
    assert_eq!(snapshot(&h.root), before);

    // Cut, then paste in another folder.
    eventually("listing back", || {
        t.listing().index_of("Applications").is_some()
    });
    t.set_selection(SelectionChange::Names(vec!["Applications".into()]));
    h.engine
        .perform(CommandInvocation::new("edit.cut"), Via::Keyboard)
        .unwrap();
    t.go(&h.s("Documents")).unwrap();
    wait_complete(&t);
    let out = h
        .engine
        .perform(CommandInvocation::new("edit.paste"), Via::Keyboard)
        .unwrap();
    let (token, title) = confirm_token(&out);
    assert_eq!(title, "Move “Applications” into “Documents”?");
    h.engine.cancel_move(token);
    assert_eq!(snapshot(&h.root), before);
}

#[test]
fn folders_open_in_tabs_and_the_setting_are_respected() {
    let (h, t) = home();
    // A tab inside Alpha protects Alpha (and the tab's own folder).
    mkdirs(&h.p("Alpha/inner"));
    let inner = h.engine.open_tab(Some(h.s("Alpha/inner"))).unwrap();
    wait_complete(&inner);
    let out = h
        .engine
        .drop_files(vec![h.p("Alpha")], h.p("Beta"), Mods::NONE)
        .unwrap();
    let (token, _) = confirm_token(&out);
    if let Some(ShellRequest::ConfirmMove { detail, .. }) = &out.shell {
        assert!(detail.starts_with("A tab is showing it"), "{detail}");
    }
    h.engine.cancel_move(token);
    let _ = t;
    // The list is a setting: emptied, a standard folder moves straight away…
    h.engine
        .set_setting("operations.protected_folders", "[]")
        .unwrap();
    let out = h
        .engine
        .drop_files(vec![h.p("Downloads")], h.p("Beta"), Mods::NONE)
        .unwrap();
    assert!(matches!(out.shell, Some(ShellRequest::Moved { .. })));
    wait_ops(&h, &out);
    // …and an added folder asks.
    h.engine
        .set_setting(
            "operations.protected_folders",
            &serde_json::to_string(&[h.s("Pictures")]).unwrap(),
        )
        .unwrap();
    let out = h
        .engine
        .drop_files(vec![h.p("Pictures")], h.p("Beta"), Mods::NONE)
        .unwrap();
    confirm_token(&out);
    assert!(h.engine.move_protection(&h.s("Pictures")).is_some());
    assert!(h.engine.move_protection(&h.s("Documents")).is_none());
    // The root of the disk is always protected.
    assert!(h.engine.move_protection("/").is_some());
}

#[test]
fn defaults_list_the_standard_and_system_folders() {
    let d = default_protected_folders();
    for f in [
        "~",
        "~/Desktop",
        "~/Documents",
        "~/Downloads",
        "~/Applications",
        "~/Library",
        "~/Movies",
        "~/Music",
        "~/Pictures",
        "~/Public",
        "/Applications",
        "/System",
        "/Library",
        "/Users",
    ] {
        assert!(d.contains(&f.to_string()), "{f}");
    }
}

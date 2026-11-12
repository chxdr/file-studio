//! Every registered command is either handled by the core or returns a
//! `ShellRequest`; the only exceptions are the explicit later-milestone list.

mod common;

use common::*;
use fstudio_core::*;
use fstudio_keymap::{ArgKind, registry};

fn arg_for(h: &H, kind: Option<ArgKind>) -> Option<CommandArg> {
    Some(match kind? {
        ArgKind::Path => CommandArg::Path(h.p("sub")),
        ArgKind::Index => CommandArg::Index(0),
        ArgKind::ViewMode => CommandArg::ViewMode(ViewMode::List),
        ArgKind::Char => CommandArg::Char('a'),
        ArgKind::Text => CommandArg::Text("Red".into()),
    })
}

#[test]
fn every_command_is_handled_or_returns_a_shell_request() {
    let h = harness();
    for d in [
        "sub",
        "other",
        "Desktop",
        "Documents",
        "Downloads",
        ".Trash",
        "Library/Mobile Documents/com~apple~CloudDocs",
    ] {
        mkdirs(&h.p(d));
    }
    touch(&h.p("file.txt"));
    touch(&h.p("second.txt"));
    touch(&h.p("other/clip.txt"));
    // A dual-pane window with a selection, saved so every command starts from it.
    let t = h.window_at("");
    let w = h.engine.workspace_state().windows[0].id;
    h.engine.toggle_dual(w).unwrap();
    let o = h.engine.other_pane_tab(t.id()).unwrap();
    o.go(&h.s("other")).unwrap();
    wait_complete(&o);
    h.engine.activate_tab(t.id()).unwrap();
    let ws = h.engine.save_workspace("baseline").unwrap();
    h.engine
        .perform(
            CommandInvocation::new("nav.mark_set").with_arg(CommandArg::Char('a')),
            Via::Keyboard,
        )
        .unwrap();

    let mut checked = 0;
    for info in registry::all() {
        let id = info.id.as_str();
        // Fresh start for each command.
        h.engine.open_workspace(ws).unwrap();
        let t = h.engine.active_tab().unwrap();
        wait_complete(&t);
        // Restore the files the previous command may have moved or trashed.
        for f in ["file.txt", "second.txt"] {
            if !h.p(f).exists() {
                touch(&h.p(f));
            }
        }
        eventually("file listed", || t.listing().index_of("file.txt").is_some());
        // Opening commands want a folder selected; the rest a file.
        let pick = if id.starts_with("nav.open") || id == "nav.show_package_contents" {
            "sub"
        } else {
            "file.txt"
        };
        t.set_selection(SelectionChange::Names(vec![pick.into()]));
        if !h.p("other/clip.txt").exists() {
            touch(&h.p("other/clip.txt"));
        }
        for f in ["clip.txt", "other/file.txt", "other/second.txt"] {
            let _ = std::fs::remove_file(h.p(f));
        }
        h.engine.set_clipboard(vec![h.s("other/clip.txt")], false);

        let inv = CommandInvocation {
            id: id.to_string(),
            arg: arg_for(&h, info.arg),
            tab: None,
        };
        let r = h.engine.perform(inv, Via::Menu);
        match Engine::handling(id) {
            Handling::Later => {
                assert!(LATER_COMMANDS.contains(&id));
                assert!(
                    matches!(r, Err(CoreError::NotAvailable(_))),
                    "{id}: later commands say so, got {r:?}"
                );
            }
            Handling::Shell => {
                let out = r.unwrap_or_else(|e| panic!("{id}: {e}"));
                assert!(
                    out.shell.is_some(),
                    "{id}: expected a ShellRequest, got {out:?}"
                );
                assert_eq!(out.command.as_deref(), Some(id));
            }
            Handling::Core => match r {
                Ok(out) => assert_eq!(out.command.as_deref(), Some(id)),
                // Refused by design (F-007): Delete Immediately is off by default.
                Err(CoreError::NotAvailable(m)) if id == "file.delete_immediately" => {
                    assert!(m.contains("turned off"));
                }
                Err(e) => panic!("{id}: {e}"),
            },
        }
        // Let queued operations finish before the next command resets things.
        let q = h.engine.ops();
        if !q.wait_idle(std::time::Duration::from_secs(5)) {
            panic!("{id}: operations did not finish: {:?}", q.list());
        }
        checked += 1;
    }
    assert_eq!(checked, registry::all().len());
    assert!(checked >= 128, "the registry has {checked} commands");
}

#[test]
fn later_commands_are_registered() {
    for id in LATER_COMMANDS {
        assert!(
            registry::get(id).is_some(),
            "{id} is not a registered command"
        );
    }
}

#[test]
fn trash_confirmation_can_be_confirmed() {
    let h = harness();
    touch(&h.p("keep-me"));
    let t = h.window_at("");
    t.set_selection(SelectionChange::Names(vec!["keep-me".into()]));
    let run = |arg: Option<CommandArg>| {
        h.engine.perform(
            CommandInvocation {
                id: "file.trash".into(),
                arg,
                tab: None,
            },
            Via::Menu,
        )
    };
    // Default: no question, straight to the Trash (as in Finder).
    assert!(!h.engine.settings().general.confirm_trash);
    h.engine
        .set_setting("general.confirm_trash", "true")
        .unwrap();
    // With the setting on, the core asks, however often it is performed.
    for arg in [None, Some(CommandArg::Confirm)] {
        let out = run(arg).unwrap();
        assert!(matches!(out.shell, Some(ShellRequest::ConfirmTrash { .. })));
        assert!(out.ops.is_empty());
        assert!(h.p("keep-me").exists());
    }
    // A bare "Confirmed" is refused: the yes must carry the question's token (S1).
    assert!(run(Some(CommandArg::Confirmed)).is_err());
    assert!(h.p("keep-me").exists());
    // "Yes" carries the action out, once.
    let Some(ShellRequest::ConfirmTrash { token, paths }) = run(None).unwrap().shell else {
        panic!("expected a question");
    };
    assert_eq!(paths, vec![h.s("keep-me")]);
    let out = h.engine.confirm(token).unwrap();
    assert!(
        matches!(out.shell, Some(ShellRequest::Moved { .. })),
        "the Trash toast (S11)"
    );
    h.engine.wait_op(out.ops[0], T).unwrap();
    assert!(!h.p("keep-me").exists());
    assert!(h.engine.confirm(token).is_err(), "a token works once");
}

#[test]
fn delete_immediately_only_when_enabled_and_confirmed() {
    let h = harness();
    touch(&h.p("doomed"));
    let t = h.window_at("");
    t.set_selection(SelectionChange::Names(vec!["doomed".into()]));
    let run = |arg: Option<CommandArg>| {
        h.engine.perform(
            CommandInvocation {
                id: "file.delete_immediately".into(),
                arg,
                tab: None,
            },
            Via::Keyboard,
        )
    };
    assert!(run(Some(CommandArg::Confirm)).is_err());
    assert!(h.p("doomed").exists());
    h.engine
        .set_setting("operations.allow_delete_immediately", "true")
        .unwrap();
    let out = run(None).unwrap();
    assert!(matches!(
        out.shell,
        Some(ShellRequest::ConfirmDeleteImmediately { .. })
    ));
    assert!(h.p("doomed").exists());
    // Even with `Confirm`, it only asks; the yes is the token.
    let Some(ShellRequest::ConfirmDeleteImmediately { token, .. }) =
        run(Some(CommandArg::Confirm)).unwrap().shell
    else {
        panic!("expected a question");
    };
    assert!(h.p("doomed").exists());
    // No: nothing happens, and the token is gone.
    h.engine.cancel_confirmation(token);
    assert!(h.engine.confirm(token).is_err());
    assert!(h.p("doomed").exists());
    let Some(ShellRequest::ConfirmDeleteImmediately { token, .. }) = run(None).unwrap().shell
    else {
        panic!("expected a question");
    };
    let out = h.engine.confirm(token).unwrap();
    h.engine.wait_op(out.ops[0], T).unwrap();
    assert!(!h.p("doomed").exists());
}

#[test]
fn keys_can_be_rebound_and_reset() {
    let h = harness();
    assert_eq!(
        h.engine.command_keys("file.duplicate"),
        vec!["⌘D".to_string()]
    );
    h.engine
        .set_command_key("file.duplicate", true, Some("⌃⌘D"))
        .unwrap();
    assert_eq!(
        h.engine.command_keys("file.duplicate"),
        vec!["⌃⌘D".to_string()]
    );
    assert!(
        h.engine
            .set_command_key("file.duplicate", true, Some("⌘Q"))
            .is_err()
    );
    assert!(h.engine.set_command_key("no.such", true, None).is_err());
    h.engine.reset_command_keys(Some("file.duplicate")).unwrap();
    assert_eq!(
        h.engine.command_keys("file.duplicate"),
        vec!["⌘D".to_string()]
    );
}

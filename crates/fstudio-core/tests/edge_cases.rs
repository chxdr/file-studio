//! Regression tests for the hands-on edge-case test (end of M2): toasts that
//! claimed success for a Trash that failed, a silent broken symlink,
//! duplicate favourites and drives named "/". Each failed on the old code.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use common::*;
use fstudio_core::*;

fn run(h: &H, t: &Tab, id: &str) -> Result<Outcome> {
    h.engine.perform(
        CommandInvocation {
            id: id.into(),
            arg: None,
            tab: Some(t.id()),
        },
        Via::Keyboard,
    )
}

fn ok(h: &H, out: &Outcome) {
    for id in &out.ops {
        h.engine.wait_op(*id, T).unwrap();
    }
}

fn chmod(p: &Path, mode: u32) {
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// Gives a mode back when the test ends, so the temp folder can be removed.
struct Restore<'a>(&'a Path, u32);
impl Drop for Restore<'_> {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(self.1));
    }
}

fn toast_parts(r: ShellRequest) -> (String, Option<String>, ToastStyle) {
    match r {
        ShellRequest::Toast {
            text,
            command,
            style,
            ..
        } => (text, command, style),
        ShellRequest::Moved { text } => (text, Some("edit.undo".into()), ToastStyle::Success),
        other => panic!("expected a toast, got {other:?}"),
    }
}

/// Moving a folder to a new parent needs write permission on the folder
/// itself (its ".." changes), so a mode-000 folder can't go to the Trash.
/// The toast used to say "Moved “locked” to the Trash" with Undo.
#[test]
fn a_trash_that_fails_says_so_without_undo() {
    let h = harness();
    mkdirs(&h.p("locked"));
    touch(&h.p("next.txt"));
    chmod(&h.p("locked"), 0o000);
    let _r = Restore(&h.root.join("locked"), 0o755);
    let t = h.window_at("");
    t.set_selection(SelectionChange::Names(vec!["locked".into()]));
    let out = run(&h, &t, "file.trash").unwrap();
    assert_eq!(out.shell, None, "no toast before the outcome is known");
    ok(&h, &out);
    let (text, command, style) = toast_parts(h.expect_toast());
    assert_eq!(
        text,
        "Couldn’t move “locked” to the Trash: permission denied"
    );
    assert_eq!(style, ToastStyle::Warning);
    assert_ne!(command.as_deref(), Some("edit.undo"), "nothing to undo");
    assert_eq!(command.as_deref(), Some("view.show_operations"));
    assert!(h.p("locked").is_dir());
    assert_eq!(h.engine.undo_state().undo, None);
    // The cursor had moved on to the next row; the item that stayed is
    // selected again, so a second ⌘⌫ retries it instead of trashing that.
    eventually("locked selected again", || {
        t.selection().names == vec!["locked".to_string()]
    });
}

/// Four items, one locked: "Moved 3 items to the Trash · 1 couldn’t be
/// moved" (it used to say "Moved 4 items to the Trash").
#[test]
fn a_partly_failed_trash_counts_what_moved() {
    let h = harness();
    for n in ["a.txt", "b.txt", "c.txt"] {
        touch(&h.p(n));
    }
    mkdirs(&h.p("locked"));
    chmod(&h.p("locked"), 0o000);
    let _r = Restore(&h.root.join("locked"), 0o755);
    let t = h.window_at("");
    t.set_selection(SelectionChange::All);
    let out = run(&h, &t, "file.trash").unwrap();
    ok(&h, &out);
    let (text, command, style) = toast_parts(h.expect_toast());
    assert_eq!(text, "Moved 3 items to the Trash · 1 couldn’t be moved");
    assert_eq!(style, ToastStyle::Warning);
    assert_eq!(command.as_deref(), Some("view.show_operations"));
    // Details opens the queue.
    let d = h
        .engine
        .perform(CommandInvocation::new("view.show_operations"), Via::Mouse)
        .unwrap();
    assert_eq!(d.shell, Some(ShellRequest::ShowOperations));
}

/// Every op kind that toasts is built from its outcome: copy, move,
/// Delete Immediately and undo each say what didn't happen.
#[test]
fn copy_move_delete_and_undo_failures_say_so() {
    let h = harness();
    mkdirs(&h.p("dst"));
    std::fs::write(h.p("secret.txt"), b"s").unwrap();
    chmod(&h.p("secret.txt"), 0o000);
    let _r1 = Restore(&h.root.join("secret.txt"), 0o644);
    // Copy of an unreadable file.
    let id = h
        .engine
        .enqueue(OpRequest::Copy {
            sources: vec![h.p("secret.txt")],
            dest_dir: h.p("dst"),
        })
        .unwrap();
    h.engine.wait_op(id, T).unwrap();
    let (text, _, style) = toast_parts(h.expect_toast());
    assert_eq!(
        text,
        "Couldn’t copy “secret.txt” to “dst”: permission denied"
    );
    assert_eq!(style, ToastStyle::Warning);

    // A move (F6/drag) of a locked folder.
    mkdirs(&h.p("locked"));
    chmod(&h.p("locked"), 0o000);
    let _r2 = Restore(&h.root.join("locked"), 0o755);
    let out = h
        .engine
        .drop_files(vec![h.p("locked")], h.p("dst"), Mods::NONE)
        .unwrap();
    ok(&h, &out);
    let (text, command, _) = toast_parts(h.expect_toast());
    assert_eq!(text, "Couldn’t move “locked” to “dst”: permission denied");
    assert_ne!(command.as_deref(), Some("edit.undo"));

    // Delete Immediately in a folder that can't be written.
    h.engine
        .set_setting("operations.allow_delete_immediately", "true")
        .unwrap();
    mkdirs(&h.p("ro"));
    touch(&h.p("ro/keep.txt"));
    chmod(&h.p("ro"), 0o555);
    let _r3 = Restore(&h.root.join("ro"), 0o755);
    let t = h.window_at("ro");
    t.set_selection(SelectionChange::Names(vec!["keep.txt".into()]));
    let ask = run(&h, &t, "file.delete_immediately").unwrap();
    let Some(ShellRequest::ConfirmDeleteImmediately { token, .. }) = ask.shell else {
        panic!("expected the question");
    };
    let out = h.engine.confirm(token).unwrap();
    ok(&h, &out);
    let (text, _, style) = toast_parts(h.expect_toast());
    assert_eq!(text, "Couldn’t delete “keep.txt”: permission denied");
    assert_eq!(style, ToastStyle::Warning);
    assert!(h.p("ro/keep.txt").exists());

    // Undo that can't put an item back: its folder became read-only.
    mkdirs(&h.p("work"));
    touch(&h.p("work/a.txt"));
    let t = h.window_at("work");
    t.set_selection(SelectionChange::Names(vec!["a.txt".into()]));
    let out = run(&h, &t, "file.trash").unwrap();
    ok(&h, &out);
    assert!(matches!(h.expect_toast(), ShellRequest::Moved { .. }));
    chmod(&h.p("work"), 0o555);
    let _r4 = Restore(&h.root.join("work"), 0o755);
    let out = run(&h, &t, "edit.undo").unwrap();
    ok(&h, &out);
    let (text, command, style) = toast_parts(h.expect_toast());
    assert!(
        text.starts_with("Couldn’t undo Move to Trash of “a.txt”: "),
        "{text}"
    );
    assert_eq!(style, ToastStyle::Warning);
    assert_ne!(command.as_deref(), Some("edit.redo"));
}

/// A copy that works still says nothing (as before), and a trash that
/// works still says "Moved … to the Trash" with Undo.
#[test]
fn successes_are_unchanged() {
    let h = harness();
    mkdirs(&h.p("dst"));
    touch(&h.p("a.txt"));
    let id = h
        .engine
        .enqueue(OpRequest::Copy {
            sources: vec![h.p("a.txt")],
            dest_dir: h.p("dst"),
        })
        .unwrap();
    h.engine.wait_op(id, T).unwrap();
    h.expect_no_toast();
    let t = h.window_at("");
    t.set_selection(SelectionChange::Names(vec!["a.txt".into()]));
    let out = run(&h, &t, "file.trash").unwrap();
    ok(&h, &out);
    let (text, command, style) = toast_parts(h.expect_toast());
    assert_eq!(text, "Moved “a.txt” to the Trash");
    assert_eq!(command.as_deref(), Some("edit.undo"));
    assert_eq!(style, ToastStyle::Success);
}

// ───────────────────────── fix 2: broken symlinks ─────────────────────────

fn open(h: &H, t: &Tab, arg: Option<CommandArg>) -> Outcome {
    h.engine
        .perform(
            CommandInvocation {
                id: "nav.open".into(),
                arg,
                tab: Some(t.id()),
            },
            Via::Keyboard,
        )
        .unwrap()
}

/// ⌘↓ on `broken-link` (→ /does/not/exist) used to hand it to the OS,
/// which did nothing at all. Now the core says why, as a message and a
/// warning toast, and asks the shell to open nothing.
#[test]
fn opening_a_broken_symlink_says_where_it_points() {
    let h = harness();
    std::os::unix::fs::symlink("/does/not/exist", h.p("broken-link")).unwrap();
    std::os::unix::fs::symlink("broken-loop", h.p("broken-loop")).unwrap();
    touch(&h.p("real.txt"));
    std::os::unix::fs::symlink(h.p("real.txt"), h.p("good-link")).unwrap();
    let t = h.window_at("");
    let want = "“broken-link” points to /does/not/exist, which doesn’t exist";

    // ⌘↓ on the selection.
    t.set_selection(SelectionChange::Names(vec!["broken-link".into()]));
    let out = open(&h, &t, None);
    assert_eq!(out.message.as_deref(), Some(want));
    let (text, _, style) = toast_parts(out.shell.unwrap());
    assert_eq!((text.as_str(), style), (want, ToastStyle::Warning));

    // A double-click (the row's path as the argument) is the same.
    let out = open(&h, &t, Some(CommandArg::Path(h.p("broken-link"))));
    assert_eq!(out.message.as_deref(), Some(want));
    assert!(!matches!(out.shell, Some(ShellRequest::Open { .. })));

    // A loop says so.
    let out = open(&h, &t, Some(CommandArg::Path(h.p("broken-loop"))));
    assert!(
        out.message
            .as_deref()
            .unwrap()
            .contains("leads back to itself"),
        "{out:?}"
    );

    // A good link still opens; with a broken one alongside, the good one
    // opens and the broken one is explained.
    let out = open(&h, &t, Some(CommandArg::Path(h.p("good-link"))));
    assert_eq!(
        out.shell,
        Some(ShellRequest::Open {
            paths: vec![h.s("good-link")]
        })
    );
    t.set_selection(SelectionChange::Names(vec![
        "broken-link".into(),
        "good-link".into(),
    ]));
    let out = open(&h, &t, None);
    assert_eq!(
        out.shell,
        Some(ShellRequest::Open {
            paths: vec![h.s("good-link")]
        })
    );
    assert_eq!(out.message.as_deref(), Some(want));
}

/// The sidebar and the address bar go through `Tab::go`: the link is shown
/// selected in its folder, with the same message.
#[test]
fn going_to_a_broken_symlink_says_why_it_did_not_open() {
    let h = harness();
    mkdirs(&h.p("w"));
    std::os::unix::fs::symlink("/does/not/exist", h.p("w/broken-link")).unwrap();
    let t = h.window_at("");
    let out = t.go(&h.s("w/broken-link")).unwrap();
    assert_eq!(
        out.message.as_deref(),
        Some("“broken-link” points to /does/not/exist, which doesn’t exist")
    );
    assert!(matches!(
        out.shell,
        Some(ShellRequest::Toast {
            style: ToastStyle::Warning,
            ..
        })
    ));
    wait_complete(&t);
    eventually("the link is selected", || {
        t.selection().names == vec!["broken-link".to_string()]
    });
}

// ───────────────────────── fix 4: favourites ─────────────────────────

fn fav_paths(h: &H) -> Vec<String> {
    h.engine.favourites().into_iter().map(|f| f.path).collect()
}

/// Pinning a folder that is already a favourite does nothing, by every pin
/// path (the context menu's path argument, a drop at an index, ⌃⌘T on the
/// selection, a reset of the defaults) and by every spelling (a symlink,
/// other case, a trailing slash), and also when two pins race.
#[test]
fn pinning_a_favourite_again_does_nothing_by_any_path() {
    let h = harness();
    mkdirs(&h.p("Apps"));
    std::os::unix::fs::symlink(h.p("Apps"), h.p("apps-link")).unwrap();
    h.engine.add_favourite(&h.s("Apps"), None).unwrap();
    let before = fav_paths(&h);
    // Context menu / command with the folder as the argument.
    let out = h
        .engine
        .perform(
            CommandInvocation {
                id: "favourites.add".into(),
                arg: Some(CommandArg::Path(h.p("apps-link"))),
                tab: None,
            },
            Via::Mouse,
        )
        .unwrap();
    assert_eq!(
        out.message.as_deref(),
        Some("“apps-link” is already in Favourites")
    );
    // A drop at the top, by a spelling with other case and a trailing slash.
    let upper = format!("{}/", h.s("APPS"));
    if std::path::Path::new(&upper).is_dir() {
        h.engine.add_favourite(&upper, Some(0)).unwrap();
    }
    h.engine
        .add_favourite(&format!("{}/", h.s("Apps")), Some(0))
        .unwrap();
    // ⌃⌘T on the selected folder.
    let t = h.window_at("");
    t.set_selection(SelectionChange::Names(vec!["Apps".into()]));
    run(&h, &t, "favourites.add").unwrap();
    assert_eq!(fav_paths(&h), before);
    // Two pins of a new folder at the same moment add it once.
    mkdirs(&h.p("Race"));
    std::os::unix::fs::symlink(h.p("Race"), h.p("race-link")).unwrap();
    std::thread::scope(|s| {
        for i in 0..8 {
            let (e, p) = (
                h.engine.clone(),
                if i % 2 == 0 {
                    h.s("Race")
                } else {
                    h.s("race-link")
                },
            );
            s.spawn(move || e.add_favourite(&p, None).unwrap());
        }
    });
    let n = h
        .engine
        .favourites()
        .iter()
        .filter(|f| f.path.ends_with("/Race") || f.path.ends_with("/race-link"))
        .count();
    assert_eq!(n, 1);
    // Unpinning and pinning again leaves one.
    h.engine.remove_favourite(&h.s("apps-link")).unwrap();
    h.engine.add_favourite(&h.s("Apps"), None).unwrap();
    h.engine.add_favourite(&h.s("apps-link"), None).unwrap();
    let apps = h
        .engine
        .favourites()
        .iter()
        .filter(|f| f.path.ends_with("/Apps"))
        .count();
    assert_eq!(apps, 1);
    // The defaults never hold one folder twice.
    h.engine.reset_favourites().unwrap();
    let mut seen = std::collections::HashSet::new();
    for f in h.engine.favourites() {
        let m = std::fs::metadata(&f.path).unwrap();
        use std::os::unix::fs::MetadataExt;
        assert!(seen.insert((m.dev(), m.ino())), "{} twice", f.path);
    }
}

/// The edge-case test's "Applications twice": the store held `/Applications`
/// and `~/Applications`, two folders with one name, drawn as two identical
/// rows. Their shown names now say where each one is.
#[test]
fn two_folders_with_one_name_are_told_apart() {
    let h = harness();
    // Only the test's own folders (the defaults include the real one).
    let _ = h.engine.remove_favourite("/Applications");
    mkdirs(&h.p("Applications"));
    mkdirs(&h.p("home/Applications"));
    h.engine.add_favourite(&h.s("Applications"), None).unwrap();
    h.engine
        .add_favourite(&h.s("home/Applications"), None)
        .unwrap();
    let names: Vec<String> = h
        .engine
        .favourites()
        .into_iter()
        .filter(|f| f.path.starts_with(&h.s("")) && f.path.ends_with("Applications"))
        .map(|f| f.name)
        .collect();
    assert_eq!(names, vec!["Applications (root)", "Applications (home)"]);
    // Unpin one: the other goes back to its plain name.
    h.engine
        .remove_favourite(&h.s("home/Applications"))
        .unwrap();
    assert!(
        h.engine
            .favourites()
            .iter()
            .any(|f| f.path == h.s("Applications") && f.name == "Applications")
    );
}

// ───────────────────────── fix 5: volume names ─────────────────────────

/// The startup disk showed as "/" in some places. Every place gets its name
/// from one rule: breadcrumbs, tab titles' source, toasts and questions,
/// favourites, and the FFI's display_name.
#[test]
fn the_startup_disk_is_named_everywhere() {
    let h = harness();
    let disk = fstudio_core::display_name("/");
    assert_ne!(disk, "/");
    // The address bar's first crumb.
    let t = h.window_at("");
    t.go("/").unwrap();
    wait_complete(&t);
    let crumbs = t.breadcrumbs();
    assert_eq!(crumbs[0].name, disk);
    assert_eq!(h.engine.breadcrumbs("/")[0].name, disk);
    // The sidebar's volume list.
    assert!(
        h.engine
            .volumes()
            .iter()
            .all(|v| v.name != "/" && !v.name.is_empty())
    );
    assert!(
        h.engine
            .volumes()
            .iter()
            .any(|v| v.mount_path == "/" && v.name == disk)
    );
    // A favourite of the disk itself, and a same-named folder hint.
    h.engine.add_favourite("/", None).unwrap();
    let f = h
        .engine
        .favourites()
        .into_iter()
        .find(|f| f.path == "/")
        .unwrap();
    assert_eq!(f.name, disk);
    mkdirs(&h.p("Applications"));
    h.engine.add_favourite(&h.s("Applications"), None).unwrap();
    if h.engine
        .favourites()
        .iter()
        .any(|f| f.path == "/Applications")
    {
        let sys = h
            .engine
            .favourites()
            .into_iter()
            .find(|f| f.path == "/Applications")
            .unwrap();
        assert_eq!(sys.name, format!("Applications ({disk})"));
    }
    // The context-menu message for pinning it again.
    let out = h
        .engine
        .perform(
            CommandInvocation {
                id: "favourites.add".into(),
                arg: Some(CommandArg::Path("/".into())),
                tab: None,
            },
            Via::Mouse,
        )
        .unwrap();
    assert_eq!(
        out.message,
        Some(format!("“{disk}” is already in Favourites"))
    );
}

// ───────────────────────── extras ─────────────────────────

/// Going from a folder with a selection into one that can't be read: the
/// shell is told the selection is now empty (it said "1 of 0 items
/// selected"), and the reason is in plain words, not "(os error 13)".
#[test]
fn an_unreadable_folder_has_no_stale_selection_or_raw_error() {
    let h = harness();
    mkdirs(&h.p("locked"));
    touch(&h.p("a.txt"));
    chmod(&h.p("locked"), 0o000);
    let _r = Restore(&h.root.join("locked"), 0o755);
    let t = h.window_at("");
    t.set_selection(SelectionChange::Names(vec!["a.txt".into()]));
    h.drain();
    t.go(&h.s("locked")).unwrap();
    h.expect_event(
        "the selection count going to 0",
        |e| matches!(e, CoreEvent::SelectionChanged { tab, count: 0, .. } if *tab == t.id()),
    );
    wait_complete(&t);
    let l = t.listing();
    assert_eq!(l.state(), ListingState::Error);
    let err = l.error().unwrap_or_default();
    assert!(err.ends_with(": permission denied"), "{err}");
    assert!(!err.contains("os error"), "{err}");
    assert_eq!(t.selection().names.len(), 0);
}

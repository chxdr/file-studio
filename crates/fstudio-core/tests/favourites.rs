//! Sidebar favourites: add, remove, reorder, rename and persist through the
//! core's store; one favourite per folder whatever path reaches it (NFC/NFD,
//! case, symlinks); a favourite whose folder has gone; ⌃⌘T and the context
//! menus' Add to / Remove from Favourites.

mod common;

use common::*;
use fstudio_core::*;

fn paths(h: &H) -> Vec<String> {
    h.engine.favourites().into_iter().map(|f| f.path).collect()
}

/// A harness with no favourites (its home has no Desktop etc. anyway, but
/// clear whatever defaults exist) and three folders.
fn setup() -> (H, std::sync::Arc<Tab>) {
    let h = harness();
    for f in h.engine.favourites() {
        h.engine.remove_favourite(&f.path).unwrap();
    }
    for d in ["one", "two", "three"] {
        mkdirs(&h.p(d));
    }
    touch(&h.p("file.txt"));
    let t = h.window_at("");
    (h, t)
}

fn find(items: &[MenuItem], id: &str) -> Option<MenuCommand> {
    fn flat(items: &[MenuItem], out: &mut Vec<MenuCommand>) {
        for i in items {
            match i {
                MenuItem::Command(c) => out.push(c.clone()),
                MenuItem::Submenu { items, .. } => flat(items, out),
                _ => {}
            }
        }
    }
    let mut v = Vec::new();
    flat(items, &mut v);
    v.into_iter().find(|c| c.entry == id)
}

#[test]
fn add_remove_reorder_rename_and_persist() {
    let (mut h, _t) = setup();
    h.drain();
    h.engine.add_favourite(&h.s("one"), None).unwrap();
    h.expect_event("favourites changed", |e| {
        matches!(e, CoreEvent::FavouritesChanged)
    });
    h.engine.add_favourite(&h.s("two"), None).unwrap();
    // An index puts it at the drop position.
    h.engine.add_favourite(&h.s("three"), Some(0)).unwrap();
    assert_eq!(paths(&h), [h.s("three"), h.s("one"), h.s("two")]);
    // Reorder.
    h.engine.move_favourite(&h.s("three"), 2).unwrap();
    assert_eq!(paths(&h), [h.s("one"), h.s("two"), h.s("three")]);
    h.engine.move_favourite(&h.s("two"), 0).unwrap();
    assert_eq!(paths(&h), [h.s("two"), h.s("one"), h.s("three")]);
    // Past the end clamps.
    h.engine.move_favourite(&h.s("two"), 99).unwrap();
    assert_eq!(paths(&h), [h.s("one"), h.s("three"), h.s("two")]);
    // Rename shows another name; empty goes back to the folder's name.
    h.engine.rename_favourite(&h.s("one"), "  Work  ").unwrap();
    let f = &h.engine.favourites()[0];
    assert_eq!((f.name.as_str(), f.renamed), ("Work", true));
    // Remove leaves the folder alone.
    h.engine.remove_favourite(&h.s("three")).unwrap();
    assert!(h.p("three").is_dir());
    assert_eq!(paths(&h), [h.s("one"), h.s("two")]);
    assert!(h.engine.remove_favourite(&h.s("three")).is_err());
    // Everything survives a relaunch.
    h.relaunch();
    assert_eq!(paths(&h), [h.s("one"), h.s("two")]);
    assert_eq!(h.engine.favourites()[0].name, "Work");
    h.engine.rename_favourite(&h.s("one"), "").unwrap();
    assert_eq!(h.engine.favourites()[0].name, "one");
    // Files can't be favourites.
    assert!(h.engine.add_favourite(&h.s("file.txt"), None).is_err());
    assert!(h.engine.add_favourite(&h.s("nope"), None).is_err());
}

#[test]
fn one_folder_is_one_favourite_whatever_path_reaches_it() {
    let (h, _t) = setup();
    // NFD on disk, NFC typed.
    let nfd = "Cafe\u{301}";
    let nfc = "Caf\u{e9}";
    mkdirs(&h.p(nfd));
    h.engine.add_favourite(&h.s(nfd), None).unwrap();
    h.engine.add_favourite(&h.s(nfc), None).unwrap();
    assert_eq!(paths(&h), [h.s(nfd)], "the stored name is kept as added");
    assert!(h.engine.is_favourite(&h.s(nfc)));
    // Case (the temp volume is case-insensitive APFS on a default Mac).
    let upper = h.s("ONE");
    if std::path::Path::new(&upper).is_dir() {
        h.engine.add_favourite(&h.s("one"), None).unwrap();
        h.engine.add_favourite(&upper, None).unwrap();
        assert_eq!(paths(&h).len(), 2);
        h.engine.remove_favourite(&upper).unwrap();
        assert!(!h.engine.is_favourite(&h.s("one")));
    }
    // A trailing slash.
    h.engine
        .add_favourite(&format!("{}/", h.s(nfd)), None)
        .unwrap();
    assert_eq!(paths(&h).len(), 1);
    // A symlink to a pinned folder is the same favourite, both ways.
    std::os::unix::fs::symlink(h.p("two"), h.p("link-to-two")).unwrap();
    h.engine.add_favourite(&h.s("link-to-two"), None).unwrap();
    h.engine.add_favourite(&h.s("two"), None).unwrap();
    assert_eq!(paths(&h), [h.s(nfd), h.s("link-to-two")]);
    assert!(h.engine.is_favourite(&h.s("two")));
    // Removing by the real path removes the symlinked favourite.
    h.engine.remove_favourite(&h.s("two")).unwrap();
    assert_eq!(paths(&h), [h.s(nfd)]);
    // Moving and renaming find it by another spelling too.
    h.engine.add_favourite(&h.s("three"), Some(0)).unwrap();
    h.engine.move_favourite(&h.s(nfc), 0).unwrap();
    assert_eq!(paths(&h), [h.s(nfd), h.s("three")]);
    h.engine.rename_favourite(&h.s(nfc), "Coffee").unwrap();
    assert_eq!(h.engine.favourites()[0].name, "Coffee");
}

#[test]
fn a_symlink_loop_or_dangling_link_is_not_a_folder() {
    let (h, _t) = setup();
    std::os::unix::fs::symlink(h.p("loop-a"), h.p("loop-b")).unwrap();
    std::os::unix::fs::symlink(h.p("loop-b"), h.p("loop-a")).unwrap();
    assert!(h.engine.add_favourite(&h.s("loop-a"), None).is_err());
    assert!(h.engine.favourites().is_empty());
}

#[test]
fn a_favourite_whose_folder_has_gone_is_missing_and_can_be_removed() {
    let (mut h, t) = setup();
    h.engine.add_favourite(&h.s("one"), None).unwrap();
    h.engine.add_favourite(&h.s("two"), None).unwrap();
    std::fs::remove_dir(h.p("one")).unwrap();
    let favs = h.engine.favourites();
    assert!(favs[0].missing);
    assert!(!favs[1].missing);
    // `missing` is worked out on every read, never stored.
    h.relaunch();
    assert!(h.engine.favourites()[0].missing);
    mkdirs(&h.p("one"));
    assert!(!h.engine.favourites()[0].missing);
    std::fs::remove_dir(h.p("one")).unwrap();
    // The sidebar menu offers Remove; it works without the folder.
    let items = h
        .engine
        .context_menu(
            MenuTarget::SidebarItem {
                path: h.s("one"),
                kind: SidebarItemKind::Favourite,
            },
            MenuContext::default(),
        )
        .unwrap();
    let rm = find(&items, "sidebar.remove").unwrap();
    assert_eq!(rm.title, "Remove from Favourites");
    let out = h.engine.perform(rm.invocation, Via::Menu).unwrap();
    assert_eq!(
        out.message.as_deref(),
        Some("Removed “one” from Favourites")
    );
    assert_eq!(paths(&h), [h.s("two")]);
    let _ = t;
}

#[test]
fn ctrl_cmd_t_adds_the_selected_folders_or_this_folder() {
    let (h, t) = setup();
    let ctrl_cmd_t = KeyChord::new(
        Key::Char('t'),
        Mods {
            cmd: true,
            ctrl: true,
            ..Mods::NONE
        },
    );
    // Nothing selected: this folder.
    let out = h
        .engine
        .handle_key(ctrl_cmd_t, InputContext::default())
        .expect("⌃⌘T is bound");
    assert_eq!(out.command.as_deref(), Some("favourites.add"));
    assert_eq!(paths(&h), [h.s("")]);
    // Selected folders only; files are skipped.
    t.set_selection(SelectionChange::Names(vec![
        "one".into(),
        "two".into(),
        "file.txt".into(),
    ]));
    let out = h
        .engine
        .handle_key(ctrl_cmd_t, InputContext::default())
        .unwrap();
    assert_eq!(
        out.message.as_deref(),
        Some("Added 2 folders to Favourites")
    );
    assert_eq!(paths(&h), [h.s(""), h.s("one"), h.s("two")]);
    // Again: nothing new.
    let out = h
        .engine
        .handle_key(ctrl_cmd_t, InputContext::default())
        .unwrap();
    assert_eq!(
        out.message.as_deref(),
        Some("They’re already in Favourites")
    );
    // Only a file selected.
    t.set_selection(SelectionChange::Names(vec!["file.txt".into()]));
    let out = h
        .engine
        .perform(CommandInvocation::new("favourites.add"), Via::Palette)
        .unwrap();
    assert_eq!(
        out.message.as_deref(),
        Some("Only folders can be favourites")
    );
    // Remove with the selection.
    t.set_selection(SelectionChange::Names(vec!["one".into()]));
    h.engine
        .perform(CommandInvocation::new("favourites.remove"), Via::Palette)
        .unwrap();
    assert_eq!(paths(&h), [h.s(""), h.s("two")]);
}

#[test]
fn menus_show_add_or_remove_by_whether_it_is_a_favourite() {
    let (h, t) = setup();
    let folder = |h: &H| {
        h.engine
            .context_menu(
                MenuTarget::Folder {
                    tab: t.id(),
                    path: h.s("one"),
                },
                MenuContext::default(),
            )
            .unwrap()
    };
    let items = folder(&h);
    assert!(find(&items, "favourites.remove").is_none());
    let add = find(&items, "favourites.add").unwrap();
    assert_eq!(add.title, "Add to Favourites");
    assert_eq!(add.shortcut.as_deref(), Some("⌃⌘T"));
    h.engine.perform(add.invocation, Via::Menu).unwrap();
    assert!(h.engine.is_favourite(&h.s("one")));
    let items = folder(&h);
    assert!(find(&items, "favourites.add").is_none());
    let rm = find(&items, "favourites.remove").unwrap();
    h.engine.perform(rm.invocation, Via::Menu).unwrap();
    assert!(!h.engine.is_favourite(&h.s("one")));

    // Files never get either.
    let items = h
        .engine
        .context_menu(
            MenuTarget::File {
                tab: t.id(),
                path: h.s("file.txt"),
            },
            MenuContext::default(),
        )
        .unwrap();
    assert!(find(&items, "favourites.add").is_none());
    assert!(find(&items, "favourites.remove").is_none());

    // Several items: Add adds only the folders; once all are pinned, Remove.
    let multi = |h: &H| {
        h.engine
            .context_menu(
                MenuTarget::Multi {
                    tab: t.id(),
                    paths: vec![h.s("two"), h.s("three"), h.s("file.txt")],
                },
                MenuContext::default(),
            )
            .unwrap()
    };
    let add = find(&multi(&h), "favourites.add").unwrap();
    h.engine.perform(add.invocation, Via::Menu).unwrap();
    // In list order (the selection's order).
    assert_eq!(paths(&h), [h.s("three"), h.s("two")]);
    let items = multi(&h);
    assert!(find(&items, "favourites.add").is_none());
    let rm = find(&items, "favourites.remove").unwrap();
    h.engine.perform(rm.invocation, Via::Menu).unwrap();
    assert!(paths(&h).is_empty());

    // A tab acts on its folder.
    let tab_menu = |h: &H| {
        h.engine
            .context_menu(MenuTarget::Tab { tab: t.id() }, MenuContext::default())
            .unwrap()
    };
    let add = find(&tab_menu(&h), "favourites.add").unwrap();
    h.engine.perform(add.invocation, Via::Menu).unwrap();
    assert_eq!(paths(&h), [h.s("")]);
    assert!(find(&tab_menu(&h), "favourites.remove").is_some());
}

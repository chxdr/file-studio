//! Context menus (F-022): default contents match `docs/CONTEXT-MENUS.md`,
//! items that don't apply are hidden, Paste greys out, overrides persist.

mod common;

use common::*;
use fstudio_core::menus::{SEPARATOR, default_entries};
use fstudio_core::*;

const SEP: &str = SEPARATOR;

/// The spec, transcribed separately from the implementation.
fn spec(kind: MenuKind) -> Vec<&'static str> {
    let tail = [
        "file.rename",
        "file.duplicate",
        "file.make_alias",
        "file.compress",
        SEP,
        "edit.copy",
        "edit.cut",
        "edit.paste",
        "file.copy_to_other_pane",
        "file.move_to_other_pane",
        SEP,
        "menu.copy_path",
        SEP,
        "menu.tags",
        SEP,
        "file.reveal_in_finder",
        "terminal.open",
        "file.open_in_editor",
        "system.share",
        "system.quick_actions",
        "system.services",
        SEP,
        "file.trash",
        "file.delete_immediately",
    ];
    let folder_head = |pkg: bool| {
        let mut v = vec!["nav.open"];
        if pkg {
            v.push("nav.show_package_contents");
        }
        v.extend([
            "nav.open_new_tab",
            "nav.open_other_pane",
            "menu.open_with",
            SEP,
            "preview.quick_look",
            "preview.get_info",
            "file.folder_size",
            SEP,
        ]);
        v
    };
    match kind {
        MenuKind::File => [
            "nav.open",
            "menu.open_with",
            "nav.open_new_tab",
            "nav.open_other_pane",
            SEP,
            "preview.quick_look",
            "preview.get_info",
            SEP,
        ]
        .into_iter()
        .chain(tail)
        .collect(),
        MenuKind::Folder | MenuKind::Package => {
            let mut v: Vec<&str> = folder_head(kind == MenuKind::Package)
                .into_iter()
                .chain(tail)
                .collect();
            let at = v.iter().position(|e| *e == "file.open_in_editor").unwrap() + 1;
            v.splice(at..at, ["favourites.add", "favourites.remove"]);
            v
        }
        MenuKind::Multi => vec![
            "nav.open",
            "preview.quick_look",
            "preview.get_info",
            SEP,
            "file.batch_rename",
            "file.duplicate",
            "file.compress",
            "file.new_folder_with_selection",
            SEP,
            "edit.copy",
            "edit.cut",
            "edit.paste",
            "file.copy_to_other_pane",
            "file.move_to_other_pane",
            "menu.copy_path",
            SEP,
            "favourites.add",
            "favourites.remove",
            SEP,
            "menu.tags",
            "system.share",
            SEP,
            "file.trash",
        ],
        MenuKind::EmptySpace => vec![
            "file.new_folder",
            "file.new_file",
            "edit.paste",
            "file.move_here",
            SEP,
            "menu.view_as",
            "menu.sort_by",
            "view.show_hidden",
            SEP,
            "select.all",
            "select.invert",
            "select.by_pattern",
            SEP,
            "path.copy_folder_path",
            "terminal.open",
            "file.reveal_in_finder",
            "view.refresh",
        ],
        MenuKind::Crumb => vec![
            "nav.open",
            "nav.open_new_tab",
            "nav.open_other_pane",
            SEP,
            "menu.copy_path",
            SEP,
            "terminal.open",
            "file.reveal_in_finder",
            "favourites.add",
            "favourites.remove",
            "preview.get_info",
        ],
        MenuKind::SidebarItem => vec![
            "nav.open",
            "nav.open_new_tab",
            "nav.open_other_pane",
            "file.reveal_in_finder",
            "preview.get_info",
            "path.copy_path",
            SEP,
            "sidebar.remove",
            "sidebar.rename",
            "drive.eject",
            "drive.info",
            // Shown only for a disk with interrupted transfers (§6.1).
            "transfer.resume",
            "transfer.resume_all",
        ],
        MenuKind::Tab => vec![
            "tab.new",
            "tab.duplicate",
            "tab.reopen_closed",
            "tab.move_to_other_pane",
            SEP,
            "tab.close",
            "tab.close_others",
            SEP,
            "path.copy_path",
            "favourites.add",
            "favourites.remove",
            "workspace.save",
        ],
    }
}

/// Every command item, depth first, as `(entry, title, enabled, checked)`.
fn flat(items: &[MenuItem]) -> Vec<MenuCommand> {
    let mut out = Vec::new();
    for i in items {
        match i {
            MenuItem::Command(c) => out.push(c.clone()),
            MenuItem::Submenu { items, .. } => out.extend(flat(items)),
            _ => {}
        }
    }
    out
}

fn ids(items: &[MenuItem]) -> Vec<String> {
    flat(items).into_iter().map(|c| c.entry).collect()
}

fn top(items: &[MenuItem]) -> Vec<String> {
    items
        .iter()
        .map(|i| match i {
            MenuItem::Command(c) => c.entry.clone(),
            MenuItem::Separator => SEP.into(),
            MenuItem::Submenu { entry, .. } => entry.clone(),
            MenuItem::System { kind, .. } => format!("{kind:?}"),
        })
        .collect()
}

fn find(items: &[MenuItem], id: &str) -> Option<MenuCommand> {
    flat(items).into_iter().find(|c| c.entry == id)
}

fn setup() -> (H, std::sync::Arc<Tab>) {
    let h = harness();
    touch(&h.p("a.txt"));
    touch(&h.p("b.txt"));
    mkdirs(&h.p("dir"));
    mkdirs(&h.p("Thing.app/Contents"));
    let t = h.window_at("");
    (h, t)
}

fn menu(h: &H, target: MenuTarget) -> Vec<MenuItem> {
    h.engine
        .context_menu(target, MenuContext::default())
        .unwrap()
}

#[test]
fn defaults_match_the_spec() {
    let (h, _) = setup();
    for kind in MenuKind::ALL {
        assert_eq!(default_entries(kind), spec(kind), "{kind:?}");
        let eff: Vec<String> = h
            .engine
            .menu_entries(kind)
            .into_iter()
            .map(|e| e.entry)
            .collect();
        assert_eq!(eff, spec(kind), "{kind:?} effective");
    }
}

#[test]
fn file_menu_hides_what_does_not_apply() {
    let (h, t) = setup();
    let items = menu(
        &h,
        MenuTarget::File {
            tab: t.id(),
            path: h.s("a.txt"),
        },
    );
    let got = ids(&items);
    for hidden in [
        "nav.open_new_tab",        // folders only
        "nav.open_other_pane",     // dual pane only
        "file.copy_to_other_pane", // dual pane only
        "file.make_alias",         // later
        "file.compress",           // later
        "file.delete_immediately", // off by default
    ] {
        assert!(
            !got.contains(&hidden.to_string()),
            "{hidden} shown: {got:?}"
        );
    }
    // The menu opened for a.txt: a.txt is now the selection.
    assert_eq!(t.selection().names, vec!["a.txt".to_string()]);
    // Shortcuts are the user's current bindings.
    assert_eq!(
        find(&items, "file.duplicate").unwrap().shortcut.as_deref(),
        Some("⌘D")
    );
    // System submenus are there for the shell to fill.
    assert!(items.iter().any(|i| matches!(
        i,
        MenuItem::System {
            kind: SystemMenu::Share,
            ..
        }
    )));
    // No doubled or edge separators.
    let tops = top(&items);
    assert_ne!(tops.first().map(String::as_str), Some(SEP));
    assert_ne!(tops.last().map(String::as_str), Some(SEP));
    assert!(!tops.windows(2).any(|w| w[0] == SEP && w[1] == SEP));
    // Every command item maps to a registered command and runs.
    for c in flat(&items) {
        assert!(fstudio_keymap::registry::get(&c.invocation.id).is_some());
    }
}

#[test]
fn paste_greys_out_when_there_is_nothing_to_paste() {
    let (h, t) = setup();
    let empty = MenuTarget::EmptySpace { tab: t.id() };
    let p = find(&menu(&h, empty.clone()), "edit.paste").unwrap();
    assert!(!p.enabled, "greyed, not hidden");
    let with_pb = h
        .engine
        .context_menu(
            empty.clone(),
            MenuContext {
                pasteboard_has_files: true,
            },
        )
        .unwrap();
    assert!(find(&with_pb, "edit.paste").unwrap().enabled);
    h.engine.set_clipboard(vec![h.s("a.txt")], false);
    assert!(find(&menu(&h, empty), "edit.paste").unwrap().enabled);
}

#[test]
fn dual_pane_and_delete_immediately_items_appear_when_they_apply() {
    let (h, t) = setup();
    let w = h.engine.workspace_state().windows[0].id;
    h.engine.toggle_dual(w).unwrap();
    h.engine.activate_tab(t.id()).unwrap();
    h.engine
        .set_setting("operations.allow_delete_immediately", "true")
        .unwrap();
    let items = menu(
        &h,
        MenuTarget::Folder {
            tab: t.id(),
            path: h.s("dir"),
        },
    );
    let got = ids(&items);
    for shown in [
        "nav.open_other_pane",
        "nav.open_new_tab",
        "file.copy_to_other_pane",
        "file.move_to_other_pane",
        "file.delete_immediately",
    ] {
        assert!(got.contains(&shown.to_string()), "{shown} missing: {got:?}");
    }
    // Open in Terminal on a folder opens inside it.
    let term = find(&items, "terminal.open").unwrap();
    assert_eq!(
        term.invocation.arg,
        Some(CommandArg::Path(h.p("dir"))),
        "{term:?}"
    );
    let out = h.engine.perform(term.invocation, Via::Menu).unwrap();
    assert_eq!(
        out.shell,
        Some(ShellRequest::OpenTerminal { dir: h.s("dir") })
    );
}

#[test]
fn right_click_follows_finders_selection_rule() {
    let (h, t) = setup();
    let a = t.listing().index_of("a.txt").unwrap();
    let b = t.listing().index_of("b.txt").unwrap();
    let app = t.listing().index_of("Thing.app").unwrap();
    // An unselected row becomes the only selection.
    t.set_selection(SelectionChange::Only(b));
    let target = h.engine.menu_target_at(t.id(), Some(a)).unwrap();
    assert_eq!(
        target,
        MenuTarget::File {
            tab: t.id(),
            path: h.s("a.txt")
        }
    );
    assert_eq!(t.selection().names, vec!["a.txt".to_string()]);
    // A selected row keeps the whole selection.
    t.set_selection(SelectionChange::Names(vec!["a.txt".into(), "b.txt".into()]));
    let target = h.engine.menu_target_at(t.id(), Some(a)).unwrap();
    assert!(matches!(&target, MenuTarget::Multi { paths, .. } if paths.len() == 2));
    let items = menu(&h, target);
    assert_eq!(
        find(&items, "file.trash").unwrap().title,
        "Move 2 Items to Trash"
    );
    assert_eq!(
        find(&items, "preview.quick_look").unwrap().title,
        "Quick Look 2 Items"
    );
    // Packages are their own target and get Show Package Contents.
    let target = h.engine.menu_target_at(t.id(), Some(app)).unwrap();
    assert!(matches!(target, MenuTarget::Package { .. }));
    let items = menu(&h, target);
    let spc = find(&items, "nav.show_package_contents").unwrap();
    h.engine.perform(spc.invocation, Via::Menu).unwrap();
    assert_eq!(t.location(), h.s("Thing.app"));
    // Empty space clears the selection.
    let target = h.engine.menu_target_at(t.id(), None).unwrap();
    assert_eq!(target, MenuTarget::EmptySpace { tab: t.id() });
    assert!(t.selection().names.is_empty());
}

#[test]
fn crumb_items_act_on_the_segment_not_the_selection() {
    let (h, t) = setup();
    t.set_selection(SelectionChange::Names(vec!["a.txt".into()]));
    let items = menu(
        &h,
        MenuTarget::Crumb {
            tab: t.id(),
            path: h.s("dir"),
        },
    );
    // The Copy Path submenu has no Name style for crumbs.
    assert!(find(&items, "path.copy_name").is_none());
    let cp = find(&items, "path.copy_path").unwrap();
    let out = h.engine.perform(cp.invocation, Via::Menu).unwrap();
    assert_eq!(out.shell, Some(ShellRequest::CopyText { text: h.s("dir") }));
    assert!(find(&items, "favourites.remove").is_none());
    let fav = find(&items, "favourites.add").unwrap();
    h.engine.perform(fav.invocation, Via::Menu).unwrap();
    assert!(h.engine.is_favourite(&h.s("dir")));
    // Already a favourite: Add is hidden and Remove shows instead.
    let items = menu(
        &h,
        MenuTarget::Crumb {
            tab: t.id(),
            path: h.s("dir"),
        },
    );
    assert!(find(&items, "favourites.add").is_none());
    assert!(find(&items, "favourites.remove").is_some());
    let open = find(&items, "nav.open").unwrap();
    h.engine.perform(open.invocation, Via::Menu).unwrap();
    assert_eq!(t.location(), h.s("dir"));
}

#[test]
fn sidebar_items_differ_for_favourites_and_volumes() {
    let (h, _t) = setup();
    h.engine.add_favourite(&h.s("dir"), None).unwrap();
    let fav = menu(
        &h,
        MenuTarget::SidebarItem {
            path: h.s("dir"),
            kind: SidebarItemKind::Favourite,
        },
    );
    let got = ids(&fav);
    assert!(got.contains(&"sidebar.remove".into()) && got.contains(&"sidebar.rename".into()));
    assert!(!got.contains(&"drive.eject".into()) && !got.contains(&"drive.info".into()));
    let rename = find(&fav, "sidebar.rename").unwrap();
    let out = h.engine.perform(rename.invocation, Via::Menu).unwrap();
    assert_eq!(
        out.shell,
        Some(ShellRequest::AskFavouriteName {
            path: h.s("dir"),
            name: "dir".into()
        })
    );
    h.engine.rename_favourite(&h.s("dir"), "Work").unwrap();
    assert_eq!(
        h.engine
            .favourites()
            .iter()
            .find(|f| f.path == h.s("dir"))
            .unwrap()
            .name,
        "Work"
    );
    let remove = find(&fav, "sidebar.remove").unwrap();
    h.engine.perform(remove.invocation, Via::Menu).unwrap();
    assert!(!h.engine.is_favourite(&h.s("dir")));

    // The startup disk: info, but no eject (it isn't removable).
    let vol = menu(
        &h,
        MenuTarget::SidebarItem {
            path: "/".into(),
            kind: SidebarItemKind::Volume,
        },
    );
    let got = ids(&vol);
    assert!(got.contains(&"drive.info".into()));
    assert!(!got.contains(&"drive.eject".into()));
    assert!(!got.contains(&"sidebar.remove".into()));
    let info = find(&vol, "drive.info").unwrap();
    let out = h.engine.perform(info.invocation, Via::Menu).unwrap();
    assert_eq!(
        out.shell,
        Some(ShellRequest::ShowDriveInfo { path: "/".into() })
    );
}

#[test]
fn tab_menu_hides_what_cannot_happen() {
    let (h, t) = setup();
    let items = menu(&h, MenuTarget::Tab { tab: t.id() });
    let got = ids(&items);
    assert!(!got.contains(&"tab.close_others".into()), "only one tab");
    assert!(!got.contains(&"tab.reopen_closed".into()), "nothing closed");
    assert!(
        !got.contains(&"tab.move_to_other_pane".into()),
        "single pane"
    );
    let cp = find(&items, "path.copy_path").unwrap();
    assert_eq!(cp.invocation.arg, Some(CommandArg::Path(h.p(""))));
    let extra = h.engine.open_tab(None).unwrap();
    h.engine.close_tab(extra.id()).unwrap();
    h.engine.open_tab(None).unwrap();
    let got = ids(&menu(&h, MenuTarget::Tab { tab: t.id() }));
    assert!(got.contains(&"tab.close_others".into()));
    assert!(got.contains(&"tab.reopen_closed".into()));
}

#[test]
fn empty_space_ticks_the_current_view() {
    let (h, t) = setup();
    let items = menu(&h, MenuTarget::EmptySpace { tab: t.id() });
    assert!(find(&items, "sort.name").unwrap().checked);
    assert!(!find(&items, "sort.size").unwrap().checked);
    assert!(find(&items, "view.list").unwrap().checked);
    assert!(!find(&items, "view.show_hidden").unwrap().checked);
    let sh = find(&items, "view.show_hidden").unwrap();
    h.engine.perform(sh.invocation, Via::Menu).unwrap();
    let items = menu(&h, MenuTarget::EmptySpace { tab: t.id() });
    assert!(find(&items, "view.show_hidden").unwrap().checked);
}

#[test]
fn tags_toggle_and_tick() {
    let (h, t) = setup();
    let target = MenuTarget::File {
        tab: t.id(),
        path: h.s("a.txt"),
    };
    let items = menu(&h, target.clone());
    let red = flat(&items)
        .into_iter()
        .find(|c| c.entry == "tags.toggle" && c.title == "Red")
        .unwrap();
    assert_eq!(red.tag_color, Some(6));
    assert!(!red.checked);
    h.engine.perform(red.invocation.clone(), Via::Menu).unwrap();
    let items = menu(&h, target.clone());
    let red2 = flat(&items)
        .into_iter()
        .find(|c| c.entry == "tags.toggle" && c.title == "Red")
        .unwrap();
    assert!(red2.checked, "Red is set now");
    h.engine.perform(red.invocation, Via::Menu).unwrap();
    assert!(
        flat(&menu(&h, target))
            .into_iter()
            .any(|c| c.title == "Red" && !c.checked)
    );
}

#[test]
fn overrides_add_remove_move_reset_and_persist() {
    let (mut h, t) = setup();
    let k = MenuKind::File;
    h.engine.menu_remove(k, "file.duplicate").unwrap();
    h.engine
        .menu_add(k, "file.new_folder", Some("nav.open".into()))
        .unwrap();
    h.engine.menu_move(k, "file.trash", None).unwrap();
    let eff: Vec<String> = h
        .engine
        .menu_entries(k)
        .into_iter()
        .map(|e| e.entry)
        .collect();
    assert_eq!(eff[0], "file.trash");
    assert_eq!(eff[1], "nav.open");
    assert_eq!(eff[2], "file.new_folder");
    assert!(!eff.contains(&"file.duplicate".into()));
    // The built menu follows.
    let items = menu(
        &h,
        MenuTarget::File {
            tab: t.id(),
            path: h.s("a.txt"),
        },
    );
    assert_eq!(top(&items)[0], "file.trash");
    assert!(find(&items, "file.duplicate").is_none());
    // Choices offer what isn't there.
    let choices: Vec<String> = h
        .engine
        .menu_choices(k)
        .into_iter()
        .map(|e| e.entry)
        .collect();
    assert!(choices.contains(&"file.duplicate".into()));
    assert!(!choices.contains(&"file.trash".into()));
    // Adding back a removed default brings it back.
    h.engine
        .menu_add(k, "file.duplicate", Some("file.rename".into()))
        .unwrap();
    let eff: Vec<String> = h
        .engine
        .menu_entries(k)
        .into_iter()
        .map(|e| e.entry)
        .collect();
    let r = eff.iter().position(|e| e == "file.rename").unwrap();
    assert_eq!(eff[r + 1], "file.duplicate");
    // Unknown entries are refused.
    assert!(h.engine.menu_add(k, "no.such", None).is_err());
    // Other menus are untouched; changes survive a relaunch.
    assert_eq!(
        h.engine
            .menu_entries(MenuKind::Folder)
            .into_iter()
            .map(|e| e.entry)
            .collect::<Vec<_>>(),
        spec(MenuKind::Folder)
    );
    h.relaunch();
    let eff: Vec<String> = h
        .engine
        .menu_entries(k)
        .into_iter()
        .map(|e| e.entry)
        .collect();
    assert_eq!(eff[0], "file.trash");
    h.engine.reset_menu(k).unwrap();
    let eff: Vec<String> = h
        .engine
        .menu_entries(k)
        .into_iter()
        .map(|e| e.entry)
        .collect();
    assert_eq!(eff, spec(k));
    assert!(h.engine.settings().menus.overrides.is_empty());
}

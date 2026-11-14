//! Folder opens and reveal requests from other apps (File Studio as the
//! default file viewer): where they land and what ends up selected.

mod common;

use common::*;
use fstudio_core::*;

fn tab_count(h: &H) -> usize {
    h.engine
        .workspace_state()
        .windows
        .iter()
        .flat_map(|w| &w.panes)
        .map(|p| p.tabs.len())
        .sum()
}

fn selected_when_loaded(h: &H, out: &Outcome) -> (String, Vec<String>) {
    let t = h.engine.tab(out.tab.expect("a tab")).unwrap();
    wait_complete(&t);
    eventually("the selection", || !t.selection().names.is_empty());
    (t.location(), t.selection().names)
}

#[test]
fn reveal_opens_the_parent_with_the_items_selected() {
    let h = harness();
    mkdirs(&h.p("a/sub"));
    touch(&h.p("a/one.txt"));
    touch(&h.p("a/two.txt"));
    h.window_at("");
    let out = h
        .engine
        .open_from_system(&[h.s("a/one.txt"), h.s("a/two.txt")], true)
        .unwrap();
    let (loc, mut sel) = selected_when_loaded(&h, &out);
    sel.sort();
    assert_eq!(loc, h.s("a"));
    assert_eq!(sel, vec!["one.txt", "two.txt"]);
    // A new tab in the front window, by default.
    assert_eq!(h.engine.workspace_state().windows.len(), 1);
    assert_eq!(tab_count(&h), 2);
    assert_eq!(h.engine.active_tab().unwrap().id(), out.tab.unwrap());
}

#[test]
fn opening_a_folder_shows_the_folder_itself() {
    let h = harness();
    mkdirs(&h.p("dir/inner"));
    h.window_at("");
    let out = h.engine.open_from_system(&[h.s("dir")], false).unwrap();
    let t = h.engine.tab(out.tab.unwrap()).unwrap();
    assert_eq!(t.location(), h.s("dir"));
    // Revealing a folder selects it in its parent instead.
    let out = h
        .engine
        .open_from_system(&[h.s("dir/inner")], true)
        .unwrap();
    let (loc, sel) = selected_when_loaded(&h, &out);
    assert_eq!(loc, h.s("dir"));
    assert_eq!(sel, vec!["inner"]);
}

#[test]
fn a_folder_the_front_tab_shows_is_reused() {
    let h = harness();
    touch(&h.p("x.txt"));
    let front = h.window_at("");
    let out = h.engine.open_from_system(&[h.s("x.txt")], true).unwrap();
    assert_eq!(out.tab, Some(front.id()));
    assert_eq!(tab_count(&h), 1);
    eventually("x selected", || front.selection().names == vec!["x.txt"]);
}

#[test]
fn new_window_setting_and_no_window_at_all() {
    let h = harness();
    mkdirs(&h.p("w"));
    // No window open: a window appears whatever the setting.
    let out = h.engine.open_from_system(&[h.s("w")], false).unwrap();
    assert_eq!(h.engine.workspace_state().windows.len(), 1);
    assert_eq!(h.engine.tab(out.tab.unwrap()).unwrap().location(), h.s("w"));
    h.engine
        .set_setting("integration.folder_opens_in", "\"NewWindow\"")
        .unwrap();
    mkdirs(&h.p("v"));
    h.engine.open_from_system(&[h.s("v")], false).unwrap();
    assert_eq!(h.engine.workspace_state().windows.len(), 2);
}

#[test]
fn names_stay_literal_and_unicode_exact() {
    let h = harness();
    // `$HOME`, `~` and a quote would be expanded or parsed if the path went
    // through the address bar's parser; NFD and case variants stay as named.
    let odd = "$HOME ~ 'q'";
    mkdirs(&h.p(odd));
    let nfd = "Cafe\u{301}.txt";
    touch(&h.p(&format!("{odd}/{nfd}")));
    touch(&h.p(&format!("{odd}/README")));
    h.window_at("");
    let out = h
        .engine
        .open_from_system(&[h.s(&format!("{odd}/{nfd}"))], true)
        .unwrap();
    let (loc, sel) = selected_when_loaded(&h, &out);
    assert_eq!(loc, h.s(odd));
    assert_eq!(sel, vec![nfd.to_string()]);
}

#[test]
fn missing_paths_are_reported_not_fatal() {
    let h = harness();
    touch(&h.p("here.txt"));
    h.window_at("");
    let out = h
        .engine
        .open_from_system(&[h.s("gone.txt"), h.s("here.txt")], true)
        .unwrap();
    assert_eq!(out.message.as_deref(), Some("Couldn't find “gone.txt”"));
    assert!(h.engine.open_from_system(&[h.s("gone.txt")], true).is_err());
}

#[cfg(unix)]
#[test]
fn symlink_loops_and_dangling_links_reveal_in_their_folder() {
    let h = harness();
    mkdirs(&h.p("l"));
    std::os::unix::fs::symlink(h.p("l/loop"), h.p("l/loop")).unwrap();
    std::os::unix::fs::symlink(h.p("l/nowhere"), h.p("l/dangling")).unwrap();
    h.window_at("");
    let out = h
        .engine
        .open_from_system(&[h.s("l/loop"), h.s("l/dangling")], true)
        .unwrap();
    let (loc, mut sel) = selected_when_loaded(&h, &out);
    sel.sort();
    assert_eq!(loc, h.s("l"));
    assert_eq!(sel, vec!["dangling", "loop"]);
}

#[test]
fn panel_jump_command_reports_the_active_folder() {
    let h = harness();
    mkdirs(&h.p("here"));
    let t = h.window_at("here");
    let out = h
        .engine
        .perform(
            CommandInvocation {
                id: "panel.jump_here".into(),
                tab: None,
                arg: None,
            },
            Via::Keyboard,
        )
        .unwrap();
    assert_eq!(
        out.shell,
        Some(ShellRequest::JumpFilePanel { dir: h.s("here") })
    );
    assert_eq!(out.tab, Some(t.id()));
}

#[test]
fn integration_settings_default_off_and_round_trip() {
    let h = harness();
    let s = h.engine.settings();
    assert!(!s.integration.panel_jump);
    assert_eq!(h.engine.setting("integration.panel_jump").unwrap(), "false");
    h.engine
        .set_setting("integration.panel_jump", "true")
        .unwrap();
    assert!(h.engine.settings().integration.panel_jump);
    assert!(
        h.engine
            .set_setting("integration.folder_opens_in", "\"Sideways\"")
            .is_err()
    );
}

//! Live workspaces: each keeps its own windows, panes, tabs, history, view
//! options and layout; switching saves one and restores the other.

mod common;

use common::*;
use fstudio_core::*;

/// Everything a workspace must bring back, without the ids (a switch gives
/// tabs and panes new ones).
#[derive(Debug, PartialEq)]
struct Shape {
    active_window: Option<usize>,
    windows: Vec<WindowShape>,
}

#[derive(Debug, PartialEq)]
struct WindowShape {
    orientation: Orientation,
    split_ratio: String,
    sidebar: (String, bool),
    active_pane: u32,
    panes: Vec<PaneShape>,
    hidden: Option<PaneShape>,
}

#[derive(Debug, PartialEq)]
struct PaneShape {
    active_tab: u32,
    tabs: Vec<TabShape>,
}

#[derive(Debug, PartialEq)]
struct TabShape {
    location: String,
    back: Vec<String>,
    forward: Vec<String>,
    view: ViewState,
    filter: String,
}

fn shape(ws: &WorkspaceState) -> Shape {
    let pane = |p: &PaneState| PaneShape {
        active_tab: p.active_tab,
        tabs: p
            .tabs
            .iter()
            .map(|t| TabShape {
                location: t.location.clone(),
                back: t.back.clone(),
                forward: t.forward.clone(),
                view: t.view.clone(),
                filter: t.filter.clone(),
            })
            .collect(),
    };
    Shape {
        active_window: ws
            .active_window
            .and_then(|a| ws.windows.iter().position(|w| w.id == a)),
        windows: ws
            .windows
            .iter()
            .map(|w| WindowShape {
                orientation: w.orientation,
                split_ratio: format!("{:.3}", w.split_ratio),
                sidebar: (format!("{:.0}", w.sidebar_width), w.sidebar_hidden),
                active_pane: w.active_pane,
                panes: w.panes.iter().map(pane).collect(),
                hidden: w.hidden_pane.as_ref().map(pane),
            })
            .collect(),
    }
}

fn now(h: &H) -> Shape {
    shape(&h.engine.workspace_state())
}

fn names_of(h: &H) -> Vec<String> {
    h.engine
        .workspaces()
        .unwrap()
        .into_iter()
        .map(|w| w.name)
        .collect()
}

fn active(h: &H) -> WorkspaceSummary {
    h.engine.active_workspace().unwrap()
}

/// "Everyday": a dual-pane, stacked window with history, a sorted and
/// filtered tab, hidden files on, a 0.3 split and a wide sidebar.
fn build_everyday(h: &H) -> Shape {
    for d in ["a", "b", "c", "photos/raw"] {
        mkdirs(&h.p(d));
    }
    touch(&h.p("a/one.txt"));
    let t = h.window_at("a");
    t.go(&h.s("b")).unwrap();
    wait_complete(&t);
    t.go(&h.s("c")).unwrap();
    wait_complete(&t);
    t.back().unwrap(); // at b: back [a], forward [c]
    wait_complete(&t);
    let mut v = t.view();
    v.sort = SortBy::Size;
    v.ascending = false;
    v.show_hidden = true;
    t.set_view(v);
    t.set_filter_now("on");
    let second = h.engine.open_tab(Some(h.s("a"))).unwrap();
    wait_complete(&second);
    let w = h.engine.workspace_state().windows[0].id;
    h.engine.toggle_dual(w).unwrap();
    h.engine.set_layout(w, Orientation::Vertical, 0.3).unwrap();
    h.engine.set_sidebar(w, 250.0, false).unwrap();
    h.engine.activate_tab(t.id()).unwrap();
    let s = now(h);
    assert_eq!(s.windows[0].panes.len(), 2);
    assert_eq!(s.windows[0].panes[0].tabs[0].back, [h.s("a")]);
    assert_eq!(s.windows[0].panes[0].tabs[0].forward, [h.s("c")]);
    s
}

#[test]
fn first_use_makes_everyday_from_the_open_windows() {
    let h = harness();
    h.window_at("");
    let list = h.engine.workspaces().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "Everyday");
    assert_eq!(list[0].icon, "house");
    assert!(list[0].active);
    assert_eq!(list[0].tabs, 1);
    assert_eq!(list[0].shortcut.as_deref(), Some("⌃⌘1"));
}

#[test]
fn switching_round_trips_every_piece_of_state() {
    let h = harness();
    let everyday = build_everyday(&h);
    let windows_before: Vec<WindowId> = h
        .engine
        .workspace_state()
        .windows
        .iter()
        .map(|w| w.id)
        .collect();

    // A new, empty workspace: one tab at home, single pane.
    let photo = h
        .engine
        .create_workspace("Photo work", "camera", NewWorkspace::Empty)
        .unwrap();
    assert_eq!(active(&h).id, photo);
    let s = h.engine.workspace_state();
    assert_eq!(s.tab_count(), 1);
    assert_eq!(s.windows[0].panes.len(), 1);
    assert_eq!(s.windows[0].panes[0].tabs[0].location, h.s(""));
    // The shell's window stays open (same id) and shows the new workspace.
    assert_eq!(s.windows[0].id, windows_before[0]);
    // The sidebar width carries over to the new workspace's window.
    assert_eq!(s.windows[0].sidebar_width, 250.0);

    // Photo work gets its own tabs and a side-by-side dual pane.
    let t = h.engine.active_tab().unwrap();
    t.go(&h.s("photos/raw")).unwrap();
    wait_complete(&t);
    h.engine.open_tab(Some(h.s("photos"))).unwrap();
    let w = s.windows[0].id;
    h.engine.toggle_dual(w).unwrap();
    h.engine
        .set_layout(w, Orientation::Horizontal, 0.6)
        .unwrap();
    h.engine.set_sidebar(w, 180.0, true).unwrap();
    let photo_shape = now(&h);
    assert_ne!(photo_shape, everyday);

    // Back and forth: each comes back exactly as it was left.
    let list = h.engine.workspaces().unwrap();
    h.engine.switch_workspace(list[0].id).unwrap();
    assert_eq!(now(&h), everyday);
    assert_eq!(active(&h).name, "Everyday");
    h.engine.switch_workspace(photo).unwrap();
    assert_eq!(now(&h), photo_shape);
    h.engine.switch_workspace_by(1).unwrap(); // wraps to Everyday
    assert_eq!(now(&h), everyday);
    // Switching to the active workspace changes nothing.
    let before = h.engine.workspace_state();
    h.engine.switch_workspace(list[0].id).unwrap();
    assert_eq!(h.engine.workspace_state(), before);
    // The tab models are live, not stale copies: the restored tab lists.
    let t = h.engine.active_tab().unwrap();
    wait_complete(&t);
    assert_eq!(t.location(), h.s("b"));
}

#[test]
fn switching_by_shortcut_number_and_commands() {
    let h = harness();
    h.window_at("");
    let photo = h
        .engine
        .create_workspace("Photo work", "camera", NewWorkspace::CopyCurrent)
        .unwrap();
    // ⌃⌘1 goes to Everyday, ⌃⌘2 to Photo work.
    let ctx = InputContext {
        focus: Focus::List,
        tab: h.engine.active_tab().map(|t| t.id()),
        is_repeat: false,
    };
    let chord = |k: char| KeyChord {
        key: Key::Char(k),
        mods: Mods {
            shift: false,
            cmd: true,
            alt: false,
            ctrl: true,
        },
    };
    h.engine.handle_key(chord('1'), ctx);
    assert_eq!(active(&h).name, "Everyday");
    h.engine.handle_key(chord('2'), ctx);
    assert_eq!(active(&h).id, photo);
    // A number with no workspace is refused without changing anything.
    let out = h.engine.handle_key(chord('9'), ctx);
    assert!(out.is_some());
    assert_eq!(active(&h).id, photo);
    h.engine
        .perform(CommandInvocation::new("workspace.previous"), Via::Palette)
        .unwrap();
    assert_eq!(active(&h).name, "Everyday");
    h.engine
        .perform(CommandInvocation::new("workspace.duplicate"), Via::Palette)
        .unwrap();
    assert_eq!(names_of(&h), ["Everyday", "Everyday copy", "Photo work"]);
    assert_eq!(active(&h).name, "Everyday");
}

#[test]
fn restore_after_restart_brings_back_every_workspace() {
    let mut h = harness();
    let everyday = build_everyday(&h);
    let photo = h
        .engine
        .create_workspace("Photo work", "camera", NewWorkspace::Empty)
        .unwrap();
    let t = h.engine.active_tab().unwrap();
    t.go(&h.s("photos")).unwrap();
    wait_complete(&t);
    let w = h.engine.workspace_state().windows[0].id;
    h.engine.toggle_dual(w).unwrap();
    let photo_shape = now(&h);

    h.relaunch();
    let restored = h.engine.restore_session();
    assert_eq!(shape(&restored), photo_shape);
    let a = active(&h);
    assert_eq!(
        (a.id, a.name.as_str(), a.icon.as_str()),
        (photo, "Photo work", "camera")
    );
    assert_eq!(names_of(&h), ["Everyday", "Photo work"]);
    let everyday_id = h.engine.workspaces().unwrap()[0].id;
    h.engine.switch_workspace(everyday_id).unwrap();
    assert_eq!(now(&h), everyday);

    // And once more, from Everyday this time.
    h.relaunch();
    assert_eq!(shape(&h.engine.restore_session()), everyday);
    assert_eq!(active(&h).name, "Everyday");
}

#[test]
fn restore_without_reopening_windows_keeps_other_workspaces() {
    let mut h = harness();
    h.window_at("");
    mkdirs(&h.p("photos"));
    h.engine
        .create_workspace("Photo work", "camera", NewWorkspace::Empty)
        .unwrap();
    h.engine.active_tab().unwrap().go(&h.s("photos")).unwrap();
    let photo_shape = now(&h);
    h.engine.switch_workspace_at(0).unwrap();
    h.engine
        .set_setting("general.restore_session", "false")
        .unwrap();
    h.relaunch();
    h.engine.restore_session();
    assert_eq!(active(&h).name, "Everyday");
    h.engine.switch_workspace_at(1).unwrap();
    assert_eq!(now(&h), photo_shape);
}

#[test]
fn deleting_the_active_workspace_falls_back_to_another() {
    let h = harness();
    mkdirs(&h.p("p"));
    h.window_at("");
    let everyday = active(&h).id;
    let photo = h
        .engine
        .create_workspace("Photo work", "camera", NewWorkspace::Empty)
        .unwrap();
    h.engine.active_tab().unwrap().go(&h.s("p")).unwrap();
    let third = h
        .engine
        .create_workspace("Music", "music.note", NewWorkspace::Empty)
        .unwrap();
    assert_eq!(names_of(&h), ["Everyday", "Photo work", "Music"]);
    // Deleting the active last one falls back to the previous one.
    h.engine.delete_workspace(third).unwrap();
    assert_eq!(active(&h).id, photo);
    assert_eq!(
        h.engine.active_tab().unwrap().location(),
        h.s("p"),
        "Photo work came back with its tab"
    );
    // Deleting the active first one falls back to the next one.
    h.engine.switch_workspace(everyday).unwrap();
    h.engine.delete_workspace(everyday).unwrap();
    assert_eq!(active(&h).id, photo);
    assert_eq!(names_of(&h), ["Photo work"]);
    // The last one stays.
    assert!(h.engine.delete_workspace(photo).is_err());
    assert!(h.engine.ask_delete_workspace(photo).is_err());
    // Nothing on disk was touched.
    assert!(h.p("p").is_dir());
}

#[test]
fn deleting_asks_first_and_touches_no_files() {
    let h = harness();
    mkdirs(&h.p("keep"));
    touch(&h.p("keep/file.txt"));
    h.window_at("keep");
    let photo = h
        .engine
        .create_workspace("Photo work", "camera", NewWorkspace::CopyCurrent)
        .unwrap();
    let out = h
        .engine
        .perform(CommandInvocation::new("workspace.delete"), Via::Menu)
        .unwrap();
    let Some(ShellRequest::Confirm {
        token,
        title,
        detail,
        ..
    }) = out.shell
    else {
        panic!("expected a question, got {out:?}");
    };
    assert_eq!(title, "Delete the workspace “Photo work”?");
    assert!(detail.contains("No files or folders are touched"));
    // Nothing happens until the answer.
    assert_eq!(h.engine.workspaces().unwrap().len(), 2);
    h.engine.cancel_confirmation(token);
    assert_eq!(h.engine.workspaces().unwrap().len(), 2);
    let out = h
        .engine
        .perform(
            CommandInvocation::new("workspace.delete").with_arg(CommandArg::Index(1)),
            Via::Menu,
        )
        .unwrap();
    let Some(ShellRequest::Confirm { token, .. }) = out.shell else {
        panic!("expected a question");
    };
    h.engine.confirm(token).unwrap();
    assert!(h.engine.workspaces().unwrap().iter().all(|w| w.id != photo));
    assert_eq!(active(&h).name, "Everyday");
    assert!(h.p("keep/file.txt").is_file());
    // A token works once.
    assert!(h.engine.confirm(token).is_err());
}

#[test]
fn names_unicode_duplicates_and_empty() {
    let h = harness();
    h.window_at("");
    // Empty and blank names are refused.
    for bad in ["", "   ", "\t", "a\nb"] {
        assert!(matches!(
            h.engine
                .create_workspace(bad, "camera", NewWorkspace::Empty),
            Err(CoreError::InvalidName(_))
        ));
    }
    // Duplicates clash ignoring case and Unicode form.
    assert!(
        h.engine
            .create_workspace("everyday", "", NewWorkspace::Empty)
            .is_err()
    );
    let nfd = "Cafe\u{301} photos";
    let id = h
        .engine
        .create_workspace(nfd, "camera", NewWorkspace::Empty)
        .unwrap();
    // Kept exactly as typed (NFD stays NFD).
    assert_eq!(active(&h).name, nfd);
    assert!(
        h.engine
            .create_workspace("CAF\u{c9} PHOTOS", "", NewWorkspace::Empty)
            .is_err()
    );
    // Renaming to another workspace's name is refused; to its own name in
    // another case is fine.
    let everyday = h.engine.workspaces().unwrap()[0].id;
    assert!(
        h.engine
            .rename_workspace(everyday, " café PHOTOS ")
            .is_err()
    );
    h.engine.rename_workspace(id, "CAFÉ PHOTOS").unwrap();
    assert_eq!(active(&h).name, "CAFÉ PHOTOS");
    h.engine
        .rename_workspace(everyday, "  Tous les jours 📁 ")
        .unwrap();
    assert_eq!(names_of(&h)[0], "Tous les jours 📁");
    assert!(h.engine.rename_workspace(everyday, "").is_err());
    // Duplicating twice numbers the copies.
    h.engine.duplicate_workspace(id).unwrap();
    h.engine.duplicate_workspace(id).unwrap();
    assert_eq!(
        names_of(&h),
        [
            "Tous les jours 📁",
            "CAFÉ PHOTOS",
            "CAFÉ PHOTOS copy 2",
            "CAFÉ PHOTOS copy"
        ]
    );
    // A bad icon falls back to the default; a symbol name is kept.
    h.engine.set_workspace_icon(id, "../../x").unwrap();
    assert_eq!(active(&h).icon, "square.grid.2x2");
    h.engine
        .set_workspace_icon(id, "photo.on.rectangle")
        .unwrap();
    assert_eq!(active(&h).icon, "photo.on.rectangle");
}

#[test]
fn reordering_moves_the_shortcuts() {
    let h = harness();
    h.window_at("");
    let photo = h
        .engine
        .create_workspace("Photo work", "camera", NewWorkspace::Empty)
        .unwrap();
    h.engine
        .create_workspace("Music", "music.note", NewWorkspace::Empty)
        .unwrap();
    h.engine.move_workspace(photo, 0).unwrap();
    let list = h.engine.workspaces().unwrap();
    assert_eq!(
        list.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(),
        ["Photo work", "Everyday", "Music"]
    );
    assert_eq!(list[0].shortcut.as_deref(), Some("⌃⌘1"));
    assert_eq!(list[0].position, 0);
    h.engine.move_workspace(photo, 99).unwrap();
    assert_eq!(names_of(&h), ["Everyday", "Music", "Photo work"]);
    h.engine.switch_workspace_at(0).unwrap();
    assert_eq!(active(&h).name, "Everyday");
    assert!(h.engine.move_workspace(WorkspaceId(9999), 0).is_err());
}

#[test]
fn closed_tabs_stay_with_their_workspace() {
    let h = harness();
    mkdirs(&h.p("x"));
    h.window_at("");
    let t = h.engine.open_tab(Some(h.s("x"))).unwrap();
    h.engine.close_tab(t.id()).unwrap();
    h.engine
        .create_workspace("Photo work", "camera", NewWorkspace::Empty)
        .unwrap();
    // Nothing to reopen in Photo work…
    assert!(h.engine.reopen_closed_tab().unwrap().is_none());
    // …but Everyday still has its closed tab.
    h.engine.switch_workspace_at(0).unwrap();
    let back = h.engine.reopen_closed_tab().unwrap().unwrap();
    assert_eq!(back.location(), h.s("x"));
}

#[test]
fn workspace_changes_are_announced() {
    let h = harness();
    h.window_at("");
    h.drain();
    h.engine
        .create_workspace("Photo work", "camera", NewWorkspace::Empty)
        .unwrap();
    h.expect_event("workspaces changed", |e| {
        matches!(e, CoreEvent::WorkspacesChanged)
    });
}

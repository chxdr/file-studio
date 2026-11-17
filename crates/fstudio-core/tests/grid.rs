//! Icon and gallery view (M3): 2D arrow keys through the real key path, the
//! per-folder view mode, the thumbnail size commands and the order of
//! thumbnail requests.

mod common;

use common::*;
use fstudio_core::*;
use fstudio_model::{EntryKind, file_flags};

fn chord(k: Key, shift: bool) -> KeyChord {
    KeyChord {
        key: k,
        mods: Mods {
            shift,
            ..Mods::NONE
        },
    }
}

fn ctx(t: &Tab) -> InputContext {
    InputContext {
        focus: Focus::List,
        tab: Some(t.id()),
        is_repeat: false,
    }
}

fn run(h: &H, t: &Tab, id: &str) {
    h.engine
        .perform(
            CommandInvocation {
                id: id.into(),
                arg: None,
                tab: Some(t.id()),
            },
            Via::Keyboard,
        )
        .unwrap();
}

fn cursor(t: &Tab) -> Option<u32> {
    t.selection().cursor
}

/// Ten files `f00`…`f09` in "grid", shown as icons with 4 per row.
fn icon_tab(h: &H) -> std::sync::Arc<Tab> {
    mkdirs(&h.p("grid"));
    for i in 0..10 {
        touch(&h.p(&format!("grid/f{i:02}.jpg")));
    }
    let t = h.window_at("grid");
    wait_complete(&t);
    run(h, &t, "view.icons");
    h.engine.set_grid_columns(t.id(), 4).unwrap();
    t
}

#[test]
fn arrows_move_in_two_dimensions_in_icon_view() {
    let h = harness();
    let t = icon_tab(&h);
    t.set_selection(SelectionChange::Only(5));
    let press = |k, shift| {
        h.engine
            .handle_key(chord(k, shift), ctx(&t))
            .expect("taken")
    };
    press(Key::Down, false);
    assert_eq!(cursor(&t), Some(9));
    press(Key::Up, false);
    assert_eq!(cursor(&t), Some(5));
    press(Key::Up, false);
    assert_eq!(cursor(&t), Some(1));
    press(Key::Up, false);
    assert_eq!(cursor(&t), Some(1), "first row: ↑ stays");
    press(Key::Right, false);
    press(Key::Right, false);
    press(Key::Right, false);
    assert_eq!(cursor(&t), Some(4), "→ runs on to the next row");
    // ⇧↓ extends a range from the anchor.
    press(Key::Down, true);
    assert_eq!(cursor(&t), Some(8));
    assert_eq!(t.selection().rows, (4..=8).collect::<Vec<u32>>());
    // A plain arrow collapses it again.
    press(Key::Left, false);
    assert_eq!(t.selection().rows, vec![7]);
}

#[test]
fn up_and_down_commands_move_a_row_in_icon_view_only() {
    let h = harness();
    let t = icon_tab(&h);
    t.set_selection(SelectionChange::Only(1));
    run(&h, &t, "cursor.down");
    assert_eq!(
        cursor(&t),
        Some(5),
        "the command (Vim j, the menu) moves a row too"
    );
    run(&h, &t, "select.extend_up");
    assert_eq!(t.selection().rows, vec![1, 2, 3, 4, 5]);
    // In the list, ↓ is one row again and ←/→ aren't the grid's.
    run(&h, &t, "view.list");
    t.set_selection(SelectionChange::Only(1));
    h.engine.handle_key(chord(Key::Down, false), ctx(&t));
    assert_eq!(cursor(&t), Some(2));
}

#[test]
fn gallery_steps_one_item_with_left_and_right() {
    let h = harness();
    let t = icon_tab(&h);
    run(&h, &t, "view.gallery");
    t.set_selection(SelectionChange::Only(3));
    h.engine
        .handle_key(chord(Key::Right, false), ctx(&t))
        .unwrap();
    assert_eq!(cursor(&t), Some(4), "the grid's 4 columns don't apply");
    h.engine
        .handle_key(chord(Key::Left, false), ctx(&t))
        .unwrap();
    h.engine
        .handle_key(chord(Key::Left, false), ctx(&t))
        .unwrap();
    assert_eq!(cursor(&t), Some(2));
    h.engine
        .handle_key(chord(Key::Down, false), ctx(&t))
        .unwrap();
    assert_eq!(cursor(&t), Some(3), "↓ is the list's one-item move");
    // With nothing selected the gallery shows the first item: → goes to the second.
    t.set_selection(SelectionChange::Rows(vec![]));
    assert_eq!(cursor(&t), None);
    assert_eq!(h.engine.gallery_requests(t.id(), true).unwrap()[0], 0);
    h.engine
        .handle_key(chord(Key::Right, false), ctx(&t))
        .unwrap();
    assert_eq!(cursor(&t), Some(1));
}

#[test]
fn the_view_mode_is_remembered_per_folder() {
    let h = harness();
    let t = icon_tab(&h);
    mkdirs(&h.p("other"));
    t.go(&h.s("other")).unwrap();
    wait_complete(&t);
    assert_eq!(
        t.view().mode,
        ViewMode::List,
        "a new folder uses the default"
    );
    run(&h, &t, "view.gallery");
    t.go(&h.s("grid")).unwrap();
    wait_complete(&t);
    assert_eq!(t.view().mode, ViewMode::Icons);
    t.go(&h.s("other")).unwrap();
    wait_complete(&t);
    assert_eq!(t.view().mode, ViewMode::Gallery);
}

#[test]
fn thumbnail_size_commands_step_and_stop() {
    let h = harness();
    let t = icon_tab(&h);
    assert_eq!(h.engine.settings().grid.icon_size, 128);
    run(&h, &t, "view.icons_bigger");
    assert_eq!(h.engine.settings().grid.icon_size, 160);
    run(&h, &t, "view.icons_smaller");
    run(&h, &t, "view.icons_smaller");
    assert_eq!(h.engine.settings().grid.icon_size, 96);
    for _ in 0..20 {
        run(&h, &t, "view.icons_smaller");
    }
    assert_eq!(h.engine.settings().grid.icon_size, 32);
    // The slider: any value, kept in range; settings refuse out-of-range JSON.
    h.engine.set_icon_size(1000).unwrap();
    assert_eq!(h.engine.settings().grid.icon_size, 256);
    h.engine.set_icon_size(77).unwrap();
    assert_eq!(h.engine.setting("grid.icon_size").unwrap(), "77");
    assert!(h.engine.set_setting("grid.icon_size", "4").is_err());
}

#[test]
fn thumbnails_are_asked_for_files_only_cursor_first() {
    let h = harness();
    mkdirs(&h.p("mix/a folder"));
    mkdirs(&h.p("mix/Photos.app/Contents"));
    // NFD and NFC names are both files and both get thumbnails.
    touch(&h.p("mix/cafe\u{301}.jpg"));
    touch(&h.p("mix/caf\u{e9}2.jpg"));
    touch(&h.p("mix/z.arw"));
    let t = h.window_at("mix");
    wait_complete(&t);
    run(&h, &t, "view.icons");
    let ns = names(&t);
    let at = |n: &str| ns.iter().position(|x| x == n).unwrap() as u32;
    t.set_selection(SelectionChange::Only(at("z.arw")));
    let order = h.engine.thumbnail_requests(t.id(), 0, 10, true).unwrap();
    assert_eq!(order[0], at("z.arw"), "the cursor first");
    let mut got: Vec<String> = order.iter().map(|&i| ns[i as usize].clone()).collect();
    got.sort();
    let mut want = vec![
        "cafe\u{301}.jpg".to_string(),
        "caf\u{e9}2.jpg".into(),
        "z.arw".into(),
    ];
    want.sort();
    assert_eq!(got, want, "no folder, no package; names exactly as on disk");
    // The gallery's big pictures: cursor, then onward.
    t.set_selection(SelectionChange::Only(at("caf\u{e9}2.jpg")));
    let g = h.engine.gallery_requests(t.id(), true).unwrap();
    assert_eq!(g[0], at("caf\u{e9}2.jpg"));
    assert!(g.iter().all(|&i| order.contains(&i)));
}

#[test]
fn placeholders_folders_and_packages_never_get_thumbnails() {
    let e = |kind, flags, is_package| fstudio_model::Entry {
        name: "x".into(),
        kind,
        size: 1,
        alloc_size: None,
        mtime_ns: 0,
        btime_ns: None,
        mode: 0o644,
        flags,
        inode: 1,
        dev: 1,
        is_package,
    };
    assert!(wants_thumbnail(&e(EntryKind::File, 0, false)));
    assert!(!wants_thumbnail(&e(
        EntryKind::File,
        file_flags::DATALESS,
        false
    )));
    assert!(!wants_thumbnail(&e(EntryKind::Dir, 0, false)));
    assert!(!wants_thumbnail(&e(EntryKind::Dir, 0, true)));
}

#[test]
fn a_big_folder_orders_only_what_is_near_the_screen() {
    let h = harness();
    mkdirs(&h.p("big"));
    let n = if std::env::var("FSTUDIO_HUGE").is_ok() {
        20_000
    } else {
        2_000
    };
    for i in 0..n {
        touch(&h.p(&format!("big/IMG_{i:05}.CR3")));
    }
    let t = h.window_at("big");
    wait_complete(&t);
    run(&h, &t, "view.icons");
    h.engine.set_grid_columns(t.id(), 6).unwrap();
    let order = h.engine.thumbnail_requests(t.id(), 600, 48, true).unwrap();
    assert_eq!(order.len(), 48 * 4, "a screen, two ahead, one behind");
    assert_eq!(order[0], 600);
    assert_eq!(order[48], 648, "then what scrolls in below");
}

//! Disk usage through the engine: the view's rows, Get Folder Size, and
//! Move to Trash from the view through the ops queue with undo (F-007).

mod common;

use std::sync::Arc;

use common::*;
use fstudio_core::*;

struct Quiet;
impl UsageObserver for Quiet {
    fn progress(&self, _: UsageProgress) {}
    fn changed(&self, _: UsageChange) {}
}

fn write(h: &H, rel: &str, n: usize) {
    let p = h.p(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, vec![3u8; n]).unwrap();
}

fn scanned(v: &UsageView) {
    eventually("scan done", || v.summary().state != UsageState::Scanning);
}

#[test]
fn view_rows_are_formatted() {
    let h = harness();
    write(&h, "shoot/a.cr3", 3_000_000);
    write(&h, "shoot/b.jpg", 1_000_000);
    write(&h, "misc.txt", 1_000);
    let v = h.engine.usage_scan(&h.s(""), Arc::new(Quiet)).unwrap();
    scanned(&v);
    let s = v.summary();
    assert_eq!(s.state, UsageState::Done);
    assert_eq!(s.items_text, "4 items");
    assert!(s.detail.starts_with("Scanned in"));
    v.set_mode(UsageSizeMode::Logical);
    let rows = v.rows(0, 10);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].depth, 0);
    assert_eq!(rows[0].size_text, "4 MB");
    assert_eq!(rows[1].name, "shoot");
    assert_eq!(rows[1].kind, RowKind::Folder);
    assert_eq!(rows[1].items_text, "2");
    assert!((rows[1].fraction - 4_000_000.0 / 4_001_000.0).abs() < 1e-9);
    assert_eq!(rows[2].ext, "txt");
    let types = v.types();
    assert_eq!(types[0].name, "CR3");
    assert_eq!(types[0].kind_text, "RAW Photo");
    assert_eq!(types[0].slot, 1);
    assert_eq!(v.legend()[0], ("CR3".to_string(), 1));
    let shoot = v.find(&h.s("shoot")).unwrap();
    let info = v.info(shoot).unwrap();
    assert_eq!(info.items_text, "2 items");
    assert!(info.is_folder);
    assert!(
        info.share_text.starts_with("100.0 % of “root”"),
        "{}",
        info.share_text
    );
    v.set_zoom(shoot);
    assert_eq!(v.zoom_path().len(), 2);
    // A file can't be scanned.
    assert!(
        h.engine
            .usage_scan(&h.s("misc.txt"), Arc::new(Quiet))
            .is_err()
    );
}

#[test]
fn get_folder_size_toasts_the_total() {
    let h = harness();
    write(&h, "big/one.bin", 2_000_000);
    write(&h, "big/deep/two.bin", 500_000);
    let t = h.window_at("");
    t.set_selection(SelectionChange::Names(vec!["big".into()]));
    h.drain();
    let out = h
        .engine
        .perform(CommandInvocation::new("file.folder_size"), Via::Menu)
        .unwrap();
    assert_eq!(out.command.as_deref(), Some("file.folder_size"));
    match h.expect_toast() {
        ShellRequest::Toast { text, style, .. } => {
            assert!(text.starts_with("“big”: "), "{text}");
            assert!(text.contains("(2.5 MB)"), "{text}");
            assert!(text.contains("3 items"), "{text}");
            assert_eq!(style, ToastStyle::Info);
        }
        other => panic!("{other:?}"),
    }
    let cached = h.engine.cached_folder_size(&h.s("big")).unwrap();
    assert_eq!(cached.logical, 2_500_000);
}

#[test]
fn show_disk_usage_from_the_go_menu_uses_the_current_folder() {
    let h = harness();
    let _t = h.window_at("");
    let out = h
        .engine
        .perform(CommandInvocation::new("nav.disk_usage"), Via::Menu)
        .unwrap();
    assert_eq!(
        out.shell,
        Some(ShellRequest::ShowDiskUsage { path: h.s("") })
    );
}

#[test]
fn trash_from_the_view_goes_through_the_queue_and_undoes() {
    let h = harness();
    write(&h, "keep/a.bin", 1_000);
    write(&h, "old/b.bin", 9_000);
    let v = h.engine.usage_scan(&h.s(""), Arc::new(Quiet)).unwrap();
    scanned(&v);
    v.set_mode(UsageSizeMode::Logical);
    let old = v.find(&h.s("old")).unwrap();
    let paths = v.paths(&[old]);
    let out = h.engine.usage_trash(paths.clone()).unwrap();
    assert_eq!(out.ops.len(), 1);
    assert!(h.engine.ops().wait_idle(T));
    assert!(!h.p("old").exists());
    v.sync_paths(&paths);
    assert_eq!(v.rows(0, 1)[0].size, 1_000);
    assert!(v.find(&h.s("old")).is_none());

    // Undo puts it back; syncing rescans its folder.
    h.engine.undo().unwrap();
    assert!(h.engine.ops().wait_idle(T));
    assert!(h.p("old/b.bin").exists());
    v.sync_paths(&paths);
    eventually("old back in the tree", || v.find(&h.s("old")).is_some());
    eventually("sizes back", || v.rows(0, 1)[0].size == 10_000);

    // With "Confirm Move to Trash" on, the view asks first.
    h.engine
        .set_setting("general.confirm_trash", "true")
        .unwrap();
    let out = h.engine.usage_trash(paths).unwrap();
    assert!(matches!(out.shell, Some(ShellRequest::ConfirmTrash { .. })));
    assert!(h.p("old").exists());
}

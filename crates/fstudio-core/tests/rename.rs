//! Batch rename through the engine: the command, the session's preview,
//! applying through the queue, one-step undo, and presets that persist.

mod common;

use common::*;
use fstudio_core::*;

fn tpl(t: &str) -> Pipeline {
    Pipeline::of(vec![Step::Template { template: t.into() }])
}

fn wait_ops(h: &H, out: &Outcome) {
    for id in &out.ops {
        let st = h.engine.ops().wait(*id, std::time::Duration::from_secs(60));
        assert!(st.is_some_and(|s| s.state.is_finished()));
    }
}

fn dir_names(p: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(p)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn the_command_asks_the_shell_for_the_sheet_with_the_selection() {
    let h = harness();
    touch(&h.p("a.txt"));
    touch(&h.p("b.txt"));
    let t = h.window_at("");
    wait_complete(&t);
    t.set_selection(SelectionChange::Names(vec!["a.txt".into(), "b.txt".into()]));
    let out = h
        .engine
        .perform(CommandInvocation::new("file.batch_rename"), Via::Keyboard)
        .unwrap();
    match out.shell {
        Some(ShellRequest::BatchRename { tab, paths }) => {
            assert_eq!(tab, t.id());
            assert_eq!(paths, vec![h.s("a.txt"), h.s("b.txt")]);
        }
        other => panic!("{other:?}"),
    }
    // The multi-selection menu says how many, with the shortcut.
    let items = h
        .engine
        .context_menu(
            MenuTarget::Multi {
                tab: t.id(),
                paths: vec![h.s("a.txt"), h.s("b.txt")],
            },
            MenuContext::default(),
        )
        .unwrap();
    let br = items
        .iter()
        .find_map(|i| match i {
            MenuItem::Command(c) if c.entry == "file.batch_rename" => Some(c.clone()),
            _ => None,
        })
        .expect("Batch Rename is in the menu");
    assert_eq!(br.title, "Batch Rename 2 Items…");
    assert!(br.enabled);
    assert_eq!(br.shortcut.as_deref(), Some("⌃⌘R"));
}

#[test]
fn preview_apply_and_undo() {
    let h = harness();
    for n in ["IMG_1.JPG", "IMG_2.JPG", "IMG_3.JPG", "keep.txt"] {
        touch(&h.p(n));
    }
    let t = h.window_at("");
    wait_complete(&t);
    let paths: Vec<String> = ["IMG_1.JPG", "IMG_2.JPG", "IMG_3.JPG"]
        .iter()
        .map(|n| h.s(n))
        .collect();
    let s = h.engine.batch_rename(Some(t.id()), paths).unwrap();
    let sum = s.set_pipeline(Pipeline::of(vec![
        Step::Template {
            template: "Trip {n:2}".into(),
        },
        Step::Extension {
            change: ExtChange::Lower,
        },
    ]));
    assert_eq!((sum.ready, sum.problems), (3, 0), "{sum:?}");
    let rows = s.rows(0, 10);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1].old_name, "IMG_2.JPG");
    assert_eq!(rows[1].new_name, "Trip 02.jpg");
    // A bad regex names its step; nothing would be renamed.
    let bad = s.set_pipeline(Pipeline::of(vec![
        Step::Case {
            case: CaseKind::Upper,
            target: Target::Name,
        },
        Step::Replace {
            find: "(".into(),
            replace: String::new(),
            regex: true,
            ignore_case: false,
            first_only: false,
            target: Target::Name,
        },
    ]));
    assert_eq!(bad.error_step, Some(1));
    assert!(s.apply().is_err());
    // A name that is taken by an item that stays is flagged.
    let clash = s.set_pipeline(tpl("keep"));
    assert_eq!(clash.problems, 3, "{clash:?}");
    assert_eq!(s.rows(0, 1)[0].status, RenameRowStatus::Problem);

    s.set_pipeline(Pipeline::of(vec![
        Step::Template {
            template: "Trip {n:2}".into(),
        },
        Step::Extension {
            change: ExtChange::Lower,
        },
    ]));
    let out = s.apply().unwrap();
    wait_ops(&h, &out);
    assert_eq!(
        dir_names(&h.root),
        ["Trip 01.jpg", "Trip 02.jpg", "Trip 03.jpg", "keep.txt"]
    );
    match h.expect_toast() {
        ShellRequest::Toast { text, button, .. } => {
            assert_eq!(text, "Renamed 3 items");
            assert_eq!(button.as_deref(), Some("Undo"));
        }
        other => panic!("{other:?}"),
    }
    // One undo puts every name back.
    let out = h.engine.undo().unwrap();
    wait_ops(&h, &out);
    assert_eq!(
        dir_names(&h.root),
        ["IMG_1.JPG", "IMG_2.JPG", "IMG_3.JPG", "keep.txt"]
    );
}

#[test]
fn swaps_apply_through_the_session() {
    let h = harness();
    std::fs::write(h.p("a"), "a").unwrap();
    std::fs::write(h.p("b"), "b").unwrap();
    let s = h
        .engine
        .batch_rename(None, vec![h.s("a"), h.s("b")])
        .unwrap();
    // a→b, b→a with one regex: swap the letters.
    let sum = s.set_pipeline(Pipeline::of(vec![
        Step::Replace {
            find: "^a$".into(),
            replace: "\u{1}".into(),
            regex: true,
            ignore_case: false,
            first_only: false,
            target: Target::Whole,
        },
        Step::Replace {
            find: "^b$".into(),
            replace: "a".into(),
            regex: true,
            ignore_case: false,
            first_only: false,
            target: Target::Whole,
        },
        Step::Replace {
            find: "\u{1}".into(),
            replace: "b".into(),
            regex: false,
            ignore_case: false,
            first_only: false,
            target: Target::Whole,
        },
    ]));
    assert_eq!(sum.ready, 2, "{sum:?}");
    wait_ops(&h, &s.apply().unwrap());
    assert_eq!(std::fs::read_to_string(h.p("a")).unwrap(), "b");
    assert_eq!(std::fs::read_to_string(h.p("b")).unwrap(), "a");
    assert_eq!(dir_names(&h.root), ["a", "b"]);
}

#[test]
fn apply_refuses_when_an_item_changed_since_the_sheet_opened() {
    let h = harness();
    touch(&h.p("a"));
    let s = h.engine.batch_rename(None, vec![h.s("a")]).unwrap();
    s.set_pipeline(tpl("z"));
    std::fs::remove_file(h.p("a")).unwrap();
    touch(&h.p("a"));
    let out = s.apply().unwrap();
    assert!(out.ops.is_empty());
    assert!(out.message.is_some(), "{out:?}");
    assert_eq!(dir_names(&h.root), ["a"]);
}

#[test]
fn exif_tokens_load_in_the_background() {
    let h = harness();
    touch(&h.p("notes.txt"));
    let s = h.engine.batch_rename(None, vec![h.s("notes.txt")]).unwrap();
    s.set_pipeline(tpl("{exif.camera}x"));
    eventually("exif read", || !s.summary().exif_loading);
    let sum = s.summary();
    assert_eq!(sum.warnings, 1, "{sum:?}");
    let r = &s.rows(0, 1)[0];
    assert_eq!(r.new_name, "x.txt");
    assert!(!r.warning.is_empty());
}

#[test]
fn presets_persist_and_the_last_steps_come_back() {
    let mut h = harness();
    let defaults = h.engine.rename_presets();
    assert!(!defaults.is_empty());
    h.engine.save_rename_preset("Mine", tpl("{n:4}")).unwrap();
    assert!(h.engine.save_rename_preset("  ", tpl("x")).is_err());
    touch(&h.p("f"));
    let s = h.engine.batch_rename(None, vec![h.s("f")]).unwrap();
    s.set_pipeline(tpl("g"));
    wait_ops(&h, &s.apply().unwrap());
    h.relaunch();
    let ps = h.engine.rename_presets();
    assert_eq!(ps.last().unwrap().name, "Mine");
    assert_eq!(ps.last().unwrap().pipeline, tpl("{n:4}"));
    assert!(h.engine.delete_rename_preset("mine").unwrap());
    assert_eq!(h.engine.rename_presets().len(), defaults.len());
    // A new sheet starts with the steps last applied.
    let s = h.engine.batch_rename(None, vec![h.s("g")]).unwrap();
    assert_eq!(s.pipeline(), tpl("g"));
}

#[test]
fn many_folders_and_ten_thousand_items() {
    let h = harness();
    mkdirs(&h.p("x"));
    mkdirs(&h.p("y"));
    touch(&h.p("x/a"));
    touch(&h.p("y/a"));
    let s = h
        .engine
        .batch_rename(None, vec![h.s("x/a"), h.s("y/a")])
        .unwrap();
    let sum = s.set_pipeline(tpl("same"));
    assert!(sum.many_folders);
    assert_eq!(sum.ready, 2);
    assert_eq!(s.rows(1, 1)[0].folder, "y");

    if std::env::var("FSTUDIO_BIG").is_err() {
        return;
    }
    mkdirs(&h.p("big"));
    let paths: Vec<String> = (0..10_000)
        .map(|i| {
            let p = h.p(&format!("big/IMG_{i:05}.JPG"));
            touch(&p);
            p.to_string_lossy().into_owned()
        })
        .collect();
    let t = std::time::Instant::now();
    let s = h.engine.batch_rename(None, paths).unwrap();
    eprintln!("open 10k: {:?}", t.elapsed());
    let t = std::time::Instant::now();
    let sum = s.set_pipeline(tpl("Trip {n:5}"));
    eprintln!("preview 10k: {:?}", t.elapsed());
    assert_eq!(sum.ready, 10_000);
    let t = std::time::Instant::now();
    wait_ops(&h, &s.apply().unwrap());
    eprintln!("apply 10k: {:?}", t.elapsed());
    assert!(h.p("big/Trip 10000.JPG").exists());
}

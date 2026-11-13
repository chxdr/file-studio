//! Undo and redo round trips, refusals when the world changed, and the
//! persisted stack.

mod common;
use common::*;
use fstudio_ops::*;

fn label(e: &Env) -> Option<String> {
    e.q.undo_state().undo
}

#[test]
fn copy_undo_redo() {
    let e = env();
    write(&e.p("s/a.txt"), b"a");
    write(&e.p("s/b.txt"), b"b");
    std::fs::create_dir_all(e.p("d")).unwrap();
    e.run(OpRequest::Copy {
        sources: vec![e.p("s/a.txt"), e.p("s/b.txt")],
        dest_dir: e.p("d"),
    });
    assert_eq!(label(&e).as_deref(), Some("Undo Copy of 2 Items"));
    e.undo().unwrap();
    assert!(!exists(&e.p("d/a.txt")) && !exists(&e.p("d/b.txt")));
    assert!(exists(&e.p("s/a.txt")));
    assert_eq!(
        e.q.undo_state().redo.as_deref(),
        Some("Redo Copy of 2 Items")
    );
    let id = e.q.redo().unwrap().unwrap();
    assert_eq!(e.q.wait(id, WAIT).unwrap().state, OpState::Completed);
    assert_eq!(read(&e.p("d/a.txt")), b"a");
    assert_eq!(label(&e).as_deref(), Some("Undo Copy of 2 Items"));
    assert_eq!(e.q.undo_state().redo, None);
}

#[test]
fn copy_undo_refuses_after_a_change() {
    let e = env();
    write(&e.p("s/a.txt"), b"a");
    std::fs::create_dir_all(e.p("d")).unwrap();
    e.run(OpRequest::Copy {
        sources: vec![e.p("s/a.txt")],
        dest_dir: e.p("d"),
    });
    std::thread::sleep(std::time::Duration::from_millis(10));
    write(&e.p("d/a.txt"), b"edited");
    match e.q.undo() {
        Err(OpsError::UndoRefused(why)) => assert!(why.contains("changed"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(read(&e.p("d/a.txt")), b"edited");
}

#[test]
fn move_undo_same_and_cross_volume() {
    let img = Image::new("APFS", 64);
    for dest in [None, Some(img.mount.clone())] {
        let e = env();
        write(&e.p("s/dir/a.txt"), b"a");
        write(&e.p("s/f.txt"), b"f");
        let before = tree(&e.p("s"));
        let dest = dest.clone().unwrap_or_else(|| e.p("d"));
        let dest = dest.join(format!("x{}", rand_tag()));
        std::fs::create_dir_all(&dest).unwrap();
        e.run(OpRequest::Move {
            sources: vec![e.p("s/dir"), e.p("s/f.txt")],
            dest_dir: dest.clone(),
        });
        assert!(!exists(&e.p("s/dir")) && exists(&dest.join("dir/a.txt")));
        assert_eq!(label(&e).as_deref(), Some("Undo Move of 2 Items"));
        e.undo().unwrap();
        assert_eq!(tree(&e.p("s")), before);
        assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 0);
    }
}

fn rand_tag() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

#[test]
fn move_undo_refuses_when_origin_is_taken() {
    let e = env();
    write(&e.p("s/a.txt"), b"a");
    std::fs::create_dir_all(e.p("d")).unwrap();
    e.run(OpRequest::Move {
        sources: vec![e.p("s/a.txt")],
        dest_dir: e.p("d"),
    });
    write(&e.p("s/a.txt"), b"new one");
    assert!(matches!(e.q.undo(), Err(OpsError::UndoRefused(_))));
    assert_eq!(read(&e.p("s/a.txt")), b"new one");
    assert_eq!(read(&e.p("d/a.txt")), b"a");
}

#[test]
fn trash_undo_puts_back() {
    let e = env();
    write(&e.p("s/a.txt"), b"a");
    write(&e.p("s/dir/b.txt"), b"b");
    e.run(OpRequest::Trash {
        paths: vec![e.p("s/a.txt"), e.p("s/dir")],
    });
    assert!(!exists(&e.p("s/a.txt")));
    assert_eq!(label(&e).as_deref(), Some("Undo Move to Trash of 2 Items"));
    e.undo().unwrap();
    assert_eq!(read(&e.p("s/a.txt")), b"a");
    assert_eq!(read(&e.p("s/dir/b.txt")), b"b");
    assert_eq!(std::fs::read_dir(&e.trash).unwrap().count(), 0);
}

#[test]
fn rename_undo() {
    let e = env();
    write(&e.p("s/a.txt"), b"a");
    e.run(OpRequest::Rename {
        path: e.p("s/a.txt"),
        new_name: "b.txt".into(),
    });
    assert_eq!(label(&e).as_deref(), Some("Undo Rename of “a.txt”"));
    e.undo().unwrap();
    assert_eq!(read(&e.p("s/a.txt")), b"a");
    assert!(!exists(&e.p("s/b.txt")));
    let id = e.q.redo().unwrap().unwrap();
    e.q.wait(id, WAIT);
    assert!(exists(&e.p("s/b.txt")));
}

#[test]
fn new_folder_undo_only_when_empty() {
    let e = env();
    std::fs::create_dir_all(e.p("d")).unwrap();
    e.run(OpRequest::NewFolder {
        parent: e.p("d"),
        name: "New".into(),
    });
    assert_eq!(label(&e).as_deref(), Some("Undo New Folder"));
    write(&e.p("d/New/x"), b"x");
    assert!(matches!(e.q.undo(), Err(OpsError::UndoRefused(_))));
    std::fs::rename(e.p("d/New/x"), e.p("d/x")).unwrap();
    e.undo().unwrap();
    assert!(!exists(&e.p("d/New")));
}

#[test]
fn duplicate_undo() {
    let e = env();
    write(&e.p("d/a.txt"), b"a");
    e.run(OpRequest::Duplicate {
        paths: vec![e.p("d/a.txt")],
    });
    assert!(exists(&e.p("d/a copy.txt")));
    e.undo().unwrap();
    assert!(!exists(&e.p("d/a copy.txt")) && exists(&e.p("d/a.txt")));
}

#[test]
fn undo_stack_survives_a_restart() {
    let e = env();
    write(&e.p("s/a.txt"), b"a");
    std::fs::create_dir_all(e.p("d")).unwrap();
    e.run(OpRequest::Copy {
        sources: vec![e.p("s/a.txt")],
        dest_dir: e.p("d"),
    });
    let q2 = open_queue(
        &e.db_path,
        OpsConfig::default(),
        &e.trash,
        std::sync::Arc::new(NullSink),
    );
    assert_eq!(
        q2.undo_state().undo.as_deref(),
        Some("Undo Copy of “a.txt”")
    );
    for id in q2.undo().unwrap() {
        q2.wait(id, WAIT);
    }
    assert!(!exists(&e.p("d/a.txt")));
}

#[test]
fn a_new_op_clears_redo() {
    let e = env();
    write(&e.p("s/a.txt"), b"a");
    e.run(OpRequest::Duplicate {
        paths: vec![e.p("s/a.txt")],
    });
    e.undo().unwrap();
    assert!(e.q.undo_state().redo.is_some());
    e.run(OpRequest::NewFile {
        parent: e.p("s"),
        name: "n".into(),
    });
    assert_eq!(e.q.undo_state().redo, None);
}

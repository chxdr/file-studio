//! Moves: same-volume renames, cross-volume copy → verify → sync → remove,
//! and a failure injected at every step (the source always survives).

mod common;
use common::*;
use fstudio_ops::*;
use std::sync::Arc;

const STEPS: [FaultStep; 5] = [
    FaultStep::BeforeCopy,
    FaultStep::AfterCopy,
    FaultStep::Verify,
    FaultStep::BeforePlace,
    FaultStep::BeforeSourceRemove,
];

fn make_src(e: &Env) {
    write(&e.p("src/victim.bin"), &vec![9u8; 2 << 20]);
    write(&e.p("src/ok1.txt"), b"one");
    write(&e.p("src/sub/ok2.txt"), b"two");
}

#[test]
fn same_volume_move_is_a_rename() {
    let e = env();
    make_src(&e);
    let ino =
        std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(e.p("src/victim.bin")).unwrap());
    std::fs::create_dir_all(e.p("dst")).unwrap();
    let st = e.run(OpRequest::Move {
        sources: vec![e.p("src")],
        dest_dir: e.p("dst"),
    });
    assert_eq!(st.state, OpState::Completed);
    assert!(!exists(&e.p("src")));
    let ino2 =
        std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(e.p("dst/src/victim.bin")).unwrap());
    assert_eq!(ino, ino2, "a same-volume move must not copy");
    assert_eq!(st.files_total, 1);
}

#[test]
fn cross_volume_move_moves_everything() {
    let img = Image::new("APFS", 64);
    let e = env_with(OpsConfig {
        verify: true,
        ..Default::default()
    });
    make_src(&e);
    let before = tree(&e.p("src"));
    let st = e.run(OpRequest::Move {
        sources: vec![e.p("src")],
        dest_dir: img.mount.clone(),
    });
    assert_eq!(st.state, OpState::Completed, "{st:?}");
    assert_eq!(tree(&img.mount.join("src")), before);
    assert!(!exists(&e.p("src")), "the emptied source folder is removed");
    assert!(partials(&img.mount).is_empty());
}

#[test]
fn failure_at_each_step_keeps_the_source() {
    let img = Image::new("APFS", 64);
    for step in STEPS {
        for verify in [false, true] {
            if step == FaultStep::Verify && !verify {
                continue;
            }
            let e = env_with(OpsConfig {
                verify,
                faults: Some(Arc::new(FailAt {
                    step,
                    needle: "victim".into(),
                })),
                ..Default::default()
            });
            make_src(&e);
            let victim = hash_file(&e.p("src/victim.bin")).unwrap();
            let dest_root = img.mount.join(format!("{step:?}-{verify}"));
            std::fs::create_dir_all(&dest_root).unwrap();
            let st = e.run(OpRequest::Move {
                sources: vec![e.p("src")],
                dest_dir: dest_root.clone(),
            });
            let what = format!("{step:?} verify={verify}");
            assert_eq!(st.state, OpState::CompletedWithErrors, "{what}: {st:?}");
            assert_eq!(st.errors.len(), 1, "{what}: {st:?}");
            assert!(st.errors[0].path.ends_with("victim.bin"), "{what}");
            // The source survives, intact.
            assert_eq!(hash_file(&e.p("src/victim.bin")).unwrap(), victim, "{what}");
            // So does its folder, because it isn't empty.
            assert!(exists(&e.p("src")), "{what}");
            // The others moved.
            assert_eq!(read(&dest_root.join("src/ok1.txt")), b"one", "{what}");
            assert!(!exists(&e.p("src/ok1.txt")), "{what}");
            assert!(
                !exists(&e.p("src/sub")),
                "{what}: fully moved sub-folder is removed"
            );
            // No temporary files anywhere.
            assert!(partials(&dest_root).is_empty(), "{what}");
            // Only a failure after placing leaves the (complete) copy.
            let placed = exists(&dest_root.join("src/victim.bin"));
            assert_eq!(placed, step == FaultStep::BeforeSourceRemove, "{what}");
            if placed {
                assert_eq!(
                    hash_file(&dest_root.join("src/victim.bin")).unwrap(),
                    victim
                );
            }
        }
    }
}

#[test]
fn same_volume_move_failure_keeps_the_source() {
    let e = env_with(OpsConfig {
        faults: Some(Arc::new(FailAt {
            step: FaultStep::BeforePlace,
            needle: "victim".into(),
        })),
        ..Default::default()
    });
    make_src(&e);
    std::fs::create_dir_all(e.p("dst")).unwrap();
    let st = e.run(OpRequest::Move {
        sources: vec![e.p("src/victim.bin"), e.p("src/ok1.txt")],
        dest_dir: e.p("dst"),
    });
    assert_eq!(st.state, OpState::CompletedWithErrors);
    assert!(exists(&e.p("src/victim.bin")));
    assert!(exists(&e.p("dst/ok1.txt")));
}

#[test]
fn verify_failure_on_copy_keeps_nothing_half_done() {
    let img = Image::new("APFS", 64);
    let e = env_with(OpsConfig {
        verify: true,
        faults: Some(Arc::new(FailAt {
            step: FaultStep::Verify,
            needle: "victim".into(),
        })),
        ..Default::default()
    });
    make_src(&e);
    let st = e.run(OpRequest::Copy {
        sources: vec![e.p("src")],
        dest_dir: img.mount.clone(),
    });
    assert_eq!(st.state, OpState::CompletedWithErrors);
    assert!(st.errors[0].message.contains("verification failed"));
    assert!(!exists(&img.mount.join("src/victim.bin")));
    assert!(partials(&img.mount).is_empty());
}

#[test]
fn folder_into_itself_is_refused() {
    let e = env();
    make_src(&e);
    for req in [
        OpRequest::Copy {
            sources: vec![e.p("src")],
            dest_dir: e.p("src/sub"),
        },
        OpRequest::Move {
            sources: vec![e.p("src")],
            dest_dir: e.p("src/sub"),
        },
    ] {
        let plan = e.q.plan(&req);
        assert!(!plan.errors.is_empty(), "{plan:?}");
        let st = e.run(req);
        assert_eq!(st.state, OpState::CompletedWithErrors);
        assert!(st.errors[0].message.contains("into itself"));
    }
    assert_eq!(read(&e.p("src/sub/ok2.txt")), b"two");
    assert_eq!(std::fs::read_dir(e.p("src/sub")).unwrap().count(), 1);
}

#[test]
fn symlinks_are_copied_as_links_and_loops_dont_hang() {
    let e = env();
    write(&e.p("src/real/a.txt"), b"a");
    std::os::unix::fs::symlink("../real", e.p("src/real/up")).unwrap(); // a loop if followed
    std::os::unix::fs::symlink("/nonexistent/target", e.p("src/dangling")).unwrap();
    std::os::unix::fs::symlink(e.p("src"), e.p("src/self")).unwrap();
    std::fs::create_dir_all(e.p("dst")).unwrap();
    let st = e.run(OpRequest::Copy {
        sources: vec![e.p("src")],
        dest_dir: e.p("dst"),
    });
    assert_eq!(st.state, OpState::Completed, "{st:?}");
    assert_eq!(tree(&e.p("src")), tree(&e.p("dst/src")));
    assert!(
        std::fs::symlink_metadata(e.p("dst/src/self"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    // Same through the cross-volume path.
    let img = Image::new("APFS", 64);
    let st = e.run(OpRequest::Copy {
        sources: vec![e.p("src")],
        dest_dir: img.mount.clone(),
    });
    assert_eq!(st.state, OpState::Completed, "{st:?}");
    assert_eq!(tree(&e.p("src")), tree(&img.mount.join("src")));
}

#[test]
fn exfat_uses_check_then_rename() {
    let img = Image::new("ExFAT", 64);
    let e = env();
    make_src(&e);
    write(&img.mount.join("ok1.txt"), b"old");
    let st = e.run_with(
        OpRequest::Copy {
            sources: vec![e.p("src/ok1.txt"), e.p("src/victim.bin")],
            dest_dir: img.mount.clone(),
        },
        EnqueueOptions {
            rules: ConflictRules::all(ConflictDecision::KeepBoth),
            ..Default::default()
        },
    );
    assert_eq!(st.state, OpState::Completed, "{st:?}");
    assert_eq!(read(&img.mount.join("ok1.txt")), b"old");
    assert_eq!(read(&img.mount.join("ok1 2.txt")), b"one");
    assert!(partials(&img.mount).is_empty());
}

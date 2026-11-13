//! Per-file errors: permission-denied and read-only destinations, a full
//! disk mid-copy (ignored by default), and Delete Immediately's gate.

mod common;
use common::*;
use fstudio_ops::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn permission_denied_destination_is_a_per_file_error() {
    let e = env();
    write(&e.p("src/a.txt"), b"a");
    write(&e.p("src/b.txt"), b"b");
    std::fs::create_dir_all(e.p("locked")).unwrap();
    std::fs::create_dir_all(e.p("open")).unwrap();
    std::fs::set_permissions(e.p("locked"), std::fs::Permissions::from_mode(0o555)).unwrap();
    let st = e.run(OpRequest::Copy {
        sources: vec![e.p("src/a.txt"), e.p("src/b.txt")],
        dest_dir: e.p("locked"),
    });
    std::fs::set_permissions(e.p("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(st.state, OpState::CompletedWithErrors);
    assert_eq!(st.errors.len(), 2, "one error per file: {st:?}");
    assert!(
        st.errors.iter().all(|e| e.message == "permission denied"),
        "{st:?}"
    );
    assert_eq!(std::fs::read_dir(e.p("locked")).unwrap().count(), 0);
    // Source permission denied: one bad file doesn't stop the batch.
    write(&e.p("src/secret.txt"), b"s");
    std::fs::set_permissions(
        e.p("src/secret.txt"),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    let st = e.run(OpRequest::Copy {
        sources: vec![e.p("src")],
        dest_dir: e.p("open"),
    });
    std::fs::set_permissions(
        e.p("src/secret.txt"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert_eq!(st.state, OpState::CompletedWithErrors, "{st:?}");
    assert_eq!(st.errors.len(), 1);
    assert!(st.errors[0].path.ends_with("secret.txt"));
    assert_eq!(read(&e.p("open/src/a.txt")), b"a");
    assert!(partials(e.root()).is_empty());
}

#[test]
fn read_only_volume() {
    let img = Image::with("HFS+", 16, true);
    let e = env();
    write(&e.p("src/a.txt"), b"a");
    let st = e.run(OpRequest::Copy {
        sources: vec![e.p("src/a.txt")],
        dest_dir: img.mount.clone(),
    });
    assert_eq!(st.state, OpState::CompletedWithErrors);
    assert_eq!(st.errors[0].message, "the volume is read-only", "{st:?}");
    // A move off a read-only volume would have to remove the source: kept.
    let st = e.run(OpRequest::Trash {
        paths: vec![img.mount.join("nope")],
    });
    assert_eq!(st.state, OpState::CompletedWithErrors);
}

#[test]
#[ignore = "creates and fills a disk image; run with --ignored"]
fn full_disk_mid_copy() {
    let img = Image::new("HFS+", 8);
    for verify in [false, true] {
        let e = env_with(OpsConfig {
            verify,
            ..Default::default()
        });
        write(&e.p("src/small.txt"), b"fits");
        write(&e.p("src/huge.bin"), &vec![5u8; 32 << 20]);
        let huge = hash_file(&e.p("src/huge.bin")).unwrap();
        let dest = img.mount.join(format!("v{verify}"));
        std::fs::create_dir_all(&dest).unwrap();
        // The small file first (in parallel it could lose the race for space).
        let st = e.run(OpRequest::Copy {
            sources: vec![e.p("src/small.txt")],
            dest_dir: dest.clone(),
        });
        assert_eq!(st.state, OpState::Completed);
        let st = e.run(OpRequest::Copy {
            sources: vec![e.p("src/huge.bin")],
            dest_dir: dest.clone(),
        });
        assert_eq!(
            st.state,
            OpState::CompletedWithErrors,
            "verify={verify}: {st:?}"
        );
        assert_eq!(st.errors.len(), 1, "{st:?}");
        assert_eq!(st.errors[0].message, "there isn’t enough free space");
        assert_eq!(read(&dest.join("small.txt")), b"fits");
        assert!(!exists(&dest.join("huge.bin")));
        assert!(
            partials(&img.mount).is_empty(),
            "the partial file is cleaned up"
        );
        assert_eq!(
            hash_file(&e.p("src/huge.bin")).unwrap(),
            huge,
            "the source is intact"
        );
    }
}

#[test]
fn delete_immediately_is_refused_when_disabled() {
    let e = env();
    write(&e.p("keep.txt"), b"k");
    let req = OpRequest::DeleteImmediately {
        paths: vec![e.p("keep.txt")],
    };
    assert!(matches!(
        e.q.enqueue(req.clone()),
        Err(OpsError::DeleteImmediatelyDisabled)
    ));
    assert!(e.q.plan(&req).refused.is_some());
    assert!(exists(&e.p("keep.txt")));
}

#[test]
fn delete_immediately_works_when_enabled_and_cant_be_undone() {
    let e = env_with(OpsConfig {
        allow_delete_immediately: true,
        ..Default::default()
    });
    write(&e.p("gone/a.txt"), b"a");
    write(&e.p("target.txt"), b"t");
    std::os::unix::fs::symlink(e.p("target.txt"), e.p("link")).unwrap();
    let st = e.run(OpRequest::DeleteImmediately {
        paths: vec![e.p("gone"), e.p("link")],
    });
    assert_eq!(st.state, OpState::Completed);
    assert!(!exists(&e.p("gone")) && !exists(&e.p("link")));
    assert!(
        exists(&e.p("target.txt")),
        "a symlink's target is never touched"
    );
    assert_eq!(e.q.undo_state().undo, None);
}

#[test]
fn missing_source_is_a_per_file_error() {
    let e = env();
    write(&e.p("a.txt"), b"a");
    std::fs::create_dir_all(e.p("dst")).unwrap();
    let st = e.run(OpRequest::Copy {
        sources: vec![e.p("nope.txt"), e.p("a.txt")],
        dest_dir: e.p("dst"),
    });
    assert_eq!(st.state, OpState::CompletedWithErrors);
    assert_eq!(st.errors[0].message, "the item no longer exists");
    assert!(exists(&e.p("dst/a.txt")));
}

#[test]
fn invalid_requests_are_rejected_up_front() {
    let e = env();
    for req in [
        OpRequest::Copy {
            sources: vec!["relative".into()],
            dest_dir: e.p("x"),
        },
        OpRequest::Rename {
            path: e.p("a"),
            new_name: "a/b".into(),
        },
        OpRequest::NewFolder {
            parent: e.p(""),
            name: "..".into(),
        },
        OpRequest::Trash { paths: vec![] },
    ] {
        assert!(matches!(e.q.enqueue(req), Err(OpsError::InvalidRequest(_))));
    }
}

//! Every conflict decision × file / folder / symlink, Keep Both naming,
//! apply-to-all, and case / Unicode collisions.

mod common;
use common::*;
use fstudio_ops::*;

const KINDS: [K; 3] = [K::File, K::Dir, K::Link];

fn setup(e: &Env, k: K) {
    make(&e.p("src/item"), k, "new");
    make(&e.p("dst/item"), k, "old");
}

fn snapshot(e: &Env, rel: &str) -> std::collections::BTreeMap<String, String> {
    let p = e.p(rel);
    let mut t = std::collections::BTreeMap::new();
    if std::fs::symlink_metadata(&p).unwrap().is_dir() {
        t = tree(&p);
    } else if std::fs::symlink_metadata(&p)
        .unwrap()
        .file_type()
        .is_symlink()
    {
        t.insert(
            String::new(),
            format!("l:{}", std::fs::read_link(&p).unwrap().display()),
        );
    } else {
        t.insert(String::new(), hash_file(&p).unwrap());
    }
    t
}

fn conflict_run(k: K, d: ConflictDecision, op_move: bool) -> (Env, OpStatus) {
    let e = env();
    setup(&e, k);
    let req = if op_move {
        OpRequest::Move {
            sources: vec![e.p("src/item")],
            dest_dir: e.p("dst"),
        }
    } else {
        OpRequest::Copy {
            sources: vec![e.p("src/item")],
            dest_dir: e.p("dst"),
        }
    };
    let id = e.q.enqueue(req).unwrap();
    let c = wait_conflict(&e, id);
    assert_eq!(c.dest, e.p("dst/item"));
    assert_eq!(c.can_merge, k == K::Dir);
    e.q.resolve_conflict(id, c.id, d, false).unwrap();
    let st = e.q.wait(id, WAIT).unwrap();
    (e, st)
}

#[test]
fn skip_leaves_both_untouched() {
    for k in KINDS {
        for mv in [false, true] {
            let e0 = env();
            setup(&e0, k);
            let before_src = snapshot(&e0, "src/item");
            let before_dst = snapshot(&e0, "dst/item");
            drop(e0);
            let (e, st) = conflict_run(k, ConflictDecision::Skip, mv);
            assert_eq!(st.state, OpState::Completed, "{k:?} {st:?}");
            assert_eq!(snapshot(&e, "src/item"), before_src, "{k:?}");
            assert_eq!(snapshot(&e, "dst/item"), before_dst, "{k:?}");
            assert!(partials(e.root()).is_empty());
        }
    }
}

#[test]
fn keep_both_names_the_new_item_2() {
    for k in KINDS {
        for mv in [false, true] {
            let (e, st) = conflict_run(k, ConflictDecision::KeepBoth, mv);
            assert_eq!(st.state, OpState::Completed, "{k:?} {st:?}");
            assert!(
                exists(&e.p("dst/item")) && exists(&e.p("dst/item 2")),
                "{k:?}"
            );
            assert_eq!(exists(&e.p("src/item")), !mv);
            if k == K::Dir {
                assert_eq!(read(&e.p("dst/item 2/shared.txt")), b"new");
                assert_eq!(read(&e.p("dst/item/shared.txt")), b"old");
            }
        }
    }
}

#[test]
fn replace_trashes_the_old_item_first() {
    for k in KINDS {
        for mv in [false, true] {
            let (e, st) = conflict_run(k, ConflictDecision::Replace, mv);
            assert_eq!(st.state, OpState::Completed, "{k:?} {st:?}");
            // The old item is in the trash, never unlinked.
            let trashed = e.trash.join("item");
            assert!(exists(&trashed), "{k:?}: old item not in trash");
            match k {
                K::File => {
                    assert_eq!(read(&e.p("dst/item")), b"new");
                    assert_eq!(read(&trashed), b"old");
                }
                K::Dir => {
                    assert_eq!(read(&e.p("dst/item/shared.txt")), b"new");
                    assert!(!exists(&e.p("dst/item/old.txt")));
                    assert_eq!(read(&trashed.join("old.txt")), b"old");
                }
                K::Link => {
                    assert_eq!(
                        std::fs::read_link(e.p("dst/item")).unwrap().to_str(),
                        Some("target-new")
                    );
                }
            }
            // Undo puts the replaced item back.
            e.undo().unwrap();
            assert_eq!(exists(&e.p("src/item")), true, "{k:?}");
            match k {
                K::File => assert_eq!(read(&e.p("dst/item")), b"old"),
                K::Dir => assert_eq!(read(&e.p("dst/item/shared.txt")), b"old"),
                K::Link => assert_eq!(
                    std::fs::read_link(e.p("dst/item")).unwrap().to_str(),
                    Some("target-old")
                ),
            }
        }
    }
}

#[test]
fn merge_folders_asks_again_inside() {
    for mv in [false, true] {
        let (e, st) = {
            let e = env();
            setup(&e, K::Dir);
            let req = if mv {
                OpRequest::Move {
                    sources: vec![e.p("src/item")],
                    dest_dir: e.p("dst"),
                }
            } else {
                OpRequest::Copy {
                    sources: vec![e.p("src/item")],
                    dest_dir: e.p("dst"),
                }
            };
            let id = e.q.enqueue(req).unwrap();
            let c = wait_conflict(&e, id);
            e.q.resolve_conflict(id, c.id, ConflictDecision::Merge, false)
                .unwrap();
            // shared.txt collides inside the merged folder.
            let c2 = wait_conflict(&e, id);
            assert_eq!(c2.dest, e.p("dst/item/shared.txt"));
            assert!(!c2.can_merge);
            assert!(matches!(
                e.q.resolve_conflict(id, c2.id, ConflictDecision::Merge, false),
                Err(OpsError::InvalidDecision(_))
            ));
            e.q.resolve_conflict(id, c2.id, ConflictDecision::KeepBoth, false)
                .unwrap();
            let st = e.q.wait(id, WAIT).unwrap();
            (e, st)
        };
        assert_eq!(st.state, OpState::Completed, "{st:?}");
        assert_eq!(read(&e.p("dst/item/old.txt")), b"old");
        assert_eq!(read(&e.p("dst/item/new.txt")), b"new");
        assert_eq!(read(&e.p("dst/item/shared.txt")), b"old");
        assert_eq!(read(&e.p("dst/item/shared 2.txt")), b"new");
        if mv {
            assert!(
                !exists(&e.p("src/item")),
                "merged-away source folder should be gone"
            );
        }
        // Undo removes only what the op added.
        e.undo().unwrap();
        assert_eq!(read(&e.p("dst/item/old.txt")), b"old");
        assert!(!exists(&e.p("dst/item/new.txt")));
        assert!(!exists(&e.p("dst/item/shared 2.txt")));
        if mv {
            assert_eq!(read(&e.p("src/item/new.txt")), b"new");
        }
    }
}

#[test]
fn merge_is_refused_for_non_folders() {
    for k in [K::File, K::Link] {
        let e = env();
        setup(&e, k);
        let id =
            e.q.enqueue(OpRequest::Copy {
                sources: vec![e.p("src/item")],
                dest_dir: e.p("dst"),
            })
            .unwrap();
        let c = wait_conflict(&e, id);
        assert!(matches!(
            e.q.resolve_conflict(id, c.id, ConflictDecision::Merge, false),
            Err(OpsError::InvalidDecision(ConflictDecision::Merge))
        ));
        e.q.cancel(id).unwrap();
        e.q.wait(id, WAIT);
    }
}

#[test]
fn file_onto_folder_and_folder_onto_file() {
    let e = env();
    make(&e.p("src/a"), K::File, "new");
    make(&e.p("dst/a"), K::Dir, "old");
    make(&e.p("src/b"), K::Dir, "new");
    make(&e.p("dst/b"), K::File, "old");
    let id =
        e.q.enqueue(OpRequest::Copy {
            sources: vec![e.p("src/a"), e.p("src/b")],
            dest_dir: e.p("dst"),
        })
        .unwrap();
    let mut seen = 0;
    while seen < 2 {
        let c = wait_conflict(&e, id);
        assert!(!c.can_merge);
        e.q.resolve_conflict(id, c.id, ConflictDecision::Replace, false)
            .unwrap();
        seen += 1;
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let st = e.q.wait(id, WAIT).unwrap();
    assert_eq!(st.state, OpState::Completed, "{st:?}");
    assert_eq!(read(&e.p("dst/a")), b"new");
    assert_eq!(read(&e.p("dst/b/shared.txt")), b"new");
    assert!(exists(&e.trash.join("a")) && exists(&e.trash.join("b")));
}

#[test]
fn a_conflict_only_pauses_its_item() {
    let e = env();
    make(&e.p("src/clash"), K::File, "new");
    make(&e.p("dst/clash"), K::File, "old");
    for i in 0..20 {
        write(&e.p(&format!("src/f{i}")), b"x");
    }
    let mut sources = vec![e.p("src/clash")];
    sources.extend((0..20).map(|i| e.p(&format!("src/f{i}"))));
    let id =
        e.q.enqueue(OpRequest::Copy {
            sources,
            dest_dir: e.p("dst"),
        })
        .unwrap();
    let c = wait_conflict(&e, id);
    // Everything else finishes while the conflict waits.
    let t = std::time::Instant::now();
    while e.q.status(id).unwrap().state != OpState::WaitingForDecision {
        assert!(t.elapsed() < WAIT);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    for i in 0..20 {
        assert!(exists(&e.p(&format!("dst/f{i}"))));
    }
    e.q.resolve_conflict(id, c.id, ConflictDecision::Skip, false)
        .unwrap();
    assert_eq!(e.q.wait(id, WAIT).unwrap().state, OpState::Completed);
}

#[test]
fn apply_to_all_answers_later_conflicts() {
    let e = env();
    for i in 0..5 {
        make(&e.p(&format!("src/f{i}")), K::File, "new");
        make(&e.p(&format!("dst/f{i}")), K::File, "old");
    }
    let sources = (0..5).map(|i| e.p(&format!("src/f{i}"))).collect();
    let id =
        e.q.enqueue(OpRequest::Copy {
            sources,
            dest_dir: e.p("dst"),
        })
        .unwrap();
    let c = wait_conflict(&e, id);
    e.q.resolve_conflict(id, c.id, ConflictDecision::KeepBoth, true)
        .unwrap();
    let st = e.q.wait(id, WAIT).unwrap();
    assert_eq!(st.state, OpState::Completed, "{st:?}");
    for i in 0..5 {
        assert_eq!(read(&e.p(&format!("dst/f{i}"))), b"old");
        assert_eq!(read(&e.p(&format!("dst/f{i} 2"))), b"new");
    }
}

#[test]
fn rules_up_front_never_ask() {
    let e = env();
    setup(&e, K::Dir);
    let id =
        e.q.enqueue_with(
            OpRequest::Copy {
                sources: vec![e.p("src/item")],
                dest_dir: e.p("dst"),
            },
            EnqueueOptions {
                rules: ConflictRules::all(ConflictDecision::Merge),
                ..Default::default()
            },
        )
        .unwrap();
    // The folder merges without asking; the file inside still asks, because
    // Merge only applies to folders.
    let c = wait_conflict(&e, id);
    assert_eq!(c.dest, e.p("dst/item/shared.txt"));
    e.q.resolve_conflict(id, c.id, ConflictDecision::Skip, false)
        .unwrap();
    assert_eq!(e.q.wait(id, WAIT).unwrap().state, OpState::Completed);
    assert_eq!(read(&e.p("dst/item/new.txt")), b"new");
}

#[test]
fn keep_both_sequence_on_disk() {
    let e = env();
    write(&e.p("src/report.pdf"), b"new");
    write(&e.p("dst/report.pdf"), b"0");
    write(&e.p("dst/report 2.pdf"), b"2");
    write(&e.p("dst/report 3.pdf"), b"3");
    let opts = EnqueueOptions {
        rules: ConflictRules::all(ConflictDecision::KeepBoth),
        ..Default::default()
    };
    for expect in ["report 4.pdf", "report 5.pdf"] {
        let st = e.run_with(
            OpRequest::Copy {
                sources: vec![e.p("src/report.pdf")],
                dest_dir: e.p("dst"),
            },
            opts.clone(),
        );
        assert_eq!(st.state, OpState::Completed);
        assert_eq!(read(&e.p(&format!("dst/{expect}"))), b"new");
    }
    // Copying "report 2.pdf" itself continues the sequence.
    let st = e.run_with(
        OpRequest::Copy {
            sources: vec![e.p("dst/report 2.pdf")],
            dest_dir: e.p("dst"),
        },
        opts.clone(),
    );
    assert_eq!(st.state, OpState::Completed);
    assert_eq!(read(&e.p("dst/report 6.pdf")), b"2");
    // A folder.
    std::fs::create_dir_all(e.p("src/Photos")).unwrap();
    std::fs::create_dir_all(e.p("dst/Photos")).unwrap();
    let st = e.run_with(
        OpRequest::Copy {
            sources: vec![e.p("src/Photos")],
            dest_dir: e.p("dst"),
        },
        EnqueueOptions {
            rules: ConflictRules {
                folders: Some(ConflictDecision::KeepBoth),
                others: None,
            },
            ..Default::default()
        },
    );
    assert_eq!(st.state, OpState::Completed);
    assert!(exists(&e.p("dst/Photos 2")));
}

#[test]
fn copy_onto_itself_keeps_both_and_never_replaces() {
    let e = env();
    write(&e.p("d/a.txt"), b"me");
    let id =
        e.q.enqueue(OpRequest::Copy {
            sources: vec![e.p("d/a.txt")],
            dest_dir: e.p("d"),
        })
        .unwrap();
    let c = wait_conflict(&e, id);
    e.q.resolve_conflict(id, c.id, ConflictDecision::Replace, false)
        .unwrap();
    let st = e.q.wait(id, WAIT).unwrap();
    assert_eq!(st.state, OpState::CompletedWithErrors);
    assert_eq!(read(&e.p("d/a.txt")), b"me");
    assert!(!exists(&e.trash.join("a.txt")));
    let st = e.run_with(
        OpRequest::Copy {
            sources: vec![e.p("d/a.txt")],
            dest_dir: e.p("d"),
        },
        EnqueueOptions {
            rules: ConflictRules::all(ConflictDecision::KeepBoth),
            ..Default::default()
        },
    );
    assert_eq!(st.state, OpState::Completed);
    assert_eq!(read(&e.p("d/a 2.txt")), b"me");
}

#[test]
fn replace_never_trashes_a_folder_holding_the_source() {
    let e = env();
    write(&e.p("x/Foo/Foo/inner.txt"), b"data");
    let id =
        e.q.enqueue(OpRequest::Move {
            sources: vec![e.p("x/Foo/Foo")],
            dest_dir: e.p("x"),
        })
        .unwrap();
    let c = wait_conflict(&e, id);
    e.q.resolve_conflict(id, c.id, ConflictDecision::Replace, false)
        .unwrap();
    let st = e.q.wait(id, WAIT).unwrap();
    assert_eq!(st.state, OpState::CompletedWithErrors);
    assert_eq!(read(&e.p("x/Foo/Foo/inner.txt")), b"data");
}

#[test]
fn case_only_collision_onto_case_insensitive_volume() {
    // Source: a case-sensitive volume holding "File" and "file".
    let cs = Image::new("Case-sensitive APFS", 64);
    write(&cs.mount.join("src/File"), b"upper");
    write(&cs.mount.join("src/file"), b"lower");
    assert!(is_case_sensitive(&cs.mount).unwrap());
    let e = env();
    std::fs::create_dir_all(e.p("dst")).unwrap();
    assert!(
        !is_case_sensitive(&e.p("dst")).unwrap(),
        "temp dir should be case-insensitive APFS"
    );
    let req = OpRequest::Copy {
        sources: vec![cs.mount.join("src/File"), cs.mount.join("src/file")],
        dest_dir: e.p("dst"),
    };
    let plan = e.q.plan(&req);
    assert_eq!(plan.dest_case_sensitive, Some(false));
    assert!(
        plan.case_collisions.iter().any(|c| c.names.len() == 2),
        "{plan:?}"
    );
    // The folder copy also predicts it inside the tree.
    let plan2 = e.q.plan(&OpRequest::Copy {
        sources: vec![cs.mount.join("src")],
        dest_dir: e.p("dst"),
    });
    assert_eq!(plan2.case_collisions.len(), 1, "{plan2:?}");
    // At run time the second one is a conflict, never an overwrite.
    let id = e.q.enqueue(req).unwrap();
    let c = wait_conflict(&e, id);
    e.q.resolve_conflict(id, c.id, ConflictDecision::KeepBoth, false)
        .unwrap();
    let st = e.q.wait(id, WAIT).unwrap();
    assert_eq!(st.state, OpState::Completed, "{st:?}");
    let mut got: Vec<Vec<u8>> = std::fs::read_dir(e.p("dst"))
        .unwrap()
        .map(|d| read(&d.unwrap().path()))
        .collect();
    got.sort();
    assert_eq!(got, vec![b"lower".to_vec(), b"upper".to_vec()]);
}

#[test]
fn nfc_nfd_names_collide() {
    let e = env();
    let nfc = "caf\u{e9}.txt";
    let nfd = "cafe\u{301}.txt";
    write(&e.p(&format!("src/{nfd}")), b"nfd");
    write(&e.p(&format!("dst/{nfc}")), b"nfc");
    let req = OpRequest::Copy {
        sources: vec![e.p(&format!("src/{nfd}"))],
        dest_dir: e.p("dst"),
    };
    let plan = e.q.plan(&req);
    assert_eq!(plan.conflicts.len(), 1, "{plan:?}");
    let id = e.q.enqueue(req).unwrap();
    let c = wait_conflict(&e, id);
    e.q.resolve_conflict(id, c.id, ConflictDecision::KeepBoth, false)
        .unwrap();
    assert_eq!(e.q.wait(id, WAIT).unwrap().state, OpState::Completed);
    assert_eq!(read(&e.p(&format!("dst/{nfc}"))), b"nfc");
    // The new name keeps the source's (NFD) spelling, numbered.
    let names: Vec<String> = std::fs::read_dir(e.p("dst"))
        .unwrap()
        .map(|d| d.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        names.contains(&"cafe\u{301} 2.txt".to_string()),
        "{names:?}"
    );
}

#[test]
fn case_only_rename() {
    let e = env();
    write(&e.p("d/File.txt"), b"x");
    let st = e.run(OpRequest::Rename {
        path: e.p("d/File.txt"),
        new_name: "file.txt".into(),
    });
    assert_eq!(st.state, OpState::Completed, "{st:?}");
    let names: Vec<String> = std::fs::read_dir(e.p("d"))
        .unwrap()
        .map(|d| d.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, vec!["file.txt".to_string()]);
    e.undo().unwrap();
    let names: Vec<String> = std::fs::read_dir(e.p("d"))
        .unwrap()
        .map(|d| d.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, vec!["File.txt".to_string()]);
}

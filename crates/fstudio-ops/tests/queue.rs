//! Queue controls: pause/resume/cancel, reorder, per-volume workers,
//! coalesced events, plans.

mod common;
use common::*;
use fstudio_ops::*;
use std::time::{Duration, Instant};

const BIG: usize = 384 << 20;

fn big_file(e: &Env, name: &str) -> std::path::PathBuf {
    let p = e.p(name);
    // Incompressible-ish data so nothing shortcuts the write.
    let mut v = vec![0u8; BIG];
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    for c in v.chunks_mut(8) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        c.copy_from_slice(&x.to_le_bytes()[..c.len()]);
    }
    write(&p, &v);
    p
}

/// Waits until op `id` has copied `min` bytes. `at` names the step for the
/// failure message, which also shows the op's whole status.
fn wait_bytes_at(e: &Env, id: OpId, min: u64, at: &str) {
    let t = Instant::now();
    println!("{at}: waiting for {min} bytes");
    loop {
        let st = e.q.status(id).unwrap();
        if st.bytes_done >= min {
            println!("{at}: {} bytes after {:?}", st.bytes_done, t.elapsed());
            return;
        }
        // An op that has ended below `min` can never get there; say so now
        // rather than after the whole WAIT.
        assert!(
            !st.state.is_finished(),
            "{at}: the op ended at {} of {min} bytes after {:?}: {st:#?}",
            st.bytes_done,
            t.elapsed()
        );
        assert!(t.elapsed() < WAIT, "{at}: no progress in {WAIT:?}: {st:#?}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn wait_bytes(e: &Env, id: OpId, min: u64) {
    wait_bytes_at(e, id, min, "wait_bytes");
}

#[test]
fn pause_resume_and_cancel() {
    // Printed only when the test fails: a timeline of the steps.
    let start = Instant::now();
    let step = |what: &str| println!("[{:>8.2?}] {what}", start.elapsed());
    let img = Image::new("APFS", 1024);
    let e = env();
    let src = big_file(&e, "big.bin");
    for verify in [false, true] {
        let e2 = env_with(OpsConfig {
            verify,
            ..Default::default()
        });
        let dest = img.mount.join(format!("p{verify}"));
        std::fs::create_dir_all(&dest).unwrap();
        let id =
            e2.q.enqueue(OpRequest::Copy {
                sources: vec![src.clone()],
                dest_dir: dest.clone(),
            })
            .unwrap();
        let at = format!("pause loop, verify={verify}");
        step(&at);
        wait_bytes_at(&e2, id, 8 << 20, &at);
        e2.q.pause(id).unwrap();
        let paused_at = Instant::now();
        std::thread::sleep(Duration::from_millis(100));
        let a = e2.q.status(id).unwrap();
        let a_at = paused_at.elapsed();
        std::thread::sleep(Duration::from_millis(300));
        let b = e2.q.status(id).unwrap();
        let b_at = paused_at.elapsed();
        assert_eq!(b.state, OpState::Paused, "{at}: {b:#?}");
        assert_eq!(
            a.bytes_done, b.bytes_done,
            "{at}: no bytes move while paused ({} bytes at {a_at:?} after pause(), {} at {b_at:?}): {b:#?}",
            a.bytes_done, b.bytes_done
        );
        assert!(b.bytes_done < BIG as u64, "{at}: {b:#?}");
        e2.q.resume(id).unwrap();
        step(&format!("{at}: resumed"));
        let st = e2.q.wait(id, WAIT).unwrap();
        step(&format!("{at}: {:?}", st.state));
        assert_eq!(st.state, OpState::Completed, "{at}: {st:#?}");
        assert_eq!(
            hash_file(&dest.join("big.bin")).unwrap(),
            hash_file(&src).unwrap()
        );
    }
    // Cancel mid-file: the partial disappears, the source stays.
    for verify in [false, true] {
        let e2 = env_with(OpsConfig {
            verify,
            ..Default::default()
        });
        let dest = img.mount.join(format!("c{verify}"));
        std::fs::create_dir_all(&dest).unwrap();
        let id =
            e2.q.enqueue(OpRequest::Copy {
                sources: vec![src.clone()],
                dest_dir: dest.clone(),
            })
            .unwrap();
        let at = format!("cancel loop, verify={verify}");
        step(&at);
        wait_bytes_at(&e2, id, 8 << 20, &at);
        let t = Instant::now();
        e2.q.cancel(id).unwrap();
        let st = e2.q.wait(id, WAIT).unwrap();
        assert!(
            t.elapsed() < Duration::from_secs(2),
            "{at}: cancel took {:?}: {st:#?}",
            t.elapsed()
        );
        assert_eq!(st.state, OpState::Cancelled, "{at}: {st:#?}");
        assert!(!exists(&dest.join("big.bin")));
        assert!(partials(&dest).is_empty(), "verify={verify}");
        assert!(exists(&src));
    }
    // Cancel while paused works too.
    step("cancel while paused");
    let dest = img.mount.join("cp");
    std::fs::create_dir_all(&dest).unwrap();
    let id =
        e.q.enqueue(OpRequest::Copy {
            sources: vec![src.clone()],
            dest_dir: dest.clone(),
        })
        .unwrap();
    wait_bytes_at(&e, id, 8 << 20, "cancel while paused");
    e.q.pause(id).unwrap();
    e.q.cancel(id).unwrap();
    let st = e.q.wait(id, WAIT).unwrap();
    assert_eq!(st.state, OpState::Cancelled, "cancel while paused: {st:#?}");
    assert!(partials(&dest).is_empty());
}

#[test]
fn ops_on_other_volumes_dont_wait() {
    let img = Image::new("APFS", 1024);
    let e = env();
    let src = big_file(&e, "big.bin");
    let slow =
        e.q.enqueue(OpRequest::Copy {
            sources: vec![src],
            dest_dir: img.mount.clone(),
        })
        .unwrap();
    wait_bytes(&e, slow, 1 << 20);
    // A same-volume op runs at once on its own worker.
    write(&e.p("s/a.txt"), b"a");
    std::fs::create_dir_all(e.p("d")).unwrap();
    let fast =
        e.q.enqueue(OpRequest::Copy {
            sources: vec![e.p("s/a.txt")],
            dest_dir: e.p("d"),
        })
        .unwrap();
    assert_eq!(e.q.wait(fast, WAIT).unwrap().state, OpState::Completed);
    assert!(
        !e.q.status(slow).unwrap().state.is_finished(),
        "the slow copy should still be running"
    );
    // A second op on the busy volume pair queues behind it.
    write(&e.p("s/b.txt"), b"b");
    let queued =
        e.q.enqueue(OpRequest::Copy {
            sources: vec![e.p("s/b.txt")],
            dest_dir: img.mount.clone(),
        })
        .unwrap();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(e.q.status(queued).unwrap().state, OpState::Queued);
    assert!(e.q.wait_idle(WAIT));
    assert_eq!(e.q.status(queued).unwrap().state, OpState::Completed);
}

#[test]
fn reorder_changes_what_runs_next() {
    let img = Image::new("APFS", 1024);
    let e = env();
    let src = big_file(&e, "big.bin");
    let first =
        e.q.enqueue(OpRequest::Copy {
            sources: vec![src],
            dest_dir: img.mount.clone(),
        })
        .unwrap();
    wait_bytes(&e, first, 1 << 20);
    write(&e.p("s/b.txt"), b"b");
    write(&e.p("s/c.txt"), b"c");
    let b =
        e.q.enqueue(OpRequest::Copy {
            sources: vec![e.p("s/b.txt")],
            dest_dir: img.mount.clone(),
        })
        .unwrap();
    let c =
        e.q.enqueue(OpRequest::Copy {
            sources: vec![e.p("s/c.txt")],
            dest_dir: img.mount.clone(),
        })
        .unwrap();
    e.q.reorder(c, 1).unwrap();
    let order: Vec<OpId> = e.q.list().iter().map(|s| s.id).collect();
    assert_eq!(order, vec![first, c, b]);
    assert!(e.q.wait_idle(WAIT));
    let finished: Vec<OpId> = e
        .drain()
        .into_iter()
        .filter_map(|ev| match ev {
            OpEvent::OpFinished { op, .. } => Some(op),
            _ => None,
        })
        .collect();
    assert_eq!(finished, vec![first, c, b]);
}

#[test]
fn progress_events_are_coalesced() {
    let img = Image::new("APFS", 1024);
    let e = env();
    let src = big_file(&e, "big.bin");
    let (tx, rx) = crossbeam_channel::unbounded::<(Instant, OpEvent)>();
    let events = e.events.clone();
    let collector = std::thread::spawn(move || {
        for ev in events.iter() {
            let done = matches!(ev, OpEvent::OpFinished { .. });
            tx.send((Instant::now(), ev)).unwrap();
            if done {
                break;
            }
        }
    });
    let t = Instant::now();
    let id =
        e.q.enqueue(OpRequest::Copy {
            sources: vec![src],
            dest_dir: img.mount.clone(),
        })
        .unwrap();
    e.q.wait(id, WAIT);
    let elapsed = t.elapsed();
    collector.join().unwrap();
    let progress: Vec<(Instant, OpStatus)> = rx
        .try_iter()
        .filter_map(|(t, ev)| match ev {
            OpEvent::OpProgress(s) => Some((t, s)),
            _ => None,
        })
        .collect();
    assert!(
        progress.len() >= 3,
        "too few progress events: {}",
        progress.len()
    );
    let max = elapsed.as_millis() as usize / 16 + 4;
    assert!(
        progress.len() <= max,
        "{} events in {elapsed:?}",
        progress.len()
    );
    // All but the final one are at least ~16 ms apart.
    for w in progress[..progress.len() - 1].windows(2) {
        assert!(
            w[1].0 - w[0].0 >= Duration::from_millis(12),
            "events too close"
        );
    }
    let last = &progress.last().unwrap().1;
    assert_eq!(last.bytes_done, last.bytes_total);
    assert!(
        progress
            .iter()
            .any(|(_, s)| s.speed_bps > 0.0 && s.eta_secs.is_some())
    );
}

#[test]
fn plan_counts_without_touching_anything() {
    let e = env();
    write(&e.p("src/a.txt"), b"12345");
    write(&e.p("src/sub/b.txt"), b"123");
    std::os::unix::fs::symlink("a.txt", e.p("src/l")).unwrap();
    write(&e.p("dst/src/x"), b"");
    let before = tree(e.root());
    let plan = e.q.plan(&OpRequest::Copy {
        sources: vec![e.p("src")],
        dest_dir: e.p("dst"),
    });
    assert_eq!(tree(e.root()), before);
    assert_eq!(plan.files, 3);
    assert_eq!(plan.dirs, 2);
    assert_eq!(
        plan.bytes,
        8 + std::fs::symlink_metadata(e.p("src/l")).unwrap().len()
    );
    assert_eq!(plan.conflicts.len(), 1);
    assert!(!plan.cross_volume);
    assert_eq!(plan.dest_case_sensitive, Some(false));
    assert!(plan.dataless.is_empty());
    let img = Image::new("APFS", 64);
    let plan = e.q.plan(&OpRequest::Move {
        sources: vec![e.p("src")],
        dest_dir: img.mount.clone(),
    });
    assert!(plan.cross_volume && plan.roots[0].cross_volume);
}

#[test]
fn new_folder_file_rename_duplicate_trash() {
    let e = env();
    std::fs::create_dir_all(e.p("d")).unwrap();
    assert_eq!(
        e.run(OpRequest::NewFolder {
            parent: e.p("d"),
            name: "untitled folder".into()
        })
        .state,
        OpState::Completed
    );
    assert!(
        std::fs::metadata(e.p("d/untitled folder"))
            .unwrap()
            .is_dir()
    );
    assert_eq!(
        e.run(OpRequest::NewFile {
            parent: e.p("d"),
            name: "note.txt".into()
        })
        .state,
        OpState::Completed
    );
    assert_eq!(read(&e.p("d/note.txt")), b"");
    // A taken name is a conflict, here answered up front.
    let st = e.run_with(
        OpRequest::NewFolder {
            parent: e.p("d"),
            name: "untitled folder".into(),
        },
        EnqueueOptions {
            rules: ConflictRules::all(ConflictDecision::KeepBoth),
            ..Default::default()
        },
    );
    assert_eq!(st.state, OpState::Completed);
    assert!(exists(&e.p("d/untitled folder 2")));
    write(&e.p("d/a.txt"), b"a");
    assert_eq!(
        e.run(OpRequest::Duplicate {
            paths: vec![e.p("d/a.txt")]
        })
        .state,
        OpState::Completed
    );
    assert_eq!(
        e.run(OpRequest::Duplicate {
            paths: vec![e.p("d/a.txt")]
        })
        .state,
        OpState::Completed
    );
    assert_eq!(read(&e.p("d/a copy.txt")), b"a");
    assert_eq!(read(&e.p("d/a copy 2.txt")), b"a");
    assert_eq!(
        e.run(OpRequest::Rename {
            path: e.p("d/a.txt"),
            new_name: "b.txt".into()
        })
        .state,
        OpState::Completed
    );
    assert!(!exists(&e.p("d/a.txt")) && exists(&e.p("d/b.txt")));
    // Rename onto a taken name asks; it never overwrites.
    let id =
        e.q.enqueue(OpRequest::Rename {
            path: e.p("d/b.txt"),
            new_name: "note.txt".into(),
        })
        .unwrap();
    let c = wait_conflict(&e, id);
    e.q.resolve_conflict(id, c.id, ConflictDecision::Skip, false)
        .unwrap();
    e.q.wait(id, WAIT);
    assert_eq!(read(&e.p("d/b.txt")), b"a");
    assert_eq!(read(&e.p("d/note.txt")), b"");
    let st = e.run(OpRequest::Trash {
        paths: vec![e.p("d/b.txt"), e.p("d/untitled folder")],
    });
    assert_eq!(st.state, OpState::Completed);
    assert!(exists(&e.trash.join("b.txt")) && exists(&e.trash.join("untitled folder")));
}

#[test]
fn huge_folder() {
    if std::env::var_os("FSTUDIO_HUGE").is_none() {
        eprintln!("set FSTUDIO_HUGE=1 to copy a 20k-entry folder");
        return;
    }
    let e = env();
    for i in 0..20_000 {
        write(
            &e.p(&format!("src/{}/f{i}.txt", i % 50)),
            format!("{i}").as_bytes(),
        );
    }
    let img = Image::new("APFS", 512);
    let st = e.run(OpRequest::Copy {
        sources: vec![e.p("src")],
        dest_dir: img.mount.clone(),
    });
    assert_eq!(st.state, OpState::Completed);
    assert_eq!(tree(&e.p("src")), tree(&img.mount.join("src")));
}

/// Duplicating a folder names only the folder "… copy"; its contents keep
/// their names.
#[test]
fn duplicate_keeps_the_names_inside_a_folder() {
    let e = env();
    std::fs::create_dir_all(e.p("Site/css")).unwrap();
    std::fs::write(e.p("Site/index.html"), b"x").unwrap();
    std::fs::write(e.p("Site/css/a.css"), b"y").unwrap();
    let st = e.run(OpRequest::Duplicate {
        paths: vec![e.p("Site")],
    });
    assert_eq!(st.state, OpState::Completed);
    assert!(e.p("Site copy/index.html").exists());
    assert!(e.p("Site copy/css/a.css").exists());
}

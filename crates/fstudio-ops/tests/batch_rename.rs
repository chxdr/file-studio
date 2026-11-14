//! Batch rename (`OpRequest::RenameMany`): swaps, cycles, chains, case-only
//! changes, collisions, read-only folders, failures inside a cycle, crash
//! recovery and one-step undo. No temporary name is ever left behind.

mod common;
use common::*;
use fstudio_ops::*;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn no_temps(dir: &Path) {
    let left: Vec<String> = names(dir)
        .into_iter()
        .filter(|n| n.contains("fstudio-renaming") || n.contains("fstudio-partial"))
        .collect();
    assert!(left.is_empty(), "temporary names left behind: {left:?}");
}

fn pairs(e: &Env, dir: &str, list: &[(&str, &str)]) -> OpRequest {
    OpRequest::RenameMany {
        pairs: list
            .iter()
            .map(|(a, b)| (e.p(&format!("{dir}/{a}")), (*b).to_string()))
            .collect(),
    }
}

/// Each file's content is its original name, so a swap is checkable.
fn files(e: &Env, dir: &str, list: &[&str]) {
    std::fs::create_dir_all(e.p(dir)).unwrap();
    for n in list {
        write(&e.p(&format!("{dir}/{n}")), n.as_bytes());
    }
}

fn content(e: &Env, rel: &str) -> String {
    String::from_utf8(read(&e.p(rel))).unwrap()
}

#[test]
fn renames_and_undoes_as_one_step() {
    let e = env();
    files(&e, "d", &["a.txt", "b.txt", "c.txt"]);
    let st = e.run(pairs(
        &e,
        "d",
        &[
            ("a.txt", "one.txt"),
            ("b.txt", "two.txt"),
            ("c.txt", "three.txt"),
        ],
    ));
    assert_eq!(st.state, OpState::Completed, "{st:?}");
    assert_eq!(names(&e.p("d")), ["one.txt", "three.txt", "two.txt"]);
    assert_eq!(
        e.q.undo_state().undo.as_deref(),
        Some("Undo Rename of 3 Items")
    );
    e.undo().unwrap();
    assert_eq!(names(&e.p("d")), ["a.txt", "b.txt", "c.txt"]);
    assert_eq!(content(&e, "d/a.txt"), "a.txt");
    // Redo puts the new names back.
    let id = e.q.redo().unwrap().unwrap();
    assert_eq!(e.q.wait(id, WAIT).unwrap().state, OpState::Completed);
    assert_eq!(names(&e.p("d")), ["one.txt", "three.txt", "two.txt"]);
}

#[test]
fn swaps_and_cycles_use_temporary_names_that_never_stay() {
    let e = env();
    files(&e, "d", &["a", "b", "x", "y", "z"]);
    // a↔b, and x→y→z→x.
    let st = e.run(pairs(
        &e,
        "d",
        &[("a", "b"), ("b", "a"), ("x", "y"), ("y", "z"), ("z", "x")],
    ));
    assert_eq!(st.state, OpState::Completed, "{st:?}");
    no_temps(&e.p("d"));
    assert_eq!(content(&e, "d/a"), "b");
    assert_eq!(content(&e, "d/b"), "a");
    assert_eq!(content(&e, "d/y"), "x");
    assert_eq!(content(&e, "d/z"), "y");
    assert_eq!(content(&e, "d/x"), "z");
    // Undo is a swap too.
    e.undo().unwrap();
    no_temps(&e.p("d"));
    for n in ["a", "b", "x", "y", "z"] {
        assert_eq!(content(&e, &format!("d/{n}")), n);
    }
}

#[test]
fn chains_shift_without_temporary_names() {
    let e = env();
    let list: Vec<String> = (1..=50).map(|i| format!("file{i}")).collect();
    let refs: Vec<&str> = list.iter().map(String::as_str).collect();
    files(&e, "d", &refs);
    // file1→file2, …, file50→file51: every name but the last is taken by the next.
    let req = OpRequest::RenameMany {
        pairs: (1..=50)
            .map(|i| (e.p(&format!("d/file{i}")), format!("file{}", i + 1)))
            .collect(),
    };
    let st = e.run(req);
    assert_eq!(st.state, OpState::Completed, "{st:?}");
    assert!(!e.p("d/file1").exists());
    assert_eq!(content(&e, "d/file51"), "file50");
    assert_eq!(content(&e, "d/file2"), "file1");
    e.undo().unwrap();
    assert_eq!(content(&e, "d/file1"), "file1");
    assert!(!e.p("d/file51").exists());
}

#[test]
fn case_only_renames_work_and_undo() {
    let e = env();
    files(&e, "d", &["File.TXT", "photo.jpg"]);
    let st = e.run(pairs(
        &e,
        "d",
        &[("File.TXT", "file.txt"), ("photo.jpg", "Photo.JPG")],
    ));
    assert_eq!(st.state, OpState::Completed, "{st:?}");
    assert_eq!(names(&e.p("d")), ["Photo.JPG", "file.txt"]);
    no_temps(&e.p("d"));
    e.undo().unwrap();
    assert_eq!(names(&e.p("d")), ["File.TXT", "photo.jpg"]);
}

#[test]
fn case_swap_on_an_insensitive_volume() {
    // "A"→"b", "B"→"a": on a case-insensitive volume each new name is the
    // other item's old one (a cycle across case).
    let e = env();
    if is_case_sensitive(e.root()).unwrap() {
        return;
    }
    files(&e, "d", &["A", "B"]);
    let st = e.run(pairs(&e, "d", &[("A", "b"), ("B", "a")]));
    assert_eq!(st.state, OpState::Completed, "{st:?}");
    assert_eq!(names(&e.p("d")), ["a", "b"]);
    assert_eq!(content(&e, "d/b"), "A");
    assert_eq!(content(&e, "d/a"), "B");
    no_temps(&e.p("d"));
}

#[test]
fn unicode_names_nfd_to_nfc_and_back() {
    let e = env();
    let nfd = "Cafe\u{301}.txt";
    files(&e, "d", &[nfd, "x.txt"]);
    let st = e.run(pairs(
        &e,
        "d",
        &[(nfd, "Résumé.txt"), ("x.txt", "Café.txt")],
    ));
    assert_eq!(st.state, OpState::Completed, "{st:?}");
    assert_eq!(content(&e, "d/Café.txt"), "x.txt");
    assert_eq!(content(&e, "d/Résumé.txt"), nfd);
    e.undo().unwrap();
    assert!(
        names(&e.p("d")).iter().any(|n| n == nfd),
        "{:?}",
        names(&e.p("d"))
    );
}

#[test]
fn taken_names_fail_that_item_only_and_nothing_is_overwritten() {
    let e = env();
    files(&e, "d", &["a", "b", "keep"]);
    let st = e.run(pairs(&e, "d", &[("a", "keep"), ("b", "bee")]));
    assert_eq!(st.state, OpState::CompletedWithErrors, "{st:?}");
    assert_eq!(st.errors.len(), 1);
    assert!(
        st.errors[0].message.contains("already taken"),
        "{:?}",
        st.errors
    );
    assert_eq!(content(&e, "d/keep"), "keep");
    assert_eq!(content(&e, "d/a"), "a");
    assert_eq!(content(&e, "d/bee"), "b");
    no_temps(&e.p("d"));
}

#[test]
fn read_only_folders_fail_per_item_and_leave_nothing() {
    let e = env();
    files(&e, "ro", &["a", "b", "c"]);
    let ro = e.p("ro");
    let set = |mode: u32| {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    set(0o555);
    let st = e.run(pairs(&e, "ro", &[("a", "b"), ("b", "a"), ("c", "see")]));
    set(0o755);
    assert_eq!(st.state, OpState::CompletedWithErrors, "{st:?}");
    assert!(
        st.errors.iter().any(|x| x.message.contains("permission")),
        "{:?}",
        st.errors
    );
    assert_eq!(names(&ro), ["a", "b", "c"]);
    assert_eq!(content(&e, "ro/a"), "a");
    no_temps(&ro);
}

#[test]
fn a_failing_member_unwinds_its_cycle() {
    let e = env();
    files(&e, "d", &["a", "b", "c", "free"]);
    // b can't be renamed (immutable), so the cycle a→b→c→a can't finish.
    let b = e.p("d/b");
    let flag = |f: &str| {
        let ok = Command::new("chflags").arg(f).arg(&b).status().unwrap();
        assert!(ok.success());
    };
    flag("uchg");
    let st = e.run(pairs(
        &e,
        "d",
        &[("a", "b"), ("b", "c"), ("c", "a"), ("free", "freed")],
    ));
    flag("nouchg");
    assert_eq!(st.state, OpState::CompletedWithErrors, "{st:?}");
    no_temps(&e.p("d"));
    for n in ["a", "b", "c"] {
        assert_eq!(content(&e, &format!("d/{n}")), n, "{:?}", st.errors);
    }
    // The item outside the cycle still went.
    assert_eq!(content(&e, "d/freed"), "free");
}

#[test]
fn folders_rename_whole() {
    let e = env();
    std::fs::create_dir_all(e.p("d/one/inner")).unwrap();
    std::fs::create_dir_all(e.p("d/two")).unwrap();
    write(&e.p("d/one/inner/f"), b"x");
    let st = e.run(pairs(&e, "d", &[("one", "two"), ("two", "one")]));
    assert_eq!(st.state, OpState::Completed, "{st:?}");
    assert!(e.p("d/two/inner/f").exists());
    no_temps(&e.p("d"));
}

#[test]
fn symlinks_are_renamed_not_followed() {
    let e = env();
    files(&e, "d", &["target"]);
    std::os::unix::fs::symlink("target", e.p("d/link")).unwrap();
    std::os::unix::fs::symlink("loop", e.p("d/loop")).unwrap();
    let st = e.run(pairs(&e, "d", &[("link", "link2"), ("loop", "loop2")]));
    assert_eq!(st.state, OpState::Completed, "{st:?}");
    assert_eq!(
        std::fs::read_link(e.p("d/link2")).unwrap(),
        PathBuf::from("target")
    );
    assert!(std::fs::symlink_metadata(e.p("d/loop2")).is_ok());
}

#[test]
fn undo_refuses_when_an_item_was_replaced() {
    let e = env();
    files(&e, "d", &["a"]);
    e.run(pairs(&e, "d", &[("a", "b")]));
    std::fs::remove_file(e.p("d/b")).unwrap();
    write(&e.p("d/b"), b"new");
    assert!(e.q.undo().is_err());
}

// ───────────── crash recovery (a child process aborts mid-batch) ─────────────

/// Aborts the process before the item whose source ends with `needle` is placed.
struct CrashAt(String);

impl FaultHook for CrashAt {
    fn check(&self, step: FaultStep, path: &Path) -> std::io::Result<()> {
        if step == FaultStep::BeforePlace && path.to_string_lossy().ends_with(&self.0) {
            std::process::abort();
        }
        Ok(())
    }
}

/// The child side: only does something when spawned by a test below.
#[test]
fn rename_child() {
    let Ok(db) = std::env::var("FSTUDIO_RENAME_CHILD_DB") else {
        return;
    };
    let dir = PathBuf::from(std::env::var("FSTUDIO_RENAME_CHILD_DIR").unwrap());
    let trash = PathBuf::from(std::env::var("FSTUDIO_RENAME_CHILD_TRASH").unwrap());
    let needle = std::env::var("FSTUDIO_RENAME_CHILD_CRASH").unwrap();
    let q = open_queue(
        Path::new(&db),
        OpsConfig {
            faults: Some(Arc::new(CrashAt(needle))),
            ..Default::default()
        },
        &trash,
        Arc::new(NullSink),
    );
    let req = OpRequest::RenameMany {
        pairs: vec![
            (dir.join("a"), "b".into()),
            (dir.join("b"), "c".into()),
            (dir.join("c"), "a".into()),
        ],
    };
    let id = q.enqueue(req).unwrap();
    q.wait(id, WAIT);
}

fn crash_child(e: &Env, needle: &str) {
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "rename_child", "--nocapture", "--test-threads=1"])
        .env("FSTUDIO_RENAME_CHILD_DB", &e.db_path)
        .env("FSTUDIO_RENAME_CHILD_DIR", e.p("d"))
        .env("FSTUDIO_RENAME_CHILD_TRASH", &e.trash)
        .env("FSTUDIO_RENAME_CHILD_CRASH", needle)
        .status()
        .unwrap();
    assert!(!status.success(), "the child should have crashed");
}

fn reopen(e: &Env) -> OpQueue {
    open_queue(
        &e.db_path,
        OpsConfig::default(),
        &e.trash,
        Arc::new(NullSink),
    )
}

#[test]
fn a_crash_mid_cycle_resumes_to_the_new_names() {
    let e = env();
    files(&e, "d", &["a", "b", "c"]);
    // The cycle parks one item, then crashes before placing the next.
    crash_child(&e, "/d/b");
    let q = reopen(&e);
    let ops = q.interrupted().unwrap();
    assert_eq!(ops.len(), 1, "{ops:?}");
    q.resume(ops[0].id).unwrap();
    let st = q.wait(ops[0].id, WAIT).unwrap();
    assert_eq!(st.state, OpState::Completed, "{st:?}");
    no_temps(&e.p("d"));
    assert_eq!(content(&e, "d/b"), "a");
    assert_eq!(content(&e, "d/c"), "b");
    assert_eq!(content(&e, "d/a"), "c");
}

#[test]
fn a_crash_mid_cycle_then_cancel_puts_every_name_back() {
    for needle in ["/d/a", "/d/b", "/d/c"] {
        let e = env();
        files(&e, "d", &["a", "b", "c"]);
        crash_child(&e, needle);
        let q = reopen(&e);
        let ops = q.interrupted().unwrap();
        assert_eq!(ops.len(), 1, "{needle}: {ops:?}");
        q.cancel(ops[0].id).unwrap();
        no_temps(&e.p("d"));
        // Each file is either all the way renamed or back home; nothing lost.
        let mut got: Vec<String> = ["a", "b", "c"]
            .iter()
            .map(|n| content(&e, &format!("d/{n}")))
            .collect();
        got.sort();
        assert_eq!(got, ["a", "b", "c"], "{needle}");
        assert_eq!(names(&e.p("d")), ["a", "b", "c"], "{needle}");
    }
}

// ───────────── review fixes: regressions ─────────────

/// Runs `f` for one fault step on sources ending with `needle`.
struct Hook<F: Fn(&Path) -> std::io::Result<()> + Send + Sync>(FaultStep, String, F);

impl<F: Fn(&Path) -> std::io::Result<()> + Send + Sync> FaultHook for Hook<F> {
    fn check(&self, step: FaultStep, path: &Path) -> std::io::Result<()> {
        if step == self.0 && path.to_string_lossy().ends_with(&self.1) {
            (self.2)(path)
        } else {
            Ok(())
        }
    }
}

fn env_hook(h: impl FaultHook + 'static) -> Env {
    env_with(OpsConfig {
        faults: Some(Arc::new(h)),
        ..Default::default()
    })
}

#[test]
fn a_failed_journal_write_renames_nothing_for_that_item() {
    let e = env_hook(Hook(FaultStep::JournalWrite, "/d/a".into(), |_: &Path| {
        Err(std::io::Error::other("disk full"))
    }));
    files(&e, "d", &["a", "b", "c"]);
    let st = e.run(pairs(&e, "d", &[("a", "b"), ("b", "a"), ("c", "cee")]));
    assert_eq!(st.state, OpState::CompletedWithErrors, "{st:?}");
    assert!(
        st.errors
            .iter()
            .any(|x| x.message.contains("journal write failed")),
        "{:?}",
        st.errors
    );
    // The swap couldn't be journaled, so neither half happened.
    assert_eq!(content(&e, "d/a"), "a");
    assert_eq!(content(&e, "d/b"), "b");
    assert_eq!(content(&e, "d/cee"), "c");
    no_temps(&e.p("d"));
}

#[test]
fn a_case_only_rename_that_loses_a_race_is_never_left_hidden() {
    // Between the two halves of File → file, another app creates "file".
    let probe = tempfile::tempdir().unwrap();
    if is_case_sensitive(probe.path()).unwrap() {
        return;
    }
    let e = env_hook(Hook(
        FaultStep::AfterPark,
        "/d/File".into(),
        |src: &Path| std::fs::write(src.with_file_name("file"), "outsider"),
    ));
    files(&e, "d", &["File"]);
    let st = e.run(pairs(&e, "d", &[("File", "file")]));
    assert_eq!(st.state, OpState::CompletedWithErrors, "{st:?}");
    no_temps(&e.p("d"));
    assert_eq!(names(&e.p("d")), ["File 2", "file"]);
    assert_eq!(content(&e, "d/File 2"), "File");
    assert_eq!(content(&e, "d/file"), "outsider");
}

#[test]
fn a_cycle_whose_home_name_is_taken_meanwhile_gets_a_visible_name() {
    // a is parked; meanwhile another app creates a new "a", so b can't move
    // in and a can't go home: a must come back as "a 2", never stay hidden.
    let e = env_hook(Hook(FaultStep::AfterPark, "/d/a".into(), |src: &Path| {
        std::fs::write(src, "outsider")
    }));
    files(&e, "d", &["a", "b"]);
    let st = e.run(pairs(&e, "d", &[("a", "b"), ("b", "a")]));
    assert_eq!(st.state, OpState::CompletedWithErrors, "{st:?}");
    no_temps(&e.p("d"));
    assert_eq!(names(&e.p("d")), ["a", "a 2", "b"]);
    assert_eq!(content(&e, "d/a 2"), "a");
    assert_eq!(content(&e, "d/a"), "outsider");
    assert_eq!(content(&e, "d/b"), "b");
}

#[test]
fn recovery_never_moves_a_temporary_name_that_isnt_the_item() {
    let e = env();
    files(&e, "d", &["a", "b", "c"]);
    // a is parked, c moved into "a", then a crash before b moves.
    crash_child(&e, "/d/b");
    let tmp = e.p("d/.a.fstudio-renaming");
    assert!(tmp.exists());
    // Something else now sits under the temporary name.
    std::fs::remove_file(&tmp).unwrap();
    write(&tmp, b"stale");
    let q = reopen(&e);
    let ops = q.interrupted().unwrap();
    assert_eq!(ops.len(), 1);
    q.resume(ops[0].id)
        .expect("recovery goes on despite the stranger");
    let st = q.wait(ops[0].id, WAIT).unwrap();
    assert!(st.state.is_finished(), "{st:?}");
    assert_eq!(String::from_utf8(read(&tmp)).unwrap(), "stale");
    // Nothing was moved onto a name with the wrong object: "a" still holds c.
    assert_eq!(content(&e, "d/a"), "c");
}

#[test]
fn a_folder_and_its_contents_cant_be_renamed_together() {
    let e = env();
    std::fs::create_dir_all(e.p("d/x")).unwrap();
    write(&e.p("d/x/a"), b"a");
    let err =
        e.q.enqueue(OpRequest::RenameMany {
            pairs: vec![(e.p("d/x"), "y".into()), (e.p("d/x/a"), "b".into())],
        })
        .unwrap_err();
    assert!(err.to_string().contains("inside it"), "{err}");
    assert!(e.p("d/x/a").exists());
}

#[test]
fn duplicate_new_names_are_refused_before_anything_runs() {
    let e = env();
    files(&e, "d", &["a", "b"]);
    let same = e.q.enqueue(pairs(&e, "d", &[("a", "same"), ("b", "same")]));
    assert!(same.is_err());
    let twice = e.q.enqueue(pairs(&e, "d", &[("a", "x"), ("a", "y")]));
    assert!(twice.is_err());
    if !is_case_sensitive(e.root()).unwrap() {
        let folded =
            e.q.enqueue(pairs(&e, "d", &[("a", "Same"), ("b", "same")]))
                .unwrap_err();
        assert!(folded.to_string().contains("same name here"), "{folded}");
        let nfd =
            e.q.enqueue(pairs(&e, "d", &[("a", "Caf\u{e9}"), ("b", "Cafe\u{301}")]));
        assert!(nfd.is_err());
    }
    assert_eq!(names(&e.p("d")), ["a", "b"]);
}

#[test]
fn a_swap_given_through_two_spellings_of_one_folder_still_works() {
    let e = env();
    files(&e, "d", &["a", "b"]);
    let real = std::fs::canonicalize(e.p("d")).unwrap();
    let st = e.run(OpRequest::RenameMany {
        pairs: vec![(e.p("d/a"), "b".into()), (real.join("b"), "a".into())],
    });
    assert_eq!(st.state, OpState::Completed, "{st:?}");
    assert_eq!(content(&e, "d/a"), "b");
    assert_eq!(content(&e, "d/b"), "a");
    no_temps(&e.p("d"));
}

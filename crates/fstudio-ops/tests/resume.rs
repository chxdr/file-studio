//! Crash and resume. A child process runs the queue and is killed (SIGKILL at
//! a random moment, or aborted at a chosen step); the parent then finds the
//! op in `interrupted()`, resumes it, and compares trees.

mod common;
use common::*;
use fstudio_ops::*;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Aborts the process (a crash) at `step` for sources containing `needle`.
struct CrashAt(FaultStep, String);

impl FaultHook for CrashAt {
    fn check(&self, step: FaultStep, path: &Path) -> std::io::Result<()> {
        if step == self.0 && path.to_string_lossy().contains(&self.1) {
            std::process::abort();
        }
        Ok(())
    }
}

fn parse_step(s: &str) -> FaultStep {
    match s {
        "BeforeCopy" => FaultStep::BeforeCopy,
        "AfterCopy" => FaultStep::AfterCopy,
        "Verify" => FaultStep::Verify,
        "BeforePlace" => FaultStep::BeforePlace,
        _ => FaultStep::BeforeSourceRemove,
    }
}

/// The child side: only does something when spawned by a test below.
#[test]
fn child_runner() {
    let Ok(db) = std::env::var("FSTUDIO_CHILD_DB") else {
        return;
    };
    let src = PathBuf::from(std::env::var("FSTUDIO_CHILD_SRC").unwrap());
    let dst = PathBuf::from(std::env::var("FSTUDIO_CHILD_DST").unwrap());
    let trash = PathBuf::from(std::env::var("FSTUDIO_CHILD_TRASH").unwrap());
    let verify = std::env::var("FSTUDIO_CHILD_VERIFY").is_ok();
    let faults: Option<Arc<dyn FaultHook>> = std::env::var("FSTUDIO_CHILD_CRASH").ok().map(|c| {
        let (step, needle) = c.split_once(':').unwrap();
        Arc::new(CrashAt(parse_step(step), needle.to_string())) as Arc<dyn FaultHook>
    });
    let q = OpQueue::open(
        fstudio_db::Db::open(Path::new(&db)).unwrap(),
        OpsConfig {
            verify,
            faults,
            ..Default::default()
        },
        Arc::new(DirTrasher::new(trash).unwrap()),
        Arc::new(NullSink),
    )
    .unwrap();
    let req = if std::env::var("FSTUDIO_CHILD_MOVE").is_ok() {
        OpRequest::Move {
            sources: vec![src],
            dest_dir: dst,
        }
    } else {
        OpRequest::Copy {
            sources: vec![src],
            dest_dir: dst,
        }
    };
    let id = q.enqueue(req).unwrap();
    q.wait(id, Duration::from_secs(600));
}

struct Case {
    e: Env,
    img: Image,
    src: PathBuf,
    before: std::collections::BTreeMap<String, String>,
}

fn case(big_files: usize) -> Case {
    let e = env();
    let img = Image::new("APFS", 1024);
    let src = e.p("src");
    for i in 0..big_files {
        let mut v = vec![0u8; 16 << 20];
        v[..8].copy_from_slice(&(i as u64).to_le_bytes());
        v[(8 << 20)..(8 << 20) + 8].copy_from_slice(&(i as u64 * 7).to_le_bytes());
        write(&src.join(format!("big/b{i:02}.bin")), &v);
    }
    for i in 0..200 {
        write(
            &src.join(format!("small/{}/s{i}.txt", i % 7)),
            format!("small {i}").as_bytes(),
        );
    }
    write(&src.join("victim.bin"), &vec![3u8; 4 << 20]);
    let before = tree(&src);
    Case {
        e,
        img,
        src,
        before,
    }
}

fn spawn(c: &Case, mv: bool, verify: bool, crash: Option<&str>) -> Child {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "child_runner", "--nocapture", "--test-threads=1"])
        .env("FSTUDIO_CHILD_DB", &c.e.db_path)
        .env("FSTUDIO_CHILD_SRC", &c.src)
        .env("FSTUDIO_CHILD_DST", &c.img.mount)
        .env("FSTUDIO_CHILD_TRASH", &c.e.trash)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if mv {
        cmd.env("FSTUDIO_CHILD_MOVE", "1");
    }
    if verify {
        cmd.env("FSTUDIO_CHILD_VERIFY", "1");
    }
    if let Some(cr) = crash {
        cmd.env("FSTUDIO_CHILD_CRASH", cr);
    }
    cmd.spawn().unwrap()
}

/// Opens the queue in this process, resumes the interrupted op and checks the result.
fn resume_and_check(c: &Case, mv: bool) {
    // A stranger's temporary file in the destination must survive resume.
    let foreign = c.img.mount.join(".someone-else.fstudio-partial");
    write(&foreign, b"not ours");
    let q = OpQueue::open(
        fstudio_db::Db::open(&c.e.db_path).unwrap(),
        OpsConfig::default(),
        Arc::new(DirTrasher::new(&c.e.trash).unwrap()),
        Arc::new(NullSink),
    )
    .unwrap();
    let int = q.interrupted().unwrap();
    assert_eq!(int.len(), 1, "{int:?}");
    assert_eq!(int[0].state, OpState::Interrupted);
    q.resume(int[0].id).unwrap();
    let st = q.wait(int[0].id, WAIT).unwrap();
    assert_eq!(st.state, OpState::Completed, "{st:?}");
    assert!(q.interrupted().unwrap().is_empty());
    assert_eq!(read(&foreign), b"not ours");
    let _ = std::process::Command::new("mv")
        .arg(&foreign)
        .arg(c.e.p("foreign-out"))
        .status();
    let ours: Vec<_> = partials(&c.img.mount);
    assert!(ours.is_empty(), "leftover partials: {ours:?}");
    assert_eq!(
        tree(&c.img.mount.join("src")),
        c.before,
        "trees differ after resume"
    );
    if mv {
        assert!(!exists(&c.src), "a resumed move removes the source");
    } else {
        assert_eq!(tree(&c.src), c.before);
    }
}

fn kill_mid_copy(mv: bool, verify: bool) {
    let c = case(24);
    let mut child = spawn(&c, mv, verify, None);
    // Kill once some, but not all, data has landed.
    let t = Instant::now();
    loop {
        let landed = std::fs::read_dir(c.img.mount.join("src/big"))
            .map(|r| r.count())
            .unwrap_or(0);
        if landed >= 6 {
            break;
        }
        assert!(
            t.elapsed() < Duration::from_secs(60),
            "child made no progress"
        );
        assert!(
            child.try_wait().unwrap().is_none(),
            "child finished too early"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    child.kill().unwrap(); // SIGKILL
    child.wait().unwrap();
    resume_and_check(&c, mv);
}

#[test]
fn sigkill_mid_copy_then_resume() {
    kill_mid_copy(false, false);
}

#[test]
fn sigkill_mid_verified_move_then_resume() {
    kill_mid_copy(true, true);
}

#[test]
fn crash_at_each_step_then_resume() {
    for (step, mv, verify) in [
        ("BeforeCopy", false, false),
        ("AfterCopy", false, false),
        ("AfterCopy", true, true),
        ("BeforePlace", true, true),
        ("BeforeSourceRemove", true, false),
    ] {
        let c = case(2);
        let status = spawn(&c, mv, verify, Some(&format!("{step}:victim")))
            .wait()
            .unwrap();
        assert!(!status.success(), "{step}: the child should have crashed");
        let placed_ino = std::fs::symlink_metadata(c.img.mount.join("src/victim.bin"))
            .ok()
            .map(|m| std::os::unix::fs::MetadataExt::ino(&m));
        let left: Vec<_> = partials(&c.img.mount)
            .into_iter()
            .filter(|p| p.to_string_lossy().contains("victim"))
            .collect();
        assert_eq!(
            !left.is_empty(),
            matches!(step, "AfterCopy" | "BeforePlace"),
            "{step}: partials before resume: {left:?}"
        );
        resume_and_check(&c, mv);
        if step == "BeforeSourceRemove" {
            // The copy had landed: resume only removed the source.
            let ino = std::os::unix::fs::MetadataExt::ino(
                &std::fs::symlink_metadata(c.img.mount.join("src/victim.bin")).unwrap(),
            );
            assert_eq!(Some(ino), placed_ino, "the file was copied twice");
        }
    }
}

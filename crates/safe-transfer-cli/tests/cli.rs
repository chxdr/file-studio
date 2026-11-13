//! The `safe-transfer` binary, end to end. Every test uses its own
//! temporary journal folder, never the real one in Application Support.
// Uses the macOS copy path (F-024: macOS-only tests are gated).
#![cfg(target_os = "macos")]

use std::path::Path;
use std::process::Command;

fn st(journal: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_safe-transfer"));
    c.arg("--journal-dir").arg(journal);
    c
}

fn bytes(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

/// A shoot: a folder with small sidecars, a few raws and one big file.
fn shoot(root: &Path, big: usize) {
    let d = root.join("DCIM").join("100MSDCF");
    std::fs::create_dir_all(&d).unwrap();
    for i in 0..30 {
        std::fs::write(
            d.join(format!("DSC{i:05}.XMP")),
            bytes(1000 + i * 37, i as u64),
        )
        .unwrap();
    }
    std::fs::write(d.join("DSC00100.ARW"), bytes(big, 99)).unwrap();
    std::fs::write(d.join("empty.txt"), b"").unwrap();
}

fn json_lines(out: &[u8]) -> Vec<serde_json::Value> {
    String::from_utf8_lossy(out)
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[cfg_attr(not(feature = "fault-injection"), allow(dead_code))]
fn partials(dir: &Path) -> Vec<String> {
    let mut v = Vec::new();
    for e in walk(dir) {
        let n = e.file_name().unwrap().to_string_lossy().into_owned();
        if n.ends_with(".st-partial") {
            v.push(n);
        }
    }
    v
}

#[cfg_attr(not(feature = "fault-injection"), allow(dead_code))]
fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            }
            out.push(p);
        }
    }
    out
}

#[test]
fn copy_a_folder_then_compare() {
    let (s, d, j) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    shoot(s.path(), 3 << 20);
    let out = st(j.path())
        .args(["--json", "copy", "--no-clone", "--reread-source"])
        .arg(s.path().join("DCIM"))
        .arg(d.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ev = json_lines(&out.stdout);
    let done: Vec<_> = ev.iter().filter(|v| v["event"] == "file_done").collect();
    assert_eq!(done.len(), 32);
    assert!(done.iter().all(|v| v["source_reread"] == true));
    let summary = ev.iter().find(|v| v["event"] == "summary").unwrap();
    assert_eq!(summary["state"], "finished");
    assert_eq!(summary["copied_verified"], 32);
    assert_eq!(summary["folders"], 2);

    let out = st(j.path())
        .arg("compare")
        .arg(s.path().join("DCIM"))
        .arg(d.path().join("DCIM"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    // A second copy skips what exists: exit 1, nothing replaced.
    std::fs::write(s.path().join("DCIM/100MSDCF/DSC00000.XMP"), b"changed").unwrap();
    let out = st(j.path())
        .arg("copy")
        .arg(s.path().join("DCIM"))
        .arg(d.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        std::fs::read(d.path().join("DCIM/100MSDCF/DSC00000.XMP")).unwrap(),
        bytes(1000, 0)
    );
    let out = st(j.path()).args(["--json", "jobs"]).output().unwrap();
    assert_eq!(json_lines(&out.stdout).len(), 2);
}

#[test]
fn plan_and_volumes() {
    let (s, d, j) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    shoot(s.path(), 1000);
    let out = st(j.path())
        .args(["--json", "plan"])
        .arg(s.path().join("DCIM"))
        .arg(d.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let v = &json_lines(&out.stdout)[0];
    assert_eq!(v["files"], 32);
    assert_eq!(v["fits"], true);
    let out = st(j.path()).args(["--json", "volumes"]).output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("\"mount\":\"/\""));
}

#[test]
fn bad_destination_exits_2() {
    let (s, j) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = s.path().join("x");
    std::fs::write(&a, b"1").unwrap();
    let out = st(j.path())
        .arg("copy")
        .arg(&a)
        .arg("/no/such/folder")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

/// `kill -9` at every named point, then `safe-transfer resume`: the real
/// names never hold an incomplete file, and the resumed job finishes with
/// every file identical to its original.
#[cfg(feature = "fault-injection")]
#[test]
fn kill_9_at_every_point_then_resume() {
    use std::io::{BufRead, BufReader};
    let s = tempfile::tempdir().unwrap();
    shoot(s.path(), 9 << 20);
    let src = s.path().join("DCIM");
    for spec in [
        "pause@mid_chunk:4194304",
        "pause@before_flush",
        "pause@after_flush",
        "pause@mid_verify:2097152",
        "pause@before_rename",
        "pause@before_rename#32",
        "pause@after_rename",
        "pause@before_group_barrier",
        "pause@after_group_barrier",
        "pause@mid_group_rename:3",
        "pause@before_folder_barrier",
        "pause@mid_journal_write#6",
        "pause@before_journal_rename#6",
        "pause@mid_journal_write#7",
        "pause@before_journal_rename#7",
    ] {
        let (d, j) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let mut child = st(j.path())
            .args([
                "--json",
                "--journal-interval-ms",
                "0",
                "copy",
                "--no-clone",
                "--chunk",
                "1M",
            ])
            .arg(&src)
            .arg(d.path())
            .env("ST_FAULT", spec)
            .stderr(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        let job: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
        let job = job["job"]
            .as_str()
            .unwrap_or_else(|| panic!("{spec}: no job id: {line}"))
            .to_string();
        let mut err = BufReader::new(child.stderr.take().unwrap());
        let mut l = String::new();
        loop {
            l.clear();
            assert!(err.read_line(&mut l).unwrap() > 0, "{spec}: never reached");
            if l.contains("REACHED") {
                break;
            }
        }
        child.kill().unwrap(); // SIGKILL
        child.wait().unwrap();

        // Right after the crash: every real-named file is complete.
        for p in walk(d.path()) {
            let rel = p.strip_prefix(d.path()).unwrap();
            if p.is_file() && !rel.to_string_lossy().ends_with(".st-partial") {
                assert_eq!(
                    std::fs::read(&p).unwrap(),
                    std::fs::read(s.path().join(rel)).unwrap(),
                    "{spec}: {rel:?} is under its real name but incomplete"
                );
            }
        }
        // Resume (no fault this time) finishes the job.
        let out = st(j.path())
            .args(["--json", "resume", &job])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{spec}: resume failed: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        let summary = json_lines(&out.stdout)
            .into_iter()
            .find(|v| v["event"] == "summary")
            .unwrap();
        if spec.contains("mid_verify") || spec.ends_with("before_rename#32") {
            // Every chunk of the big file was flushed and journaled before the
            // kill: resume re-hashes and keeps them instead of copying again.
            assert_eq!(summary["chunks_reused"], 9, "{spec}: {summary}");
        }
        if spec.contains("mid_chunk") {
            let reused = summary["chunks_reused"].as_u64().unwrap();
            assert!((1..9).contains(&reused), "{spec}: {summary}");
        }
        let cmp = st(j.path())
            .arg("compare")
            .arg(&src)
            .arg(d.path().join("DCIM"))
            .output()
            .unwrap();
        assert!(
            cmp.status.success(),
            "{spec}: {}",
            String::from_utf8_lossy(&cmp.stdout)
        );
        assert!(
            partials(d.path()).is_empty(),
            "{spec}: {:?}",
            partials(d.path())
        );
    }
}

/// A crashed copy is listed as ready, and `resume --all` finishes it.
#[cfg(feature = "fault-injection")]
#[test]
fn resume_all_finishes_a_crashed_copy() {
    use std::io::{BufRead, BufReader};
    let (s, d, j) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    shoot(s.path(), 2 << 20);
    let src = s.path().join("DCIM");
    let mut child = st(j.path())
        .args(["copy", "--no-clone"])
        .arg(&src)
        .arg(d.path())
        .env("ST_FAULT", "pause@before_group_barrier")
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut err = BufReader::new(child.stderr.take().unwrap());
    let mut l = String::new();
    while !l.contains("REACHED") {
        l.clear();
        assert!(err.read_line(&mut l).unwrap() > 0, "never reached");
    }
    child.kill().unwrap();
    child.wait().unwrap();
    let out = st(j.path()).args(["--json", "jobs"]).output().unwrap();
    let jobs = json_lines(&out.stdout);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["state"], "interrupted");
    assert_eq!(jobs[0]["resume"], "ready");
    let out = st(j.path()).args(["resume", "--all"]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cmp = st(j.path())
        .arg("compare")
        .arg(&src)
        .arg(d.path().join("DCIM"))
        .output()
        .unwrap();
    assert!(
        cmp.status.success(),
        "{}",
        String::from_utf8_lossy(&cmp.stdout)
    );
    let out = st(j.path()).args(["--json", "jobs"]).output().unwrap();
    assert_eq!(json_lines(&out.stdout)[0]["resume"], "not_needed");
}

/// `kill -9` while replacing existing files: each name holds the complete
/// old or the complete new version, never anything else, and every old
/// version is either still there or in the Trash. Resume finishes the job.
#[cfg(feature = "fault-injection")]
#[test]
fn kill_9_while_replacing_never_loses_the_old_or_shows_a_half_new() {
    use std::io::{BufRead, BufReader};
    let s = tempfile::tempdir().unwrap();
    shoot(s.path(), 3 << 20);
    let src = s.path().join("DCIM");
    for spec in [
        "pause@before_replace_trash",
        "pause@before_replace_trash#20",
        "pause@after_rename#5",
        "pause@before_rename#32",
    ] {
        let (d, j, t) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        // The destination already has an older version of every file.
        for p in walk(&src) {
            if p.is_file() {
                let rel = p.strip_prefix(s.path()).unwrap();
                let q = d.path().join(rel);
                std::fs::create_dir_all(q.parent().unwrap()).unwrap();
                std::fs::write(&q, format!("old {}", rel.display())).unwrap();
            }
        }
        let trash = t.path().join("Trash");
        let mut child = st(j.path())
            .args(["--json", "--journal-interval-ms", "0", "--trash-dir"])
            .arg(&trash)
            .args([
                "copy",
                "--no-clone",
                "--chunk",
                "1M",
                "--on-conflict",
                "replace",
            ])
            .arg(&src)
            .arg(d.path())
            .env("ST_FAULT", spec)
            .stderr(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        let job: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
        let job = job["job"]
            .as_str()
            .unwrap_or_else(|| panic!("{spec}: {line}"))
            .to_string();
        let mut err = BufReader::new(child.stderr.take().unwrap());
        let mut l = String::new();
        while !l.contains("REACHED") {
            l.clear();
            assert!(err.read_line(&mut l).unwrap() > 0, "{spec}: never reached");
        }
        child.kill().unwrap();
        child.wait().unwrap();

        let trashed: Vec<Vec<u8>> = walk(&trash)
            .into_iter()
            .filter(|p| p.is_file())
            .map(|p| std::fs::read(p).unwrap())
            .collect();
        for p in walk(&src) {
            if !p.is_file() {
                continue;
            }
            let rel = p.strip_prefix(s.path()).unwrap();
            let old = format!("old {}", rel.display()).into_bytes();
            let new = std::fs::read(&p).unwrap();
            let now = std::fs::read(d.path().join(rel)).ok();
            // The name may be briefly empty (old in the Trash, new not yet
            // renamed in, proposal §8); it never holds anything incomplete.
            match &now {
                None => {}
                Some(b) if *b == old || *b == new => {}
                Some(b) => panic!(
                    "{spec}: {rel:?} holds {} bytes, neither old nor new",
                    b.len()
                ),
            }
            assert!(
                now.as_deref() == Some(&old[..]) || trashed.contains(&old),
                "{spec}: the old {rel:?} is neither in place nor in the Trash"
            );
        }
        let out = st(j.path())
            .args(["--json", "--trash-dir"])
            .arg(&trash)
            .args(["resume", &job])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{spec}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        let cmp = st(j.path())
            .arg("compare")
            .arg(&src)
            .arg(d.path().join("DCIM"))
            .output()
            .unwrap();
        assert!(
            cmp.status.success(),
            "{spec}: {}",
            String::from_utf8_lossy(&cmp.stdout)
        );
        assert!(partials(d.path()).is_empty(), "{spec}");
        let trashed_now = walk(&trash).into_iter().filter(|p| p.is_file()).count();
        assert_eq!(trashed_now, 32, "{spec}: every old version is in the Trash");
    }
}

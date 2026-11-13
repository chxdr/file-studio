//! The `safe-transfer` binary, end to end.
// Uses the macOS copy path, xattr, chmod +a, chflags and ditto (F-024: macOS-only tests are gated).
#![cfg(target_os = "macos")]

use std::process::Command;

fn st() -> Command {
    Command::new(env!("CARGO_BIN_EXE_safe-transfer"))
}

#[test]
fn copy_then_compare() {
    let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = s.path().join("DSC0001.ARW");
    let b = s.path().join("DSC0002.ARW");
    std::fs::write(&a, vec![7u8; 3 << 20]).unwrap();
    std::fs::write(&b, b"small").unwrap();
    let out = st()
        .args(["copy", "--json", "--no-clone", "--reread-source"])
        .arg(&a)
        .arg(&b)
        .arg(d.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    let done: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .filter(|v: &serde_json::Value| v["event"] == "file_done")
        .collect();
    assert_eq!(done.len(), 2);
    assert!(
        done.iter()
            .all(|v| v["outcome"] == "verified" && v["source_reread"] == true)
    );
    assert_eq!(done[0]["xxh3_128"].as_str().unwrap().len(), 32);

    let out = st()
        .arg("compare")
        .arg(s.path())
        .arg(d.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    // A second copy conflicts: exit 1, nothing replaced.
    std::fs::write(&a, b"changed").unwrap();
    let out = st().arg("copy").arg(&a).arg(d.path()).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        std::fs::read(d.path().join("DSC0001.ARW")).unwrap(),
        vec![7u8; 3 << 20]
    );
    let out = st()
        .arg("compare")
        .arg(s.path())
        .arg(d.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn plan_and_volumes() {
    let (s, d) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = s.path().join("x");
    std::fs::write(&a, b"1234").unwrap();
    let out = st()
        .args(["plan", "--json"])
        .arg(&a)
        .arg(d.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("\"fits\":true"));
    let out = st().args(["volumes", "--json"]).output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("\"mount\":\"/\""));
}

#[test]
fn bad_destination_exits_2() {
    let s = tempfile::tempdir().unwrap();
    let a = s.path().join("x");
    std::fs::write(&a, b"1").unwrap();
    let out = st()
        .arg("copy")
        .arg(&a)
        .arg("/no/such/folder")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

/// `kill -9` at a paused fault point: the real name never holds an
/// incomplete file, and the original is untouched.
#[cfg(feature = "fault-injection")]
#[test]
fn kill_9_at_each_point_never_leaves_an_incomplete_real_file() {
    use std::io::{BufRead, BufReader};
    let s = tempfile::tempdir().unwrap();
    let a = s.path().join("DSC0003.ARW");
    let data: Vec<u8> = (0..(12u32 << 20)).map(|i| (i % 249) as u8).collect();
    std::fs::write(&a, &data).unwrap();
    for spec in [
        "pause@mid_chunk:4194304",
        "pause@before_flush",
        "pause@mid_verify:4194304",
        "pause@before_rename",
        "pause@after_rename",
    ] {
        let d = tempfile::tempdir().unwrap();
        let mut child = st()
            .args(["copy", "--no-clone", "--chunk", "1M"])
            .arg(&a)
            .arg(d.path())
            .env("ST_FAULT", spec)
            .stderr(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut err = BufReader::new(child.stderr.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert!(
                err.read_line(&mut line).unwrap() > 0,
                "{spec}: never reached"
            );
            if line.contains("REACHED") {
                break;
            }
        }
        child.kill().unwrap(); // SIGKILL
        child.wait().unwrap();
        let dest = d.path().join("DSC0003.ARW");
        let names: Vec<String> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        if spec == "pause@after_rename" {
            assert_eq!(std::fs::read(&dest).unwrap(), data, "{spec}");
        } else {
            assert!(!dest.exists(), "{spec}: {names:?}");
            assert!(
                names.iter().all(|n| n.ends_with(".st-partial")),
                "{spec}: {names:?}"
            );
        }
    }
    assert_eq!(std::fs::read(&a).unwrap(), data);
}

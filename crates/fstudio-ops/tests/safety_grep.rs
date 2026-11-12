//! F-007, structurally: the only places that remove anything on disk are in
//! `src/remove.rs` (Delete Immediately, our temporary files, and the source
//! of a cross-volume move after its copy is placed). Non-exclusive renames,
//! which can replace an item, appear only in the exclusive-rename fallback.

use std::path::Path;

fn sources(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Lines of code (comments stripped) that contain `needle`, as "file:line".
fn hits(needle: &str) -> Vec<String> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    let mut out = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        for (i, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap();
            if code.contains(needle) {
                let rel = f.strip_prefix(&src).unwrap().display().to_string();
                out.push(format!("{rel}:{}", i + 1));
            }
        }
    }
    out
}

fn only_in(needle: &str, allowed: &[&str]) {
    let bad: Vec<String> = hits(needle)
        .into_iter()
        .filter(|h| !allowed.iter().any(|a| h.starts_with(&format!("{a}:"))))
        .collect();
    assert!(bad.is_empty(), "`{needle}` outside {allowed:?}: {bad:?}");
}

#[test]
fn removal_lives_only_in_remove_rs() {
    for needle in [
        "remove_file",
        "remove_dir",
        "unlink",
        "rmdir",
        "COPYFILE_MOVE",
        "COPYFILE_UNLINK",
        "removeItem",
    ] {
        only_in(needle, &["remove.rs"]);
    }
    // And remove.rs's removals are exactly the four documented paths.
    let n = hits("std::fs::remove_")
        .into_iter()
        .filter(|h| h.starts_with("remove.rs:"))
        .count();
    assert_eq!(
        n, 6,
        "remove.rs should have exactly 6 std::fs::remove_* calls"
    );
}

#[test]
fn replacing_renames_live_only_in_the_fallback() {
    only_in("fs::rename(", &[]);
    only_in("libc::rename(", &["sys/mod.rs"]);
    // COPYFILE_UNLINK replaces the destination; never used.
    only_in("COPYFILE_UNLINK", &[]);
}

#[test]
fn every_copyfile_write_is_exclusive() {
    // Data-writing copyfile calls in the engine carry COPYFILE_EXCL or are
    // CLONE_FORCE (which implies it).
    let engine =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/engine.rs"))
            .unwrap();
    assert!(engine.contains("libc::COPYFILE_EXCL"));
    for (i, l) in engine.lines().enumerate() {
        if l.contains("COPYFILE_DATA")
            && !l.contains("COPYFILE_DATA_SPARSE")
            && !l.contains("base |")
        {
            panic!(
                "engine.rs:{}: COPYFILE_DATA without the exclusive base flags",
                i + 1
            );
        }
    }
}

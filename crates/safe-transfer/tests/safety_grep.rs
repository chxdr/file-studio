//! Keeps removal in one place (F-007): no file in `src/` except `remove.rs`
//! may remove anything.

use std::path::Path;

const FORBIDDEN: &[&str] = &["remove_file", "remove_dir", "unlink", "rmdir"];

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn only_remove_rs_removes_anything() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(files.len() > 5);
    let mut bad = Vec::new();
    for f in files {
        if f.file_name().is_some_and(|n| n == "remove.rs") {
            continue;
        }
        for (i, line) in std::fs::read_to_string(&f).unwrap().lines().enumerate() {
            if FORBIDDEN.iter().any(|w| line.contains(w)) {
                bad.push(format!("{}:{}: {}", f.display(), i + 1, line.trim()));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "removal outside remove.rs:\n{}",
        bad.join("\n")
    );
}

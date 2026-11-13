//! Small-file benchmark for the M2 gate (proposal §4.7): copies N files of
//! 1–64 KiB into `<dest>` through the engine (group commit) or, with
//! `--per-file`, one `copy_file` at a time (a drive flush per file).
//!
//! cargo run --release -p safe-transfer --example bench_small -- <dest> [N] [--per-file]
//!
//! Prints one JSON line. Every file is verified either way.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use safe_transfer::{
    Control, CopyRequest, Engine, EngineConfig, Item, JobSpec, JobState, NoEvents, NoProgress,
    Options, Platform,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dest = PathBuf::from(
        args.get(1)
            .expect("usage: bench_small <dest> [N] [--per-file]"),
    );
    let n: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5000);
    let per_file = args.iter().any(|a| a == "--per-file");
    let src_root = tempfile::tempdir().unwrap();
    let src = src_root.path().join("small");
    std::fs::create_dir(&src).unwrap();
    let mut bytes = 0u64;
    for i in 0..n {
        let size = 1024 + (i * 2_654_435_761) % (63 * 1024);
        let data: Vec<u8> = (0..size).map(|k| (k * 31 + i) as u8).collect();
        std::fs::write(src.join(format!("f{i:05}.bin")), &data).unwrap();
        bytes += size as u64;
    }
    let opts = Options {
        clone_same_volume: false,
        ..Options::default()
    };
    let journal = tempfile::tempdir().unwrap();
    let t = Instant::now();
    if per_file {
        let out = dest.join("small");
        std::fs::create_dir_all(&out).unwrap();
        let p = Platform::native();
        for i in 0..n {
            let req = CopyRequest::into_dir(src.join(format!("f{i:05}.bin")), &out).unwrap();
            safe_transfer::copy_file(&p, &req, &opts, &Control::new(), &NoProgress).unwrap();
        }
    } else {
        let e = Engine::open(
            EngineConfig::new(journal.path()),
            Platform::native(),
            Arc::new(NoEvents),
        )
        .unwrap();
        let mut spec = JobSpec::new(vec![Item::Path(src.clone())], &dest);
        spec.options = opts;
        let id = e.create(spec).unwrap();
        e.start(id).unwrap();
        let info = e.wait(id, Duration::from_secs(3600)).unwrap();
        assert_eq!(info.state, JobState::Finished);
        assert_eq!(e.report(id).unwrap().copied_verified as usize, n);
    }
    let secs = t.elapsed().as_secs_f64();
    println!(
        "{{\"mode\":\"{}\",\"files\":{n},\"bytes\":{bytes},\"seconds\":{secs:.3},\"ms_per_file\":{:.3}}}",
        if per_file { "per_file" } else { "group" },
        secs * 1000.0 / n as f64
    );
}

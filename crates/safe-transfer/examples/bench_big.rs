//! Big-file benchmark: copies one existing file into `<dest>` through the
//! engine (64 MiB chunks, a flush per chunk, uncached read-back), and
//! prints one JSON line with the time and the time spent in each part.
//!
//! cargo run --release -p safe-transfer --example bench_big -- <file> <dest>

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use safe_transfer::{Engine, EngineConfig, Event, Item, JobSpec, JobState, Options, Platform};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = PathBuf::from(args.get(1).expect("usage: bench_big <file> <dest>"));
    let dest = PathBuf::from(args.get(2).expect("usage: bench_big <file> <dest>"));
    let journal = tempfile::tempdir().unwrap();
    let report = Arc::new(Mutex::new(None));
    let r2 = report.clone();
    let sink = move |e: Event| {
        if let Event::FileDone { report, .. } = e {
            *r2.lock().unwrap() = Some(report);
        }
    };
    let e = Engine::open(
        EngineConfig::new(journal.path()),
        Platform::native(),
        Arc::new(sink),
    )
    .unwrap();
    let mut spec = JobSpec::new(vec![Item::Path(src.clone())], &dest);
    spec.options = Options {
        clone_same_volume: false,
        ..Options::default()
    };
    let t = Instant::now();
    let id = e.create(spec).unwrap();
    e.start(id).unwrap();
    let info = e.wait(id, Duration::from_secs(3600)).unwrap();
    let secs = t.elapsed().as_secs_f64();
    assert_eq!(info.state, JobState::Finished);
    let r = report.lock().unwrap().take().unwrap();
    let mb = r.bytes as f64 / 1e6;
    println!(
        "{{\"bytes\":{},\"seconds\":{secs:.3},\"mb_per_s\":{:.0},\"copy_s\":{:.3},\"flush_s\":{:.3},\"verify_s\":{:.3},\"outcome\":\"{}\",\"chunks\":{}}}",
        r.bytes,
        mb / secs,
        r.copy_time.as_secs_f64(),
        r.flush_time.as_secs_f64(),
        r.verify_time.as_secs_f64(),
        r.outcome.as_str(),
        r.hashes.chunks.len()
    );
}

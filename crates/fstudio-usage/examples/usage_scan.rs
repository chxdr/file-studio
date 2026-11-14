//! Times a scan: `cargo run --release -p fstudio-usage --example usage_scan -- <path> [auto|portable] [threads]`.

use std::path::PathBuf;
use std::time::Instant;

use fstudio_usage::*;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().unwrap_or_else(|| ".".into()));
    let method = match args.next().as_deref() {
        Some("portable") => Method::Portable,
        _ => Method::Auto,
    };
    let threads = args.next().and_then(|t| t.parse().ok()).unwrap_or(0);
    let opts = ScanOptions {
        method,
        threads,
        ..ScanOptions::default()
    };
    let t0 = Instant::now();
    let tree = scan(&path, &opts, &ScanControl::new()).expect("scan");
    let el = t0.elapsed();
    let r = tree.node(tree.root());
    println!(
        "{} {:?} threads={} items={} folders_denied={} allocated={:.2} GB logical={:.2} GB icloud={:.2} GB time={:.2?} nodes={}",
        path.display(),
        method,
        if threads == 0 {
            default_threads()
        } else {
            threads
        },
        r.items,
        r.denied,
        r.allocated as f64 / 1e9,
        r.logical as f64 / 1e9,
        r.cloud as f64 / 1e9,
        el,
        tree.len()
    );
}

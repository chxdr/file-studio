//! Prints first-chunk and fully-sorted times for 10k and 100k entries (warm cache).
//!
//! Run: `cargo run --release -p fstudio-fs --example bench_list [N ...]`
//! Budgets: 10k first rows < 50 ms and sorted < 200 ms; 100k sorted < 1 s.

use fstudio_fs::Lister;
use fstudio_view::{Change, ListingBuilder, ViewOptions};
use std::time::Instant;

fn main() {
    let sizes: Vec<usize> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let sizes = if sizes.is_empty() {
        vec![10_000, 100_000]
    } else {
        sizes
    };
    for n in sizes {
        let dir = tempdir(n);
        for run in 0..3 {
            let t0 = Instant::now();
            let mut lister = Lister::open(&dir).expect("open");
            let mut b = ListingBuilder::new(ViewOptions::default());
            let first = lister.next_chunk().expect("chunk").expect("ok");
            let first_len = first.len();
            b.push_chunk(first);
            let first_snapshot = b.snapshot();
            let t_first = t0.elapsed();
            while let Some(c) = lister.next_chunk() {
                b.push_chunk(c.expect("ok"));
            }
            let t_listed = t0.elapsed();
            let listing = b.finish();
            let t_sorted = t0.elapsed();

            let t = Instant::now();
            let l2 = listing.apply([
                Change::Upsert(fstudio_fs::stat_entry(dir.join("file_000001.dat")).unwrap()),
                Change::Remove("file_000002.dat".into()),
            ]);
            let t_apply = t.elapsed();
            assert_eq!(l2.total_len(), listing.total_len() - 1);
            println!(
                "n={n:>6} run {run}: first chunk ({first_len} rows, {} visible) {:>7.2} ms | listed {:>7.1} ms | fully sorted {:>7.1} ms | 2-change apply {:>6.2} ms",
                first_snapshot.len(),
                ms(t_first),
                ms(t_listed),
                ms(t_sorted),
                ms(t_apply)
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn tempdir(n: usize) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("fstudio-bench-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..n {
        std::fs::write(dir.join(format!("file_{i:06}.dat")), b"").unwrap();
    }
    dir
}

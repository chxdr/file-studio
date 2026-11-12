//! Keystroke-latency benchmark for completion: 10k frecency entries, a 100k-child
//! folder in the cache, and a replay of typed prefixes. Prints p50/p99 per scenario
//! and overall. Budget: p99 under 16 ms.
//!
//! Run: `cargo run --release -p fstudio-path --example bench_complete`

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use fstudio_path::testing::{MemDirSource, MemFs};
use fstudio_path::{Completer, CompleterConfig, Frecency, FrecencyConfig, ParseContext};

const NOW: u64 = 1_800_000_000;

fn main() {
    let frec = Frecency::in_memory(FrecencyConfig {
        max_age: 1e9,
        ..Default::default()
    }); // no ageing: keep all 10k
    frec.set_clock(|| NOW);
    let words = [
        "gig", "photos", "project", "invoice", "client", "archive", "raw", "export", "draft",
        "music", "shoot", "wedding", "backup", "scans", "design", "build", "notes", "travel",
        "2024", "2025", "2026",
    ];
    for i in 0..10_000u64 {
        let a = words[(i * 7 % words.len() as u64) as usize];
        let b = words[(i * 13 % words.len() as u64) as usize];
        let c = words[(i * 29 % words.len() as u64) as usize];
        let p = format!("/Users/me/{a}/{b}-{i}/{c}_{i}");
        for _ in 0..(1 + i % 3) {
            frec.visit_at(Path::new(&p), NOW - (i % 5000) * 60);
        }
    }
    frec.visit_at(Path::new("/Users/me/Pictures/2026/2026-11-11_gig"), NOW);

    let mut src = MemDirSource::new();
    src.insert(
        "/big",
        (0..100_000).map(|i| format!("file_{i:06}_{}", words[i % words.len()])),
    );
    src.insert("/Users/me", words.iter().map(|w| w.to_string()));
    let comp = Completer::new(Arc::new(src), frec.clone(), CompleterConfig::default());
    let ctx = ParseContext::new("/Users/me", "/Users/me", Box::new(MemFs::new()));

    println!("frecency entries: {}", frec.len());
    let scenarios: Vec<(&str, Vec<String>)> = vec![
        ("prefix in 100k folder", prefixes("/big/file_054321_music")),
        (
            "one char in 100k folder",
            ["/big/f", "/big/e", "/big/z", "/big/_", "/big/0"]
                .map(String::from)
                .to_vec(),
        ),
        ("no prefix, subsequence in 100k", prefixes("/big/xq")),
        ("worst: 100k subsequence survivors", prefixes("/big/le_0")),
        ("fuzzy in 100k (mid-word)", prefixes("/big/54321mus")),
        ("empty segment in 100k", vec!["/big/".into()]),
        ("bare word", prefixes("gig")),
        ("bare word, long", prefixes("wedding raw 2026")),
        ("path under home + frecent", prefixes("~/Pictures/2026/gig")),
    ];
    let mut all = Vec::new();
    for (name, inputs) in &scenarios {
        let mut t = Vec::new();
        for _ in 0..200 {
            for s in inputs {
                let start = Instant::now();
                let r = comp.complete(s, s.len(), &ctx);
                t.push(start.elapsed().as_secs_f64() * 1000.0);
                std::hint::black_box(r);
            }
        }
        let (p50, p99, max) = stats(&mut t);
        println!(
            "{name:<34} n={:<6} p50={p50:>7.3} ms  p99={p99:>7.3} ms  max={max:>7.3} ms",
            t.len()
        );
        all.extend(t);
    }
    let (p50, p99, max) = stats(&mut all);
    println!(
        "{:<34} n={:<6} p50={p50:>7.3} ms  p99={p99:>7.3} ms  max={max:>7.3} ms",
        "OVERALL",
        all.len()
    );
    println!(
        "budget p99 < 16 ms: {}",
        if p99 < 16.0 { "PASS" } else { "FAIL" }
    );
}

fn prefixes(s: &str) -> Vec<String> {
    s.char_indices()
        .map(|(i, c)| s[..i + c.len_utf8()].to_owned())
        .collect()
}

fn stats(v: &mut [f64]) -> (f64, f64, f64) {
    v.sort_by(|a, b| a.total_cmp(b));
    let at = |q: f64| v[((v.len() as f64 * q) as usize).min(v.len() - 1)];
    (at(0.5), at(0.99), *v.last().unwrap())
}

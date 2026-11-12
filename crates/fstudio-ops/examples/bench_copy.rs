//! Copy throughput through the full queue (journal, partial files, exclusive
//! renames): a 1 GiB file and 5,000 small files, clone vs non-clone, verify
//! off/on.
//!
//! `cargo run --release -p fstudio-ops --example bench_copy`
//!
//! "Same volume" clones (APFS). "Cross volume" goes to a fresh APFS disk
//! image on the same SSD (no clone possible), which is how docs/SPIKES.md §3
//! measured it too; each case gets its own fresh image. Times are wall-clock
//! for the op, including the journal. Verified copies write the destination
//! with `F_NOCACHE` and re-read it from the device; plain copies may still
//! have data in the page cache when the op ends.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fstudio_ops::*;

fn sh(args: &[&str]) {
    let ok = Command::new(args[0])
        .args(&args[1..])
        .status()
        .unwrap()
        .success();
    assert!(ok, "{args:?}");
}

struct Image {
    mount: PathBuf,
}

impl Image {
    fn new(dir: &Path, tag: &str) -> Image {
        let img = dir.join(format!("bench-{tag}.dmg"));
        let mount = dir.join(format!("mnt-{tag}"));
        std::fs::create_dir_all(&mount).unwrap();
        sh(&[
            "hdiutil",
            "create",
            "-quiet",
            "-megabytes",
            "4096",
            "-fs",
            "APFS",
            "-volname",
            "BENCH",
            img.to_str().unwrap(),
        ]);
        sh(&[
            "hdiutil",
            "attach",
            "-quiet",
            "-nobrowse",
            "-noverify",
            "-mountpoint",
            mount.to_str().unwrap(),
            img.to_str().unwrap(),
        ]);
        Image { mount }
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        let _ = Command::new("hdiutil")
            .args(["detach", "-quiet", "-force"])
            .arg(&self.mount)
            .status();
    }
}

fn gen_big(p: &Path) {
    let mut v = vec![0u8; 1 << 30];
    let mut x: u64 = 0x2545_F491_4F6C_DD1D;
    for c in v.chunks_mut(8) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        c.copy_from_slice(&x.to_le_bytes()[..c.len()]);
    }
    std::fs::write(p, v).unwrap();
}

fn gen_small(dir: &Path) -> u64 {
    let mut total = 0;
    for i in 0..5000u64 {
        let size = 1024 + (i * 7919) % (63 * 1024);
        let p = dir.join(format!("d{:02}/f{i:04}.bin", i % 50));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, vec![(i % 251) as u8; size as usize]).unwrap();
        total += size;
    }
    total
}

fn run(src: &Path, dest: &Path, verify: bool, tmp: &Path) -> Duration {
    std::fs::create_dir_all(dest).unwrap();
    let q = OpQueue::open(
        fstudio_db::Db::open(&tmp.join(format!("db-{}.sqlite3", std::process::id()))).unwrap(),
        OpsConfig {
            verify,
            ..Default::default()
        },
        Arc::new(DirTrasher::new(tmp.join("trash")).unwrap()),
        Arc::new(NullSink),
    )
    .unwrap();
    let t = Instant::now();
    let id = q
        .enqueue(OpRequest::Copy {
            sources: vec![src.to_path_buf()],
            dest_dir: dest.to_path_buf(),
        })
        .unwrap();
    let st = q.wait(id, Duration::from_secs(3600)).unwrap();
    let el = t.elapsed();
    assert_eq!(st.state, OpState::Completed, "{st:?}");
    el
}

fn main() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let big = root.join("src-big");
    std::fs::create_dir_all(&big).unwrap();
    gen_big(&big.join("big.bin"));
    let small = root.join("src-small");
    let small_bytes = gen_small(&small);
    let rounds = 3;
    println!("| Case | Verify | Median ms | MB/s |");
    println!("|---|---|---|---|");
    for (name, src, bytes) in [
        ("1 GiB file", &big, 1u64 << 30),
        ("5,000 small files", &small, small_bytes),
    ] {
        for vol in ["same volume (clone)", "cross volume"] {
            for verify in [false, true] {
                let img = (vol == "cross volume")
                    .then(|| Image::new(root, &format!("{}{verify}", name.len())));
                let dest_root = img
                    .as_ref()
                    .map_or_else(|| root.join("dst"), |i| i.mount.clone());
                let mut times = Vec::new();
                for r in 0..rounds {
                    let dest = dest_root.join(format!("{}-{verify}-{r}", name.len()));
                    times.push(run(src, &dest, verify, root));
                    // Keep the image from filling up.
                    let _ = Command::new("rm").arg("-rf").arg(&dest).status();
                }
                times.sort();
                let med = times[rounds / 2];
                let mbs = bytes as f64 / med.as_secs_f64() / 1e6;
                println!(
                    "| {name}, {vol} | {} | {:.0} | {:.0} |",
                    if verify { "on" } else { "off" },
                    med.as_secs_f64() * 1e3,
                    mbs
                );
            }
        }
    }
}

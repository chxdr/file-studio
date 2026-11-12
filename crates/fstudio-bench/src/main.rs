//! `fstudio-bench`: measures File Studio's M1 budgets through the
//! `fstudio-core` API (not the crates directly), on synthetic fixtures.
//!
//! `cargo run --release -p fstudio-bench -- [--record] [--large] [--quick]`
//!
//! Prints a markdown table and writes JSON (`--json PATH`). With `--record`
//! the table is appended to `bench/history.md`.

mod fixtures;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use clap::Parser;
use crossbeam_channel::{Receiver, unbounded};
use fstudio_core::*;
use serde::Serialize;

use fixtures::Fixtures;

#[derive(Parser)]
#[command(
    name = "fstudio-bench",
    about = "File Studio budgets, measured through the core API"
)]
struct Args {
    /// Where fixtures are generated and cached (default: $TMPDIR/fstudio-bench-fixtures).
    #[arg(long)]
    fixtures: Option<PathBuf>,
    /// Also run the 10 GiB copy.
    #[arg(long)]
    large: bool,
    /// Fewer iterations, skip the copy benchmarks.
    #[arg(long)]
    quick: bool,
    /// Append the table to bench/history.md.
    #[arg(long)]
    record: bool,
    /// The heading for --record.
    #[arg(long, default_value = "M1")]
    label: String,
    /// Write results as JSON here.
    #[arg(long)]
    json: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
struct Measure {
    name: String,
    unit: String,
    median: f64,
    p95: f64,
    samples: usize,
    /// The budget, in `unit`, when this row has one.
    budget: Option<f64>,
    /// Whether the budget was met (by `p95` for latency, `median` for throughput ratios).
    pass: Option<bool>,
    note: String,
}

fn stats(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(f64::total_cmp);
    let pick = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
    (pick(0.5), pick(0.95))
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

struct Bench {
    engine: Arc<Engine>,
    rx: Receiver<CoreEvent>,
    out: Vec<Measure>,
    _tmp: tempfile::TempDir,
    tmp: PathBuf,
}

impl Bench {
    fn push(&mut self, name: &str, unit: &str, samples: Vec<f64>, budget: Option<f64>, note: &str) {
        let n = samples.len();
        let (median, mut p95) = stats(samples.clone());
        if unit == "MB/s" {
            // Higher is better: the slow tail is the 5th percentile.
            let mut v = samples;
            v.sort_by(f64::total_cmp);
            p95 = v[((v.len() - 1) as f64 * 0.05).round() as usize];
        }
        let pass = budget.map(|b| p95 <= b);
        eprintln!(
            "{name}: median {median:.3} {unit}, p95 {p95:.3} {unit}{}",
            pass.map(|p| if p { "  PASS" } else { "  FAIL" })
                .unwrap_or("")
        );
        self.out.push(Measure {
            name: name.into(),
            unit: unit.into(),
            median,
            p95,
            samples: n,
            budget,
            pass,
            note: note.into(),
        });
    }

    fn drain(&self) {
        self.engine.flush_events();
        while self.rx.try_recv().is_ok() {}
    }

    /// Waits for a listing event for `tab` matching `kinds`; returns when it arrived.
    fn wait_listing(
        &self,
        tab: TabId,
        kinds: &[ChangeKind],
        timeout: Duration,
    ) -> Result<(Instant, ChangeKind)> {
        let end = Instant::now() + timeout;
        loop {
            match self.rx.recv_deadline(end) {
                Ok(CoreEvent::ListingChanged { tab: t, kind, .. })
                    if t == tab && kinds.contains(&kind) =>
                {
                    return Ok((Instant::now(), kind));
                }
                Ok(_) => {}
                Err(_) => bail!("timed out waiting for {kinds:?}"),
            }
        }
    }

    fn settle(&self, tab: &Tab) -> Result<()> {
        let end = Instant::now() + Duration::from_secs(60);
        while tab.listing().state() == ListingState::Loading {
            if Instant::now() > end {
                bail!("listing never finished");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        // Let the coalescing window pass so the next event is not held.
        std::thread::sleep(COALESCE_WINDOW + Duration::from_millis(4));
        self.drain();
        Ok(())
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let fx_root = args
        .fixtures
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join("fstudio-bench-fixtures"));
    let fx = Fixtures::new(fx_root)?;
    let tmp = tempfile::tempdir()?;
    let tmp_path = std::fs::canonicalize(tmp.path())?;
    std::fs::create_dir_all(tmp_path.join("trash"))?;
    let (tx, rx) = unbounded();
    let engine = Engine::new(
        EngineConfig {
            data_dir: tmp_path.join("data").to_string_lossy().into_owned(),
            home: None,
            trash_dir: Some(tmp_path.join("trash").to_string_lossy().into_owned()),
            poll_watcher: false,
        },
        Arc::new(tx),
    )?;
    engine.set_setting("path_bar.frecency_excluded", "[]")?;
    let mut b = Bench {
        engine,
        rx,
        out: Vec::new(),
        _tmp: tmp,
        tmp: tmp_path,
    };
    let iters = if args.quick { 5 } else { 21 };

    let flat1k = fx.flat(1_000)?;
    let flat10k = fx.flat(10_000)?;
    let flat100k = fx.flat(100_000)?;
    fx.deep()?;
    fx.unicode()?;
    fx.sparse()?;
    let neutral = fx.unicode()?;

    // ── listing: first rows and fully sorted ──
    b.engine
        .new_window(Some(neutral.to_string_lossy().into_owned()))?;
    let tab = b.engine.active_tab().expect("a tab");
    b.settle(&tab)?;
    for (dir, n, first_budget, sorted_budget) in [
        (&flat1k, "1k", None, None),
        (&flat10k, "10k", Some(50.0), Some(200.0)),
        (&flat100k, "100k", Some(50.0), Some(1000.0)),
    ] {
        let (mut first, mut sorted) = (Vec::new(), Vec::new());
        for _ in 0..iters {
            tab.go(&neutral.to_string_lossy())?;
            b.settle(&tab)?;
            let t0 = Instant::now();
            tab.go(&dir.to_string_lossy())?;
            let (t1, kind) = b.wait_listing(
                tab.id(),
                &[ChangeKind::Partial, ChangeKind::Sorted],
                Duration::from_secs(30),
            )?;
            first.push(ms(t1 - t0));
            let t2 = if kind == ChangeKind::Sorted {
                t1
            } else {
                b.wait_listing(tab.id(), &[ChangeKind::Sorted], Duration::from_secs(30))?
                    .0
            };
            sorted.push(ms(t2 - t0));
            assert_eq!(tab.listing().total_len() as usize, dir_len(dir));
        }
        b.push(
            &format!("{n} folder: first rows (ListingChanged)"),
            "ms",
            first,
            first_budget,
            "warm cache",
        );
        b.push(
            &format!("{n} folder: fully sorted"),
            "ms",
            sorted,
            sorted_budget,
            "warm cache",
        );
    }

    // ── re-sort and filter per keystroke ──
    for (dir, n) in [(&flat10k, "10k"), (&flat100k, "100k")] {
        tab.go(&dir.to_string_lossy())?;
        b.settle(&tab)?;
        let mut resort = Vec::new();
        for i in 0..iters {
            let mut v = tab.view();
            v.sort = if i % 2 == 0 {
                SortBy::Size
            } else {
                SortBy::Name
            };
            v.ascending = i % 3 != 0;
            let t0 = Instant::now();
            tab.set_view(v);
            let (t1, _) =
                b.wait_listing(tab.id(), &[ChangeKind::Resorted], Duration::from_secs(10))?;
            resort.push(ms(t1 - t0));
            std::thread::sleep(COALESCE_WINDOW + Duration::from_millis(4));
            b.drain();
        }
        b.push(
            &format!("{n} folder: re-sort"),
            "ms",
            resort,
            None,
            "set_view → Resorted event",
        );
        let mut v = tab.view();
        v.sort = SortBy::Name;
        v.ascending = true;
        tab.set_view(v);
        b.settle(&tab)?;
        let mut filter = Vec::new();
        for _ in 0..(iters / 5).max(1) {
            let text = "file_12345";
            for k in 1..=text.len() {
                let t0 = Instant::now();
                tab.set_filter(&text[..k]);
                let (t1, _) =
                    b.wait_listing(tab.id(), &[ChangeKind::Resorted], Duration::from_secs(10))?;
                filter.push(ms(t1 - t0));
                std::thread::sleep(COALESCE_WINDOW + Duration::from_millis(4));
                b.drain();
            }
            tab.set_filter("");
            b.wait_listing(tab.id(), &[ChangeKind::Resorted], Duration::from_secs(10))?;
            b.settle(&tab)?;
        }
        let budget = (n == "10k").then_some(16.0);
        let note = if budget.is_some() {
            "set_filter → Resorted event; budget: one 60 Hz frame"
        } else {
            "set_filter → Resorted event"
        };
        b.push(
            &format!("{n} folder: filter per keystroke"),
            "ms",
            filter,
            budget,
            note,
        );
    }

    // ── rows() call cost ──
    tab.go(&flat100k.to_string_lossy())?;
    b.settle(&tab)?;
    let len = tab.listing().len();
    let mut rows = Vec::new();
    for i in 0..(iters * 50) {
        let start = (i as u32 * 997) % (len - 60);
        let t0 = Instant::now();
        let r = tab.rows(start, 60);
        rows.push(t0.elapsed().as_secs_f64() * 1e6);
        assert_eq!(r.len(), 60);
    }
    b.push(
        "rows(start, 60) on 100k",
        "µs",
        rows,
        Some(1000.0),
        "formatted sizes and dates; budget: well under a frame",
    );

    // ── watcher change → Patched ──
    tab.go(&flat10k.to_string_lossy())?;
    b.settle(&tab)?;
    let mut patched = Vec::new();
    for i in 0..iters {
        let p = flat10k.join(format!("zz_bench_{i}.tmp"));
        let t0 = Instant::now();
        std::fs::write(&p, b"x")?;
        let (t1, _) = b.wait_listing(tab.id(), &[ChangeKind::Patched], Duration::from_secs(5))?;
        patched.push(ms(t1 - t0));
        std::fs::remove_file(&p)?;
        b.wait_listing(tab.id(), &[ChangeKind::Patched], Duration::from_secs(5))?;
        std::thread::sleep(Duration::from_millis(60));
        b.drain();
    }
    b.push(
        "watcher: file created → Patched (10k folder)",
        "ms",
        patched,
        Some(100.0),
        "FSEvents with NoDefer; write → event at the shell",
    );

    // ── completion per keystroke, 10k frecency entries ──
    for i in 0..10_000u32 {
        b.engine.add_frecent(
            &format!("/Users/bench/Projects/p{:03}/module_{i}", i % 300),
            1.0 + (i % 17) as f64,
            1_700_000_000,
        );
    }
    let mut comp = Vec::new();
    let cwd = fx.root.to_string_lossy().into_owned();
    for _ in 0..(iters / 4).max(2) {
        let path_typed = format!("{}/flat_100k/Folder 99", fx.root.display());
        let base = fx.root.to_string_lossy().len() + 1;
        for text in [&path_typed[..], "modul", "p12 mod", "module_99"] {
            let start = if text.starts_with('/') { base } else { 1 };
            for k in start..=text.len() {
                if !text.is_char_boundary(k) {
                    continue;
                }
                let t0 = Instant::now();
                let c = b.engine.complete(&text[..k], k as u32, &cwd);
                comp.push(ms(t0.elapsed()));
                std::hint::black_box(c);
            }
        }
    }
    b.push(
        "completion per keystroke (10k frecent, 100k-entry folder)",
        "ms",
        comp,
        Some(16.0),
        "path-like and bare-word input",
    );

    if !args.quick {
        copies(&mut b, &fx, args.large)?;
    }

    b.engine.app_will_quit();
    let table = markdown(&b.out, &args);
    println!("{table}");
    if let Some(p) = &args.json {
        std::fs::write(p, serde_json::to_string_pretty(&b.out)?)?;
    }
    if args.record {
        let hist = workspace_root().join("bench/history.md");
        let mut text = std::fs::read_to_string(&hist).unwrap_or_default();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push('\n');
        text.push_str(&table);
        std::fs::write(&hist, text)?;
        eprintln!("appended to {}", hist.display());
    }
    Ok(())
}

fn dir_len(dir: &Path) -> usize {
    std::fs::read_dir(dir).map(|r| r.count()).unwrap_or(0)
}

fn workspace_root() -> PathBuf {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::canonicalize(&p).unwrap_or(p)
}

// ───────────────────────── copies ─────────────────────────

struct Image {
    mount: PathBuf,
}

impl Image {
    fn new(dir: &Path, mb: u64) -> Option<Image> {
        let img = dir.join("bench.dmg");
        let mount = dir.join("mnt");
        std::fs::create_dir_all(&mount).ok()?;
        let ok = Command::new("hdiutil")
            .args([
                "create",
                "-quiet",
                "-megabytes",
                &mb.to_string(),
                "-fs",
                "APFS",
                "-volname",
                "FSBENCH",
            ])
            .arg(&img)
            .status()
            .ok()?
            .success()
            && Command::new("hdiutil")
                .args(["attach", "-quiet", "-nobrowse", "-noverify", "-mountpoint"])
                .arg(&mount)
                .arg(&img)
                .status()
                .ok()?
                .success();
        ok.then_some(Image { mount })
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

fn rm_rf(p: &Path) {
    // Benchmark scratch data only (our own temp folders and disk image).
    let _ = Command::new("rm").arg("-rf").arg(p).status();
}

fn time_cmd(args: &[&str], src: &Path, dest: &Path) -> Result<f64> {
    let t0 = Instant::now();
    let ok = Command::new(args[0])
        .args(&args[1..])
        .arg(src)
        .arg(dest)
        .status()?
        .success();
    if !ok {
        bail!("{args:?} failed");
    }
    Ok(ms(t0.elapsed()))
}

fn time_queue(b: &Bench, src: &Path, dest_dir: &Path) -> Result<f64> {
    std::fs::create_dir_all(dest_dir)?;
    let t0 = Instant::now();
    let id = b.engine.enqueue(OpRequest::Copy {
        sources: vec![src.to_path_buf()],
        dest_dir: dest_dir.to_path_buf(),
    })?;
    let st = b
        .engine
        .wait_op(id, Duration::from_secs(3600))
        .ok_or_else(|| anyhow::anyhow!("op vanished"))?;
    let el = ms(t0.elapsed());
    if st.state != OpState::Completed {
        bail!("copy ended {:?}: {:?}", st.state, st.errors);
    }
    Ok(el)
}

/// Runs `rounds` alternating rounds of the queue and a reference command
/// into fresh destinations under `dest_root`; returns (queue ms, ref ms).
fn compare(
    b: &Bench,
    src: &Path,
    dest_root: &Path,
    reference: &[&str],
    rounds: usize,
) -> Result<(Vec<f64>, Vec<f64>)> {
    let (mut q, mut r) = (Vec::new(), Vec::new());
    for i in 0..rounds {
        let dq = dest_root.join(format!("q{i}"));
        q.push(time_queue(b, src, &dq)?);
        rm_rf(&dq);
        let dr = dest_root.join(format!("r{i}"));
        std::fs::create_dir_all(&dr)?;
        r.push(time_cmd(reference, src, &dr)?);
        rm_rf(&dr);
    }
    Ok((q, r))
}

fn copies(b: &mut Bench, fx: &Fixtures, large: bool) -> Result<()> {
    let big = fx.big(1)?;
    let (small, small_bytes) = fx.small_files()?;
    let gib = (1u64 << 30) as f64;
    let rounds = 5;
    let same = b.tmp.join("copies");
    std::fs::create_dir_all(&same)?;

    // Same volume: the queue clones (copyfile CLONE); compare with `cp -c`.
    let (q, r) = compare(b, &big, &same, &["cp", "-c"], rounds)?;
    let (qm, _) = stats(q.clone());
    let (rm, _) = stats(r);
    b.push(
        "copy 1 GiB, same volume (clone) — queue",
        "ms",
        q,
        None,
        &format!("cp -c median {rm:.1} ms"),
    );
    let _ = qm;

    // 5,000 small files, same volume: queue (clones) vs `cp -R` (plain copy).
    let (q, r) = compare(b, &small, &same, &["cp", "-R"], 3)?;
    let (qm, _) = stats(q.clone());
    let (rm, _) = stats(r.clone());
    b.push(
        "copy 5,000 small files, same volume — queue",
        "ms",
        q,
        None,
        &format!("cp -R median {rm:.0} ms (cp -R does not clone)"),
    );
    let _ = qm;

    // Cross volume: a fresh APFS disk image.
    let mb = if large { 24_000 } else { 4_096 };
    let Some(img) = Image::new(&b.tmp, mb) else {
        eprintln!("hdiutil unavailable: skipping cross-volume copies");
        return Ok(());
    };
    let dest = img.mount.join("x");
    std::fs::create_dir_all(&dest)?;
    let (q, r) = compare(b, &big, &dest, &["cp"], rounds)?;
    let (qm, _) = stats(q.clone());
    let (rm, _) = stats(r);
    let qmbs: Vec<f64> = q.iter().map(|t| gib / (t / 1000.0) / 1e6).collect();
    b.push(
        "copy 1 GiB, cross volume (disk image) — queue",
        "MB/s",
        qmbs,
        None,
        &format!("cp: {:.0} MB/s", gib / (rm / 1000.0) / 1e6),
    );
    let ratio = qm / rm;
    b.out.push(Measure {
        name: "copy 1 GiB cross volume: queue time ÷ cp time".into(),
        unit: "×".into(),
        median: ratio,
        p95: ratio,
        samples: rounds,
        budget: Some(1.05),
        pass: Some(ratio <= 1.05),
        note: "medians; page cache not purged (no sudo), see F-015".into(),
    });
    eprintln!("cross-volume ratio {ratio:.3}");

    let (q, r) = compare(b, &small, &dest, &["cp", "-R"], 3)?;
    let (qm, _) = stats(q.clone());
    let (rm, _) = stats(r);
    b.push(
        "copy 5,000 small files, cross volume — queue",
        "ms",
        q,
        None,
        &format!("cp -R median {rm:.0} ms"),
    );
    let per_item_us = (qm - rm) / 5000.0 * 1000.0;
    b.out.push(Measure {
        name: "journal overhead per item (5,000 files, queue − cp -R)".into(),
        unit: "µs".into(),
        median: per_item_us,
        p95: per_item_us,
        samples: 3,
        budget: None,
        pass: None,
        note: format!(
            "{} bytes total; negative means the queue's parallel workers beat cp",
            small_bytes
        ),
    });
    eprintln!("journal overhead {per_item_us:.1} µs/item");

    // Verify on: our own loop with an F_NOCACHE re-read and xxh3.
    b.engine.set_setting("operations.verify_copies", "true")?;
    let mut v = Vec::new();
    for i in 0..3 {
        let d = dest.join(format!("v{i}"));
        let t = time_queue(b, &big, &d)?;
        v.push(gib / (t / 1000.0) / 1e6);
        rm_rf(&d);
    }
    b.push(
        "copy 1 GiB cross volume with verify — queue",
        "MB/s",
        v,
        None,
        "copy + xxh3 re-read",
    );
    b.engine.set_setting("operations.verify_copies", "false")?;

    if large {
        let big10 = fx.big(10)?;
        let (q, r) = compare(b, &big10, &dest, &["cp"], 1)?;
        let ratio = q[0] / r[0];
        b.out.push(Measure {
            name: "copy 10 GiB cross volume: queue time ÷ cp time".into(),
            unit: "×".into(),
            median: ratio,
            p95: ratio,
            samples: 1,
            budget: Some(1.05),
            pass: Some(ratio <= 1.05),
            note: format!(
                "queue {:.0} MB/s, cp {:.0} MB/s",
                10.0 * gib / q[0] * 1e-3,
                10.0 * gib / r[0] * 1e-3
            ),
        });
    }
    drop(img);
    Ok(())
}

fn markdown(out: &[Measure], args: &Args) -> String {
    let date = Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    let mut s = format!(
        "## {}: {} (fstudio-bench{}{})\n\nMeasured through the `fstudio-core` API on synthetic fixtures (warm cache). Latency budgets are checked against p95; for MB/s rows the p95 column is the slow tail (5th percentile).\n\n| Measure | median | p95 | budget | result | note |\n|---|---|---|---|---|---|\n",
        args.label,
        date.trim(),
        if args.quick { ", --quick" } else { "" },
        if args.large { ", --large" } else { "" },
    );
    for m in out {
        let f = |x: f64| {
            if m.unit == "×" {
                format!("{x:.3} ×")
            } else if x.abs() >= 100.0 {
                format!("{x:.0} {}", m.unit)
            } else if x.abs() >= 1.0 {
                format!("{x:.1} {}", m.unit)
            } else {
                format!("{x:.3} {}", m.unit)
            }
        };
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            m.name,
            f(m.median),
            f(m.p95),
            m.budget
                .map(|b| format!("≤ {}", f(b)))
                .unwrap_or_else(|| "–".into()),
            match m.pass {
                Some(true) => "PASS",
                Some(false) => "FAIL",
                None => "–",
            },
            m.note
        ));
    }
    s
}

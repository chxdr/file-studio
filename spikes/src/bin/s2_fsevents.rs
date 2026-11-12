//! Spike 2: FSEvents latency, notify comparison, relist vs incremental.
//! Usage: s2_fsevents <scratch-dir> [iters]
use fstudio_spikes::{fsevents::*, list::*, ms, sortkey, stats::summarize};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

fn drain<T>(rx: &Receiver<T>) { while rx.try_recv().is_ok() {} }

/// Wait for a batch containing an event whose path satisfies `pred`; returns callback instant.
fn wait_for(rx: &Receiver<Batch>, pred: impl Fn(&str, u32) -> bool) -> Option<Instant> {
    let deadline = Instant::now() + Duration::from_secs(3);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        if let Ok((t, evs)) = rx.recv_timeout(left) {
            if evs.iter().any(|(p, f)| pred(p, *f)) { return Some(t); }
        }
    }
    None
}

fn bench_stream(label: &str, dir: &Path, latency: f64, flags: u32, kind: &str, iters: usize) {
    let (tx, rx) = channel();
    let st = Stream::start(dir.to_str().unwrap(), latency, flags, tx);
    std::thread::sleep(Duration::from_millis(300));
    let file_level = flags & CF_FILE_EVENTS != 0;
    let mut v = vec![];
    let mut misses = 0;
    for i in 0..iters {
        drain(&rx);
        let name = if kind == "rename" { format!("probe_rename_{i}") } else { format!("probe_{kind}_{i}") };
        if kind == "delete" { std::fs::write(dir.join(format!("probe_delete_{i}")), b"d").unwrap(); }
        if kind == "rename" || kind == "delete" {
            if kind == "rename" { let a = dir.join(format!("rn_src_{i}")); std::fs::write(&a, b"r").unwrap(); }
            std::thread::sleep(Duration::from_millis(300 + (latency * 1000.0) as u64)); drain(&rx);
        }
        let t0 = Instant::now();
        match kind {
            "create" => { std::fs::File::create(dir.join(&name)).unwrap().write_all(&[1u8; 4096]).unwrap(); }
            "modify" => { std::fs::OpenOptions::new().append(true).open(dir.join("modtarget")).unwrap().write_all(b"x").unwrap(); }
            "rename" => { std::fs::rename(dir.join(format!("rn_src_{i}")), dir.join(&name)).unwrap(); }
            "delete" => { let _ = std::fs::remove_file(dir.join(format!("probe_delete_{i}"))); }
            _ => {}
        }
        let target = if kind == "modify" { "modtarget".to_string() } else if kind == "delete" { format!("probe_delete_{i}") } else { name.clone() };
        let d = dir.to_str().unwrap().to_string();
        let hit = wait_for(&rx, |p, _| if file_level { p.ends_with(&target) } else { p.trim_end_matches('/') == d.trim_end_matches('/') || p.starts_with(&d) });
        match hit { Some(t) => v.push((t - t0).as_secs_f64() * 1000.0), None => misses += 1 }
        std::thread::sleep(Duration::from_millis(150 + (latency * 2000.0) as u64));
    }
    drop(st);
    if v.is_empty() { println!("| {label} | {kind} | - | - | - | {misses} |"); return; }
    let s = summarize(&v);
    println!("| {label} | {kind} | {:.2} | {:.2} | {:.2} | {misses} |", s.min, s.median, s.p95);
}

fn bench_notify(dir: &Path, kind: &str, iters: usize) {
    use notify::{RecursiveMode, Watcher};
    let (tx, rx) = channel::<(Instant, Vec<PathBuf>)>();
    let mut w = notify::recommended_watcher(move |r: notify::Result<notify::Event>| {
        let t = Instant::now();
        if let Ok(e) = r { let _ = tx.send((t, e.paths)); }
    }).unwrap();
    w.watch(dir, RecursiveMode::NonRecursive).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let mut v = vec![];
    for i in 0..iters {
        drain(&rx);
        let name = format!("probe_{kind}_{i}");
        let t0 = Instant::now();
        match kind {
            "create" => { std::fs::File::create(dir.join(&name)).unwrap().write_all(&[1u8; 4096]).unwrap(); }
            _ => { std::fs::OpenOptions::new().append(true).open(dir.join("modtarget")).unwrap().write_all(b"x").unwrap(); }
        }
        let target = if kind == "create" { name } else { "modtarget".into() };
        let deadline = Instant::now() + Duration::from_secs(3);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            if let Ok((t, ps)) = rx.recv_timeout(left) {
                if ps.iter().any(|p| p.ends_with(&target)) { v.push((t - t0).as_secs_f64() * 1000.0); break; }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let s = summarize(&v);
    println!("| notify 8 recommended_watcher (fsevent backend) | {kind} | {:.2} | {:.2} | {:.2} | {} |", s.min, s.median, s.p95, iters - v.len());
}

/// Sorted in-memory model for the incremental-update comparison.
struct Model { rows: Vec<(String, Entry)> }
impl Model {
    fn build(es: Vec<Entry>) -> Model {
        let mut rows: Vec<(String, Entry)> = es.into_iter().map(|e| (sortkey::fold(&e.name), e)).collect();
        rows.sort_unstable_by(|a, b| sortkey::natural_cmp(&a.0, &b.0));
        Model { rows }
    }
    fn apply(&mut self, dirfd: i32, name: &[u8]) {
        let key = sortkey::fold(name);
        let pos = self.rows.binary_search_by(|r| sortkey::natural_cmp(&r.0, &key));
        match (lstat_at(dirfd, name), pos) {
            (Some(st), Ok(i)) => self.rows[i].1.size = st.st_size as u64,
            (Some(st), Err(i)) => self.rows.insert(i, (key, Entry { name: name.to_vec(), kind: K_REG, size: st.st_size as u64, mtime_s: st.st_mtime, mtime_ns: st.st_mtime_nsec, flags: st.st_flags })),
            (None, Ok(i)) => { self.rows.remove(i); }
            (None, Err(_)) => {}
        }
    }
}

fn relist_vs_incremental(dir: &Path, iters: usize) {
    let (tx, rx) = channel();
    let st = Stream::start(dir.to_str().unwrap(), 0.0, CF_FILE_EVENTS | CF_NO_DEFER, tx);
    std::thread::sleep(Duration::from_millis(300));
    let f = std::fs::File::open(dir).unwrap();
    let fd = f.as_raw_fd();
    let mut model = Model::build(list_bulk(dir, &mut || {}));
    let (mut ev, mut full_apply, mut inc_apply) = (vec![], vec![], vec![]);
    for i in 0..iters {
        drain(&rx);
        let name = format!("incr_{i}");
        let t0 = Instant::now();
        std::fs::write(dir.join(&name), b"hello").unwrap();
        let Some(t) = wait_for(&rx, |p, _| p.ends_with(&name)) else { continue };
        ev.push((t - t0).as_secs_f64() * 1000.0);
        // (a) incremental
        let t1 = Instant::now();
        model.apply(fd, name.as_bytes());
        inc_apply.push(ms(t1));
        // (b) full relist + resort (what the same event would cost without incremental updates)
        let t2 = Instant::now();
        let m2 = Model::build(list_bulk(dir, &mut || {}));
        full_apply.push(ms(t2));
        assert_eq!(m2.rows.len(), model.rows.len());
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(st);
    let (e, a, b) = (summarize(&ev), summarize(&inc_apply), summarize(&full_apply));
    println!("| write->event (FileEvents|NoDefer, latency 0) | {:.2} | {:.2} |", e.median, e.p95);
    println!("| incremental apply (lstat + binary-search insert) | {:.4} | {:.4} |", a.median, a.p95);
    println!("| full relist (getattrlistbulk) + re-sort, {} entries | {:.2} | {:.2} |", model.rows.len(), b.median, b.p95);
    println!("| end-to-end incremental (event + apply) | {:.2} | {:.2} |", e.median + a.median, e.p95 + a.p95);
    println!("| end-to-end full relist (event + relist) | {:.2} | {:.2} |", e.median + b.median, e.p95 + b.p95);
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let base = PathBuf::from(&a[1]);
    let iters: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(30);
    let dir = base.canonicalize().unwrap().join("fse");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("modtarget"), b"x").unwrap();

    println!("| config | op | min ms | median ms | p95 ms | missed |");
    println!("|---|---|---|---|---|---|");
    let kinds = std::env::var("KINDS").unwrap_or("create,modify,rename,delete".into());
    for kind in kinds.split(',') {
        let cfgs: Vec<(String, f64, u32)> = vec![
            ("dir-level, latency 0.0".into(), 0.0, 0),
            ("dir-level, latency 0.05".into(), 0.05, 0),
            ("dir-level, latency 0.5".into(), 0.5, 0),
            ("FileEvents, latency 0.0".into(), 0.0, CF_FILE_EVENTS),
            ("FileEvents, latency 0.01".into(), 0.01, CF_FILE_EVENTS),
            ("FileEvents, latency 0.05".into(), 0.05, CF_FILE_EVENTS),
            ("FileEvents, latency 0.1".into(), 0.1, CF_FILE_EVENTS),
            ("FileEvents, latency 0.5".into(), 0.5, CF_FILE_EVENTS),
            ("FileEvents|NoDefer, latency 0.05".into(), 0.05, CF_FILE_EVENTS | CF_NO_DEFER),
            ("FileEvents|NoDefer, latency 0.5".into(), 0.5, CF_FILE_EVENTS | CF_NO_DEFER),
        ];
        for (l, lat, fl) in cfgs { bench_stream(&l, &dir, lat, fl, kind, iters); }
    }
    if kinds.contains("create") {
        bench_notify(&dir, "create", iters);
        bench_notify(&dir, "modify", iters);
    } else { return; }

    // relist vs incremental on a 10k folder
    let big = base.canonicalize().unwrap().join("fse10k");
    let _ = std::fs::remove_dir_all(&big);
    fstudio_spikes::gen::make_dir(&big, 10_000).unwrap();
    println!("\n| step (10k-entry folder, single-file create) | median ms | p95 ms |\n|---|---|---|");
    relist_vs_incremental(&big, iters);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&big);
}

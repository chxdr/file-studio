//! Spike 3: copy throughput, metadata preservation, verify cost, copyfile callback pause/cancel.
use fstudio_spikes::{ms, stats::summarize, sys::*};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::*};
use std::time::{Duration, Instant};

fn lcg(s: &mut u64) -> u64 { *s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); *s }

fn gen_big(path: &Path, bytes: u64) {
    let mut f = File::create(path).unwrap();
    let mut block = vec![0u8; 1 << 20];
    let mut s = 7u64;
    let mut left = bytes;
    while left > 0 {
        for c in block.chunks_mut(8) { c.copy_from_slice(&lcg(&mut s).to_le_bytes()[..c.len()]); }
        let n = left.min(block.len() as u64) as usize;
        f.write_all(&block[..n]).unwrap();
        left -= n as u64;
    }
    f.sync_all().unwrap();
}

fn gen_tree(dir: &Path, n: usize) {
    fs::create_dir_all(dir).unwrap();
    let mut s = 99u64;
    for i in 0..n {
        let sub = dir.join(format!("sub{}", i % 50));
        fs::create_dir_all(&sub).unwrap();
        let len = 1024 + (lcg(&mut s) >> 33) % (64 * 1024 - 1024);
        let data: Vec<u8> = (0..len).map(|_| (lcg(&mut s) >> 40) as u8).collect();
        fs::write(sub.join(format!("f{i}.dat")), data).unwrap();
    }
}

/// Manual read/write loop. `sparse`: honour holes via SEEK_DATA/SEEK_HOLE.
fn rw_copy(src: &Path, dst: &Path, buf_kb: usize, nocache_src: bool, nocache_dst: bool, sparse: bool) -> std::io::Result<u64> {
    let mut r = File::open(src)?;
    let mut w = OpenOptions::new().write(true).create_new(true).mode(0o644).open(dst)?;
    if nocache_src { set_nocache(r.as_raw_fd()); }
    if nocache_dst { set_nocache(w.as_raw_fd()); }
    let mut buf = vec![0u8; buf_kb * 1024];
    let len = r.metadata()?.len();
    let mut total = 0u64;
    if !sparse {
        loop {
            let n = r.read(&mut buf)?;
            if n == 0 { break; }
            w.write_all(&buf[..n])?;
            total += n as u64;
        }
    } else {
        let fd = r.as_raw_fd();
        let mut pos: i64 = 0;
        loop {
            // SAFETY: lseek on a valid fd; ENXIO means no more data after `pos`.
            let data = unsafe { libc::lseek(fd, pos, libc::SEEK_DATA) };
            if data < 0 { break; }
            let hole = unsafe { libc::lseek(fd, data, libc::SEEK_HOLE) };
            let mut off = data as u64;
            while off < hole as u64 {
                let want = buf.len().min((hole as u64 - off) as usize);
                let n = r.read_at(&mut buf[..want], off)?;
                if n == 0 { break; }
                w.write_all_at(&buf[..n], off)?;
                off += n as u64; total += n as u64;
            }
            pos = hole;
        }
        w.set_len(len)?;
    }
    Ok(total)
}

fn run_cmd(prog: &str, args: &[&Path], pre: &[&str]) -> std::io::Result<()> {
    let st = Command::new(prog).args(pre).args(args).status()?;
    if st.success() { Ok(()) } else { Err(std::io::Error::other(format!("{prog} failed"))) }
}

fn copy_one(method: &str, src: &Path, dst: &Path) -> std::io::Result<()> {
    let p: Vec<&str> = method.split(':').collect();
    match p[0] {
        "copyfile_all" => copyfile_path(src, dst, COPYFILE_ALL | COPYFILE_EXCL),
        "copyfile_all_clone" => copyfile_path(src, dst, COPYFILE_ALL | COPYFILE_CLONE | COPYFILE_EXCL),
        "copyfile_data" => copyfile_path(src, dst, COPYFILE_DATA | COPYFILE_EXCL),
        "copyfile_stat_xattr_acl" => copyfile_path(src, dst, COPYFILE_METADATA),
        "copyfile_all_sparse" => copyfile_path(src, dst, COPYFILE_ALL | COPYFILE_DATA_SPARSE | COPYFILE_EXCL),
        "clonefile" => clonefile_path(src, dst),
        // rw:<KiB>:<nc: n|s|d|b>:<sparse 0|1>:<meta 0|1>
        "rw" => {
            let kb: usize = p[1].parse().unwrap();
            let nc = p.get(2).copied().unwrap_or("n");
            let sparse = p.get(3) == Some(&"1");
            rw_copy(src, dst, kb, nc == "s" || nc == "b", nc == "d" || nc == "b", sparse)?;
            if p.get(4) == Some(&"1") { copyfile_path(src, dst, COPYFILE_METADATA)?; }
            if p.get(5) == Some(&"1") {
                let f = File::open(dst)?;
                // SAFETY: F_FULLFSYNC on a valid fd: flush through the drive's write cache (plain fsync does not on macOS).
                unsafe { libc::fcntl(f.as_raw_fd(), libc::F_FULLFSYNC); }
            }
            Ok(())
        }
        "cp" => run_cmd("/bin/cp", &[src, dst], &[]),
        "cp-c" => run_cmd("/bin/cp", &[src, dst], &["-c"]),
        "cp-p" => run_cmd("/bin/cp", &[src, dst], &["-p"]),
        "ditto" => run_cmd("/usr/bin/ditto", &[src, dst], &[]),
        "ditto-rsrc" => run_cmd("/usr/bin/ditto", &[src, dst], &["--rsrc"]),
        m => panic!("unknown method {m}"),
    }
}

fn bench(method: &str, src: &Path, dstdir: &Path, runs: usize) {
    let bytes = fs::metadata(src).unwrap().len();
    let dst = dstdir.join("bench_out");
    let mut t = vec![];
    for _ in 0..runs {
        let _ = fs::remove_file(&dst);
        let s = Instant::now();
        if let Err(e) = copy_one(method, src, &dst) { println!("| {method} | n/a ({e}) |"); return; }
        t.push(ms(s));
        assert_eq!(fs::metadata(&dst).unwrap().len(), bytes);
        fs::remove_file(&dst).unwrap();
    }
    let s = summarize(&t);
    println!("| {method} | {:.0} | {:.0} | {:.0} | {:.0} |", s.median, s.p95, bytes as f64 / 1e6 / (s.median / 1000.0), bytes as f64 / 1e6 / (s.p95 / 1000.0));
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() { walk(&p, out); } else { out.push(p); }
    }
}

fn tree(method: &str, src: &Path, dstparent: &Path, runs: usize) {
    use rayon::prelude::*;
    let dst = dstparent.join("tree_out");
    let mut t = vec![];
    for _ in 0..runs {
        let _ = fs::remove_dir_all(&dst);
        let s = Instant::now();
        let per_file = |f: &PathBuf| {
            let rel = f.strip_prefix(src).unwrap();
            let d = dst.join(rel);
            copyfile_path(f, &d, COPYFILE_ALL | if method.contains("clone") { COPYFILE_CLONE } else { 0 }).unwrap();
        };
        match method {
            "copyfile_recursive" => copyfile_path(src, &dst, COPYFILE_ALL | COPYFILE_RECURSIVE).unwrap(),
            "clonefile_dir" => clonefile_path(src, &dst).unwrap(),
            "cp-R" => run_cmd("/bin/cp", &[src, &dst], &["-R"]).unwrap(),
            "cp-Rc" => run_cmd("/bin/cp", &[src, &dst], &["-Rc"]).unwrap(),
            "ditto" => run_cmd("/usr/bin/ditto", &[src, &dst], &[]).unwrap(),
            m if m.starts_with("perfile") || m.starts_with("par") => {
                let mut files = vec![]; walk(src, &mut files);
                fs::create_dir_all(&dst).unwrap();
                let mut dirs: Vec<_> = files.iter().map(|f| dst.join(f.strip_prefix(src).unwrap()).parent().unwrap().to_path_buf()).collect();
                dirs.sort(); dirs.dedup();
                for d in dirs { fs::create_dir_all(d).unwrap(); }
                if m.starts_with("perfile") { files.iter().for_each(per_file); }
                else {
                    let n: usize = m.split('-').nth(1).and_then(|x| x.split(':').next()).and_then(|x| x.parse().ok()).unwrap_or(4);
                    let pool = rayon::ThreadPoolBuilder::new().num_threads(n).build().unwrap();
                    pool.install(|| files.par_iter().for_each(per_file));
                }
            }
            m => panic!("unknown tree method {m}"),
        }
        t.push(ms(s));
        fs::remove_dir_all(&dst).unwrap();
    }
    let s = summarize(&t);
    println!("| {method} | {:.0} | {:.0} |", s.median, s.p95);
}

fn read_pass(path: &Path, buf_kb: usize, nocache: bool, hash: bool) -> (f64, u64) {
    use xxhash_rust::xxh3::Xxh3;
    let mut f = File::open(path).unwrap();
    if nocache { set_nocache(f.as_raw_fd()); }
    let mut buf = vec![0u8; buf_kb * 1024];
    let mut h = Xxh3::new();
    let mut total = 0u64;
    let t = Instant::now();
    loop {
        let n = f.read(&mut buf).unwrap();
        if n == 0 { break; }
        if hash { h.update(&buf[..n]); }
        total += n as u64;
    }
    std::hint::black_box(h.digest());
    (total as f64 / 1e6 / t.elapsed().as_secs_f64(), total)
}

// ---------- copyfile progress callback: pause / cancel ----------
struct Ctl { paused: AtomicBool, cancel: AtomicBool, copied: AtomicU64, calls: AtomicU64, max_gap: AtomicU64 }

extern "C" fn progress_cb(what: i32, stage: i32, state: CopyfileState, _s: *const libc::c_char, _d: *const libc::c_char, ctx: *mut std::ffi::c_void) -> i32 {
    // SAFETY: `ctx` is the &Ctl we registered via COPYFILE_STATE_STATUS_CTX and it outlives the copy.
    let c = unsafe { &*(ctx as *const Ctl) };
    if what == CB_COPY_DATA && stage == CB_PROGRESS {
        let mut copied: i64 = 0;
        // SAFETY: `state` is the live copyfile state passed to us; COPIED writes an off_t.
        unsafe { copyfile_state_get(state, ST_COPIED, &mut copied as *mut i64 as *mut _) };
        let prev = c.copied.swap(copied as u64, SeqCst);
        c.max_gap.fetch_max(copied as u64 - prev.min(copied as u64), SeqCst);
        c.calls.fetch_add(1, SeqCst);
        // Pause = block the copying thread right here. Cancel = ask copyfile to quit.
        while c.paused.load(SeqCst) && !c.cancel.load(SeqCst) { std::thread::sleep(Duration::from_millis(2)); }
        if c.cancel.load(SeqCst) { return CB_QUIT; }
    }
    CB_CONTINUE
}

fn cbdemo(src: &Path, dst: &Path) {
    let size = fs::metadata(src).unwrap().len();
    for scenario in ["pause-resume", "cancel"] {
        let _ = fs::remove_file(dst);
        let ctl = Ctl { paused: false.into(), cancel: false.into(), copied: 0.into(), calls: 0.into(), max_gap: 0.into() };
        let (a, b) = (cstr(src), cstr(dst));
        let t0 = Instant::now();
        std::thread::scope(|sc| {
            let h = sc.spawn(|| {
                // SAFETY: state alloc/set/free are paired; the callback pointer and ctx (&ctl) outlive
                // the copyfile call because the scope joins before `ctl` is dropped.
                unsafe {
                    let st = copyfile_state_alloc();
                    copyfile_state_set(st, ST_STATUS_CB, progress_cb as *const _);
                    copyfile_state_set(st, ST_STATUS_CTX, &ctl as *const Ctl as *const _);
                    let r = copyfile(a.as_ptr(), b.as_ptr(), st, COPYFILE_ALL | COPYFILE_EXCL);
                    let err = std::io::Error::last_os_error();
                    copyfile_state_free(st);
                    (r, err)
                }
            });
            while ctl.copied.load(SeqCst) < size / 5 { std::thread::sleep(Duration::from_millis(1)); }
            if scenario == "pause-resume" {
                ctl.paused.store(true, SeqCst);
                std::thread::sleep(Duration::from_millis(50));
                let before = ctl.copied.load(SeqCst);
                std::thread::sleep(Duration::from_millis(500));
                let after = ctl.copied.load(SeqCst);
                println!("pause: copied {before} -> {after} over 500 ms while paused (delta {})", after - before);
                ctl.paused.store(false, SeqCst);
                let (r, _) = h.join().unwrap();
                println!("resume: copyfile returned {r}, total {:.0} ms, dst len {} / {size}, callbacks {}, max bytes between callbacks {}", ms(t0), fs::metadata(dst).map(|m| m.len()).unwrap_or(0), ctl.calls.load(SeqCst), ctl.max_gap.load(SeqCst));
            } else {
                while ctl.copied.load(SeqCst) < size / 2 { std::thread::sleep(Duration::from_millis(1)); }
                let tc = Instant::now();
                ctl.cancel.store(true, SeqCst);
                let (r, e) = h.join().unwrap();
                println!("cancel: copyfile returned {r} ({e}) {:.1} ms after cancel request; copied at cancel ~{} of {size}; dst exists: {} (len {})", ms(tc), ctl.copied.load(SeqCst), dst.exists(), fs::metadata(dst).map(|m| m.len()).unwrap_or(0));
            }
        });
        let _ = fs::remove_file(dst);
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let p = |i: usize| PathBuf::from(&a[i]);
    match a[1].as_str() {
        "gen-big" => gen_big(&p(2), a[3].parse().unwrap()),
        "gen-tree" => gen_tree(&p(2), a[3].parse().unwrap()),
        "one" => copy_one(&a[2], &p(3), &p(4)).unwrap(),
        "bench" => bench(&a[2], &p(3), &p(4), a[5].parse().unwrap()),
        "tree" => tree(&a[2], &p(3), &p(4), a[5].parse().unwrap()),
        "verify" => {
            let (kb, nc): (usize, bool) = (a[3].parse().unwrap(), a[4] == "1");
            let (r, _) = read_pass(&p(2), kb, nc, false);
            let (h, _) = read_pass(&p(2), kb, nc, true);
            println!("| {} KiB | nocache={nc} | read only {:.0} MB/s | read+xxh3 {:.0} MB/s |", kb, r, h);
        }
        "cbdemo" => cbdemo(&p(2), &p(3)),
        _ => eprintln!("see source"),
    }
}

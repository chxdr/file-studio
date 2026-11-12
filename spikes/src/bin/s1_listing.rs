//! Spike 1: directory listing speed. Usage:
//!   s1_listing gen <dir> <n>
//!   s1_listing run <dir> [runs] [method ...]
use fstudio_spikes::{list::*, ms, sortkey, stats::summarize};
use std::path::Path;
use std::time::Instant;

type ListFn = fn(&Path, &mut dyn FnMut()) -> Vec<Entry>;

fn cf_sort(entries: &[Entry]) -> f64 {
    use core_foundation_sys::base::*;
    use core_foundation_sys::locale::*;
    use core_foundation_sys::string::*;
    let t = Instant::now();
    // SAFETY: CF objects are created/released in matched pairs; the byte pointers are valid for
    // the length passed; the comparator only reads the CFStrings which outlive the sort.
    unsafe {
        let loc = CFLocaleCopyCurrent();
        let strs: Vec<CFStringRef> = entries
            .iter()
            .map(|e| CFStringCreateWithBytes(std::ptr::null(), e.name.as_ptr(), e.name.len() as CFIndex, kCFStringEncodingUTF8, 0))
            .collect();
        let mut idx: Vec<usize> = (0..strs.len()).collect();
        let flags = kCFCompareCaseInsensitive | kCFCompareNonliteral | kCFCompareLocalized | kCFCompareNumerically;
        idx.sort_unstable_by(|&a, &b| {
            let r = CFStringCompareWithOptionsAndLocale(strs[a], strs[b], CFRange { location: 0, length: CFStringGetLength(strs[a]) }, flags, loc);
            (r as i32).cmp(&0)
        });
        for s in strs { CFRelease(s as _); }
        CFRelease(loc as _);
        std::hint::black_box(&idx);
    }
    ms(t)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("gen") => fstudio_spikes::gen::make_dir(Path::new(&a[2]), a[3].parse().unwrap()).unwrap(),
        Some("run") => {
            let dir = Path::new(&a[2]);
            let runs: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(7);
            let all: Vec<(&str, ListFn)> = vec![
                ("std read_dir+symlink_metadata", list_std),
                ("std read_dir+DirEntry::metadata", list_std_direntry),
                ("getattrlistbulk 256K", list_bulk),
                ("getattrlistbulk adaptive 16K+256K", list_bulk_adaptive),
                ("fts (PHYSICAL|NOCHDIR)", list_fts),
                ("readdir+rayon fstatat", list_rayon),
            ];
            let mut check: Option<(usize, u64)> = None;
            let want: Vec<&String> = a.iter().skip(4).collect();
            println!("| method | n | first100 ms med/p95 | all+meta ms med/p95 | sort name ms | sort size ms | sort date ms | par sort name ms |");
            println!("|---|---|---|---|---|---|---|---|");
            for (name, f) in all {
                if !want.is_empty() && !want.iter().any(|w| name.starts_with(w.as_str())) { continue; }
                let (mut t100, mut tall, mut sn, mut ss, mut sd, mut sp) = (vec![], vec![], vec![], vec![], vec![], vec![]);
                let mut n = 0;
                for _ in 0..runs {
                    let t = Instant::now();
                    let mut f100 = 0.0;
                    let es = f(dir, &mut || f100 = ms(t));
                    tall.push(ms(t));
                    t100.push(f100);
                    n = es.len();
                    let chk: u64 = es.iter().filter(|e| e.kind == K_REG).map(|e| e.size).sum();
                    if let Some(c) = check { assert_eq!(c, (n, chk), "{name} disagrees"); } else { check = Some((n, chk)); }
                    // sorts (independent of listing method; measured on each result)
                    let t = Instant::now();
                    let mut keyed: Vec<(String, usize)> = es.iter().enumerate().map(|(i, e)| (sortkey::fold(&e.name), i)).collect();
                    keyed.sort_unstable_by(|x, y| sortkey::natural_cmp(&x.0, &y.0));
                    sn.push(ms(t));
                    let t = Instant::now();
                    let mut keyed: Vec<(String, usize)> = es.iter().enumerate().map(|(i, e)| (sortkey::fold(&e.name), i)).collect();
                    {
                        use rayon::prelude::*;
                        keyed.par_sort_unstable_by(|x, y| sortkey::natural_cmp(&x.0, &y.0));
                    }
                    sp.push(ms(t));
                    let t = Instant::now();
                    let mut ix: Vec<usize> = (0..es.len()).collect();
                    ix.sort_unstable_by_key(|&i| es[i].size);
                    ss.push(ms(t));
                    let t = Instant::now();
                    let mut ix: Vec<usize> = (0..es.len()).collect();
                    ix.sort_unstable_by_key(|&i| (es[i].mtime_s, es[i].mtime_ns));
                    sd.push(ms(t));
                }
                let m = |v: &Vec<f64>| summarize(v).median;
                println!("| {name} | {n} | {} | {} | {:.2} | {:.2} | {:.2} | {:.2} |", summarize(&t100), summarize(&tall), m(&sn), m(&ss), m(&sd), m(&sp));
            }
            // Finder-parity comparator cost (CFStringCompare, localized+numeric+caseless)
            let es = list_bulk(dir, &mut || {});
            let v: Vec<f64> = (0..3).map(|_| cf_sort(&es)).collect();
            println!("\nCFStringCompareWithOptionsAndLocale name sort (incl. CFString creation), n={}: median {:.1} ms", es.len(), summarize(&v).median);
        }
        _ => eprintln!("usage: gen <dir> <n> | run <dir> [runs] [method prefix...]"),
    }
}

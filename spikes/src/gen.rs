//! Deterministic synthetic folder generator (mixed case, digits, NFC/NFD names, varied sizes).
use std::fs::{self, OpenOptions};
use std::path::Path;

fn lcg(s: &mut u64) -> u64 {
    *s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    *s >> 33
}

pub fn make_dir(dir: &Path, n: usize) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let mut seed = 42u64;
    for i in 0..n {
        let r = lcg(&mut seed);
        let name = match i % 6 {
            0 => format!("Report {i}.txt"),
            1 => format!("IMG_{i:05}.JPG"),
            2 => format!("r\u{e9}sum\u{e9} {i}.pdf"),             // NFC
            3 => format!("re\u{301}sume\u{301} {i}.pdf"),          // NFD
            4 => format!("file{i}"),
            _ => format!("Zebra-{}-{i}.md", r % 100),
        };
        if i % 50 == 49 {
            fs::create_dir(dir.join(format!("Folder {i}")))?;
            continue;
        }
        let f = OpenOptions::new().write(true).create_new(true).open(dir.join(name))?;
        f.set_len(r % 5_000_000)?; // sparse; varied sizes
    }
    Ok(())
}

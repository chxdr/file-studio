//! Synthetic fixtures, generated once into a cache folder and reused.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;

/// Bumped when a fixture's contents change, so stale caches are rebuilt.
const FIXTURE_VERSION: u32 = 1;

pub struct Fixtures {
    pub root: PathBuf,
}

fn done(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join(".fstudio-bench-ok"))
        .map(|s| s.trim() == FIXTURE_VERSION.to_string())
        .unwrap_or(false)
}

fn mark(dir: &Path) -> Result<()> {
    std::fs::write(dir.join(".fstudio-bench-ok"), FIXTURE_VERSION.to_string())?;
    Ok(())
}

fn fresh(dir: &Path) -> Result<bool> {
    if done(dir) {
        return Ok(false);
    }
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    std::fs::create_dir_all(dir)?;
    Ok(true)
}

/// Deterministic pseudo-random bytes (xorshift).
fn fill(buf: &mut [u8], seed: u64) {
    let mut x = seed | 1;
    for c in buf.chunks_mut(8) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        c.copy_from_slice(&x.to_le_bytes()[..c.len()]);
    }
}

impl Fixtures {
    pub fn new(root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&root)?;
        Ok(Self {
            root: std::fs::canonicalize(&root)?,
        })
    }

    /// A flat folder of `n` entries: mostly files with mixed extensions and
    /// numbers (natural sort), a few folders, some hidden files.
    pub fn flat(&self, n: usize) -> Result<PathBuf> {
        let dir = self.root.join(format!("flat_{n}"));
        if fresh(&dir)? {
            eprintln!("generating {}", dir.display());
            let exts = ["txt", "jpg", "png", "pdf", "rs", "md", "mov", "zip"];
            for i in 0..n {
                let p = match i % 50 {
                    0 => dir.join(format!("Folder {i}")),
                    1 => dir.join(format!(".hidden_{i}")),
                    _ => dir.join(format!("file_{i}.{}", exts[i % exts.len()])),
                };
                if i % 50 == 0 {
                    std::fs::create_dir(&p)?;
                } else {
                    std::fs::write(&p, &[b'x'; 16][..(i % 16)])?;
                }
            }
            mark(&dir)?;
        }
        Ok(dir)
    }

    /// A tree 32 levels deep with a few files per level.
    pub fn deep(&self) -> Result<PathBuf> {
        let dir = self.root.join("deep");
        if fresh(&dir)? {
            let mut cur = dir.clone();
            for level in 0..32 {
                cur = cur.join(format!("level {level}"));
                std::fs::create_dir(&cur)?;
                for f in 0..4 {
                    std::fs::write(cur.join(format!("f{f}.txt")), b"deep")?;
                }
            }
            mark(&dir)?;
        }
        Ok(dir)
    }

    /// NFC/NFD and case twins.
    pub fn unicode(&self) -> Result<PathBuf> {
        let dir = self.root.join("unicode");
        if fresh(&dir)? {
            for n in [
                "Caf\u{e9}.txt",
                "Cafe\u{301} 2.txt",
                "\u{c5}ngstr\u{f6}m",
                "A\u{30a}ngstro\u{308}m 2",
                "Report.TXT",
                "report 2.txt",
                "\u{1f4c1} emoji folder",
                "\u{65e5}\u{672c}\u{8a9e}.md",
            ] {
                std::fs::write(dir.join(n), n.as_bytes())?;
            }
            mark(&dir)?;
        }
        Ok(dir)
    }

    /// A 1 GiB sparse file with a little data at each end.
    pub fn sparse(&self) -> Result<PathBuf> {
        let dir = self.root.join("sparse");
        if fresh(&dir)? {
            let mut f = std::fs::File::create(dir.join("sparse.img"))?;
            f.write_all(b"head")?;
            f.set_len(1 << 30)?;
            mark(&dir)?;
        }
        Ok(dir)
    }

    /// 5,000 small files (1–64 KiB) in 50 folders.
    pub fn small_files(&self) -> Result<(PathBuf, u64)> {
        let dir = self.root.join("small_5000");
        let mut total = 0;
        let mut buf = vec![0u8; 64 * 1024];
        fill(&mut buf, 7);
        let make = fresh(&dir)?;
        for i in 0..5000u64 {
            let size = 1024 + (i * 7919) % (63 * 1024);
            total += size;
            if make {
                let p = dir.join(format!("d{:02}/f{i:04}.bin", i % 50));
                std::fs::create_dir_all(p.parent().expect("has parent"))?;
                std::fs::write(&p, &buf[..size as usize])?;
            }
        }
        if make {
            mark(&dir)?;
        }
        Ok((dir, total))
    }

    /// One file of `gib` GiB of incompressible data.
    pub fn big(&self, gib: u64) -> Result<PathBuf> {
        let dir = self.root.join(format!("big_{gib}g"));
        if fresh(&dir)? {
            eprintln!("generating a {gib} GiB file");
            let mut f = std::io::BufWriter::with_capacity(
                4 << 20,
                std::fs::File::create(dir.join(format!("big_{gib}g.bin")))?,
            );
            let mut buf = vec![0u8; 4 << 20];
            for chunk in 0..(gib * 256) {
                fill(&mut buf, chunk + 1);
                f.write_all(&buf)?;
            }
            f.flush()?;
            mark(&dir)?;
        }
        Ok(dir.join(format!("big_{gib}g.bin")))
    }
}

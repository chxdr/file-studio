//! One-time import from zoxide.
//!
//! zoxide stores `db.zo` as: a little-endian `u32` version (currently 3), then a
//! bincode (fixed-int) list: `u64` count, and per entry `u64` path length, the UTF-8
//! path, an `f64` rank and a `u64` last-accessed time in seconds. That is simple
//! enough to read directly, so [`parse_db`] does (verified against zoxide 0.9.9's
//! source). If a future version changes the format, the import fails with a clear
//! error and the caller can fall back to [`parse_query_ls`], which reads the output
//! of `zoxide query -ls`.

use std::path::Path;

use crate::error::PathError;
use crate::frecency::Frecency;

/// The `db.zo` version this reader understands.
pub const SUPPORTED_VERSION: u32 = 3;

/// One imported folder.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoxideEntry {
    /// Absolute path.
    pub path: String,
    /// Rank (or score, for `query -ls` output).
    pub rank: f64,
    /// Seconds since the epoch.
    pub last_accessed: u64,
}

/// Parses the bytes of a zoxide `db.zo` (version 3).
pub fn parse_db(bytes: &[u8]) -> Result<Vec<ZoxideEntry>, PathError> {
    let bad = |m: &str| PathError::Zoxide(m.to_owned());
    let mut r = Reader { b: bytes, i: 0 };
    let version = r.u32().ok_or_else(|| bad("file too short"))?;
    if version != SUPPORTED_VERSION {
        return Err(PathError::Zoxide(format!(
            "unsupported db.zo version {version} (supported: {SUPPORTED_VERSION}); \
             use `zoxide query -ls` output instead"
        )));
    }
    let count = r.u64().ok_or_else(|| bad("truncated entry count"))? as usize;
    // Each entry is at least 24 bytes: this stops a corrupt count from allocating.
    if count > bytes.len() / 24 + 1 {
        return Err(bad("entry count does not fit the file"));
    }
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let len = r.u64().ok_or_else(|| bad("truncated path length"))? as usize;
        let raw = r.take(len).ok_or_else(|| bad("truncated path"))?;
        let path = String::from_utf8_lossy(raw).into_owned();
        let rank = f64::from_bits(r.u64().ok_or_else(|| bad("truncated rank"))?);
        let last_accessed = r.u64().ok_or_else(|| bad("truncated time"))?;
        out.push(ZoxideEntry {
            path,
            rank,
            last_accessed,
        });
    }
    Ok(out)
}

/// Parses `zoxide query -ls` output (lines like `  12.5 /Users/me/code`). The score
/// becomes the rank and the time is `now`, because the output has no timestamps.
pub fn parse_query_ls(text: &str, now: u64) -> Vec<ZoxideEntry> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (score, path) = line.split_once(char::is_whitespace)?;
            let rank: f64 = score.parse().ok()?;
            let path = path.trim_start();
            path.starts_with('/').then(|| ZoxideEntry {
                path: path.to_owned(),
                rank,
                last_accessed: now,
            })
        })
        .collect()
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.i.checked_add(n)?;
        let s = self.b.get(self.i..end)?;
        self.i = end;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
}

/// What an import did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportReport {
    /// Entries added or merged.
    pub imported: usize,
    /// Entries skipped (excluded, relative or empty paths).
    pub skipped: usize,
}

impl Frecency {
    /// Imports zoxide's `db.zo` once: ranks are added to existing entries, the later
    /// last-access time wins, and the result is aged if over the cap. Default
    /// location: `~/Library/Application Support/zoxide/db.zo` (or `$_ZO_DATA_DIR`).
    pub fn import_zoxide(&self, db_path: &Path) -> Result<ImportReport, PathError> {
        let bytes = std::fs::read(db_path)?;
        Ok(self.import_entries(parse_db(&bytes)?))
    }

    /// Imports parsed zoxide entries (from [`parse_db`] or [`parse_query_ls`]).
    pub fn import_entries(&self, entries: Vec<ZoxideEntry>) -> ImportReport {
        let mut rep = ImportReport {
            imported: 0,
            skipped: 0,
        };
        for e in entries {
            if self.add_imported(&e.path, e.rank, e.last_accessed) {
                rep.imported += 1;
            } else {
                rep.skipped += 1;
            }
        }
        self.age_now();
        rep
    }
}

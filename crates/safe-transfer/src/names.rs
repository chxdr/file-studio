//! Pure naming rules: job ids and partial names (proposal §5). No I/O.

use std::fmt;
use std::path::{Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

/// The suffix of every partial file Safe Transfer writes. File Studio's own
/// engine uses `.fstudio-partial`; neither ever touches the other's.
pub const PARTIAL_SUFFIX: &str = ".st-partial";

/// Longest name in bytes. APFS and HFS+ allow 255 UTF-8 bytes; exFAT and FAT
/// allow 255 UTF-16 units, which is never fewer.
pub const NAME_MAX: usize = 255;

/// Room kept for the `._` prefix of the AppleDouble companion that macOS
/// writes next to a file on exFAT and FAT.
const APPLEDOUBLE_PREFIX: usize = 2;

/// The base-32 alphabet of job ids (Crockford's: no I, L, O or U), lowercase.
const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// A job's id: 8 random base-32 characters (40 bits), part of every partial name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct JobId([u8; 8]);

impl JobId {
    /// An id from 5 random bytes (40 bits).
    pub fn from_random(bytes: [u8; 5]) -> JobId {
        let mut bits: u64 = 0;
        for b in bytes {
            bits = bits << 8 | u64::from(b);
        }
        let mut out = [0u8; 8];
        for (i, c) in out.iter_mut().enumerate() {
            *c = ALPHABET[((bits >> (35 - 5 * i)) & 31) as usize];
        }
        JobId(out)
    }

    /// A fresh random id.
    pub fn new() -> std::io::Result<JobId> {
        let mut b = [0u8; 5];
        crate::sys::random_bytes(&mut b)?;
        Ok(JobId::from_random(b))
    }

    /// Parses an id written by [`JobId::as_str`].
    pub fn parse(s: &str) -> Option<JobId> {
        let b = s.as_bytes();
        if b.len() != 8 || !b.iter().all(|c| ALPHABET.contains(c)) {
            return None;
        }
        let mut out = [0u8; 8];
        out.copy_from_slice(b);
        Some(JobId(out))
    }

    /// The id as text.
    pub fn as_str(&self) -> &str {
        // The bytes all come from ALPHABET, which is ASCII.
        std::str::from_utf8(&self.0).unwrap_or("????????")
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Truncates `s` to at most `max` bytes on a char boundary.
fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// `dir/.<name>.<job>-<n>.st-partial`: where file `n` of job `job` is written
/// before it gets its real name. The name part is shortened (on a character
/// boundary) so the whole name, plus a `._` AppleDouble prefix, fits in 255
/// bytes. The real name itself is never shortened.
pub fn partial_path(final_path: &Path, job: JobId, n: u32) -> PathBuf {
    let name = final_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tail = format!(".{job}-{n}{PARTIAL_SUFFIX}");
    let room = NAME_MAX - APPLEDOUBLE_PREFIX - 1 - tail.len();
    final_path.with_file_name(format!(".{}{tail}", truncate_bytes(&name, room)))
}

/// Whether `path`'s name has the shape of a Safe Transfer partial. Shape
/// alone never makes a file removable: see `remove.rs`.
pub fn is_partial_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.') && n.ends_with(PARTIAL_SUFFIX))
}

/// Splits a name into stem and extension the way Finder numbers copies:
/// folders and dot-files have no extension; otherwise the last `.ext` is kept.
fn split_ext(name: &str, is_dir: bool) -> (&str, &str) {
    if is_dir {
        return (name, "");
    }
    match name.rfind('.') {
        Some(0) | None => (name, ""),
        Some(i) => (&name[..i], &name[i..]),
    }
}

/// Splits a trailing " N" (N ≥ 2) from a stem: "photo 3" → ("photo", Some(3)).
fn split_number(stem: &str) -> (&str, Option<u64>) {
    if let Some(i) = stem.rfind(' ') {
        let tail = &stem[i + 1..];
        if !tail.is_empty()
            && tail.bytes().all(|b| b.is_ascii_digit())
            && !tail.starts_with('0')
            && let Ok(n) = tail.parse::<u64>()
            && n >= 2
            && i > 0
        {
            return (&stem[..i], Some(n));
        }
    }
    (stem, None)
}

fn fit(stem: &str, tail: &str, ext: &str) -> String {
    let room = NAME_MAX.saturating_sub(tail.len() + ext.len());
    format!("{}{tail}{ext}", truncate_bytes(stem, room))
}

/// "Keep Both" names in the order Finder tries them: `report.pdf` →
/// `report 2.pdf`, `report 3.pdf`…; `report 2.pdf` → `report 3.pdf`…;
/// folders `Photos` → `Photos 2`.
pub fn keep_both_names(name: &str, is_dir: bool) -> impl Iterator<Item = String> + '_ {
    let (stem, ext) = split_ext(name, is_dir);
    let (base, n) = split_number(stem);
    let start = n.map_or(2, |n| n + 1);
    (start..).map(move |i| fit(base, &format!(" {i}"), ext))
}

/// The comparison key for a name: NFC, plus case folding on case-insensitive
/// volumes. Only for comparing; never for display or I/O.
pub fn name_key(name: &str, case_sensitive: bool) -> String {
    let nfc: String = name.nfc().collect();
    if case_sensitive {
        nfc
    } else {
        nfc.to_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> JobId {
        JobId::from_random([0x1a, 0x2b, 0x3c, 0x4d, 0x5e])
    }

    #[test]
    fn job_ids_are_eight_base32_chars_and_round_trip() {
        let j = job();
        assert_eq!(j.as_str().len(), 8);
        assert_eq!(JobId::parse(j.as_str()), Some(j));
        assert_eq!(JobId::parse("short"), None);
        assert_eq!(JobId::parse("ILOU0000"), None);
        assert_ne!(
            JobId::from_random([0; 5]),
            JobId::from_random([0, 0, 0, 0, 1])
        );
    }

    #[test]
    fn keep_both_names_count_like_finder() {
        let take = |n: &str, d: bool| keep_both_names(n, d).take(2).collect::<Vec<_>>();
        assert_eq!(
            take("DSC01234.ARW", false),
            ["DSC01234 2.ARW", "DSC01234 3.ARW"]
        );
        assert_eq!(
            take("report 2.pdf", false),
            ["report 3.pdf", "report 4.pdf"]
        );
        assert_eq!(take("Photos", true), ["Photos 2", "Photos 3"]);
        assert!(
            keep_both_names(&"a".repeat(255), false)
                .next()
                .unwrap()
                .len()
                <= 255
        );
    }

    #[test]
    fn partial_names() {
        let p = partial_path(Path::new("/d/DSC01234.ARW"), job(), 417);
        assert_eq!(
            p,
            PathBuf::from(format!("/d/.DSC01234.ARW.{}-417.st-partial", job()))
        );
        assert!(is_partial_name(&p));
        assert!(!is_partial_name(Path::new("/d/DSC01234.ARW")));
        assert!(!is_partial_name(Path::new("/d/x.st-partial")));
    }

    #[test]
    fn long_and_unicode_names_fit_with_room_for_appledouble() {
        for name in [
            "é".repeat(200),
            "📷".repeat(80),
            "e\u{301}".repeat(120),
            "a".repeat(255),
        ] {
            let p = partial_path(&Path::new("/x").join(&name), job(), u32::MAX);
            let n = p.file_name().unwrap().to_str().unwrap();
            assert!(n.len() + 2 <= NAME_MAX, "{} bytes", n.len());
            assert!(n.starts_with('.') && n.ends_with(PARTIAL_SUFFIX));
        }
    }

    #[test]
    fn names_differing_only_by_case_or_form_get_different_partials() {
        let a = partial_path(Path::new("/d/File"), job(), 1);
        let b = partial_path(Path::new("/d/file"), job(), 2);
        assert_ne!(
            name_key(&a.to_string_lossy(), false),
            name_key(&b.to_string_lossy(), false)
        );
        assert_eq!(name_key("caf\u{e9}", true), name_key("cafe\u{301}", true));
        assert_eq!(name_key("File", false), name_key("file", false));
    }
}

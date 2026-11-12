//! Pure naming rules: temporary names, "Keep Both" and "Duplicate" names,
//! and comparison keys (F-017). No I/O.

use std::path::{Component, Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

/// The suffix of every temporary file or folder the copy engine writes.
pub const PARTIAL_SUFFIX: &str = ".fstudio-partial";

/// The suffix of the intermediate name used by a case-only rename. It is the
/// user's real item, so it is never deleted, only renamed back.
pub const RENAMING_SUFFIX: &str = ".fstudio-renaming";

/// Longest file name, in bytes, on APFS/HFS+.
const NAME_MAX: usize = 255;

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

fn hidden_with_suffix(final_path: &Path, suffix: &str) -> PathBuf {
    let name = final_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let room = NAME_MAX - 1 - suffix.len();
    final_path.with_file_name(format!(".{}{suffix}", truncate_bytes(&name, room)))
}

/// `dir/.<name>.<item>.fstudio-partial`: where a copy is written before it is
/// renamed into place. `item` is the journal item id, so two items whose
/// names differ only by case (or normalisation) never share a temporary name
/// on a case-insensitive volume. Long names are shortened so the result stays
/// a valid name.
pub fn partial_path(final_path: &Path, item: i64) -> PathBuf {
    hidden_with_suffix(final_path, &format!(".{item}{PARTIAL_SUFFIX}"))
}

/// The intermediate name of a case-only rename of `source`.
pub fn renaming_path(source: &Path) -> PathBuf {
    hidden_with_suffix(source, RENAMING_SUFFIX)
}

/// Whether `path`'s name is one of our temporary names.
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

/// "Duplicate" names: `a.txt` → `a copy.txt`, `a copy 2.txt`, `a copy 3.txt`…
pub fn duplicate_names(name: &str, is_dir: bool) -> impl Iterator<Item = String> + '_ {
    let (stem, ext) = split_ext(name, is_dir);
    std::iter::once(fit(stem, " copy", ext))
        .chain((2u64..).map(move |i| fit(stem, &format!(" copy {i}"), ext)))
}

/// The comparison key for a name (F-017): NFC, plus case folding when the
/// volume is case-insensitive. Only for comparing; never for display or I/O.
pub fn name_key(name: &str, case_sensitive: bool) -> String {
    let nfc: String = name.nfc().collect();
    if case_sensitive {
        nfc
    } else {
        nfc.to_lowercase()
    }
}

/// Checks a user-typed file name.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("the name is empty".into());
    }
    if name == "." || name == ".." {
        return Err(format!("“{name}” is not a valid name"));
    }
    if name.contains('/') || name.contains('\0') {
        return Err("names can't contain “/”".into());
    }
    if name.len() > NAME_MAX {
        return Err("the name is too long".into());
    }
    Ok(())
}

/// Lexically normalises an absolute path (drops `.`, resolves `..`).
pub fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Whether `inner` is `outer` or inside it, comparing lexically normalised
/// paths with NFC + case-folded components (the cautious answer on APFS).
pub fn is_within(inner: &Path, outer: &Path) -> bool {
    let key = |p: &Path| -> Vec<String> {
        lexical(p)
            .components()
            .map(|c| name_key(&c.as_os_str().to_string_lossy(), false))
            .collect()
    };
    let (i, o) = (key(inner), key(outer));
    i.len() >= o.len() && i[..o.len()] == o[..]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take(it: impl Iterator<Item = String>, n: usize) -> Vec<String> {
        it.take(n).collect()
    }

    #[test]
    fn keep_both_sequence() {
        assert_eq!(
            take(keep_both_names("report.pdf", false), 3),
            ["report 2.pdf", "report 3.pdf", "report 4.pdf"]
        );
        assert_eq!(
            take(keep_both_names("report 2.pdf", false), 2),
            ["report 3.pdf", "report 4.pdf"]
        );
        assert_eq!(
            take(keep_both_names("Photos", true), 2),
            ["Photos 2", "Photos 3"]
        );
        assert_eq!(
            take(keep_both_names("v1.2 final", true), 1),
            ["v1.2 final 2"]
        );
        assert_eq!(take(keep_both_names(".zshrc", false), 1), [".zshrc 2"]);
        assert_eq!(take(keep_both_names("a.tar.gz", false), 1), ["a.tar 2.gz"]);
        // "1" and "0…" are not copy numbers.
        assert_eq!(
            take(keep_both_names("Track 1.mp3", false), 1),
            ["Track 1 2.mp3"]
        );
        assert_eq!(take(keep_both_names("Take 02", true), 1), ["Take 02 2"]);
        assert_eq!(take(keep_both_names(" 5", true), 1), [" 5 2"]);
    }

    #[test]
    fn duplicate_sequence() {
        assert_eq!(
            take(duplicate_names("a.txt", false), 3),
            ["a copy.txt", "a copy 2.txt", "a copy 3.txt"]
        );
        assert_eq!(
            take(duplicate_names("Folder.d", true), 1),
            ["Folder.d copy"]
        );
    }

    #[test]
    fn long_names_stay_valid() {
        let long = "é".repeat(200); // 400 bytes
        let p = partial_path(Path::new(&format!("/x/{long}")), 12345);
        let n = p.file_name().unwrap().to_str().unwrap();
        assert!(n.len() <= 255 && n.ends_with(PARTIAL_SUFFIX) && n.starts_with('.'));
        let k = keep_both_names(&"a".repeat(255), false).next().unwrap();
        assert!(k.len() <= 255 && k.ends_with(" 2"));
    }

    #[test]
    fn partial_names() {
        assert_eq!(
            partial_path(Path::new("/d/a.txt"), 7),
            Path::new("/d/.a.txt.7.fstudio-partial")
        );
        assert!(is_partial_name(Path::new("/d/.a.txt.fstudio-partial")));
        assert!(!is_partial_name(Path::new("/d/a.txt.fstudio-partial")));
        assert!(!is_partial_name(Path::new("/d/.a.txt")));
    }

    #[test]
    fn keys_fold_case_and_normalisation() {
        let nfc = "caf\u{e9}";
        let nfd = "cafe\u{301}";
        assert_eq!(name_key(nfc, true), name_key(nfd, true));
        assert_ne!(name_key("File", true), name_key("file", true));
        assert_eq!(name_key("File", false), name_key("file", false));
    }

    #[test]
    fn names_are_validated() {
        assert!(validate_name("ok.txt").is_ok());
        for bad in ["", ".", "..", "a/b"] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn within() {
        assert!(is_within(Path::new("/a/b/c"), Path::new("/a/b")));
        assert!(is_within(Path::new("/a/B"), Path::new("/a/b")));
        assert!(is_within(Path::new("/a/b"), Path::new("/a/b")));
        assert!(!is_within(Path::new("/a/bc"), Path::new("/a/b")));
        assert!(is_within(Path::new("/a/x/../b/c"), Path::new("/a/b")));
    }
}

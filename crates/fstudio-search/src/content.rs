//! Content search: text files only, size-capped, never iCloud placeholders.

use std::io::Read;
use std::path::Path;

use fstudio_model::{Entry, EntryKind};

/// How many leading bytes are checked for a NUL to tell binary from text.
pub const CONTENT_SNIFF_BYTES: usize = 8 * 1024;

/// Does the file at `path` (listed as `e`) contain `needle_folded` (an
/// NFC, case-folded string)? Never reads:
/// - anything but a regular file (symlinks are not followed),
/// - an iCloud / File Provider placeholder (`SF_DATALESS`, F-016),
/// - a file larger than `max_bytes`,
/// - a file with a NUL byte in its first [`CONTENT_SNIFF_BYTES`] (binary).
pub fn content_matches(path: &Path, e: &Entry, needle_folded: &str, max_bytes: u64) -> bool {
    if e.kind != EntryKind::File || e.is_dataless() || e.size > max_bytes {
        return false;
    }
    if needle_folded.is_empty() {
        return true;
    }
    let Ok(f) = open_nofollow(path) else {
        return false;
    };
    let mut buf = Vec::with_capacity(e.size.min(max_bytes) as usize);
    if f.take(max_bytes).read_to_end(&mut buf).is_err() {
        return false;
    }
    let sniff = &buf[..buf.len().min(CONTENT_SNIFF_BYTES)];
    let utf16 = sniff.starts_with(&[0xFF, 0xFE]) || sniff.starts_with(&[0xFE, 0xFF]);
    if utf16 {
        return utf16_contains(&buf, needle_folded);
    }
    if sniff.contains(&0) && !crate::kind::is_text_ext(e.extension()) {
        return false;
    }
    let text = String::from_utf8_lossy(&buf);
    if needle_folded.is_ascii() && text.is_ascii() {
        return ascii_contains_folded(text.as_bytes(), needle_folded.as_bytes());
    }
    fstudio_view::fold(&text).contains(needle_folded)
}

fn open_nofollow(path: &Path) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc_nofollow())
            .open(path)
    }
    #[cfg(not(unix))]
    {
        std::fs::File::open(path)
    }
}

#[cfg(target_os = "macos")]
const fn libc_nofollow() -> i32 {
    0x0100 // O_NOFOLLOW
}
#[cfg(all(unix, not(target_os = "macos")))]
const fn libc_nofollow() -> i32 {
    0o400000 // O_NOFOLLOW (Linux)
}

fn ascii_contains_folded(hay: &[u8], needle: &[u8]) -> bool {
    if needle.len() > hay.len() {
        return false;
    }
    hay.windows(needle.len()).any(|w| {
        w.iter()
            .zip(needle)
            .all(|(a, b)| a.to_ascii_lowercase() == *b)
    })
}

fn utf16_contains(buf: &[u8], needle: &str) -> bool {
    let le = buf.starts_with(&[0xFF, 0xFE]);
    let units: Vec<u16> = buf[2..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| {
            if le {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            }
        })
        .collect();
    fstudio_view::fold(&String::from_utf16_lossy(&units)).contains(needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_for(p: &Path) -> Entry {
        fstudio_fs::stat_entry(p).unwrap()
    }

    #[test]
    fn text_binary_cap_and_dataless() {
        let d = tempfile::tempdir().unwrap();
        let t = d.path().join("a.txt");
        std::fs::write(&t, "Hello Café World").unwrap();
        let e = entry_for(&t);
        assert!(content_matches(&t, &e, "café", 1 << 20));
        assert!(content_matches(&t, &e, "hello", 1 << 20));
        assert!(!content_matches(&t, &e, "nope", 1 << 20));
        assert!(!content_matches(&t, &e, "hello", 4)); // over the cap

        let nfd = d.path().join("nfd.txt");
        std::fs::write(&nfd, "un cafe\u{301} noir").unwrap();
        assert!(content_matches(&nfd, &entry_for(&nfd), "café", 1 << 20));

        let b = d.path().join("b.bin");
        std::fs::write(&b, b"hello\0world").unwrap();
        assert!(!content_matches(&b, &entry_for(&b), "hello", 1 << 20));

        let u = d.path().join("u16.txt");
        let mut bytes = vec![0xFF, 0xFE];
        for c in "Grüße".encode_utf16() {
            bytes.extend_from_slice(&c.to_le_bytes());
        }
        std::fs::write(&u, bytes).unwrap();
        assert!(content_matches(&u, &entry_for(&u), "grüße", 1 << 20));

        // A placeholder is never opened, even when the bytes would match.
        let mut dl = e.clone();
        dl.flags |= fstudio_model::file_flags::DATALESS;
        assert!(!content_matches(&t, &dl, "hello", 1 << 20));

        // A symlink to a matching file is not followed.
        let l = d.path().join("link.txt");
        std::os::unix::fs::symlink(&t, &l).unwrap();
        assert!(!content_matches(&l, &entry_for(&l), "hello", 1 << 20));
    }
}

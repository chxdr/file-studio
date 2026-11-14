//! The start of a text file: encoding, line count, and the first lines.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::cancel::CancelToken;
use crate::highlight;
use crate::types::{PreviewOptions, TextLine, TextPreview};

/// How much of the start is read to find the first lines.
const HEAD_BYTES: usize = 256 * 1024;
/// A line longer than this is cut for display.
const MAX_LINE_CHARS: usize = 2000;
/// Longest line that is highlighted (longer ones stay plain).
const MAX_HIGHLIGHT_LINE: usize = 600;

/// How the bytes are encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Enc {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Latin1,
}

impl Enc {
    fn label(self) -> &'static str {
        match self {
            Enc::Utf8 => "UTF-8",
            Enc::Utf8Bom => "UTF-8 with BOM",
            Enc::Utf16Le => "UTF-16 LE",
            Enc::Utf16Be => "UTF-16 BE",
            Enc::Latin1 => "Latin-1 (guessed)",
        }
    }
    fn wide(self) -> bool {
        matches!(self, Enc::Utf16Le | Enc::Utf16Be)
    }
}

/// Works out the encoding from the first bytes. `None` means "not text".
/// `by_name` says the extension already promised text, which allows guessing
/// Latin-1 for bytes that aren't UTF-8.
fn detect(head: &[u8], by_name: bool, whole: bool) -> Option<Enc> {
    if head.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Some(Enc::Utf8Bom);
    }
    if head.starts_with(&[0xff, 0xfe]) {
        return Some(Enc::Utf16Le);
    }
    if head.starts_with(&[0xfe, 0xff]) {
        return Some(Enc::Utf16Be);
    }
    let probe = &head[..head.len().min(8192)];
    if probe.contains(&0) {
        return None;
    }
    match std::str::from_utf8(head) {
        Ok(_) => Some(Enc::Utf8),
        // A multi-byte character cut off by the end of the read is fine.
        Err(e) if e.error_len().is_none() && !whole => Some(Enc::Utf8),
        Err(e) if e.error_len().is_none() && whole => Some(Enc::Utf8),
        Err(_) if by_name => {
            let control = probe
                .iter()
                .filter(|&&b| b < 0x09 || (0x0e..0x20).contains(&b))
                .count();
            (control * 20 < probe.len().max(1)).then_some(Enc::Latin1)
        }
        Err(_) => None,
    }
}

fn decode(bytes: &[u8], enc: Enc) -> String {
    match enc {
        Enc::Utf8 => match std::str::from_utf8(bytes) {
            Ok(s) => s.to_string(),
            Err(e) => String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned(),
        },
        Enc::Utf8Bom => decode(bytes.get(3..).unwrap_or(&[]), Enc::Utf8),
        Enc::Latin1 => bytes.iter().map(|&b| b as char).collect(),
        Enc::Utf16Le | Enc::Utf16Be => {
            let body = bytes.get(2..).unwrap_or(&[]);
            let units: Vec<u16> = body
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| {
                    if enc == Enc::Utf16Le {
                        u16::from_le_bytes([c[0], c[1]])
                    } else {
                        u16::from_be_bytes([c[0], c[1]])
                    }
                })
                .collect();
            String::from_utf16_lossy(&units)
        }
    }
}

/// Cuts a line for display at `MAX_LINE_CHARS`, on a character boundary.
fn clip(line: &str) -> String {
    match line.char_indices().nth(MAX_LINE_CHARS) {
        Some((i, _)) => format!("{}…", &line[..i]),
        None => line.to_string(),
    }
}

/// Counts line ends in a file without keeping it in memory. `None` when cancelled.
fn count_lines(
    f: &mut File,
    enc: Enc,
    size: u64,
    cap: u64,
    cancel: &CancelToken,
) -> Option<(u64, bool)> {
    use std::io::Seek;
    f.seek(std::io::SeekFrom::Start(0)).ok()?;
    let limit = size.min(cap);
    let mut buf = vec![0u8; 1 << 20];
    let (mut newlines, mut read) = (0u64, 0u64);
    let mut last: Option<u8> = None;
    // For UTF-16 the 0x0A unit is 0A 00 (LE) or 00 0A (BE) on an even offset.
    let mut carry: Option<u8> = None;
    while read < limit {
        if cancel.is_cancelled() {
            return None;
        }
        let want = usize::try_from((limit - read).min(buf.len() as u64)).ok()?;
        let n = f.read(&mut buf[..want]).ok()?;
        if n == 0 {
            break;
        }
        let chunk = &buf[..n];
        if enc.wide() {
            let mut prev = carry.take();
            for &b in chunk {
                if let Some(p) = prev.take() {
                    let unit = if enc == Enc::Utf16Le { (b, p) } else { (p, b) };
                    // (hi, lo) of the unit: newline is hi 0, lo 0x0A.
                    if unit == (0, 0x0a) {
                        newlines += 1;
                    }
                } else {
                    prev = Some(b);
                }
            }
            carry = prev;
        } else {
            newlines += chunk.iter().filter(|&&b| b == b'\n').count() as u64;
        }
        last = chunk.last().copied();
        read += n as u64;
    }
    let exact = size <= cap;
    let mut lines = newlines;
    if exact {
        // A last line with no newline still counts.
        let ends_with_nl = if enc.wide() {
            // Judged by the final unit.
            read >= 2 && newlines_end_wide(f, enc, size)
        } else {
            last == Some(b'\n')
        };
        if read > 0 && !ends_with_nl {
            lines += 1;
        }
    } else if read > 0 {
        // Scale what was counted up to the whole file.
        lines = (newlines as f64 * size as f64 / read as f64).round() as u64;
    }
    Some((lines, exact))
}

/// Does a UTF-16 file end with a line feed?
fn newlines_end_wide(f: &mut File, enc: Enc, size: u64) -> bool {
    use std::io::{Seek, SeekFrom};
    if size < 2 || f.seek(SeekFrom::Start(size - 2)).is_err() {
        return false;
    }
    let mut b = [0u8; 2];
    if f.read_exact(&mut b).is_err() {
        return false;
    }
    if enc == Enc::Utf16Le {
        b == [0x0a, 0]
    } else {
        b == [0, 0x0a]
    }
}

/// Reads the text preview. `by_name` is true when the extension says text.
pub(crate) fn read(
    path: &Path,
    size: u64,
    by_name: bool,
    opts: &PreviewOptions,
    cancel: &CancelToken,
) -> (Option<TextPreview>, Option<String>) {
    let Ok(mut f) = File::open(path) else {
        return (None, Some("Can't read this file".into()));
    };
    let mut head = vec![0u8; HEAD_BYTES.min(usize::try_from(size).unwrap_or(HEAD_BYTES))];
    let mut got = 0;
    while got < head.len() {
        match f.read(&mut head[got..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => got += n,
        }
    }
    head.truncate(got);
    if cancel.is_cancelled() {
        return (None, None);
    }
    let whole = got as u64 >= size;
    if head.is_empty() {
        return if by_name {
            (
                Some(TextPreview {
                    encoding: Enc::Utf8.label().into(),
                    language: None,
                    line_count: 0,
                    lines_exact: true,
                    lines: Vec::new(),
                    truncated: false,
                }),
                None,
            )
        } else {
            (None, None)
        };
    }
    let Some(enc) = detect(&head, by_name, whole) else {
        let note =
            by_name.then(|| "Not a text file, or in an encoding this can't read".to_string());
        return (None, note);
    };
    let decoded = decode(&head, enc);
    let max = opts.max_text_lines as usize;
    let mut raw: Vec<&str> = decoded.split('\n').collect();
    // The text after the last newline is a line only if the file ends there.
    let ends_cleanly = decoded.ends_with('\n');
    if ends_cleanly {
        raw.pop();
    }
    let more_in_head = raw.len() > max;
    raw.truncate(max);
    let shown: Vec<String> = raw
        .iter()
        .map(|l| clip(l.strip_suffix('\r').unwrap_or(l)))
        .collect();
    let Some((line_count, lines_exact)) =
        count_lines(&mut f, enc, size, opts.line_count_cap_bytes, cancel)
    else {
        return (None, None);
    };
    let truncated = more_in_head || !whole && !(raw.len() < max) || line_count > shown.len() as u64;
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut language = None;
    let mut spans = None;
    if opts.highlight {
        language =
            highlight::language_name(&file_name, shown.first().map(String::as_str).unwrap_or(""));
        if language.is_some() {
            // Very long lines stay plain: they only slow the grammar down.
            let for_parse: Vec<String> = shown
                .iter()
                .map(|l| {
                    if l.len() > MAX_HIGHLIGHT_LINE {
                        String::new()
                    } else {
                        l.clone()
                    }
                })
                .collect();
            spans = highlight::spans(&file_name, &for_parse, cancel);
            if cancel.is_cancelled() {
                return (None, None);
            }
        }
    }
    let lines = shown
        .into_iter()
        .enumerate()
        .map(|(i, text)| {
            let sp = if text.len() > MAX_HIGHLIGHT_LINE {
                Vec::new()
            } else {
                spans
                    .as_ref()
                    .and_then(|s| s.get(i))
                    .cloned()
                    .unwrap_or_default()
            };
            TextLine { text, spans: sp }
        })
        .collect();
    (
        Some(TextPreview {
            encoding: enc.label().into(),
            language,
            line_count,
            lines_exact,
            lines,
            truncated,
        }),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(bytes: &[u8], name: &str, by_name: bool, max: u32) -> Option<TextPreview> {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(name);
        std::fs::write(&p, bytes).unwrap();
        let opts = PreviewOptions {
            max_text_lines: max,
            ..Default::default()
        };
        read(&p, bytes.len() as u64, by_name, &opts, &CancelToken::new()).0
    }

    #[test]
    fn utf8_lines_count_and_first_lines() {
        let t = run(b"one\ntwo\r\nthree\nfour", "a.txt", true, 2).unwrap();
        assert_eq!(t.encoding, "UTF-8");
        assert_eq!(t.line_count, 4);
        assert!(t.lines_exact && t.truncated);
        assert_eq!(
            t.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            ["one", "two"]
        );
        let t = run(b"a\nb\n", "a.txt", true, 10).unwrap();
        assert_eq!(t.line_count, 2);
        assert!(!t.truncated);
    }

    #[test]
    fn encodings() {
        let mut bom = vec![0xef, 0xbb, 0xbf];
        bom.extend(b"hi\n");
        assert_eq!(
            run(&bom, "a.txt", true, 5).unwrap().encoding,
            "UTF-8 with BOM"
        );
        let le: Vec<u8> = [0xff, 0xfe]
            .into_iter()
            .chain("a\nb\n".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        let t = run(&le, "a.txt", true, 5).unwrap();
        assert_eq!((t.encoding.as_str(), t.line_count), ("UTF-16 LE", 2));
        assert_eq!(t.lines[1].text, "b");
        let t = run(b"caf\xe9\nok\n", "a.txt", true, 5).unwrap();
        assert_eq!(t.encoding, "Latin-1 (guessed)");
        assert_eq!(t.lines[0].text, "café");
    }

    #[test]
    fn binary_and_unknown_types_are_not_text() {
        assert!(run(&[0, 1, 2, 3, 0, 0], "a.txt", true, 5).is_none());
        assert!(run(b"\x80\x81\x82 garbage \x80", "mystery", false, 5).is_none());
        let t = run(b"plain words\n", "mystery", false, 5).unwrap();
        assert_eq!(t.line_count, 1);
    }

    #[test]
    fn empty_text_file_and_long_lines() {
        let t = run(b"", "a.txt", true, 5).unwrap();
        assert_eq!(t.line_count, 0);
        let long = "x".repeat(5000);
        let t = run(long.as_bytes(), "a.txt", true, 5).unwrap();
        assert!(t.lines[0].text.chars().count() <= MAX_LINE_CHARS + 1);
    }

    #[test]
    fn rust_file_has_language_and_spans() {
        let t = run(b"fn main() {}\n", "main.rs", true, 5).unwrap();
        assert_eq!(t.language.as_deref(), Some("Rust"));
        assert!(!t.lines[0].spans.is_empty());
        let none = run(b"fn main() {}\n", "main.rs", true, 0).unwrap();
        assert!(none.lines.is_empty());
    }

    #[test]
    fn big_file_count_is_an_estimate_beyond_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big.log");
        let data: Vec<u8> = "line\n".repeat(10_000).into_bytes();
        std::fs::write(&p, &data).unwrap();
        let opts = PreviewOptions {
            line_count_cap_bytes: 10_000,
            ..Default::default()
        };
        let t = read(&p, data.len() as u64, true, &opts, &CancelToken::new())
            .0
            .unwrap();
        assert!(!t.lines_exact);
        assert!((9_000..=11_000).contains(&t.line_count), "{}", t.line_count);
    }

    #[test]
    fn cancelled_read_gives_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.txt");
        std::fs::write(&p, "x\n").unwrap();
        let c = CancelToken::new();
        c.cancel();
        assert!(
            read(&p, 2, true, &PreviewOptions::default(), &c)
                .0
                .is_none()
        );
    }
}

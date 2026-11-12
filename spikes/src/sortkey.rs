//! Natural, case-insensitive, Unicode-normalised name ordering (codepoint order after folding;
//! NOT locale collation — see docs/SPIKES.md for the CFStringCompare comparison).
use std::cmp::Ordering;
use unicode_normalization::UnicodeNormalization;

/// NFC + lowercase, with an ASCII fast path.
pub fn fold(name: &[u8]) -> String {
    match std::str::from_utf8(name) {
        Ok(s) if s.is_ascii() => s.to_ascii_lowercase(),
        Ok(s) => s.nfc().collect::<String>().to_lowercase(),
        Err(_) => String::from_utf8_lossy(name).nfc().collect::<String>().to_lowercase(),
    }
}

pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let (si, sj) = (i, j);
            while i < a.len() && a[i].is_ascii_digit() { i += 1; }
            while j < b.len() && b[j].is_ascii_digit() { j += 1; }
            let (x, y) = (trim0(&a[si..i]), trim0(&b[sj..j]));
            let o = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
            if o != Ordering::Equal { return o; }
        } else {
            if a[i] != b[j] { return a[i].cmp(&b[j]); }
            i += 1; j += 1;
        }
    }
    (a.len() - i).cmp(&(b.len() - j))
}

fn trim0(s: &[u8]) -> &[u8] {
    let k = s.iter().take_while(|&&c| c == b'0').count();
    &s[k..]
}

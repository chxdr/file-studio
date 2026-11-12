//! Name folding and the collation hook.

use std::cmp::Ordering;
use unicode_normalization::UnicodeNormalization;

/// The comparison key for names: NFC-normalised and lower-cased (F-017). Used for sorting,
/// filtering and type-to-select; never for display.
pub fn fold(name: &str) -> String {
    if name.is_ascii() {
        name.to_ascii_lowercase()
    } else {
        name.nfc().collect::<String>().to_lowercase()
    }
}

/// Natural ordering of two folded names: runs of ASCII digits compare by value
/// (`file2` < `file10`), everything else by byte (which is code-point order for UTF-8).
pub fn natural_cmp(a: &[u8], b: &[u8]) -> Ordering {
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let (si, sj) = (i, j);
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let (x, y) = (trim_zeros(&a[si..i]), trim_zeros(&b[sj..j]));
            let o = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
            if o != Ordering::Equal {
                return o;
            }
        } else {
            if a[i] != b[j] {
                return a[i].cmp(&b[j]);
            }
            i += 1;
            j += 1;
        }
    }
    (a.len() - i).cmp(&(b.len() - j))
}

fn trim_zeros(s: &[u8]) -> &[u8] {
    let k = s.iter().take_while(|&&c| c == b'0').count();
    &s[k..]
}

/// How names are ordered. The default ([`NaturalCollation`]) is fast and locale-free; a
/// Finder-style localised collation can be plugged in later by implementing this trait.
///
/// Keys are computed once per entry when a listing is built, so `compare` is the only
/// work done per comparison.
pub trait Collation: Send + Sync + 'static {
    /// A custom sort key for a name, or `None` to use the folded name itself (the common case,
    /// which avoids storing a second copy).
    fn key(&self, name: &str, folded: &str) -> Option<Box<[u8]>>;
    /// Orders two keys (the custom key if [`Collation::key`] gave one, else the folded name's bytes).
    fn compare(&self, a: &[u8], b: &[u8]) -> Ordering;
}

/// Natural-number, case-folded, NFC-normalised ordering. Fast and deterministic.
#[derive(Debug, Clone, Copy, Default)]
pub struct NaturalCollation;

impl Collation for NaturalCollation {
    fn key(&self, _name: &str, _folded: &str) -> Option<Box<[u8]>> {
        None
    }
    fn compare(&self, a: &[u8], b: &[u8]) -> Ordering {
        natural_cmp(a, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_numbers() {
        let c = |a: &str, b: &str| natural_cmp(a.as_bytes(), b.as_bytes());
        assert_eq!(c("file2", "file10"), Ordering::Less);
        assert_eq!(c("a01", "a1"), Ordering::Equal);
        assert_eq!(c("a", "ab"), Ordering::Less);
        assert_eq!(c("img_9.png", "img_10.png"), Ordering::Less);
    }

    #[test]
    fn fold_nfc_and_case() {
        assert_eq!(fold("Caf\u{e9}"), fold("Cafe\u{301}"));
        assert_eq!(fold("README"), "readme");
    }
}

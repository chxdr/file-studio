//! Comparison keys (F-017): NFC + lower case. Used only for matching and sorting,
//! never for display or for file operations.

use unicode_normalization::UnicodeNormalization;

/// Folds a name for comparison: Unicode NFC, then lower case.
///
/// `"Caf\u{65}\u{301}"` (NFD) and `"CAF\u{e9}"` (NFC) fold to the same key.
pub fn fold(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    s.nfc().flat_map(char::to_lowercase).collect()
}

/// True when two names are the same file name under APFS rules as we predict them
/// (normalisation- and case-insensitive).
pub fn same_name(a: &str, b: &str) -> bool {
    a == b || fold(a) == fold(b)
}

/// Fast check that `needle` is a subsequence of `hay` (both already folded).
pub fn is_subsequence(hay: &str, needle: &str) -> bool {
    if needle.is_ascii() {
        let mut it = hay.as_bytes().iter();
        needle.bytes().all(|n| it.any(|&h| h == n))
    } else {
        let mut it = hay.chars();
        needle.chars().all(|n| it.any(|h| h == n))
    }
}

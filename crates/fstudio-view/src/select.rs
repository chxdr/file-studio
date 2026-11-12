//! Type-to-select.

use crate::collation::fold;
use crate::listing::Listing;

/// Finder-style type-select: the first row, searching from row `from` (inclusive) and
/// wrapping around, whose name starts with `typed`.
///
/// Matching is on folded names (NFC, case-insensitive), so a decomposed `"cafe\u{301}"` matches `Café` however
/// the name is stored. `from` is where the current typing sequence started (the cursor row when the
/// first character was typed); passing it again for every added character keeps the match
/// stable while the prefix is extended. Returns `None` for empty input or no match.
pub fn type_select(listing: &Listing, typed: &str, from: usize) -> Option<usize> {
    if typed.is_empty() || listing.is_empty() {
        return None;
    }
    let prefix = fold(typed);
    let n = listing.len();
    let start = from.min(n - 1);
    (0..n).map(|k| (start + k) % n).find(|&i| {
        listing
            .folded_name(i)
            .is_some_and(|f| f.starts_with(&prefix))
    })
}

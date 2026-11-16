//! Culling marks (stars, colour label, pick/reject) from XMP sidecars, as the
//! listing sorts, filters and hides by them.
//!
//! The marks themselves are read elsewhere (lazily, off the listing path) and
//! handed in as a [`MarkTable`]. A table is keyed by the *folded stem*, so the
//! sidecar `DSC0001.XMP` gives its marks to `DSC0001.ARW`, `dsc0001.jpg` and
//! the sidecar itself, whatever their case or Unicode normalisation (F-017).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub use fstudio_xmp::{Flag, Label, Marks, is_sidecar_name};

use crate::collation::fold;

/// The stem of a folded name: everything before the last dot. A dot-file
/// (`.xmp`) or a name without a dot is its own stem, matching
/// `Entry::extension`.
pub fn folded_stem(folded: &str) -> &str {
    match folded.rfind('.') {
        Some(0) | None => folded,
        Some(i) => &folded[..i],
    }
}

/// The key a sidecar's marks are stored under: the folded stem of its name.
pub fn mark_key(name: &str) -> String {
    folded_stem(&fold(name)).to_string()
}

/// Marks by folded stem, as last read from a folder's sidecars. Cheap to
/// clone. Two tables are equal when they are the same table, or both empty.
#[derive(Clone, Default)]
pub struct MarkTable {
    by_stem: Arc<HashMap<Box<str>, Marks>>,
}

impl MarkTable {
    /// A table from (sidecar file name, marks) pairs. Later names win when
    /// two sidecars share a folded stem (`A.xmp` and `a.XMP` on a
    /// case-sensitive volume).
    pub fn from_sidecars(items: impl IntoIterator<Item = (String, Marks)>) -> MarkTable {
        let by_stem: HashMap<Box<str>, Marks> = items
            .into_iter()
            .map(|(name, m)| (mark_key(&name).into_boxed_str(), m))
            .collect();
        MarkTable {
            by_stem: Arc::new(by_stem),
        }
    }

    /// The marks for an entry, by its folded name.
    pub fn get_folded(&self, folded_name: &str) -> Option<Marks> {
        if self.by_stem.is_empty() {
            return None;
        }
        self.by_stem.get(folded_stem(folded_name)).copied()
    }

    /// The marks for an entry, by its name as on disk.
    pub fn get(&self, name: &str) -> Option<Marks> {
        self.get_folded(&fold(name))
    }

    /// How many sidecars have marks.
    pub fn len(&self) -> usize {
        self.by_stem.len()
    }

    /// True when no sidecar has been read.
    pub fn is_empty(&self) -> bool {
        self.by_stem.is_empty()
    }

    /// True when both tables hold exactly the same marks.
    pub fn same_contents(&self, other: &MarkTable) -> bool {
        *self.by_stem == *other.by_stem
    }
}

impl PartialEq for MarkTable {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.by_stem, &other.by_stem)
            || (self.by_stem.is_empty() && other.by_stem.is_empty())
    }
}
impl Eq for MarkTable {}

impl std::fmt::Debug for MarkTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MarkTable({} sidecars)", self.by_stem.len())
    }
}

/// The culling filter presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MarkFilter {
    /// Show everything.
    #[default]
    All,
    /// Only shots with at least this many stars that aren't rejected ("★ 3+").
    MinRating(u8),
    /// Only picks.
    Picks,
    /// Only rejects.
    Rejects,
    /// Everything except rejects.
    HideRejects,
}

impl MarkFilter {
    /// Whether a file with these marks (`None`: no sidecar, or one that
    /// couldn't be read) passes. Folders always pass, so you can still
    /// navigate while a preset is on.
    pub fn passes(self, marks: Option<Marks>) -> bool {
        let m = marks.unwrap_or_default();
        match self {
            MarkFilter::All => true,
            MarkFilter::MinRating(n) => !m.is_reject() && m.rating >= n,
            MarkFilter::Picks => m.is_pick(),
            MarkFilter::Rejects => m.is_reject(),
            MarkFilter::HideRejects => !m.is_reject(),
        }
    }
}

/// Sort rank by rating: rejects lowest, then unrated, then 1–5 stars.
pub(crate) fn rating_rank(m: Option<Marks>) -> i8 {
    match m {
        Some(m) if m.is_reject() => -1,
        Some(m) => m.rating as i8,
        None => 0,
    }
}

/// Sort rank by label: Red … Purple, then no label.
pub(crate) fn label_rank(m: Option<Marks>) -> u8 {
    match m.and_then(|m| m.label) {
        Some(l) => Label::ALL.iter().position(|x| *x == l).unwrap_or(5) as u8,
        None => 5,
    }
}

/// Sort rank by flag: picks, then unflagged, then rejects.
pub(crate) fn flag_rank(m: Option<Marks>) -> u8 {
    match m.map(|m| m.flag) {
        Some(Flag::Pick) => 0,
        Some(Flag::Reject) => 2,
        _ => 1,
    }
}

/// The folded stems whose sidecars should be hidden: the stem has a sidecar
/// *and* another regular file (the image). A sidecar with no image stays
/// visible, so nothing ever disappears without its data showing elsewhere.
///
/// `items` yields (folded name, is a regular file).
pub(crate) fn paired_stems<'a>(
    items: impl Iterator<Item = (&'a str, bool)> + Clone,
) -> HashSet<Box<str>> {
    // Most folders have no sidecar at all: one cheap pass finds that out.
    let sidecars: HashSet<&str> = items
        .clone()
        .filter(|(f, is_file)| *is_file && is_sidecar_name(f))
        .map(|(f, _)| folded_stem(f))
        .collect();
    if sidecars.is_empty() {
        return HashSet::new();
    }
    let mut out = HashSet::new();
    for (folded, is_file) in items {
        if !is_file || is_sidecar_name(folded) {
            continue;
        }
        let stem = folded_stem(folded);
        if sidecars.contains(stem) && !out.contains(stem) {
            out.insert(Box::<str>::from(stem));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(rating: u8, flag: Flag) -> Marks {
        Marks {
            rating,
            label: None,
            flag,
        }
    }

    #[test]
    fn keys_fold_case_and_normalisation() {
        assert_eq!(mark_key("DSC0001.XMP"), "dsc0001");
        assert_eq!(mark_key("Cafe\u{301}.xmp"), mark_key("Caf\u{e9}.XMP"));
        assert_eq!(mark_key(".xmp"), ".xmp");
        assert_eq!(mark_key("a.b.xmp"), "a.b");
        let t = MarkTable::from_sidecars([("Cafe\u{301}.XMP".into(), m(3, Flag::None))]);
        assert_eq!(t.get("CAF\u{c9}.ARW"), Some(m(3, Flag::None)));
        assert_eq!(t.get("Caf\u{e9}"), Some(m(3, Flag::None)));
        assert_eq!(t.get("Other.ARW"), None);
    }

    #[test]
    fn presets() {
        let pick3 = Some(m(3, Flag::Pick));
        let rej4 = Some(m(4, Flag::Reject));
        let two = Some(m(2, Flag::None));
        assert!(MarkFilter::MinRating(3).passes(pick3));
        assert!(!MarkFilter::MinRating(3).passes(rej4));
        assert!(!MarkFilter::MinRating(3).passes(two));
        assert!(!MarkFilter::MinRating(3).passes(None));
        assert!(MarkFilter::Picks.passes(pick3) && !MarkFilter::Picks.passes(two));
        assert!(MarkFilter::Rejects.passes(rej4) && !MarkFilter::Rejects.passes(None));
        assert!(MarkFilter::HideRejects.passes(None) && !MarkFilter::HideRejects.passes(rej4));
        assert!(MarkFilter::All.passes(rej4));
    }

    #[test]
    fn ranks() {
        assert!(rating_rank(Some(m(0, Flag::Reject))) < rating_rank(None));
        assert!(rating_rank(Some(m(5, Flag::Reject))) < rating_rank(Some(m(1, Flag::None))));
        assert!(flag_rank(Some(m(0, Flag::Pick))) < flag_rank(None));
        assert!(flag_rank(None) < flag_rank(Some(m(0, Flag::Reject))));
        let red = Marks {
            label: Some(Label::Red),
            ..Marks::default()
        };
        assert!(label_rank(Some(red)) < label_rank(None));
    }

    #[test]
    fn pairing_needs_an_image() {
        let names = [
            ("dsc1.arw", true),
            ("dsc1.xmp", true),
            ("lonely.xmp", true),
            ("folder", false),
            ("folder.xmp", true),
            ("dsc2.jpg", true),
            ("dsc2.xmp", false), // a folder named like a sidecar
        ];
        let p = paired_stems(names.into_iter());
        assert!(p.contains("dsc1"));
        assert!(!p.contains("lonely"));
        assert!(!p.contains("folder"));
        assert!(!p.contains("dsc2"));
    }

    #[test]
    fn table_equality_is_identity() {
        let a = MarkTable::from_sidecars([("a.xmp".into(), m(1, Flag::None))]);
        let b = MarkTable::from_sidecars([("a.xmp".into(), m(1, Flag::None))]);
        assert_eq!(a, a.clone());
        assert_ne!(a, b);
        assert!(a.same_contents(&b));
        assert_eq!(MarkTable::default(), MarkTable::from_sidecars([]));
    }
}

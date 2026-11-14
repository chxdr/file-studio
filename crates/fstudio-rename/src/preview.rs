//! The live preview: every item's new name and what is wrong with it.
//! Pure: the caller lists each folder's names once, then calls [`preview`]
//! on every keystroke. 10,000 items take a few milliseconds.

use std::collections::{HashMap, HashSet};

use rayon::prelude::*;

use crate::item::ItemInfo;
use crate::name::{invalid_reason, key};
use crate::pipeline::Compiled;
use crate::template::RenderNotes;

/// A folder the batch renames items in.
#[derive(Debug, Clone, Default)]
pub struct Folder {
    /// Whether its volume tells `File` and `file` apart.
    pub case_sensitive: bool,
    /// The comparison keys ([`key`]) of every name in it, the batch's own included.
    pub existing: HashSet<String>,
}

impl Folder {
    /// A folder holding these names.
    pub fn with_names<'a>(case_sensitive: bool, names: impl IntoIterator<Item = &'a str>) -> Self {
        Folder {
            case_sensitive,
            existing: names.into_iter().map(|n| key(n, case_sensitive)).collect(),
        }
    }
}

/// One item of the batch.
#[derive(Debug, Clone)]
pub struct Entry {
    /// What the pipeline reads.
    pub info: ItemInfo,
    /// Its folder: an index into the folders passed to [`preview`].
    pub folder: usize,
    /// Its name's comparison key in that folder.
    pub old_key: String,
}

impl Entry {
    /// An entry in `folders[folder]`.
    pub fn new(info: ItemInfo, folder: usize, folders: &[Folder]) -> Entry {
        let cs = folders.get(folder).is_some_and(|f| f.case_sensitive);
        let old_key = key(&info.name, cs);
        Entry {
            info,
            folder,
            old_key,
        }
    }
}

/// What will happen to one item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// It will be renamed.
    Ready,
    /// The steps don't change its name; it is left alone.
    Unchanged,
    /// The new name is empty.
    Empty,
    /// The new name can't be used (the reason, in words).
    Invalid(&'static str),
    /// Another item in the batch would get the same name.
    Duplicate,
    /// An item that stays in the folder already has the name.
    Exists,
}

impl Status {
    /// Whether this is a problem (the item won't be renamed).
    pub fn is_problem(self) -> bool {
        !matches!(self, Status::Ready | Status::Unchanged)
    }

    /// A short description for the preview's badge.
    pub fn message(self) -> &'static str {
        match self {
            Status::Ready => "",
            Status::Unchanged => "unchanged",
            Status::Empty => "the name is empty",
            Status::Invalid(why) => why,
            Status::Duplicate => "another item gets the same name",
            Status::Exists => "an item with this name is already in the folder",
        }
    }
}

/// One row of the preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The new name.
    pub new_name: String,
    /// What happens.
    pub status: Status,
    /// Token warnings (EXIF missing, iCloud placeholder…).
    pub notes: RenderNotes,
    /// The new name starts with a dot, so Finder will hide it.
    pub becomes_hidden: bool,
}

/// Counts for the sheet's footer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Summary {
    /// Items in the batch.
    pub total: usize,
    /// Items that will be renamed.
    pub ready: usize,
    /// Items whose name doesn't change.
    pub unchanged: usize,
    /// Items with a problem (they won't be renamed).
    pub problems: usize,
    /// Items with a warning.
    pub warnings: usize,
    /// Some `{exif.*}` values aren't read yet.
    pub exif_pending: bool,
}

/// The whole preview.
#[derive(Debug, Clone, Default)]
pub struct Preview {
    /// One per entry, in the same order.
    pub rows: Vec<Row>,
    /// The counts.
    pub summary: Summary,
}

impl Preview {
    /// `(entry index, new name)` for every item that will be renamed.
    pub fn ready(&self) -> Vec<(usize, String)> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.status == Status::Ready)
            .map(|(i, r)| (i, r.new_name.clone()))
            .collect()
    }
}

/// Works out every new name and its problems:
/// - empty or invalid names (`/`, `:`, too long)
/// - two items of the batch getting the same name (case and normalisation
///   folded as the folder's volume does)
/// - a name held by an item that stays (one that isn't in the batch, or one
///   that is but won't be renamed because of its own problem)
///
/// Swaps and cycles (a→b, b→a) are fine: those names are freed by the batch.
/// A case-only change (`File` → `file`) is fine on any volume.
pub fn preview(entries: &[Entry], folders: &[Folder], pipeline: &Compiled) -> Preview {
    let mut rows: Vec<Row> = entries
        .par_iter()
        .enumerate()
        .map(|(i, e)| {
            let a = pipeline.apply(&e.info, i);
            let status = if a.unchanged {
                Status::Unchanged
            } else if a.name.is_empty() {
                Status::Empty
            } else if let Some(why) = invalid_reason(&a.name) {
                Status::Invalid(why)
            } else {
                Status::Ready
            };
            let becomes_hidden =
                !a.unchanged && a.name.starts_with('.') && !e.info.name.starts_with('.');
            Row {
                new_name: a.name,
                status,
                notes: a.notes,
                becomes_hidden,
            }
        })
        .collect();

    let cs = |f: usize| folders.get(f).is_some_and(|f| f.case_sensitive);
    let new_keys: Vec<Option<String>> = rows
        .par_iter()
        .zip(entries.par_iter())
        .map(|(r, e)| (r.status == Status::Ready).then(|| key(&r.new_name, cs(e.folder))))
        .collect();

    // Two items of the batch landing on the same name.
    let mut by_new: HashMap<(usize, &str), Vec<usize>> = HashMap::new();
    for (i, k) in new_keys.iter().enumerate() {
        if let Some(k) = k {
            by_new
                .entry((entries[i].folder, k.as_str()))
                .or_default()
                .push(i);
        }
    }
    for group in by_new.values().filter(|g| g.len() > 1) {
        for &i in group {
            rows[i].status = Status::Duplicate;
        }
    }

    // Names held by items that stay. An item of the batch frees its name
    // only if it is renamed itself, which can depend on another item's
    // problem: repeat until nothing changes.
    let holders: HashMap<(usize, &str), usize> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| ((e.folder, e.old_key.as_str()), i))
        .collect();
    loop {
        let mut changed = false;
        for i in 0..rows.len() {
            if rows[i].status != Status::Ready {
                continue;
            }
            let Some(k) = new_keys[i].as_deref() else {
                continue;
            };
            let e = &entries[i];
            if k == e.old_key {
                continue; // a case-only or normalisation-only change
            }
            let taken = match holders.get(&(e.folder, k)) {
                Some(&j) => rows[j].status != Status::Ready,
                None => folders
                    .get(e.folder)
                    .is_some_and(|f| f.existing.contains(k)),
            };
            if taken {
                rows[i].status = Status::Exists;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut s = Summary {
        total: rows.len(),
        ..Summary::default()
    };
    for r in &rows {
        match r.status {
            Status::Ready => s.ready += 1,
            Status::Unchanged => s.unchanged += 1,
            _ => s.problems += 1,
        }
        if r.notes.exif_missing || r.notes.placeholder || r.becomes_hidden {
            s.warnings += 1;
        }
        s.exif_pending |= r.notes.exif_pending;
    }
    Preview { rows, summary: s }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{CaseKind, Pipeline, Step, Target};

    fn setup(names: &[&str], cs: bool, others: &[&str]) -> (Vec<Entry>, Vec<Folder>) {
        let all: Vec<&str> = names.iter().chain(others).copied().collect();
        let folders = vec![Folder::with_names(cs, all)];
        let entries = names
            .iter()
            .map(|n| Entry::new(ItemInfo::named(n), 0, &folders))
            .collect();
        (entries, folders)
    }

    fn replace(find: &str, rep: &str, regex: bool) -> Compiled {
        Compiled::new(&Pipeline::of(vec![Step::Replace {
            find: find.into(),
            replace: rep.into(),
            regex,
            ignore_case: false,
            first_only: false,
            target: Target::Whole,
        }]))
        .unwrap()
    }

    fn statuses(p: &Preview) -> Vec<Status> {
        p.rows.iter().map(|r| r.status).collect()
    }

    #[test]
    fn swaps_and_cycles_are_ready() {
        let (e, f) = setup(&["a", "b"], false, &[]);
        // a→b, b→a
        let p = preview(&e, &f, &replace_map(&[("a", "b"), ("b", "a")]));
        assert_eq!(statuses(&p), vec![Status::Ready, Status::Ready]);
        let (e, f) = setup(&["1", "2", "3"], false, &[]);
        let p = preview(&e, &f, &replace_map(&[("1", "2"), ("2", "3"), ("3", "1")]));
        assert_eq!(p.summary.ready, 3);
    }

    /// A pipeline that maps whole names through a table (regex alternation).
    fn replace_map(pairs: &[(&str, &str)]) -> Compiled {
        // Map via a temporary marker so the steps don't chain into each other.
        let mut steps = Vec::new();
        for (i, (from, _)) in pairs.iter().enumerate() {
            steps.push(Step::Replace {
                find: format!("^{}$", regex::escape(from)),
                replace: format!("\u{1}{i}\u{1}"),
                regex: true,
                ignore_case: false,
                first_only: false,
                target: Target::Whole,
            });
        }
        for (i, (_, to)) in pairs.iter().enumerate() {
            steps.push(Step::Replace {
                find: format!("\u{1}{i}\u{1}"),
                replace: (*to).into(),
                regex: false,
                ignore_case: false,
                first_only: false,
                target: Target::Whole,
            });
        }
        Compiled::new(&Pipeline::of(steps)).unwrap()
    }

    #[test]
    fn collisions_within_the_batch() {
        let (e, f) = setup(&["a1.txt", "a2.txt", "b.txt"], true, &[]);
        let p = preview(&e, &f, &replace(r"\d", "", true));
        assert_eq!(
            statuses(&p),
            vec![Status::Duplicate, Status::Duplicate, Status::Unchanged]
        );
    }

    #[test]
    fn names_differing_only_by_case_collide_on_insensitive_volumes() {
        let c = Compiled::new(&Pipeline::of(vec![Step::Case {
            case: CaseKind::Lower,
            target: Target::Whole,
        }]))
        .unwrap();
        // "X.TXT" → "x.txt" while an unrelated "x.txt" stays (only possible
        // on a case-sensitive volume).
        let (e, f) = setup(&["A.txt", "X.TXT"], true, &["x.txt"]);
        let p = preview(&e, &f, &c);
        assert_eq!(statuses(&p), vec![Status::Ready, Status::Exists]);
        // Two batch items folding to the same name.
        let (e, f) = setup(&["Photo.jpg", "PHOTO.JPG"], true, &[]);
        assert_eq!(
            statuses(&preview(&e, &f, &c)),
            vec![Status::Duplicate, Status::Duplicate]
        );
        // On an insensitive volume, an existing "notes.TXT" blocks "notes.txt".
        let up = replace("a", "notes", false);
        let (e, f) = setup(&["a.txt"], false, &["NOTES.TXT"]);
        assert_eq!(statuses(&preview(&e, &f, &up)), vec![Status::Exists]);
        // …but not on a case-sensitive one.
        let (e, f) = setup(&["a.txt"], true, &["NOTES.TXT"]);
        assert_eq!(statuses(&preview(&e, &f, &up)), vec![Status::Ready]);
    }

    #[test]
    fn case_only_renames_are_ready_everywhere() {
        let c = Compiled::new(&Pipeline::of(vec![Step::Case {
            case: CaseKind::Upper,
            target: Target::Whole,
        }]))
        .unwrap();
        for cs in [false, true] {
            let (e, f) = setup(&["file.txt"], cs, &[]);
            let p = preview(&e, &f, &c);
            assert_eq!(statuses(&p), vec![Status::Ready]);
            assert_eq!(p.rows[0].new_name, "FILE.TXT");
        }
    }

    #[test]
    fn nfd_and_nfc_are_the_same_name() {
        // An existing NFC "Café.txt" blocks an NFD rename to the same name.
        let (e, f) = setup(&["y.txt"], true, &["Caf\u{e9}.txt"]);
        let p = preview(&e, &f, &replace("y", "Cafe\u{301}", false));
        assert_eq!(statuses(&p), vec![Status::Exists]);
        // NFD on disk → NFC is not a change.
        let (e, f) = setup(&["Cafe\u{301}.txt"], true, &[]);
        let p = preview(&e, &f, &replace("zzz", "", false));
        assert_eq!(statuses(&p), vec![Status::Unchanged]);
    }

    #[test]
    fn existing_files_and_blocked_chains() {
        // a→b where b stays (not in the batch).
        let (e, f) = setup(&["a"], false, &["b"]);
        assert_eq!(
            statuses(&preview(&e, &f, &replace("a", "b", false))),
            vec![Status::Exists]
        );
        // x→y, y→"" (invalid): y stays, so x is blocked too.
        let (e, f) = setup(&["x", "y"], false, &[]);
        let p = preview(&e, &f, &replace_map(&[("x", "y"), ("y", "")]));
        assert_eq!(statuses(&p), vec![Status::Exists, Status::Empty]);
        // A chain through a renamed item is fine: a→b, b→c.
        let (e, f) = setup(&["a", "b"], false, &[]);
        let p = preview(&e, &f, &replace_map(&[("a", "b"), ("b", "c")]));
        assert_eq!(statuses(&p), vec![Status::Ready, Status::Ready]);
    }

    #[test]
    fn invalid_names_and_warnings() {
        let (e, f) = setup(&["a.txt", "b.txt", "c.txt"], false, &[]);
        let p = preview(
            &e,
            &f,
            &replace_map(&[("a.txt", "a/b"), ("b.txt", "x:y"), ("c.txt", ".c")]),
        );
        assert!(matches!(p.rows[0].status, Status::Invalid(_)));
        assert!(matches!(p.rows[1].status, Status::Invalid(_)));
        assert_eq!(p.rows[2].status, Status::Ready);
        assert!(p.rows[2].becomes_hidden);
        assert_eq!(p.summary.problems, 2);
        assert_eq!(p.summary.warnings, 1);
    }

    #[test]
    fn items_in_different_folders_never_collide() {
        let folders = vec![
            Folder::with_names(false, ["a.txt"]),
            Folder::with_names(false, ["b.txt"]),
        ];
        let e = vec![
            Entry::new(ItemInfo::named("a.txt"), 0, &folders),
            Entry::new(ItemInfo::named("b.txt"), 1, &folders),
        ];
        let c = Compiled::new(&Pipeline::of(vec![Step::Template {
            template: "same".into(),
        }]))
        .unwrap();
        assert_eq!(statuses(&preview(&e, &folders, &c)), vec![Status::Ready; 2]);
    }

    /// 10,000 items must preview fast enough for every keystroke.
    #[test]
    fn ten_thousand_items_preview_fast() {
        let names: Vec<String> = (0..10_000).map(|i| format!("IMG_{i:05}.JPG")).collect();
        let folders = vec![Folder::with_names(false, names.iter().map(String::as_str))];
        let entries: Vec<Entry> = names
            .iter()
            .map(|n| Entry::new(ItemInfo::named(n), 0, &folders))
            .collect();
        let c = Compiled::new(&Pipeline::of(vec![
            Step::Replace {
                find: r"IMG_(\d+)".into(),
                replace: "Trip $1".into(),
                regex: true,
                ignore_case: false,
                first_only: false,
                target: Target::Name,
            },
            Step::Case {
                case: CaseKind::Lower,
                target: Target::Extension,
            },
            Step::Template {
                template: "{name} {n:5}".into(),
            },
        ]))
        .unwrap();
        // Warm up rayon's pool, then time the best of three.
        let _ = preview(&entries, &folders, &c);
        let best = (0..3)
            .map(|_| {
                let t = std::time::Instant::now();
                let p = preview(&entries, &folders, &c);
                assert_eq!(p.summary.ready, 10_000);
                t.elapsed()
            })
            .min()
            .unwrap();
        eprintln!("10k preview: {best:?}");
        // One frame in release; debug builds are ~10× slower.
        let budget = if cfg!(debug_assertions) { 400 } else { 16 };
        assert!(best.as_millis() < budget, "10k preview took {best:?}");
    }
}

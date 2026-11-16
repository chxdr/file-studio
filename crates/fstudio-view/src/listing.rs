//! Immutable, versioned listing snapshots.
//!
//! A [`Listing`] holds every entry of a folder plus the sorted, filtered view of them.
//! It is cheap to share (`Arc` inside) and never changes; [`Listing::apply`] and
//! [`Listing::with_options`] return a new version that shares what they can.
//!
//! Entries are kept as `Arc<Item>` rather than a plain `Arc<[Entry]>` so that an incremental
//! update only copies pointers (about 0.5 ms for 100k entries) instead of cloning every
//! `Entry` and its name, and so that unchanged entries are shared between versions.

use crate::collation::{Collation, NaturalCollation, fold};
use crate::marks::{self, Marks, folded_stem, is_sidecar_name};
use crate::options::{Filter, SortKey, ViewOptions};
use fstudio_model::{Entry, EntryKind};
use rayon::prelude::*;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::Arc;

/// Folded stems whose `.xmp` sidecar is hidden (see [`marks::paired_stems`]).
type Paired = HashSet<Box<str>>;

/// Above this many entries, key building and sorting use all cores.
pub const PARALLEL_THRESHOLD: usize = 20_000;

/// One entry with its precomputed comparison keys.
#[derive(Debug)]
pub(crate) struct Item {
    pub(crate) entry: Entry,
    pub(crate) fold: Box<str>,
    pub(crate) key: Option<Box<[u8]>>,
}

impl Item {
    fn new(entry: Entry, coll: &dyn Collation) -> Item {
        let fold: Box<str> = fold(&entry.name).into_boxed_str();
        let key = coll.key(&entry.name, &fold);
        Item { entry, fold, key }
    }
    fn sort_key(&self) -> &[u8] {
        self.key.as_deref().unwrap_or(self.fold.as_bytes())
    }
}

/// A change to apply to a listing.
#[derive(Debug, Clone)]
pub enum Change {
    /// Insert the entry, or replace the one with the same name.
    Upsert(Entry),
    /// Remove the entry with this exact name (a no-op if it isn't there).
    Remove(String),
}

struct Sorter<'a> {
    opts: &'a ViewOptions,
    coll: &'a dyn Collation,
}

impl Sorter<'_> {
    fn is_folder(&self, e: &Entry) -> bool {
        e.kind == EntryKind::Dir && !(e.is_package && self.opts.packages_as_files)
    }

    fn kind_rank(&self, e: &Entry) -> u8 {
        if self.is_folder(e) {
            0
        } else if e.kind == EntryKind::Dir {
            1 // a package shown as such
        } else {
            match e.kind {
                EntryKind::File => 2,
                EntryKind::Symlink => 3,
                _ => 4,
            }
        }
    }

    /// A total order: names are unique in a folder, so the final raw-name tie-break
    /// means no two entries ever compare equal. That is what makes incremental updates
    /// give exactly the same result as a full sort.
    fn cmp(&self, a: &Item, b: &Item) -> Ordering {
        if self.opts.folders_first {
            let (fa, fb) = (self.is_folder(&a.entry), self.is_folder(&b.entry));
            if fa != fb {
                return fb.cmp(&fa);
            }
        }
        let dir = |o: Ordering| if self.opts.ascending { o } else { o.reverse() };
        let name = |a: &Item, b: &Item| self.coll.compare(a.sort_key(), b.sort_key());
        let (ea, eb) = (&a.entry, &b.entry);
        let primary = match self.opts.sort_key {
            SortKey::Name => dir(name(a, b)),
            SortKey::Size => {
                let sz = |e: &Entry| if self.is_folder(e) { 0 } else { e.size };
                dir(sz(ea).cmp(&sz(eb)))
            }
            SortKey::Modified => dir(ea.mtime_ns.cmp(&eb.mtime_ns)),
            SortKey::Created => dir(ea
                .btime_ns
                .unwrap_or(ea.mtime_ns)
                .cmp(&eb.btime_ns.unwrap_or(eb.mtime_ns))),
            SortKey::Kind => dir(self
                .kind_rank(ea)
                .cmp(&self.kind_rank(eb))
                .then_with(|| ext_cmp(ea, eb))),
            SortKey::Extension => dir(ext_cmp(ea, eb)),
            SortKey::Rating => dir(marks::rating_rank(item_marks(self.opts, a))
                .cmp(&marks::rating_rank(item_marks(self.opts, b)))),
            SortKey::Label => dir(marks::label_rank(item_marks(self.opts, a))
                .cmp(&marks::label_rank(item_marks(self.opts, b)))),
            SortKey::Flag => dir(marks::flag_rank(item_marks(self.opts, a))
                .cmp(&marks::flag_rank(item_marks(self.opts, b)))),
        };
        primary
            .then_with(|| {
                if self.opts.sort_key == SortKey::Name {
                    Ordering::Equal
                } else {
                    name(a, b)
                }
            })
            .then_with(|| ea.name.as_bytes().cmp(eb.name.as_bytes()))
    }
}

fn ext_cmp(a: &Entry, b: &Entry) -> Ordering {
    let lower = |e: &Entry| {
        e.extension()
            .chars()
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    // Cheap path: both extensions are ASCII, compare without allocating.
    let (x, y) = (a.extension(), b.extension());
    if x.is_ascii() && y.is_ascii() {
        x.bytes()
            .map(|c| c.to_ascii_lowercase())
            .cmp(y.bytes().map(|c| c.to_ascii_lowercase()))
    } else {
        lower(a).cmp(&lower(b))
    }
}

/// The sidecar marks that apply to an item. Folders never have any.
fn item_marks(opts: &ViewOptions, item: &Item) -> Option<Marks> {
    if item.entry.kind == EntryKind::Dir {
        None
    } else {
        opts.marks.get_folded(&item.fold)
    }
}

fn is_sidecar_item(item: &Item) -> bool {
    item.entry.kind == EntryKind::File && is_sidecar_name(&item.fold)
}

fn visible(item: &Item, opts: &ViewOptions, filter: &Filter, paired: &Paired) -> bool {
    (opts.show_hidden || !item.entry.is_hidden())
        && filter.matches(&item.fold)
        && !(opts.hide_sidecars
            && !paired.is_empty()
            && is_sidecar_item(item)
            && paired.contains(folded_stem(&item.fold)))
        && (opts.mark_filter == marks::MarkFilter::All
            || item.entry.kind == EntryKind::Dir
            || opts.mark_filter.passes(item_marks(opts, item)))
}

/// The paired sidecar stems of a folder, when the options hide them.
fn compute_paired(by_name: &[Arc<Item>], opts: &ViewOptions) -> Paired {
    if !opts.hide_sidecars {
        return Paired::new();
    }
    marks::paired_stems(
        by_name
            .iter()
            .map(|i| (&*i.fold, i.entry.kind == EntryKind::File)),
    )
}

/// True when hiding sidecars needs the whole view recomputed: they are
/// hidden and the folder has (or the change touches) a sidecar.
fn pairing_matters(by_name: &[Arc<Item>], opts: &ViewOptions) -> bool {
    opts.hide_sidecars && by_name.iter().any(|i| is_sidecar_item(i))
}

/// Every visible entry of `by_name`, sorted.
fn full_view(
    by_name: &[Arc<Item>],
    opts: &ViewOptions,
    filter: &Filter,
    paired: &Paired,
    s: &Sorter,
) -> Vec<Arc<Item>> {
    let mut view: Vec<Arc<Item>> = by_name
        .iter()
        .filter(|i| visible(i, opts, filter, paired))
        .cloned()
        .collect();
    sort_items(&mut view, s);
    view
}

fn sort_items(v: &mut [Arc<Item>], s: &Sorter) {
    if v.len() >= PARALLEL_THRESHOLD {
        v.par_sort_unstable_by(|a, b| s.cmp(a, b));
    } else {
        v.sort_unstable_by(|a, b| s.cmp(a, b));
    }
}

fn name_cmp(a: &Item, b: &Item) -> Ordering {
    a.entry.name.as_bytes().cmp(b.entry.name.as_bytes())
}

fn merge(
    old: Vec<Arc<Item>>,
    new: Vec<Arc<Item>>,
    cmp: impl Fn(&Item, &Item) -> Ordering,
) -> Vec<Arc<Item>> {
    if old.is_empty() {
        return new;
    }
    if new.is_empty() {
        return old;
    }
    let mut out = Vec::with_capacity(old.len() + new.len());
    let (mut a, mut b) = (old.into_iter().peekable(), new.into_iter().peekable());
    while let (Some(x), Some(y)) = (a.peek(), b.peek()) {
        if cmp(x, y) != Ordering::Greater {
            out.push(a.next().unwrap());
        } else {
            out.push(b.next().unwrap());
        }
    }
    out.extend(a);
    out.extend(b);
    out
}

/// An immutable, versioned, sorted and filtered snapshot of a folder.
#[derive(Clone)]
pub struct Listing {
    version: u64,
    opts: Arc<ViewOptions>,
    filter: Arc<Filter>,
    coll: Arc<dyn Collation>,
    /// Folded stems whose sidecar is hidden (empty unless sidecars are hidden).
    paired: Arc<Paired>,
    /// Every entry, ordered by exact name bytes (for lookup).
    by_name: Arc<Vec<Arc<Item>>>,
    /// The visible entries in display order.
    view: Arc<Vec<Arc<Item>>>,
}

impl std::fmt::Debug for Listing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Listing")
            .field("version", &self.version)
            .field("total", &self.by_name.len())
            .field("visible", &self.view.len())
            .finish()
    }
}

impl Listing {
    /// Builds a listing from all entries with the default collation. Version 1.
    pub fn build(entries: Vec<Entry>, opts: ViewOptions) -> Listing {
        let mut b = ListingBuilder::new(opts);
        b.push_chunk(entries);
        b.finish()
    }

    /// An empty listing (version 0).
    pub fn empty(opts: ViewOptions) -> Listing {
        ListingBuilder::new(opts).snapshot_at(0)
    }

    /// Monotonic version: each build, `apply` and `with_options` yields a higher one.
    pub fn version(&self) -> u64 {
        self.version
    }
    /// The options this listing was built with.
    pub fn options(&self) -> &ViewOptions {
        &self.opts
    }
    /// How many rows are visible (after filtering).
    pub fn len(&self) -> usize {
        self.view.len()
    }
    /// True when no rows are visible.
    pub fn is_empty(&self) -> bool {
        self.view.is_empty()
    }
    /// How many entries the folder has, including hidden and filtered-out ones.
    pub fn total_len(&self) -> usize {
        self.by_name.len()
    }
    /// The visible entry at row `i`.
    pub fn entry(&self, i: usize) -> Option<&Entry> {
        self.view.get(i).map(|it| &it.entry)
    }
    /// The row of the visible entry with exactly this name. O(log n).
    pub fn index_of(&self, name: &str) -> Option<usize> {
        let item = self.find(name)?;
        if !visible(item, &self.opts, &self.filter, &self.paired) {
            return None;
        }
        let s = Sorter {
            opts: &self.opts,
            coll: &*self.coll,
        };
        self.view.binary_search_by(|x| s.cmp(x, item)).ok()
    }
    /// Any entry of the folder by exact name, even if hidden or filtered out.
    pub fn get(&self, name: &str) -> Option<&Entry> {
        self.find(name).map(|i| &i.entry)
    }
    /// `count` visible rows starting at `start` (clamped to the end).
    pub fn rows(&self, start: usize, count: usize) -> impl Iterator<Item = &Entry> {
        let s = start.min(self.view.len());
        let e = s.saturating_add(count).min(self.view.len());
        self.view[s..e].iter().map(|i| &i.entry)
    }
    /// All visible entries in order.
    pub fn iter(&self) -> impl Iterator<Item = &Entry> {
        self.view.iter().map(|i| &i.entry)
    }
    /// The folded (NFC, lower-case) name of row `i`, for prefix matching.
    pub fn folded_name(&self, i: usize) -> Option<&str> {
        self.view.get(i).map(|it| &*it.fold)
    }

    /// The sidecar marks of row `i`, if its sidecar has been read.
    pub fn marks_at(&self, i: usize) -> Option<Marks> {
        self.view.get(i).and_then(|it| item_marks(&self.opts, it))
    }
    /// True when row `i`'s sidecar is hidden and its marks show on this row.
    pub fn has_hidden_sidecar(&self, i: usize) -> bool {
        self.view.get(i).is_some_and(|it| {
            it.entry.kind != EntryKind::Dir
                && !is_sidecar_item(it)
                && self.paired.contains(folded_stem(&it.fold))
        })
    }
    /// Every `.xmp` file of the folder, visible or not, by name order.
    pub fn sidecar_entries(&self) -> impl Iterator<Item = &Entry> {
        self.by_name
            .iter()
            .filter(|i| is_sidecar_item(i))
            .map(|i| &i.entry)
    }

    fn find(&self, name: &str) -> Option<&Arc<Item>> {
        self.by_name
            .binary_search_by(|x| x.entry.name.as_bytes().cmp(name.as_bytes()))
            .ok()
            .map(|i| &self.by_name[i])
    }

    /// The same entries under different options (re-sorted and re-filtered; keys are reused).
    pub fn with_options(&self, opts: ViewOptions) -> Listing {
        let filter = Filter::compile(&opts.filter);
        let s = Sorter {
            opts: &opts,
            coll: &*self.coll,
        };
        let paired = compute_paired(&self.by_name, &opts);
        let view = full_view(&self.by_name, &opts, &filter, &paired, &s);
        Listing {
            version: self.version + 1,
            opts: Arc::new(opts),
            filter: Arc::new(filter),
            coll: self.coll.clone(),
            paired: Arc::new(paired),
            by_name: self.by_name.clone(),
            view: Arc::new(view),
        }
    }

    /// Applies changes and returns the next version. Each change costs O(log n) comparisons
    /// plus a pointer shift; the pointer arrays are copied once per call (not per change).
    /// For a very large batch (more than about 5% of the folder) a full rebuild is faster.
    pub fn apply(&self, changes: impl IntoIterator<Item = Change>) -> Listing {
        let changes: Vec<Change> = changes.into_iter().collect();
        let touches_sidecar = changes.iter().any(|c| match c {
            Change::Upsert(e) => is_sidecar_name(&e.name),
            Change::Remove(n) => is_sidecar_name(n),
        });
        if self.opts.hide_sidecars && touches_sidecar || pairing_matters(&self.by_name, &self.opts)
        {
            return self.apply_full(changes);
        }
        let mut next = self.clone();
        next.version += 1;
        let by_name = Arc::make_mut(&mut next.by_name);
        let view = Arc::make_mut(&mut next.view);
        let s = Sorter {
            opts: &self.opts,
            coll: &*self.coll,
        };
        let paired = &*self.paired;
        let remove_from_view = |view: &mut Vec<Arc<Item>>, old: &Arc<Item>| {
            if visible(old, &self.opts, &self.filter, paired)
                && let Ok(pos) = view.binary_search_by(|x| s.cmp(x, old))
            {
                view.remove(pos);
            }
        };
        for ch in changes {
            match ch {
                Change::Upsert(entry) => {
                    let new = Arc::new(Item::new(entry, &*self.coll));
                    match by_name.binary_search_by(|x| name_cmp(x, &new)) {
                        Ok(i) => {
                            let old = std::mem::replace(&mut by_name[i], new.clone());
                            remove_from_view(view, &old);
                        }
                        Err(i) => by_name.insert(i, new.clone()),
                    }
                    if visible(&new, &self.opts, &self.filter, paired) {
                        let pos = view
                            .binary_search_by(|x| s.cmp(x, &new))
                            .unwrap_or_else(|p| p);
                        view.insert(pos, new);
                    }
                }
                Change::Remove(name) => {
                    if let Ok(i) =
                        by_name.binary_search_by(|x| x.entry.name.as_bytes().cmp(name.as_bytes()))
                    {
                        let old = by_name.remove(i);
                        remove_from_view(view, &old);
                    }
                }
            }
        }
        next
    }

    /// `apply` for a folder whose sidecars are hidden: a change to one entry
    /// can show or hide another (an image arriving hides its sidecar), so the
    /// view is recomputed from all entries.
    fn apply_full(&self, changes: Vec<Change>) -> Listing {
        let mut next = self.clone();
        next.version += 1;
        let by_name = Arc::make_mut(&mut next.by_name);
        for ch in changes {
            match ch {
                Change::Upsert(entry) => {
                    let new = Arc::new(Item::new(entry, &*self.coll));
                    match by_name.binary_search_by(|x| name_cmp(x, &new)) {
                        Ok(i) => by_name[i] = new,
                        Err(i) => by_name.insert(i, new),
                    }
                }
                Change::Remove(name) => {
                    if let Ok(i) =
                        by_name.binary_search_by(|x| x.entry.name.as_bytes().cmp(name.as_bytes()))
                    {
                        by_name.remove(i);
                    }
                }
            }
        }
        let s = Sorter {
            opts: &self.opts,
            coll: &*self.coll,
        };
        let paired = compute_paired(by_name, &self.opts);
        next.view = Arc::new(full_view(by_name, &self.opts, &self.filter, &paired, &s));
        next.paired = Arc::new(paired);
        next
    }
}

/// Builds a listing from chunks as they stream in, so rows can be shown before the folder is
/// fully read.
///
/// [`push_chunk`](ListingBuilder::push_chunk) only collects entries (cheaply). Sorting happens in
/// [`snapshot`](ListingBuilder::snapshot) (new entries are sorted and merged into the
/// previous snapshot, O(n)) and in [`finish`](ListingBuilder::finish) (one sort of everything
/// that is left). So take a snapshot for the first rows, then every ~100 ms at most, and finish once.
pub struct ListingBuilder {
    opts: Arc<ViewOptions>,
    filter: Arc<Filter>,
    coll: Arc<dyn Collation>,
    paired: Arc<Paired>,
    by_name: Vec<Arc<Item>>,
    view: Vec<Arc<Item>>,
    /// Entries pushed since the last flush, in arrival order.
    pending: Vec<Arc<Item>>,
    version: u64,
}

impl ListingBuilder {
    /// A builder with the default collation.
    pub fn new(opts: ViewOptions) -> Self {
        Self::with_collation(opts, Arc::new(NaturalCollation))
    }

    /// A builder with a custom collation.
    pub fn with_collation(opts: ViewOptions, coll: Arc<dyn Collation>) -> Self {
        ListingBuilder {
            filter: Arc::new(Filter::compile(&opts.filter)),
            opts: Arc::new(opts),
            coll,
            paired: Arc::new(Paired::new()),
            by_name: Vec::new(),
            view: Vec::new(),
            pending: Vec::new(),
            version: 0,
        }
    }

    /// Adds a chunk of entries. A name that was already added is replaced by the newer entry.
    pub fn push_chunk(&mut self, entries: Vec<Entry>) {
        let coll = &*self.coll;
        if entries.len() >= PARALLEL_THRESHOLD {
            let items: Vec<Arc<Item>> = entries
                .into_par_iter()
                .map(|e| Arc::new(Item::new(e, coll)))
                .collect();
            self.pending.extend(items);
        } else {
            self.pending
                .extend(entries.into_iter().map(|e| Arc::new(Item::new(e, coll))));
        }
    }

    /// Sorts what was pushed and merges it into the sorted state.
    fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let mut items = std::mem::take(&mut self.pending);
        // Within what was pushed, the last entry for a name wins.
        if items.len() >= PARALLEL_THRESHOLD {
            items.par_sort_by(|a, b| name_cmp(a, b));
        } else {
            items.sort_by(|a, b| name_cmp(a, b));
        }
        items.reverse();
        items.dedup_by(|a, b| name_cmp(a, b) == Ordering::Equal);
        items.reverse();
        let s = Sorter {
            opts: &self.opts,
            coll: &*self.coll,
        };
        // Names already in the sorted state are replaced.
        if !self.by_name.is_empty() {
            for n in &items {
                if let Ok(i) = self.by_name.binary_search_by(|x| name_cmp(x, n)) {
                    let old = self.by_name.remove(i);
                    if let Ok(p) = self.view.binary_search_by(|x| s.cmp(x, &old)) {
                        self.view.remove(p);
                    }
                }
            }
        }
        let new_sidecar = self.opts.hide_sidecars && items.iter().any(|i| is_sidecar_item(i));
        if new_sidecar || pairing_matters(&self.by_name, &self.opts) {
            // Pairing can hide entries already shown: recompute the view.
            self.by_name = merge(std::mem::take(&mut self.by_name), items, name_cmp);
            let paired = compute_paired(&self.by_name, &self.opts);
            self.view = full_view(&self.by_name, &self.opts, &self.filter, &paired, &s);
            self.paired = Arc::new(paired);
            return;
        }
        let none = Paired::new();
        let mut vis: Vec<Arc<Item>> = items
            .iter()
            .filter(|i| visible(i, &self.opts, &self.filter, &none))
            .cloned()
            .collect();
        sort_items(&mut vis, &s);
        self.view = merge(std::mem::take(&mut self.view), vis, |a, b| s.cmp(a, b));
        self.by_name = merge(std::mem::take(&mut self.by_name), items, name_cmp);
    }

    fn snapshot_at(&self, version: u64) -> Listing {
        Listing {
            version,
            opts: self.opts.clone(),
            filter: self.filter.clone(),
            coll: self.coll.clone(),
            paired: self.paired.clone(),
            by_name: Arc::new(self.by_name.clone()),
            view: Arc::new(self.view.clone()),
        }
    }

    /// A snapshot of everything pushed so far (a new version each time). Costs O(n) (it sorts
    /// the new entries, merges them in and copies two pointer arrays), so call it for the first
    /// rows and then at most every ~100 ms.
    pub fn snapshot(&mut self) -> Listing {
        self.flush();
        self.version += 1;
        self.snapshot_at(self.version)
    }

    /// The final listing. Consumes the builder without copying.
    pub fn finish(mut self) -> Listing {
        self.flush();
        self.version += 1;
        Listing {
            version: self.version,
            opts: self.opts,
            filter: self.filter,
            coll: self.coll,
            paired: self.paired,
            by_name: Arc::new(self.by_name),
            view: Arc::new(self.view),
        }
    }
}

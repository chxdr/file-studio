//! The selection model: a cursor, an anchor and a set of selected rows.
//!
//! Everything is expressed in row indices of one [`Listing`] version; [`Selection::remap`]
//! carries a selection to a newer version by name.

use crate::listing::Listing;
use crate::options::glob_match;
use std::collections::BTreeSet;

/// A cursor movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    /// One row up.
    Up,
    /// One row down.
    Down,
    /// One page up.
    PageUp,
    /// One page down.
    PageDown,
    /// First row.
    Home,
    /// Last row.
    End,
}

/// Cursor, anchor and selected rows.
///
/// The cursor is the row that has focus; the anchor is the fixed end of a Shift-extended range.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    cursor: Option<usize>,
    anchor: Option<usize>,
    selected: BTreeSet<usize>,
}

impl Selection {
    /// An empty selection with no cursor.
    pub fn new() -> Self {
        Self::default()
    }

    /// The focused row.
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }
    /// The fixed end of a range selection.
    pub fn anchor(&self) -> Option<usize> {
        self.anchor
    }
    /// Is row `i` selected?
    pub fn is_selected(&self, i: usize) -> bool {
        self.selected.contains(&i)
    }
    /// How many rows are selected.
    pub fn count(&self) -> usize {
        self.selected.len()
    }
    /// True when nothing is selected.
    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }
    /// Selected rows in ascending order.
    pub fn selected(&self) -> impl Iterator<Item = usize> + '_ {
        self.selected.iter().copied()
    }
    /// Names of the selected rows, in display order.
    pub fn names(&self, listing: &Listing) -> Vec<String> {
        self.selected()
            .filter_map(|i| listing.entry(i).map(|e| e.name.clone()))
            .collect()
    }

    /// Plain click: select only row `i`.
    pub fn select_only(&mut self, i: usize) {
        self.selected.clear();
        self.selected.insert(i);
        self.cursor = Some(i);
        self.anchor = Some(i);
    }

    /// Command-click: toggle row `i` and move the cursor and anchor to it.
    pub fn toggle(&mut self, i: usize) {
        if !self.selected.remove(&i) {
            self.selected.insert(i);
        }
        self.cursor = Some(i);
        self.anchor = Some(i);
    }

    /// Shift-click: select the range from the anchor to `i` (just `i` when there is no anchor).
    pub fn extend_to(&mut self, i: usize) {
        let a = self.anchor.unwrap_or(i);
        self.selected = range(a, i).collect();
        self.cursor = Some(i);
        self.anchor = Some(a);
    }

    /// Moves the cursor. Without `extend` the selection becomes the new cursor row; with it (Shift)
    /// the selection becomes the range from the anchor to the cursor. The cursor stops at the ends.
    /// With nothing focused, Down/PageDown/Home go to the first row and Up/PageUp/End to the last.
    /// `page` is the number of rows a page moves (at least 1).
    pub fn move_cursor(&mut self, len: usize, mv: Move, page: usize, extend: bool) {
        if len == 0 {
            *self = Selection::new();
            return;
        }
        let last = len - 1;
        let page = page.max(1);
        let target = match (self.cursor, mv) {
            (_, Move::Home) => 0,
            (_, Move::End) => last,
            (None, Move::Down | Move::PageDown) => 0,
            (None, Move::Up | Move::PageUp) => last,
            (Some(c), Move::Down) => (c + 1).min(last),
            (Some(c), Move::Up) => c.saturating_sub(1),
            (Some(c), Move::PageDown) => (c + page).min(last),
            (Some(c), Move::PageUp) => c.saturating_sub(page),
        };
        let target = target.min(last);
        if extend {
            let a = self.anchor.or(self.cursor).unwrap_or(target);
            self.selected = range(a, target).collect();
            self.anchor = Some(a);
            self.cursor = Some(target);
        } else {
            self.select_only(target);
        }
    }

    /// Selects every row. The cursor and anchor are kept (or set to the first row).
    pub fn select_all(&mut self, len: usize) {
        self.selected = (0..len).collect();
        if len > 0 {
            self.cursor = Some(self.cursor.map_or(0, |c| c.min(len - 1)));
            self.anchor = self.anchor.or(Some(0)).map(|a| a.min(len - 1));
        }
    }

    /// Clears the selection (the cursor stays).
    pub fn select_none(&mut self) {
        self.selected.clear();
        self.anchor = self.cursor;
    }

    /// Selects every unselected row and deselects the selected ones.
    pub fn invert(&mut self, len: usize) {
        self.selected = (0..len).filter(|i| !self.selected.contains(i)).collect();
    }

    /// Adds every visible row whose name matches the glob (`*`, `?`) to the selection.
    pub fn select_glob(&mut self, listing: &Listing, pattern: &str) {
        for (i, e) in listing.iter().enumerate() {
            if glob_match(pattern, &e.name) {
                self.selected.insert(i);
            }
        }
        if self.cursor.is_none() {
            self.cursor = self.selected.iter().next().copied();
            self.anchor = self.cursor;
        }
    }

    /// Removes every row whose name matches the glob from the selection.
    pub fn deselect_glob(&mut self, listing: &Listing, pattern: &str) {
        let drop: Vec<usize> = self
            .selected
            .iter()
            .copied()
            .filter(|&i| {
                listing
                    .entry(i)
                    .is_some_and(|e| glob_match(pattern, &e.name))
            })
            .collect();
        for i in drop {
            self.selected.remove(&i);
        }
    }

    /// After rows `removed` (row numbers in the *old* listing of `old_len` rows) are deleted,
    /// moves the selection to the item that followed the last removed row, or, if there is none,
    /// the nearest surviving item before it. Returns the new cursor row in the **new** listing
    /// (`None` when nothing is left), and leaves just that row selected.
    pub fn after_remove(&mut self, removed: &[usize], old_len: usize) -> Option<usize> {
        let gone: BTreeSet<usize> = removed.iter().copied().filter(|&i| i < old_len).collect();
        let Some(&last) = gone.iter().next_back() else {
            return self.cursor;
        };
        let next = (last + 1..old_len).find(|i| !gone.contains(i));
        let prev = (0..last).rev().find(|i| !gone.contains(i));
        let pick = next.or(prev);
        match pick {
            None => {
                *self = Selection::new();
                None
            }
            Some(old) => {
                let new = old - gone.range(..old).count();
                self.select_only(new);
                Some(new)
            }
        }
    }

    /// Carries the selection from `old` to a newer listing by name. Rows whose entry is gone (or
    /// hidden) are dropped. If the cursor's entry is gone, the cursor goes to the nearest row at the
    /// same position.
    pub fn remap(&self, old: &Listing, new: &Listing) -> Selection {
        let map = |i: usize| old.entry(i).and_then(|e| new.index_of(&e.name));
        let selected: BTreeSet<usize> = self.selected.iter().filter_map(|&i| map(i)).collect();
        let fallback = |i: usize| (!new.is_empty()).then(|| i.min(new.len() - 1));
        let cursor = self.cursor.and_then(|c| map(c).or_else(|| fallback(c)));
        let anchor = self.anchor.and_then(map).or(cursor);
        Selection {
            cursor,
            anchor,
            selected,
        }
    }
}

fn range(a: usize, b: usize) -> impl Iterator<Item = usize> {
    a.min(b)..=a.max(b)
}

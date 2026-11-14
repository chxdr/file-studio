//! The tree list's rows: which folders are expanded, the visible rows in
//! order, and how the keyboard moves the selection. Pure (no I/O).

use std::collections::{HashMap, HashSet};

use crate::tree::{NO_NODE, NodeId, NodeKind, Tree};

/// One visible row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutlineRow {
    /// The node.
    pub id: NodeId,
    /// Indent level (the root row is 0).
    pub depth: u16,
    /// Has children to show.
    pub expandable: bool,
    /// Is expanded.
    pub expanded: bool,
}

/// A keyboard move in the tree list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    /// The row above.
    Up,
    /// The row below.
    Down,
    /// The first row.
    Home,
    /// The last row.
    End,
    /// A page up (`n` rows).
    PageUp(u32),
    /// A page down (`n` rows).
    PageDown(u32),
    /// Collapse, or go to the parent when already collapsed (←).
    Left,
    /// Expand, or go to the first child when already expanded (→).
    Right,
}

/// Expansion state and the visible rows.
#[derive(Debug, Clone, Default)]
pub struct Outline {
    expanded: HashSet<NodeId>,
    rows: Vec<OutlineRow>,
    index: HashMap<NodeId, usize>,
}

fn expandable(tree: &Tree, id: NodeId) -> bool {
    let n = tree.node(id);
    n.kind == NodeKind::Dir && !n.children.is_empty()
}

impl Outline {
    /// An outline with the root expanded.
    pub fn new(tree: &Tree) -> Outline {
        let mut o = Outline::default();
        o.expanded.insert(tree.root());
        o.rebuild(tree);
        o
    }

    /// Recomputes the visible rows (after expanding, collapsing, a re-sort
    /// or a change to the tree).
    pub fn rebuild(&mut self, tree: &Tree) {
        self.expanded.retain(|id| tree.get(*id).is_some());
        self.rows.clear();
        self.index.clear();
        let mut stack = vec![(tree.root(), 0u16)];
        while let Some((id, depth)) = stack.pop() {
            let exp = expandable(tree, id);
            let is_open = exp && self.expanded.contains(&id);
            self.index.insert(id, self.rows.len());
            self.rows.push(OutlineRow {
                id,
                depth,
                expandable: exp,
                expanded: is_open,
            });
            if is_open {
                for c in tree.children(id).iter().rev() {
                    stack.push((*c, depth + 1));
                }
            }
        }
    }

    /// The visible rows.
    pub fn rows(&self) -> &[OutlineRow] {
        &self.rows
    }

    /// The row a node is on, if visible.
    pub fn row_of(&self, id: NodeId) -> Option<usize> {
        self.index.get(&id).copied()
    }

    /// True when the node is expanded.
    pub fn is_expanded(&self, id: NodeId) -> bool {
        self.expanded.contains(&id)
    }

    /// Expands or collapses a folder.
    pub fn set_expanded(&mut self, tree: &Tree, id: NodeId, open: bool) {
        let changed = if open {
            expandable(tree, id) && self.expanded.insert(id)
        } else {
            self.expanded.remove(&id)
        };
        if changed {
            self.rebuild(tree);
        }
    }

    /// Expands every folder above `id` so its row shows; returns the row.
    pub fn reveal(&mut self, tree: &Tree, id: NodeId) -> Option<usize> {
        tree.get(id)?;
        let mut changed = false;
        let mut cur = tree.node(id).parent;
        while cur != NO_NODE {
            changed |= self.expanded.insert(cur);
            cur = tree.node(cur).parent;
        }
        if changed {
            self.rebuild(tree);
        }
        self.row_of(id)
    }

    /// Where a key takes the selection from `from` (the first row when
    /// nothing is selected). May expand or collapse a folder.
    pub fn step(&mut self, tree: &Tree, from: Option<NodeId>, mv: Move) -> Option<NodeId> {
        if self.rows.is_empty() {
            return None;
        }
        let last = self.rows.len() - 1;
        let Some(cur) = from.and_then(|f| self.row_of(f)) else {
            return Some(self.rows[0].id);
        };
        let row = self.rows[cur];
        let to = match mv {
            Move::Up => cur.saturating_sub(1),
            Move::Down => (cur + 1).min(last),
            Move::Home => 0,
            Move::End => last,
            Move::PageUp(n) => cur.saturating_sub(n.max(1) as usize),
            Move::PageDown(n) => (cur + n.max(1) as usize).min(last),
            Move::Left => {
                if row.expanded {
                    self.set_expanded(tree, row.id, false);
                    return Some(row.id);
                }
                let p = tree.node(row.id).parent;
                return Some(if p == NO_NODE { row.id } else { p });
            }
            Move::Right => {
                if row.expandable && !row.expanded {
                    self.set_expanded(tree, row.id, true);
                    return Some(row.id);
                }
                if row.expanded {
                    return tree.children(row.id).first().copied().or(Some(row.id));
                }
                cur
            }
        };
        Some(self.rows[to].id)
    }
}

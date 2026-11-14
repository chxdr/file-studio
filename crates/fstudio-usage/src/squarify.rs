//! Squarified treemap layout (Bruls, Huizing and van Wijk, 2000), and the
//! treemap of a usage tree built from it. Pure: no I/O, no drawing.

use crate::tree::{NodeId, NodeKind, SizeMode, Tree, flags};

/// A rectangle in view points, origin top-left.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

impl Rect {
    /// A rectangle.
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h }
    }
    /// Its area.
    pub fn area(&self) -> f64 {
        self.w * self.h
    }
    /// True when the point lies inside (left/top edges inclusive).
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }
    /// The rectangle shrunk by `d` on every side (never negative).
    pub fn inset(&self, d: f64) -> Rect {
        Rect {
            x: self.x + d,
            y: self.y + d,
            w: (self.w - 2.0 * d).max(0.0),
            h: (self.h - 2.0 * d).max(0.0),
        }
    }
    /// Width over height, or its inverse, whichever is at least 1.
    pub fn aspect(&self) -> f64 {
        if self.w <= 0.0 || self.h <= 0.0 {
            return f64::INFINITY;
        }
        (self.w / self.h).max(self.h / self.w)
    }
}

/// The worst aspect ratio of a row of areas laid along a side of `side`.
fn worst(sum: f64, max: f64, min: f64, side: f64) -> f64 {
    if sum <= 0.0 || min <= 0.0 {
        return f64::INFINITY;
    }
    let s2 = sum * sum;
    let w2 = side * side;
    (w2 * max / s2).max(s2 / (w2 * min))
}

/// Lays out `values` (largest first, none negative) inside `rect`, one
/// rectangle per value in the same order, with areas proportional to the
/// values and aspect ratios kept close to 1. Zero values get empty
/// rectangles. The rectangles tile `rect` exactly.
pub fn squarify(values: &[f64], rect: Rect) -> Vec<Rect> {
    let n = values.len();
    let mut out = vec![Rect::new(rect.x, rect.y, 0.0, 0.0); n];
    let total: f64 = values.iter().map(|v| v.max(0.0)).sum();
    if total <= 0.0 || rect.area() <= 0.0 {
        return out;
    }
    let scale = rect.area() / total;
    let areas: Vec<f64> = values.iter().map(|v| v.max(0.0) * scale).collect();
    // Only lay out the positive values; they are sorted, so they come first.
    let m = areas.iter().take_while(|a| **a > 0.0).count();
    let mut r = rect;
    let mut i = 0;
    while i < m {
        let side = r.w.min(r.h);
        let mut j = i;
        let mut sum = 0.0;
        let mut best = f64::INFINITY;
        while j < m {
            let s = sum + areas[j];
            let w = worst(s, areas[i], areas[j], side);
            if j > i && w > best {
                break;
            }
            best = w;
            sum = s;
            j += 1;
        }
        let last_row = j >= m;
        if r.w >= r.h {
            // A column on the left.
            let cw = if last_row { r.w } else { sum / r.h };
            let mut y = r.y;
            for k in i..j {
                let h = if k + 1 == j {
                    r.y + r.h - y
                } else {
                    areas[k] / cw
                };
                out[k] = Rect::new(r.x, y, cw, h);
                y += h;
            }
            r.x += cw;
            r.w = (r.w - cw).max(0.0);
        } else {
            // A row along the top.
            let rh = if last_row { r.h } else { sum / r.w };
            let mut x = r.x;
            for k in i..j {
                let w = if k + 1 == j {
                    r.x + r.w - x
                } else {
                    areas[k] / rh
                };
                out[k] = Rect::new(x, r.y, w, rh);
                x += w;
            }
            r.y += rh;
            r.h = (r.h - rh).max(0.0);
        }
        i = j;
    }
    out
}

/// Colour slot of a tile. Slots `1..=N` are the top file types (see
/// [`crate::types::Palette`]); the others are fixed.
pub mod slot {
    /// Any other file type.
    pub const OTHER: u8 = 0;
    /// iCloud placeholders (only visible by logical size).
    pub const CLOUD: u8 = 250;
    /// A folder too small to show its contents.
    pub const FOLDER: u8 = 251;
    /// A folder's frame (drawn behind its children).
    pub const FRAME: u8 = 255;
}

/// One rectangle of a treemap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tile {
    /// The node it shows.
    pub id: NodeId,
    /// Where.
    pub rect: Rect,
    /// Depth below the treemap's root (the root is 0).
    pub depth: u16,
    /// True for a drawn leaf; false for a folder frame with tiles inside.
    pub leaf: bool,
    /// Its colour slot ([`slot`]).
    pub color: u8,
}

/// Treemap limits.
#[derive(Debug, Clone, Copy)]
pub struct TreemapOptions {
    /// A folder whose rectangle is narrower than this (points) on either
    /// side is drawn as one tile, not split into its contents.
    pub min_side: f64,
    /// Space around a folder's contents, so its frame shows.
    pub padding: f64,
    /// Folders smaller than this on either side get no padding.
    pub padding_min_side: f64,
    /// At most this many tiles (the rest are folded into their folders).
    pub max_tiles: usize,
}

impl Default for TreemapOptions {
    fn default() -> Self {
        TreemapOptions {
            min_side: 3.0,
            padding: 1.0,
            padding_min_side: 14.0,
            max_tiles: 60_000,
        }
    }
}

/// Lays out the tree below `root` in `rect`, parents before children, so
/// drawing in order paints frames first. `color` gives a file's slot.
pub fn treemap(
    tree: &Tree,
    root: NodeId,
    rect: Rect,
    mode: SizeMode,
    opts: &TreemapOptions,
    color: &dyn Fn(&crate::tree::Node) -> u8,
) -> Vec<Tile> {
    let mut out = Vec::new();
    if rect.w < 1.0 || rect.h < 1.0 || tree.get(root).is_none() {
        return out;
    }
    lay(tree, root, rect, 0, mode, opts, color, &mut out);
    out
}

#[allow(clippy::too_many_arguments)]
fn lay(
    tree: &Tree,
    id: NodeId,
    rect: Rect,
    depth: u16,
    mode: SizeMode,
    opts: &TreemapOptions,
    color: &dyn Fn(&crate::tree::Node) -> u8,
    out: &mut Vec<Tile>,
) {
    let node = tree.node(id);
    let kids: Vec<NodeId> = node
        .children
        .iter()
        .copied()
        .filter(|c| tree.node(*c).size(mode) > 0)
        .collect();
    let splittable = node.kind == NodeKind::Dir
        && !kids.is_empty()
        && rect.w >= opts.min_side
        && rect.h >= opts.min_side
        && out.len() < opts.max_tiles;
    if !splittable {
        let c = match node.kind {
            NodeKind::Dir => slot::FOLDER,
            _ if node.has(flags::DATALESS) => slot::CLOUD,
            _ => color(node),
        };
        out.push(Tile {
            id,
            rect,
            depth,
            leaf: true,
            color: c,
        });
        return;
    }
    out.push(Tile {
        id,
        rect,
        depth,
        leaf: false,
        color: slot::FRAME,
    });
    let inner = if depth > 0 && rect.w >= opts.padding_min_side && rect.h >= opts.padding_min_side {
        rect.inset(opts.padding)
    } else {
        rect
    };
    // Children may not fill the folder exactly when its own total differs
    // (for example items flagged after the fact); lay out what they hold.
    let values: Vec<f64> = kids
        .iter()
        .map(|c| tree.node(*c).size(mode) as f64)
        .collect();
    let rects = squarify(&values, inner);
    for (c, r) in kids.iter().zip(rects) {
        if r.w < 0.5 || r.h < 0.5 {
            continue;
        }
        lay(tree, *c, r, depth + 1, mode, opts, color, out);
    }
}

/// The deepest tile under a point: the last one in drawing order that
/// contains it.
pub fn hit(tiles: &[Tile], x: f64, y: f64) -> Option<NodeId> {
    tiles
        .iter()
        .rev()
        .find(|t| t.rect.contains(x, y))
        .map(|t| t.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn total_area(rs: &[Rect]) -> f64 {
        rs.iter().map(Rect::area).sum()
    }

    fn overlaps(a: &Rect, b: &Rect) -> bool {
        let e = 1e-9;
        a.x + e < b.x + b.w && b.x + e < a.x + a.w && a.y + e < b.y + b.h && b.y + e < a.y + a.h
    }

    #[test]
    fn paper_example_tiles_exactly() {
        // The example from the paper: 6 x 4 with 6, 6, 4, 3, 2, 2, 1.
        let v = [6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0];
        let r = Rect::new(0.0, 0.0, 6.0, 4.0);
        let rs = squarify(&v, r);
        assert_eq!(rs.len(), 7);
        assert!((total_area(&rs) - 24.0).abs() < 1e-9);
        for (val, rect) in v.iter().zip(&rs) {
            assert!((rect.area() - val).abs() < 1e-9, "{val} -> {rect:?}");
        }
        for i in 0..rs.len() {
            for j in i + 1..rs.len() {
                assert!(!overlaps(&rs[i], &rs[j]), "{:?} {:?}", rs[i], rs[j]);
            }
        }
        // The first two are the 3 x 2 squares-ish the paper ends with.
        assert!(rs[0].aspect() < 1.6 && rs[1].aspect() < 1.6);
    }

    #[test]
    fn stays_inside_and_keeps_proportions() {
        let v: Vec<f64> = (1..=50).rev().map(|x| (x * x) as f64).collect();
        let r = Rect::new(10.0, 20.0, 800.0, 300.0);
        let rs = squarify(&v, r);
        let total: f64 = v.iter().sum();
        for (val, rect) in v.iter().zip(&rs) {
            assert!(rect.x >= r.x - 1e-6 && rect.y >= r.y - 1e-6);
            assert!(rect.x + rect.w <= r.x + r.w + 1e-6);
            assert!(rect.y + rect.h <= r.y + r.h + 1e-6);
            let want = val / total * r.area();
            assert!((rect.area() - want).abs() < 1e-6 * r.area(), "{val}");
        }
        assert!((total_area(&rs) - r.area()).abs() < 1e-6);
    }

    #[test]
    fn aspect_ratios_are_reasonable() {
        let v = vec![1.0; 100];
        let rs = squarify(&v, Rect::new(0.0, 0.0, 100.0, 100.0));
        let worst = rs.iter().map(Rect::aspect).fold(0.0, f64::max);
        assert!(worst < 2.5, "worst aspect {worst}");
    }

    #[test]
    fn zeros_and_empty_input() {
        assert!(squarify(&[], Rect::new(0.0, 0.0, 10.0, 10.0)).is_empty());
        let rs = squarify(&[5.0, 0.0, 0.0], Rect::new(0.0, 0.0, 10.0, 10.0));
        assert_eq!(rs[0], Rect::new(0.0, 0.0, 10.0, 10.0));
        assert_eq!(rs[1].area(), 0.0);
        assert_eq!(rs[2].area(), 0.0);
        let rs = squarify(&[0.0, 0.0], Rect::new(0.0, 0.0, 10.0, 10.0));
        assert!(rs.iter().all(|r| r.area() == 0.0));
        let rs = squarify(&[1.0, 1.0], Rect::new(0.0, 0.0, 0.0, 10.0));
        assert!(rs.iter().all(|r| r.area() == 0.0));
    }

    #[test]
    fn one_value_fills_the_rect() {
        let r = Rect::new(3.0, 4.0, 50.0, 7.0);
        assert_eq!(squarify(&[42.0], r), vec![r]);
    }

    #[test]
    fn tall_rect_lays_rows_on_top() {
        let rs = squarify(&[1.0, 1.0], Rect::new(0.0, 0.0, 10.0, 40.0));
        assert_eq!(rs[0], Rect::new(0.0, 0.0, 10.0, 20.0));
        assert_eq!(rs[1], Rect::new(0.0, 20.0, 10.0, 20.0));
    }

    #[test]
    fn hit_finds_the_deepest_tile() {
        let tiles = [
            Tile {
                id: 1,
                rect: Rect::new(0.0, 0.0, 10.0, 10.0),
                depth: 0,
                leaf: false,
                color: slot::FRAME,
            },
            Tile {
                id: 2,
                rect: Rect::new(0.0, 0.0, 5.0, 10.0),
                depth: 1,
                leaf: true,
                color: 1,
            },
        ];
        assert_eq!(hit(&tiles, 2.0, 2.0), Some(2));
        assert_eq!(hit(&tiles, 7.0, 2.0), Some(1));
        assert_eq!(hit(&tiles, 12.0, 2.0), None);
    }
}

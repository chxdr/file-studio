//! Icon and gallery view behaviour that belongs in the core (M3).
//!
//! The shell draws the grid and the filmstrip; the core decides:
//! - how many items fit in a row and how big a tile is ([`grid_layout`]),
//! - where the arrow keys move the cursor in a grid of `columns` items per
//!   row ([`grid_target`]), and that ←/→ step through the gallery,
//! - the thumbnail size and its steps (`grid.icon_size`, ⇧⌘= / ⇧⌘-),
//! - which pixel size a thumbnail is asked for ([`thumb_pixels`]), so a
//!   slider drag reuses cached pictures instead of asking for every size,
//! - the order thumbnails are asked for ([`thumbnail_order`],
//!   [`gallery_order`]): the cursor first, then what is on screen, then
//!   what scrolls in next. iCloud placeholders are never asked for (F-016),
//!   and folders and packages keep their icons.

use fstudio_keymap::{Focus, ViewMode};
use fstudio_model::{Entry, Key, KeyChord};
use serde::{Deserialize, Serialize};

use crate::commands::InputContext;
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::ids::TabId;
use crate::outcome::Outcome;
use crate::tab::{SelectionChange, Tab};

/// The thumbnail sizes ⇧⌘= and ⇧⌘- step through, in points.
pub const ICON_SIZES: [u32; 8] = [32, 48, 64, 96, 128, 160, 192, 256];
/// The smallest and largest thumbnail, in points (the slider's range).
pub const ICON_SIZE_RANGE: (u32, u32) = (32, 256);
/// The pixel sizes thumbnails are made at; a request takes the next one up.
pub const THUMB_PIXEL_STEPS: [u32; 5] = [64, 128, 256, 512, 1024];
/// The pixel sizes of the gallery's big picture (its longer side).
pub const GALLERY_PIXEL_STEPS: [u32; 3] = [1024, 2048, 3072];

/// Icon view and gallery settings (`grid.*`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GridSettings {
    /// Thumbnail size in the icon view, in points (32–256).
    pub icon_size: u32,
    /// Thumbnail size in the gallery's filmstrip, in points (40–160).
    pub strip_size: u32,
    /// Show the photo's details (camera, exposure, size) under the gallery picture.
    pub gallery_info: bool,
}

impl Default for GridSettings {
    fn default() -> Self {
        Self {
            icon_size: 128,
            strip_size: 72,
            gallery_info: true,
        }
    }
}

impl GridSettings {
    /// Checks the ranges.
    pub fn validate(&self) -> Result<()> {
        let (lo, hi) = ICON_SIZE_RANGE;
        if !(lo..=hi).contains(&self.icon_size) {
            return Err(CoreError::InvalidSetting(format!(
                "grid.icon_size must be {lo}–{hi}"
            )));
        }
        if !(40..=160).contains(&self.strip_size) {
            return Err(CoreError::InvalidSetting(
                "grid.strip_size must be 40–160".into(),
            ));
        }
        Ok(())
    }
}

/// Keeps a thumbnail size in range (the slider can send anything).
pub fn clamp_icon_size(size: u32) -> u32 {
    size.clamp(ICON_SIZE_RANGE.0, ICON_SIZE_RANGE.1)
}

/// The next size up (`bigger`) or down from `size`, stopping at the ends.
/// A size between two steps goes to the neighbouring step.
pub fn next_icon_size(size: u32, bigger: bool) -> u32 {
    let size = clamp_icon_size(size);
    if bigger {
        ICON_SIZES
            .iter()
            .copied()
            .find(|&s| s > size)
            .unwrap_or(ICON_SIZE_RANGE.1)
    } else {
        ICON_SIZES
            .iter()
            .rev()
            .copied()
            .find(|&s| s < size)
            .unwrap_or(ICON_SIZE_RANGE.0)
    }
}

/// The pixel size to ask QuickLook for, for a thumbnail of `points` on a
/// screen with this backing `scale`: the next step up, at most 1024. A few
/// fixed sizes mean the cache keeps working while the size slider moves.
pub fn thumb_pixels(points: f64, scale: f64) -> u32 {
    let want = (points.max(1.0) * scale.clamp(1.0, 4.0)).ceil() as u32;
    THUMB_PIXEL_STEPS
        .iter()
        .copied()
        .find(|&s| s >= want)
        .unwrap_or(THUMB_PIXEL_STEPS[THUMB_PIXEL_STEPS.len() - 1])
}

/// The pixel size (longer side) of the gallery's big picture, for a view
/// whose longer side is `points`: the next step up, at most 3072. The
/// pictures made ahead of the cursor use the same size, so they are ready
/// when it arrives.
pub fn gallery_pixels(points: f64, scale: f64) -> u32 {
    let want = (points.max(1.0) * scale.clamp(1.0, 4.0)).ceil() as u32;
    GALLERY_PIXEL_STEPS
        .iter()
        .copied()
        .find(|&s| s >= want)
        .unwrap_or(GALLERY_PIXEL_STEPS[GALLERY_PIXEL_STEPS.len() - 1])
}

/// Where the tiles of the icon view go, in points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GridLayout {
    /// Items per row (at least 1).
    pub columns: u32,
    /// The width of one cell; the tile is centred in it.
    pub cell_width: f64,
    /// The height of one cell (picture, gap and two lines of name).
    pub cell_height: f64,
    /// The square the picture is fitted into.
    pub image_size: f64,
    /// The space under the picture for the name.
    pub label_height: f64,
    /// The margin around the grid.
    pub inset: f64,
}

/// Lays out the icon view for a content `width` and a thumbnail size.
/// Cells share out the spare width, so rows always fill the view.
pub fn grid_layout(width: f64, icon_size: u32) -> GridLayout {
    let image = clamp_icon_size(icon_size) as f64;
    let inset = 12.0;
    let label = 32.0;
    // Room for the name under small icons, and a little air beside big ones.
    let min_cell = (image + 24.0).max(92.0);
    let avail = (width - 2.0 * inset).max(min_cell);
    let columns = ((avail / min_cell).floor() as u32).max(1);
    GridLayout {
        columns,
        cell_width: avail / columns as f64,
        cell_height: image + 8.0 + label + 8.0,
        image_size: image,
        label_height: label,
        inset,
    }
}

/// An arrow key in a grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridDir {
    /// ←: the item before.
    Left,
    /// →: the item after.
    Right,
    /// ↑: the item above.
    Up,
    /// ↓: the item below.
    Down,
}

/// Where the cursor goes from `cursor` in a grid of `len` items with
/// `columns` per row. `None` only for an empty grid.
/// - With no cursor, → and ↓ go to the first item, ← and ↑ to the last.
/// - ← and → step through the items in reading order and stop at the ends.
/// - ↑ in the first row and ↓ in the last row stay put.
/// - ↓ into a shorter last row with nothing straight below goes to the last
///   item (as in Finder).
pub fn grid_target(
    cursor: Option<usize>,
    len: usize,
    columns: usize,
    dir: GridDir,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let last = len - 1;
    let cols = columns.max(1);
    let Some(c) = cursor.map(|c| c.min(last)) else {
        return Some(match dir {
            GridDir::Right | GridDir::Down => 0,
            GridDir::Left | GridDir::Up => last,
        });
    };
    Some(match dir {
        GridDir::Left => c.saturating_sub(1),
        GridDir::Right => (c + 1).min(last),
        GridDir::Up => c.checked_sub(cols).unwrap_or(c),
        GridDir::Down => {
            if c + cols <= last {
                c + cols
            } else if c / cols < last / cols {
                last
            } else {
                c
            }
        }
    })
}

/// The arrow key a chord is in this view, if the grid takes it: icon view
/// takes all four, the gallery ← and → (↑/↓ keep moving one item, as in
/// the list). Only bare arrows or ⇧-arrows with the list focused. Returns
/// the direction and whether ⇧ extends the selection.
pub(crate) fn grid_key_dir(
    chord: &KeyChord,
    focus: Focus,
    mode: ViewMode,
) -> Option<(GridDir, bool)> {
    if focus != Focus::List {
        return None;
    }
    let m = chord.mods;
    if m.cmd || m.alt || m.ctrl {
        return None;
    }
    let dir = match chord.key {
        Key::Left => GridDir::Left,
        Key::Right => GridDir::Right,
        Key::Up => GridDir::Up,
        Key::Down => GridDir::Down,
        _ => return None,
    };
    match (mode, dir) {
        (ViewMode::Icons, _) => Some((dir, m.shift)),
        (ViewMode::Gallery, GridDir::Left | GridDir::Right) => Some((dir, m.shift)),
        _ => None,
    }
}

/// The order to ask for thumbnails in, for a view showing items
/// `start..start+count` of `len`, with the cursor at `cursor`:
/// 1. the cursor item, if it is on screen;
/// 2. the rest of the screen, top to bottom;
/// 3. `ahead` items past the screen in the scroll direction (`forward` is
///    down or right);
/// 4. `behind` items on the other side.
///
/// Only items `wants` accepts are listed (no folders, packages or iCloud
/// placeholders). Each index appears once.
#[allow(clippy::too_many_arguments)]
pub fn thumbnail_order(
    len: usize,
    start: usize,
    count: usize,
    cursor: Option<usize>,
    ahead: usize,
    behind: usize,
    forward: bool,
    wants: impl Fn(usize) -> bool,
) -> Vec<u32> {
    let start = start.min(len);
    let end = (start + count).min(len);
    let mut out = Vec::with_capacity(count + ahead + behind);
    let push = |i: usize, out: &mut Vec<u32>| {
        if wants(i) {
            out.push(i as u32);
        }
    };
    if let Some(c) = cursor.filter(|c| (start..end).contains(c)) {
        push(c, &mut out);
    }
    for i in start..end {
        if Some(i) != cursor {
            push(i, &mut out);
        }
    }
    let after = end..(end + if forward { ahead } else { behind }).min(len);
    let before = start.saturating_sub(if forward { behind } else { ahead })..start;
    if forward {
        after.for_each(|i| push(i, &mut out));
        before.rev().for_each(|i| push(i, &mut out));
    } else {
        before.rev().for_each(|i| push(i, &mut out));
        after.for_each(|i| push(i, &mut out));
    }
    out
}

/// The order to make the gallery's big pictures in: the cursor item, then
/// the next two in the direction of travel, then the one behind. Holding →
/// through a folder of raws keeps the next pictures ready.
pub fn gallery_order(
    cursor: usize,
    len: usize,
    forward: bool,
    wants: impl Fn(usize) -> bool,
) -> Vec<u32> {
    if cursor >= len {
        return Vec::new();
    }
    let step = |k: isize| -> Option<usize> {
        let k = if forward { k } else { -k };
        let i = cursor as isize + k;
        (0..len as isize).contains(&i).then_some(i as usize)
    };
    [Some(cursor), step(1), step(2), step(-1)]
        .into_iter()
        .flatten()
        .filter(|&i| wants(i))
        .map(|i| i as u32)
        .collect()
}

/// Does this item get a QuickLook thumbnail? Files only: folders and
/// packages keep their icons, and an iCloud placeholder is never read
/// (asking QuickLook would download it, F-016).
pub fn wants_thumbnail(e: &Entry) -> bool {
    !e.is_dir() && !e.is_package && !e.is_dataless()
}

impl Engine {
    /// Arrow keys in icon and gallery view (called first by `handle_key`).
    pub(crate) fn grid_key(&self, chord: &KeyChord, ctx: &InputContext) -> Option<Outcome> {
        if !matches!(chord.key, Key::Left | Key::Right | Key::Up | Key::Down) {
            return None;
        }
        let tab = self.target(ctx.tab).ok()?;
        let (dir, extend) = grid_key_dir(chord, ctx.focus, tab.view().mode)?;
        Some(self.grid_move(&tab, dir, extend))
    }

    /// Moves the cursor of `t` one step in its grid; ⇧ (`extend`) extends
    /// the selection from the anchor.
    pub(crate) fn grid_move(&self, t: &Tab, dir: GridDir, extend: bool) -> Outcome {
        let len = t.listing().len() as usize;
        let cols = match t.view().mode {
            ViewMode::Icons => t.grid_columns() as usize,
            _ => 1,
        };
        let mut cursor = t.selection().cursor.map(|c| c as usize);
        // The gallery shows the first item when nothing is selected, so the
        // first → goes on from there.
        if cursor.is_none() && t.view().mode == ViewMode::Gallery && len > 0 {
            cursor = Some(0);
        }
        if let Some(to) = grid_target(cursor, len, cols, dir) {
            let to = to as u32;
            t.set_selection(if extend {
                SelectionChange::ExtendTo(to)
            } else {
                SelectionChange::Only(to)
            });
        }
        Outcome::none().with_tab(t.id())
    }

    /// The shell laid out the icon view of `tab` with `columns` items per row.
    pub fn set_grid_columns(&self, tab: TabId, columns: u32) -> Result<()> {
        self.target(Some(tab))?.set_grid_columns(columns);
        Ok(())
    }

    /// Which thumbnails to ask for, in order, while items
    /// `start..start+count` of `tab` are on screen and the view last moved
    /// `forward` (down or right). See [`thumbnail_order`]. Reads no disk.
    pub fn thumbnail_requests(
        &self,
        tab: TabId,
        start: u32,
        count: u32,
        forward: bool,
    ) -> Result<Vec<u32>> {
        let t = self.target(Some(tab))?;
        let l = t.listing();
        let cursor = t.selection().cursor.map(|c| c as usize);
        let count = count as usize;
        // Two screens ahead, one behind: enough to keep up with a mouse wheel.
        Ok(thumbnail_order(
            l.len() as usize,
            start as usize,
            count,
            cursor,
            count * 2,
            count,
            forward,
            |i| l.entry(i as u32).is_some_and(|e| wants_thumbnail(&e)),
        ))
    }

    /// Which big gallery pictures to make, in order, around the cursor of
    /// `tab` (`forward`: the last move was → or ↓), or around the first
    /// item when nothing is selected. See [`gallery_order`].
    pub fn gallery_requests(&self, tab: TabId, forward: bool) -> Result<Vec<u32>> {
        let t = self.target(Some(tab))?;
        let l = t.listing();
        // With nothing selected the gallery shows the first item.
        let c = t.selection().cursor.unwrap_or(0);
        Ok(gallery_order(c as usize, l.len() as usize, forward, |i| {
            l.entry(i as u32).is_some_and(|e| wants_thumbnail(&e))
        }))
    }

    /// ⇧⌘= / ⇧⌘-: the next thumbnail size up or down.
    pub(crate) fn step_icon_size(&self, bigger: bool) -> Result<Outcome> {
        let mut s = self.settings();
        let next = next_icon_size(s.grid.icon_size, bigger);
        if next == s.grid.icon_size {
            return Ok(Outcome::none());
        }
        s.grid.icon_size = next;
        self.update_settings(s)?;
        Ok(Outcome::none())
    }

    /// The size slider: sets the thumbnail size (kept in range).
    pub fn set_icon_size(&self, size: u32) -> Result<()> {
        let mut s = self.settings();
        let size = clamp_icon_size(size);
        if s.grid.icon_size == size {
            return Ok(());
        }
        s.grid.icon_size = size;
        self.update_settings(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fstudio_model::Mods;

    fn k(key: Key, mods: Mods) -> KeyChord {
        KeyChord { key, mods }
    }

    #[test]
    fn arrows_move_in_two_dimensions() {
        // 10 items, 4 per row:
        //  0 1 2 3
        //  4 5 6 7
        //  8 9
        let t = |c, d| grid_target(Some(c), 10, 4, d);
        assert_eq!(t(5, GridDir::Up), Some(1));
        assert_eq!(t(5, GridDir::Down), Some(9));
        assert_eq!(t(5, GridDir::Left), Some(4));
        assert_eq!(t(5, GridDir::Right), Some(6));
        // ← / → run on across rows, and stop at the ends.
        assert_eq!(t(3, GridDir::Right), Some(4));
        assert_eq!(t(4, GridDir::Left), Some(3));
        assert_eq!(t(0, GridDir::Left), Some(0));
        assert_eq!(t(9, GridDir::Right), Some(9));
        // ↑ in the first row and ↓ in the last stay.
        assert_eq!(t(2, GridDir::Up), Some(2));
        assert_eq!(t(8, GridDir::Down), Some(8));
        // ↓ with nothing straight below, but a shorter row under: the last item.
        assert_eq!(t(6, GridDir::Down), Some(9));
        assert_eq!(t(7, GridDir::Down), Some(9));
    }

    #[test]
    fn no_cursor_starts_at_an_end() {
        assert_eq!(grid_target(None, 10, 4, GridDir::Down), Some(0));
        assert_eq!(grid_target(None, 10, 4, GridDir::Right), Some(0));
        assert_eq!(grid_target(None, 10, 4, GridDir::Up), Some(9));
        assert_eq!(grid_target(None, 10, 4, GridDir::Left), Some(9));
        assert_eq!(grid_target(None, 0, 4, GridDir::Down), None);
        assert_eq!(grid_target(Some(3), 0, 4, GridDir::Down), None);
    }

    #[test]
    fn odd_shapes() {
        // One column behaves like a list.
        assert_eq!(grid_target(Some(2), 5, 1, GridDir::Down), Some(3));
        assert_eq!(grid_target(Some(2), 5, 1, GridDir::Up), Some(1));
        // Zero columns is treated as one.
        assert_eq!(grid_target(Some(2), 5, 0, GridDir::Down), Some(3));
        // More columns than items: one row.
        assert_eq!(grid_target(Some(1), 3, 8, GridDir::Down), Some(1));
        assert_eq!(grid_target(Some(1), 3, 8, GridDir::Up), Some(1));
        // A stale cursor past the end is pulled back first.
        assert_eq!(grid_target(Some(40), 10, 4, GridDir::Left), Some(8));
        // A full last row: ↓ from the row above lands straight below.
        assert_eq!(grid_target(Some(1), 8, 4, GridDir::Down), Some(5));
    }

    #[test]
    fn which_keys_the_grid_takes() {
        let n = Mods::NONE;
        let shift = Mods {
            shift: true,
            ..Mods::NONE
        };
        let cmd = Mods {
            cmd: true,
            ..Mods::NONE
        };
        let icons = ViewMode::Icons;
        assert_eq!(
            grid_key_dir(&k(Key::Up, n), Focus::List, icons),
            Some((GridDir::Up, false))
        );
        assert_eq!(
            grid_key_dir(&k(Key::Right, shift), Focus::List, icons),
            Some((GridDir::Right, true))
        );
        // ⌘↓ (open) and ⌘↑ (enclosing folder) stay commands.
        assert_eq!(grid_key_dir(&k(Key::Down, cmd), Focus::List, icons), None);
        // Not in the address bar, and not in list or column view.
        assert_eq!(grid_key_dir(&k(Key::Up, n), Focus::AddressBar, icons), None);
        assert_eq!(
            grid_key_dir(&k(Key::Up, n), Focus::List, ViewMode::List),
            None
        );
        assert_eq!(
            grid_key_dir(&k(Key::Left, n), Focus::List, ViewMode::Columns),
            None
        );
        // The gallery takes ← and →; ↑ and ↓ stay the list's one-item moves.
        let g = ViewMode::Gallery;
        assert_eq!(
            grid_key_dir(&k(Key::Left, n), Focus::List, g),
            Some((GridDir::Left, false))
        );
        assert_eq!(grid_key_dir(&k(Key::Up, n), Focus::List, g), None);
        assert_eq!(
            grid_key_dir(&k(Key::Char('j'), n), Focus::List, icons),
            None
        );
    }

    #[test]
    fn icon_size_steps_and_clamps() {
        assert_eq!(next_icon_size(128, true), 160);
        assert_eq!(next_icon_size(128, false), 96);
        assert_eq!(next_icon_size(256, true), 256);
        assert_eq!(next_icon_size(32, false), 32);
        // Between steps (after the slider): to the neighbouring step.
        assert_eq!(next_icon_size(100, true), 128);
        assert_eq!(next_icon_size(100, false), 96);
        // Out of range: clamped first.
        assert_eq!(next_icon_size(9999, false), 192);
        assert_eq!(next_icon_size(1, true), 48);
        assert_eq!(clamp_icon_size(0), 32);
        assert_eq!(clamp_icon_size(1000), 256);
    }

    #[test]
    fn thumbnail_pixels_come_in_steps() {
        assert_eq!(thumb_pixels(32.0, 2.0), 64);
        assert_eq!(thumb_pixels(33.0, 2.0), 128);
        assert_eq!(thumb_pixels(100.0, 2.0), 256);
        assert_eq!(thumb_pixels(128.0, 2.0), 256);
        assert_eq!(thumb_pixels(129.0, 2.0), 512);
        assert_eq!(thumb_pixels(256.0, 2.0), 512);
        assert_eq!(thumb_pixels(800.0, 2.0), 1024);
        assert_eq!(thumb_pixels(128.0, 1.0), 128);
        // A silly scale is clamped.
        assert_eq!(thumb_pixels(64.0, 0.0), 64);
        // A whole slider drag from 96 to 128 pt at 2× uses one size.
        let sizes: std::collections::BTreeSet<u32> =
            (96..=128).map(|p| thumb_pixels(p as f64, 2.0)).collect();
        assert_eq!(sizes.into_iter().collect::<Vec<_>>(), vec![256]);
    }

    #[test]
    fn gallery_pictures_come_in_steps() {
        assert_eq!(gallery_pixels(300.0, 2.0), 1024);
        assert_eq!(gallery_pixels(900.0, 2.0), 2048);
        assert_eq!(gallery_pixels(1400.0, 2.0), 3072);
        assert_eq!(gallery_pixels(5000.0, 2.0), 3072);
        // Resizing the window a little keeps the same size (and the cache).
        assert_eq!(gallery_pixels(700.0, 2.0), gallery_pixels(1000.0, 2.0));
    }

    #[test]
    fn layout_fills_rows() {
        let l = grid_layout(800.0, 128);
        assert_eq!(l.columns, 5);
        assert!((l.cell_width * l.columns as f64 + 2.0 * l.inset - 800.0).abs() < 1e-9);
        assert!(l.cell_width >= l.image_size + 24.0);
        assert_eq!(l.image_size, 128.0);
        // Small icons still leave room for a name.
        let s = grid_layout(800.0, 32);
        assert!(s.cell_width >= 92.0);
        assert_eq!(s.columns, 8);
        // A narrow pane still has one column, never zero.
        let n = grid_layout(40.0, 256);
        assert_eq!(n.columns, 1);
        assert_eq!(grid_layout(-5.0, 64).columns, 1);
        // Bigger icons, fewer columns.
        assert!(grid_layout(1200.0, 256).columns < grid_layout(1200.0, 64).columns);
    }

    #[test]
    fn thumbnails_cursor_then_screen_then_ahead() {
        let all = |_| true;
        // Items 10..20 on screen, cursor at 15, scrolling down.
        let o = thumbnail_order(100, 10, 10, Some(15), 4, 2, true, all);
        assert_eq!(o[0], 15);
        assert_eq!(&o[1..10], &[10, 11, 12, 13, 14, 16, 17, 18, 19]);
        assert_eq!(&o[10..14], &[20, 21, 22, 23]);
        assert_eq!(&o[14..], &[9, 8]);
        // Scrolling up: what comes in above goes first.
        let up = thumbnail_order(100, 10, 10, None, 4, 2, false, all);
        assert_eq!(&up[..10], &(10..20).collect::<Vec<u32>>()[..]);
        assert_eq!(&up[10..14], &[9, 8, 7, 6]);
        assert_eq!(&up[14..], &[20, 21]);
    }

    #[test]
    fn thumbnails_skip_unwanted_and_stay_in_bounds() {
        // Every third item is a folder or a placeholder.
        let o = thumbnail_order(12, 6, 10, Some(2), 10, 10, true, |i| i % 3 != 0);
        assert!(o.iter().all(|&i| i % 3 != 0 && i < 12));
        // The cursor is off screen, so it isn't first.
        assert_eq!(o[0], 7);
        let mut seen = o.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), o.len(), "no index twice");
        assert!(thumbnail_order(0, 0, 10, None, 5, 5, true, |_| true).is_empty());
        assert!(thumbnail_order(5, 9, 10, None, 5, 5, true, |_| true).len() <= 5);
    }

    #[test]
    fn gallery_keeps_the_next_pictures_ready() {
        assert_eq!(gallery_order(5, 10, true, |_| true), vec![5, 6, 7, 4]);
        assert_eq!(gallery_order(5, 10, false, |_| true), vec![5, 4, 3, 6]);
        assert_eq!(gallery_order(9, 10, true, |_| true), vec![9, 8]);
        assert_eq!(gallery_order(0, 10, false, |_| true), vec![0, 1]);
        assert_eq!(gallery_order(5, 10, true, |i| i != 6), vec![5, 7, 4]);
        assert!(gallery_order(3, 3, true, |_| true).is_empty());
    }

    #[test]
    fn settings_ranges() {
        assert!(GridSettings::default().validate().is_ok());
        let bad = GridSettings {
            icon_size: 8,
            ..Default::default()
        };
        assert!(bad.validate().is_err());
        let bad = GridSettings {
            strip_size: 400,
            ..Default::default()
        };
        assert!(bad.validate().is_err());
    }
}

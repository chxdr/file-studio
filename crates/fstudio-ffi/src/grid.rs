//! Icon and gallery view (M3): plain mirrors of `fstudio_core::grid`. The
//! decisions (layout, arrow keys, thumbnail sizes and request order) live
//! there.

use fstudio_core as core;

use crate::{FfiEngine, R};

/// Where the tiles of the icon view go, in points.
#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq)]
pub struct FfiGridLayout {
    pub columns: u32,
    pub cell_width: f64,
    pub cell_height: f64,
    pub image_size: f64,
    pub label_height: f64,
    pub inset: f64,
}

/// Lays out the icon view for a content width and a thumbnail size.
#[uniffi::export]
pub fn grid_layout(width: f64, icon_size: u32) -> FfiGridLayout {
    let l = core::grid_layout(width, icon_size);
    FfiGridLayout {
        columns: l.columns,
        cell_width: l.cell_width,
        cell_height: l.cell_height,
        image_size: l.image_size,
        label_height: l.label_height,
        inset: l.inset,
    }
}

/// The pixel size to ask QuickLook for, for a picture of `points` at `scale`.
#[uniffi::export]
pub fn grid_thumb_pixels(points: f64, scale: f64) -> u32 {
    core::thumb_pixels(points, scale)
}

/// The pixel size (longer side) of the gallery's big picture for a view of `points`.
#[uniffi::export]
pub fn grid_gallery_pixels(points: f64, scale: f64) -> u32 {
    core::gallery_pixels(points, scale)
}

/// The smallest and largest thumbnail size, in points (the slider's range).
#[uniffi::export]
pub fn grid_icon_size_range() -> Vec<u32> {
    vec![core::ICON_SIZE_RANGE.0, core::ICON_SIZE_RANGE.1]
}

#[uniffi::export]
impl FfiEngine {
    /// The shell laid out the icon view of `tab` with `columns` per row.
    pub fn set_grid_columns(&self, tab: u64, columns: u32) -> R<()> {
        Ok(self.inner.set_grid_columns(core::TabId(tab), columns)?)
    }

    /// Thumbnails to ask for, in order, for items `start..start+count` on
    /// screen. Reads no disk.
    pub fn thumbnail_requests(
        &self,
        tab: u64,
        start: u32,
        count: u32,
        forward: bool,
    ) -> R<Vec<u32>> {
        Ok(self
            .inner
            .thumbnail_requests(core::TabId(tab), start, count, forward)?)
    }

    /// The gallery's big pictures to make, in order, around the cursor.
    pub fn gallery_requests(&self, tab: u64, forward: bool) -> R<Vec<u32>> {
        Ok(self.inner.gallery_requests(core::TabId(tab), forward)?)
    }

    /// The size slider: sets `grid.icon_size` (kept in range).
    pub fn set_icon_size(&self, size: u32) -> R<()> {
        Ok(self.inner.set_icon_size(size)?)
    }
}

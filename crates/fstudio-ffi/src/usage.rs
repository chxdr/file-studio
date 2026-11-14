//! Disk usage (M3): UniFFI mirrors of `fstudio_core::usage`. Pure
//! translation; every method forwards to the core's `UsageView`.

use std::sync::Arc;

use fstudio_core as core;

use crate::{FfiEngine, FfiOutcome, FfiRowKind, R};

/// A running scan's progress.
#[derive(uniffi::Record, Debug, Clone)]
pub struct FfiUsageProgress {
    pub items: u64,
    pub folders: u64,
    pub allocated: u64,
    pub logical: u64,
    pub current: String,
    pub elapsed_ms: u64,
}

impl From<core::UsageProgress> for FfiUsageProgress {
    fn from(p: core::UsageProgress) -> Self {
        FfiUsageProgress {
            items: p.items,
            folders: p.folders,
            allocated: p.allocated,
            logical: p.logical,
            current: p.current,
            elapsed_ms: p.elapsed_ms,
        }
    }
}

/// What changed in a usage session.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiUsageChange {
    Started,
    Finished,
    TreeChanged,
}

/// Receives a scan's news on a background thread; hop to the main thread.
#[uniffi::export(with_foreign)]
pub trait FfiUsageObserver: Send + Sync {
    fn progress(&self, progress: FfiUsageProgress);
    fn changed(&self, change: FfiUsageChange);
}

struct ObserverBridge(Arc<dyn FfiUsageObserver>);

impl core::UsageObserver for ObserverBridge {
    fn progress(&self, p: core::UsageProgress) {
        self.0.progress(p.into());
    }
    fn changed(&self, c: core::UsageChange) {
        self.0.changed(match c {
            core::UsageChange::Started => FfiUsageChange::Started,
            core::UsageChange::Finished => FfiUsageChange::Finished,
            core::UsageChange::TreeChanged => FfiUsageChange::TreeChanged,
        });
    }
}

/// Where a scan is.
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum FfiUsageState {
    Scanning,
    Done,
    Cancelled,
    Failed { message: String },
}

impl From<core::UsageState> for FfiUsageState {
    fn from(s: core::UsageState) -> Self {
        match s {
            core::UsageState::Scanning => Self::Scanning,
            core::UsageState::Done => Self::Done,
            core::UsageState::Cancelled => Self::Cancelled,
            core::UsageState::Failed(message) => Self::Failed { message },
        }
    }
}

/// Size on disk or logical size.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiUsageSizeMode {
    OnDisk,
    Logical,
}

impl From<FfiUsageSizeMode> for core::UsageSizeMode {
    fn from(m: FfiUsageSizeMode) -> Self {
        match m {
            FfiUsageSizeMode::OnDisk => Self::OnDisk,
            FfiUsageSizeMode::Logical => Self::Logical,
        }
    }
}

impl From<core::UsageSizeMode> for FfiUsageSizeMode {
    fn from(m: core::UsageSizeMode) -> Self {
        match m {
            core::UsageSizeMode::OnDisk => Self::OnDisk,
            core::UsageSizeMode::Logical => Self::Logical,
        }
    }
}

/// A key in the tree list.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiUsageMove {
    Up,
    Down,
    Home,
    End,
    PageUp { rows: u32 },
    PageDown { rows: u32 },
    Left,
    Right,
}

impl From<FfiUsageMove> for core::UsageMove {
    fn from(m: FfiUsageMove) -> Self {
        match m {
            FfiUsageMove::Up => Self::Up,
            FfiUsageMove::Down => Self::Down,
            FfiUsageMove::Home => Self::Home,
            FfiUsageMove::End => Self::End,
            FfiUsageMove::PageUp { rows } => Self::PageUp(rows),
            FfiUsageMove::PageDown { rows } => Self::PageDown(rows),
            FfiUsageMove::Left => Self::Left,
            FfiUsageMove::Right => Self::Right,
        }
    }
}

/// One tree-list row (`fstudio_core::UsageRowInfo`).
#[derive(uniffi::Record, Debug, Clone)]
pub struct FfiUsageRow {
    pub id: u32,
    pub depth: u16,
    pub name: String,
    pub size_text: String,
    pub size: u64,
    pub fraction: f64,
    pub percent_text: String,
    pub items_text: String,
    pub modified_text: String,
    pub kind: FfiRowKind,
    pub ext: String,
    pub icon_key: String,
    pub expandable: bool,
    pub expanded: bool,
    pub selected: bool,
    pub in_cloud: bool,
    pub denied: bool,
    pub other_volume: bool,
    pub note: String,
}

impl From<core::UsageRowInfo> for FfiUsageRow {
    fn from(r: core::UsageRowInfo) -> Self {
        FfiUsageRow {
            id: r.id,
            depth: r.depth,
            name: r.name,
            size_text: r.size_text,
            size: r.size,
            fraction: r.fraction,
            percent_text: r.percent_text,
            items_text: r.items_text,
            modified_text: r.modified_text,
            kind: r.kind.into(),
            ext: r.ext,
            icon_key: r.icon_key,
            expandable: r.expandable,
            expanded: r.expanded,
            selected: r.selected,
            in_cloud: r.in_cloud,
            denied: r.denied,
            other_volume: r.other_volume,
            note: r.note,
        }
    }
}

/// Details of one item (`fstudio_core::UsageNodeInfo`).
#[derive(uniffi::Record, Debug, Clone)]
pub struct FfiUsageNodeInfo {
    pub id: u32,
    pub path: String,
    pub name: String,
    pub kind: FfiRowKind,
    pub allocated_text: String,
    pub logical_text: String,
    pub size_text: String,
    pub share_text: String,
    pub items_text: String,
    pub modified_text: String,
    pub type_text: String,
    pub is_folder: bool,
    pub is_root: bool,
    pub note: String,
}

impl From<core::UsageNodeInfo> for FfiUsageNodeInfo {
    fn from(i: core::UsageNodeInfo) -> Self {
        FfiUsageNodeInfo {
            id: i.id,
            path: i.path,
            name: i.name,
            kind: i.kind.into(),
            allocated_text: i.allocated_text,
            logical_text: i.logical_text,
            size_text: i.size_text,
            share_text: i.share_text,
            items_text: i.items_text,
            modified_text: i.modified_text,
            type_text: i.type_text,
            is_folder: i.is_folder,
            is_root: i.is_root,
            note: i.note,
        }
    }
}

/// One line of the breakdown by type.
#[derive(uniffi::Record, Debug, Clone)]
pub struct FfiUsageTypeRow {
    pub name: String,
    pub ext: String,
    pub kind_text: String,
    pub size_text: String,
    pub size: u64,
    pub count_text: String,
    pub fraction: f64,
    pub slot: u8,
}

impl From<core::UsageTypeRow> for FfiUsageTypeRow {
    fn from(t: core::UsageTypeRow) -> Self {
        FfiUsageTypeRow {
            name: t.name,
            ext: t.ext,
            kind_text: t.kind_text,
            size_text: t.size_text,
            size: t.size,
            count_text: t.count_text,
            fraction: t.fraction,
            slot: t.slot,
        }
    }
}

/// The header and status line.
#[derive(uniffi::Record, Debug, Clone)]
pub struct FfiUsageSummary {
    pub title: String,
    pub path: String,
    pub state: FfiUsageState,
    pub size_text: String,
    pub items_text: String,
    pub mode: FfiUsageSizeMode,
    pub detail: String,
    pub progress_text: String,
    pub current: String,
}

impl From<core::UsageSummary> for FfiUsageSummary {
    fn from(s: core::UsageSummary) -> Self {
        FfiUsageSummary {
            title: s.title,
            path: s.path,
            state: s.state.into(),
            size_text: s.size_text,
            items_text: s.items_text,
            mode: s.mode.into(),
            detail: s.detail,
            progress_text: s.progress_text,
            current: s.current,
        }
    }
}

/// One treemap rectangle, in view points (origin top-left).
#[derive(uniffi::Record, Debug, Clone, Copy)]
pub struct FfiUsageTile {
    pub id: u32,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub depth: u16,
    pub leaf: bool,
    /// 1…12: the top types; 0 other files; 250 iCloud; 251 a folder too
    /// small to split; 255 a folder frame.
    pub color: u8,
}

/// A breadcrumb or legend entry.
#[derive(uniffi::Record, Debug, Clone)]
pub struct FfiUsageLabel {
    pub id: u32,
    pub name: String,
}

/// One disk-usage scan and its view state.
#[derive(uniffi::Object)]
pub struct FfiUsageView {
    inner: Arc<core::UsageView>,
}

#[uniffi::export]
impl FfiUsageView {
    pub fn root_path(&self) -> String {
        self.inner.root_path()
    }
    pub fn summary(&self) -> FfiUsageSummary {
        self.inner.summary().into()
    }
    pub fn cancel(&self) {
        self.inner.cancel();
    }
    pub fn rescan(&self) {
        self.inner.rescan();
    }
    /// Rescans one folder in the background.
    pub fn refresh(&self, id: u32) {
        self.inner.refresh(id);
    }
    pub fn set_mode(&self, mode: FfiUsageSizeMode) {
        self.inner.set_mode(mode.into());
    }
    pub fn row_count(&self) -> u32 {
        self.inner.row_count()
    }
    pub fn rows(&self, start: u32, count: u32) -> Vec<FfiUsageRow> {
        self.inner
            .rows(start, count)
            .into_iter()
            .map(Into::into)
            .collect()
    }
    pub fn set_expanded(&self, id: u32, open: bool) {
        self.inner.set_expanded(id, open);
    }
    pub fn row_of(&self, id: u32) -> Option<u32> {
        self.inner.row_of(id)
    }
    pub fn selected(&self) -> Option<u32> {
        self.inner.selected()
    }
    pub fn select(&self, id: Option<u32>) -> Option<u32> {
        self.inner.select(id)
    }
    pub fn move_selection(&self, mv: FfiUsageMove) -> Option<u32> {
        self.inner.move_selection(mv.into())
    }
    pub fn info(&self, id: u32) -> Option<FfiUsageNodeInfo> {
        self.inner.info(id).map(Into::into)
    }
    pub fn find(&self, path: String) -> Option<u32> {
        self.inner.find(&path)
    }
    pub fn paths(&self, ids: Vec<u32>) -> Vec<String> {
        self.inner.paths(&ids)
    }
    pub fn zoom(&self) -> u32 {
        self.inner.zoom()
    }
    pub fn set_zoom(&self, id: u32) {
        self.inner.set_zoom(id);
    }
    pub fn zoom_out(&self) -> bool {
        self.inner.zoom_out()
    }
    pub fn zoom_path(&self) -> Vec<FfiUsageLabel> {
        self.inner
            .zoom_path()
            .into_iter()
            .map(|(id, name)| FfiUsageLabel { id, name })
            .collect()
    }
    /// The treemap for a view of `width` × `height` points (cached in the
    /// core until the size, zoom, mode or tree changes).
    pub fn layout(&self, width: f64, height: f64) -> Vec<FfiUsageTile> {
        self.inner
            .layout(width, height)
            .iter()
            .map(|t| FfiUsageTile {
                id: t.id,
                x: t.rect.x,
                y: t.rect.y,
                w: t.rect.w,
                h: t.rect.h,
                depth: t.depth,
                leaf: t.leaf,
                color: t.color,
            })
            .collect()
    }
    pub fn hit(&self, x: f64, y: f64) -> Option<u32> {
        self.inner.hit(x, y)
    }
    pub fn types(&self) -> Vec<FfiUsageTypeRow> {
        self.inner.types().into_iter().map(Into::into).collect()
    }
    /// The legend: `id` is the colour slot.
    pub fn legend(&self) -> Vec<FfiUsageLabel> {
        self.inner
            .legend()
            .into_iter()
            .map(|(name, slot)| FfiUsageLabel {
                id: u32::from(slot),
                name,
            })
            .collect()
    }
    /// After a trash, undo or move: drops what is gone, rescans what came
    /// back. Touches the disk (`lstat` per path).
    pub fn sync_paths(&self, paths: Vec<String>) {
        self.inner.sync_paths(&paths);
    }
}

#[uniffi::export]
impl FfiEngine {
    /// Starts a disk-usage scan of a folder or volume (`lstat` only; the
    /// scan runs in the background and reports through `observer`).
    pub fn usage_scan(
        &self,
        path: String,
        observer: Arc<dyn FfiUsageObserver>,
    ) -> R<Arc<FfiUsageView>> {
        let v = self
            .inner
            .usage_scan(&path, Arc::new(ObserverBridge(observer)))?;
        Ok(Arc::new(FfiUsageView { inner: v }))
    }
    /// Move to Trash from the usage view, through the ops queue (asks first
    /// when that setting is on; never deletes for good).
    pub fn usage_trash(&self, paths: Vec<String>) -> R<FfiOutcome> {
        Ok(self.inner.usage_trash(paths)?.into())
    }
}

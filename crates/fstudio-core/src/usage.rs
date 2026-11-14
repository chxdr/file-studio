//! Disk usage (M3): the usage view's core side and Get Folder Size.
//!
//! [`Engine::usage_scan`] starts a scan ([`fstudio_usage::UsageSession`])
//! and returns a [`UsageView`], which answers the view's questions with
//! display-ready rows: the tree list ([`UsageRowInfo`]), the treemap tiles,
//! the breakdown by type ([`UsageTypeRow`]) and a summary line. Like a
//! listing (F-005), the tree never crosses to the shell whole; the shell
//! asks for the rows it shows and the tiles for its size.
//!
//! Trashing from the view goes through the ops queue with the usual
//! question, toast and undo ([`Engine::usage_trash`]); there is no permanent
//! delete here (F-007).
//!
//! Get Folder Size (`file.folder_size`) runs the same scanner without
//! keeping the tree and answers with a toast. Results are kept in an
//! in-memory cache ([`Engine::cached_folder_size`]) until the `folder_sizes`
//! table lands in `fstudio-db`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use fstudio_keymap::CommandArg;
use fstudio_ops::OpRequest;
use fstudio_usage::types::category;
use fstudio_usage::{NodeKind, ScanControl, ScanOptions, SizeMode, Totals, UsageSession, flags};
use fstudio_view::{SizeFormat, format_date, format_size};
use parking_lot::Mutex;

pub use fstudio_usage::{
    Move as UsageMove, Progress as UsageProgress, ScanState as UsageState, Tile as UsageTile,
    UsageChange, UsageObserver,
};

use crate::confirm::{After, PendingAction, PendingOp, snapshot};
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::events::CoreEvent;
use crate::ids::TabId;
use crate::listing::{RowFormat, RowKind};
use crate::outcome::{Outcome, ShellRequest, ToastStyle};

/// Which size the usage view shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UsageSizeMode {
    /// Space used on disk (what Move to Trash would free).
    #[default]
    OnDisk,
    /// The files' own sizes, as Finder shows them; iCloud placeholders count.
    Logical,
}

impl From<UsageSizeMode> for SizeMode {
    fn from(m: UsageSizeMode) -> Self {
        match m {
            UsageSizeMode::OnDisk => SizeMode::Allocated,
            UsageSizeMode::Logical => SizeMode::Logical,
        }
    }
}

impl From<SizeMode> for UsageSizeMode {
    fn from(m: SizeMode) -> Self {
        match m {
            SizeMode::Allocated => UsageSizeMode::OnDisk,
            SizeMode::Logical => UsageSizeMode::Logical,
        }
    }
}

/// One row of the usage tree list, ready to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageRowInfo {
    /// The node (stable for this scan).
    pub id: u32,
    /// Indent level; the scanned folder is 0.
    pub depth: u16,
    /// The name exactly as on disk (the scanned folder shows its display name).
    pub name: String,
    /// "1.2 GB".
    pub size_text: String,
    /// The size in bytes, in the current mode.
    pub size: u64,
    /// Share of the parent's size, 0…1 (1 for the scanned folder).
    pub fraction: f64,
    /// "12.3 %".
    pub percent_text: String,
    /// "1,204" (items inside a folder; empty for files).
    pub items_text: String,
    /// The newest change at or below it.
    pub modified_text: String,
    /// What it is.
    pub kind: RowKind,
    /// The extension, lower case ("" for none and for folders).
    pub ext: String,
    /// The icon key, as for listing rows (`folder`, `file.jpg`, `package.app`).
    pub icon_key: String,
    /// It has rows below it.
    pub expandable: bool,
    /// They are showing.
    pub expanded: bool,
    /// The selected row.
    pub selected: bool,
    /// An iCloud placeholder, or a folder holding some.
    pub in_cloud: bool,
    /// Couldn't be read (or holds folders that couldn't).
    pub denied: bool,
    /// Another volume, not scanned.
    pub other_volume: bool,
    /// A note for the tooltip ("Couldn’t be read", "In iCloud: 1.2 GB").
    pub note: String,
}

/// Everything the view shows about one item (hover, the status line).
#[derive(Debug, Clone, PartialEq)]
pub struct UsageNodeInfo {
    /// The node.
    pub id: u32,
    /// Its full path.
    pub path: String,
    /// Its name.
    pub name: String,
    /// What it is.
    pub kind: RowKind,
    /// Size on disk.
    pub allocated_text: String,
    /// Logical size.
    pub logical_text: String,
    /// The size in the current mode.
    pub size_text: String,
    /// "4.1 % of “Photos”".
    pub share_text: String,
    /// "1,204 items", or "" for files.
    pub items_text: String,
    /// The newest change.
    pub modified_text: String,
    /// "JPG · Image", "Folder", "Package".
    pub type_text: String,
    /// A folder (can be zoomed into and expanded).
    pub is_folder: bool,
    /// The scanned folder itself.
    pub is_root: bool,
    /// Flags as a sentence, or "".
    pub note: String,
}

/// One line of the breakdown by type.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageTypeRow {
    /// "JPG", or "No extension".
    pub name: String,
    /// The extension as stored in the tree ("jpg", "" for none).
    pub ext: String,
    /// "RAW Photo", "Video", "Package"…
    pub kind_text: String,
    /// "48.2 GB".
    pub size_text: String,
    /// Bytes in the current mode.
    pub size: u64,
    /// "12,034".
    pub count_text: String,
    /// Share of the treemap's folder, 0…1.
    pub fraction: f64,
    /// The treemap colour slot (1…12 for the top types, 0 for others).
    pub slot: u8,
}

/// The view's header and status line.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageSummary {
    /// The scanned folder's display name ("Macintosh HD", "Pictures").
    pub title: String,
    /// Its path.
    pub path: String,
    /// Where the scan is.
    pub state: UsageState,
    /// The size in the current mode ("312.4 GB").
    pub size_text: String,
    /// "1,149,084 items".
    pub items_text: String,
    /// The mode.
    pub mode: UsageSizeMode,
    /// "Scanned in 3.1 s · 400 folders couldn’t be read · 150 MB in iCloud".
    pub detail: String,
    /// While scanning: "412,331 items · 81.2 GB" and the current folder.
    pub progress_text: String,
    /// While scanning: the folder being read.
    pub current: String,
}

/// One running or finished disk-usage scan, as the view sees it.
pub struct UsageView {
    session: Arc<UsageSession>,
    hour24: bool,
}

impl std::fmt::Debug for UsageView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsageView")
            .field("root", &self.session.root_path())
            .finish()
    }
}

/// `1234567` → `"1,234,567"`.
pub(crate) fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn size_text(b: u64) -> String {
    format_size(b, &SizeFormat::default())
}

fn percent_text(f: f64) -> String {
    if f <= 0.0 {
        "0 %".into()
    } else if f < 0.001 {
        "< 0.1 %".into()
    } else {
        format!("{:.1} %", f * 100.0)
    }
}

fn row_kind(k: NodeKind) -> RowKind {
    match k {
        NodeKind::Dir => RowKind::Folder,
        NodeKind::Package => RowKind::Package,
        NodeKind::File => RowKind::File,
        NodeKind::Symlink => RowKind::Symlink,
        NodeKind::Other => RowKind::Other,
    }
}

fn icon_key(kind: NodeKind, ext: &str) -> String {
    match (kind, ext) {
        (NodeKind::Dir, _) => "folder".into(),
        (NodeKind::Symlink, _) => "symlink".into(),
        (NodeKind::Other, _) => "other".into(),
        (NodeKind::Package, e) => format!("package.{e}"),
        (NodeKind::File, "") => "file".into(),
        (NodeKind::File, e) => format!("file.{e}"),
    }
}

fn note_for(n: &fstudio_usage::Node) -> String {
    let mut parts: Vec<String> = Vec::new();
    if n.has(flags::DENIED) {
        parts.push("Couldn’t be read (permission denied)".into());
    } else if n.denied > 0 {
        parts.push(format!(
            "{} folder{} inside couldn’t be read",
            thousands(u64::from(n.denied)),
            if n.denied == 1 { "" } else { "s" }
        ));
    }
    if n.has(flags::DATALESS) {
        parts.push("In iCloud, not downloaded".into());
    } else if n.has(flags::CLOUD_DIR) {
        parts.push("An iCloud folder that isn’t downloaded; not listed".into());
    } else if n.cloud > 0 {
        parts.push(format!("{} in iCloud", size_text(n.cloud)));
    }
    if n.has(flags::OTHER_VOLUME) {
        parts.push("On another volume; not scanned".into());
    }
    if n.has(flags::HARDLINK) {
        parts.push("Another hard link to a file counted elsewhere".into());
    }
    if n.has(flags::ERROR) {
        parts.push("Couldn’t be read".into());
    }
    if n.has(flags::INCOMPLETE) {
        parts.push("Scan stopped before this was finished".into());
    }
    if n.has(flags::SKIPPED) {
        parts.push("Counted through its firmlinks instead".into());
    }
    parts.join(" · ")
}

impl UsageView {
    fn row_format(&self) -> RowFormat {
        RowFormat::now(self.hour24)
    }

    /// The underlying session (for the CLI and tests).
    pub fn session(&self) -> &Arc<UsageSession> {
        &self.session
    }

    /// The scanned folder.
    pub fn root_path(&self) -> String {
        self.session.root_path().to_string_lossy().into_owned()
    }

    /// The header and status line.
    pub fn summary(&self) -> UsageSummary {
        let s = &self.session;
        let state = s.state();
        let mode = s.mode();
        let title = fstudio_fs::display_name(s.root_path());
        let path = self.root_path();
        let p = s.progress();
        let progress_text = format!(
            "{} items · {}",
            thousands(p.items),
            size_text(match mode {
                SizeMode::Allocated => p.allocated,
                SizeMode::Logical => p.logical,
            })
        );
        let elapsed = s.elapsed_ms();
        let (size, items, detail) = s
            .with_tree(|t| {
                let r = t.node(t.root());
                let mut d = Vec::new();
                match &state {
                    UsageState::Done => {
                        d.push(format!("Scanned in {:.1} s", elapsed as f64 / 1000.0))
                    }
                    UsageState::Cancelled => d.push("Scan stopped; sizes are partial".into()),
                    _ => {}
                }
                if r.denied > 0 {
                    d.push(format!(
                        "{} folder{} couldn’t be read",
                        thousands(u64::from(r.denied)),
                        if r.denied == 1 { "" } else { "s" }
                    ));
                }
                if r.cloud > 0 {
                    d.push(format!("{} in iCloud", size_text(r.cloud)));
                }
                (r.size(mode), r.items, d.join(" · "))
            })
            .unwrap_or_default();
        let detail = match &state {
            UsageState::Failed(e) => format!("Couldn’t scan: {e}"),
            _ => detail,
        };
        UsageSummary {
            title,
            path,
            size_text: size_text(size),
            items_text: format!("{} items", thousands(items)),
            mode: mode.into(),
            detail,
            progress_text,
            current: p.current,
            state,
        }
    }

    /// Stops the scan; what was read stays.
    pub fn cancel(&self) {
        self.session.cancel();
    }

    /// Scans the whole folder again.
    pub fn rescan(&self) {
        self.session.rescan();
    }

    /// Rescans one folder of the tree.
    pub fn refresh(&self, id: u32) {
        self.session.refresh(id);
    }

    /// Shows sizes on disk or logical sizes.
    pub fn set_mode(&self, mode: UsageSizeMode) {
        self.session.set_mode(mode.into());
    }

    /// How many rows the tree list has.
    pub fn row_count(&self) -> u32 {
        self.session.row_count() as u32
    }

    /// Rows `start..start+count`, formatted.
    pub fn rows(&self, start: u32, count: u32) -> Vec<UsageRowInfo> {
        let rows = self.session.rows(start as usize, count as usize);
        let sel = self.session.selected();
        let mode = self.session.mode();
        let f = self.row_format();
        self.session
            .with_tree(|t| {
                rows.iter()
                    .map(|r| {
                        let n = t.node(r.id);
                        let size = n.size(mode);
                        let parent = if n.parent == fstudio_usage::NO_NODE {
                            size
                        } else {
                            t.node(n.parent).size(mode)
                        };
                        let fraction = if parent == 0 {
                            0.0
                        } else {
                            size as f64 / parent as f64
                        };
                        let is_root = r.id == t.root();
                        UsageRowInfo {
                            id: r.id,
                            depth: r.depth,
                            name: if is_root {
                                fstudio_fs::display_name(&t.path(r.id))
                            } else {
                                n.name_lossy()
                            },
                            size_text: size_text(size),
                            size,
                            fraction,
                            percent_text: percent_text(fraction),
                            items_text: if n.kind.is_dir() {
                                thousands(n.items)
                            } else {
                                String::new()
                            },
                            modified_text: if n.mtime_ns == 0 {
                                String::new()
                            } else {
                                format_date(n.mtime_ns, f.clock, &f.dates)
                            },
                            kind: row_kind(n.kind),
                            ext: t.ext_name(n.ext).to_string(),
                            icon_key: icon_key(n.kind, t.ext_name(n.ext)),
                            expandable: r.expandable,
                            expanded: r.expanded,
                            selected: sel == Some(r.id),
                            in_cloud: n.cloud > 0 || n.has(flags::DATALESS | flags::CLOUD_DIR),
                            denied: n.denied > 0 || n.has(flags::DENIED),
                            other_volume: n.has(flags::OTHER_VOLUME),
                            note: note_for(n),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Expands or collapses a folder row.
    pub fn set_expanded(&self, id: u32, open: bool) {
        self.session.set_expanded(id, open);
    }

    /// The row of a node, if showing.
    pub fn row_of(&self, id: u32) -> Option<u32> {
        self.session.row_of(id).map(|r| r as u32)
    }

    /// The selected node.
    pub fn selected(&self) -> Option<u32> {
        self.session.selected()
    }

    /// Selects a node (from the list or the treemap), expanding the rows
    /// above it; returns its row.
    pub fn select(&self, id: Option<u32>) -> Option<u32> {
        self.session.select(id).map(|r| r as u32)
    }

    /// Moves the selection by a key; returns the new row.
    pub fn move_selection(&self, mv: UsageMove) -> Option<u32> {
        self.session.move_selection(mv).map(|r| r as u32)
    }

    /// Details of one node, for hover and the status line.
    pub fn info(&self, id: u32) -> Option<UsageNodeInfo> {
        let mode = self.session.mode();
        let f = self.row_format();
        self.session.with_tree(|t| {
            let n = t.get(id)?;
            let size = n.size(mode);
            let share_text = if n.parent == fstudio_usage::NO_NODE {
                String::new()
            } else {
                let p = t.node(n.parent);
                let ps = p.size(mode);
                let frac = if ps == 0 {
                    0.0
                } else {
                    size as f64 / ps as f64
                };
                let pname = if n.parent == t.root() {
                    fstudio_fs::display_name(&t.path(n.parent))
                } else {
                    p.name_lossy()
                };
                format!("{} of “{pname}”", percent_text(frac))
            };
            let ext = t.ext_name(n.ext);
            let type_text = match n.kind {
                NodeKind::Dir => "Folder".to_string(),
                NodeKind::Symlink => "Symbolic link".to_string(),
                NodeKind::Other => "Special file".to_string(),
                NodeKind::Package | NodeKind::File => {
                    let cat = category(ext, n.kind).label();
                    if ext.is_empty() {
                        cat.to_string()
                    } else {
                        format!("{} · {cat}", ext.to_uppercase())
                    }
                }
            };
            let path = t.path(id);
            Some(UsageNodeInfo {
                id,
                name: if id == t.root() {
                    fstudio_fs::display_name(&path)
                } else {
                    n.name_lossy()
                },
                path: path.to_string_lossy().into_owned(),
                kind: row_kind(n.kind),
                allocated_text: size_text(n.allocated),
                logical_text: size_text(n.logical),
                size_text: size_text(size),
                share_text,
                items_text: if n.kind.is_dir() {
                    format!("{} items", thousands(n.items))
                } else {
                    String::new()
                },
                modified_text: format_date(n.mtime_ns, f.clock, &f.dates),
                type_text,
                is_folder: n.kind.is_dir(),
                is_root: id == t.root(),
                note: note_for(n),
            })
        })?
    }

    /// The node at a path, if scanned.
    pub fn find(&self, path: &str) -> Option<u32> {
        self.session.find(Path::new(path))
    }

    /// Paths of nodes (for Reveal, Quick Look, Get Info, Trash).
    pub fn paths(&self, ids: &[u32]) -> Vec<String> {
        ids.iter()
            .filter_map(|id| self.session.path(*id))
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    }

    /// The folder the treemap shows.
    pub fn zoom(&self) -> u32 {
        self.session.zoom()
    }

    /// Shows a folder (or a file's folder) in the whole treemap.
    pub fn set_zoom(&self, id: u32) {
        self.session.set_zoom(id);
    }

    /// Zooms out one level; false at the top.
    pub fn zoom_out(&self) -> bool {
        self.session.zoom_out()
    }

    /// The path from the scanned folder to the treemap's folder, for its
    /// breadcrumb: `(id, name)` pairs, top first.
    pub fn zoom_path(&self) -> Vec<(u32, String)> {
        let z = self.session.zoom();
        self.session
            .with_tree(|t| {
                t.ancestors(z)
                    .into_iter()
                    .map(|id| {
                        let name = if id == t.root() {
                            fstudio_fs::display_name(&t.path(id))
                        } else {
                            t.node(id).name_lossy()
                        };
                        (id, name)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The treemap's tiles for a view of `width` × `height` points.
    pub fn layout(&self, width: f64, height: f64) -> Arc<Vec<UsageTile>> {
        self.session.layout(width, height)
    }

    /// The node under a point of the last layout.
    pub fn hit(&self, x: f64, y: f64) -> Option<u32> {
        self.session.hit(x, y)
    }

    /// The breakdown by file type below the treemap's folder.
    pub fn types(&self) -> Vec<UsageTypeRow> {
        let mode = self.session.mode();
        let stats = self.session.types();
        let zoom = self.session.zoom();
        let total = self
            .session
            .with_tree(|t| t.node(zoom).size(mode))
            .unwrap_or(0);
        stats
            .iter()
            .map(|s| {
                let size = s.size(mode);
                UsageTypeRow {
                    name: if s.ext_name.is_empty() {
                        "No extension".into()
                    } else {
                        s.ext_name.to_uppercase()
                    },
                    ext: s.ext_name.clone(),
                    kind_text: s.category.label().into(),
                    size_text: size_text(size),
                    size,
                    count_text: thousands(s.count),
                    fraction: if total == 0 {
                        0.0
                    } else {
                        size as f64 / total as f64
                    },
                    slot: self.session.slot(s.ext),
                }
            })
            .collect()
    }

    /// The legend: the types with their own colour, slot 1 first, as
    /// `(name, slot)`.
    pub fn legend(&self) -> Vec<(String, u8)> {
        let top = self.session.palette();
        self.session
            .with_tree(|t| {
                top.iter()
                    .enumerate()
                    .map(|(i, e)| (t.ext_name(*e).to_uppercase(), (i + 1) as u8))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Brings the tree in line with the disk for these paths (after a
    /// trash, an undo or a move). Touches the disk (`lstat` per path).
    pub fn sync_paths(&self, paths: &[String]) {
        let v: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        self.session.sync_paths(&v);
    }
}

// ───────────── Get Folder Size cache ─────────────

/// A cached Get Folder Size answer.
#[derive(Debug, Clone)]
struct CachedSize {
    totals: Totals,
    /// The folder's own modification time when it was sized (a cheap
    /// fingerprint: it changes when items are added or removed directly
    /// inside it, not deeper down).
    mtime_ns: i64,
    at: Instant,
}

/// How long a cached size is trusted without rescanning.
const CACHE_FRESH: Duration = Duration::from_secs(60);

fn cache() -> &'static Mutex<HashMap<PathBuf, CachedSize>> {
    static C: OnceLock<Mutex<HashMap<PathBuf, CachedSize>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn dir_mtime(p: &Path) -> Option<i64> {
    use std::os::unix::fs::MetadataExt;
    let m = p.symlink_metadata().ok()?;
    Some(m.mtime() * 1_000_000_000 + m.mtime_nsec())
}

fn cached(p: &Path) -> Option<Totals> {
    let c = cache().lock();
    let e = c.get(p)?;
    (e.at.elapsed() < CACHE_FRESH && dir_mtime(p) == Some(e.mtime_ns)).then(|| e.totals.clone())
}

fn remember(p: &Path, t: &Totals) {
    if t.incomplete {
        return;
    }
    if let Some(m) = dir_mtime(p) {
        cache().lock().insert(
            p.to_path_buf(),
            CachedSize {
                totals: t.clone(),
                mtime_ns: m,
                at: Instant::now(),
            },
        );
    }
}

/// A single item's own size, as totals (Get Folder Size on a file).
fn file_totals(p: &Path) -> Totals {
    use std::os::unix::fs::MetadataExt;
    match p.symlink_metadata() {
        Ok(m) => Totals {
            logical: m.len(),
            allocated: m.blocks() * 512,
            ..Totals::default()
        },
        Err(_) => Totals {
            incomplete: true,
            ..Totals::default()
        },
    }
}

/// The toast text for Get Folder Size.
pub(crate) fn folder_size_text(names: &[String], t: &Totals) -> String {
    let who = match names {
        [one] => format!("“{one}”"),
        _ => format!("{} items", names.len()),
    };
    let mut s = format!(
        "{who}: {} on disk ({}) · {} items",
        size_text(t.allocated),
        size_text(t.logical),
        thousands(t.items)
    );
    if t.cloud > 0 {
        s.push_str(&format!(" · {} in iCloud", size_text(t.cloud)));
    }
    if t.denied > 0 {
        s.push_str(&format!(
            " · {} folder{} couldn’t be read",
            thousands(u64::from(t.denied)),
            if t.denied == 1 { "" } else { "s" }
        ));
    }
    s
}

impl Engine {
    /// Starts a disk-usage scan of the folder or volume at `path` and
    /// returns its view at once; progress and the end arrive through
    /// `observer`. Blocks only on one `lstat` of `path`.
    pub fn usage_scan(
        &self,
        path: &str,
        observer: Arc<dyn UsageObserver>,
    ) -> Result<Arc<UsageView>> {
        let p = PathBuf::from(path);
        let m = p
            .symlink_metadata()
            .map_err(|e| CoreError::NotFound(format!("{path}: {e}")))?;
        if !m.is_dir() {
            return Err(CoreError::NotAvailable(format!(
                "“{}” isn’t a folder",
                fstudio_fs::display_name(&p)
            )));
        }
        let hour24 = self.settings().view.hour24;
        let session = UsageSession::start(p, ScanOptions::default(), observer);
        Ok(Arc::new(UsageView { session, hour24 }))
    }

    /// The last Get Folder Size answer for a folder, if it is recent and the
    /// folder's own contents haven't changed since.
    pub fn cached_folder_size(&self, path: &str) -> Option<Totals> {
        cached(Path::new(path))
    }

    /// Moves items chosen in the usage view to the Trash, through the ops
    /// queue: asks first when "Confirm Move to Trash" is on, toasts with
    /// Undo, and journals the move like any trash. Never deletes for good.
    pub fn usage_trash(&self, paths: Vec<String>) -> Result<Outcome> {
        if paths.is_empty() {
            return Err(CoreError::NotAvailable("nothing is selected".into()));
        }
        let sel: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        let op = PendingOp {
            req: OpRequest::Trash { paths: sel.clone() },
            items: snapshot(&sel),
            tab: None,
            after: After::default(),
            toast: Some(ShellRequest::Moved {
                text: crate::feedback::trashed_text(&sel),
            }),
        };
        if self.settings().general.confirm_trash {
            let token = self.ask(PendingAction::Op(Box::new(op)));
            return Ok(Outcome::shell(ShellRequest::ConfirmTrash { token, paths }));
        }
        self.run_pending_op(op, false)
    }

    /// `nav.disk_usage` and `file.folder_size`.
    pub(crate) fn usage_command(
        &self,
        id: &str,
        arg: Option<&CommandArg>,
        tab: Option<TabId>,
    ) -> Result<Outcome> {
        let t = tab.and_then(|t| self.tab(t)).or_else(|| self.active_tab());
        let given = match arg {
            Some(CommandArg::Path(p)) => Some(p.clone()),
            _ => None,
        };
        match id {
            "nav.disk_usage" => {
                let path = match given {
                    Some(p) => p,
                    None => t
                        .as_ref()
                        .map(|t| t.location_path())
                        .ok_or_else(|| CoreError::NotAvailable("no folder is open".into()))?,
                };
                let mut out = Outcome::shell(ShellRequest::ShowDiskUsage {
                    path: path.to_string_lossy().into_owned(),
                });
                out.tab = t.map(|t| t.id());
                Ok(out)
            }
            "file.folder_size" => {
                let paths: Vec<PathBuf> = match given {
                    Some(p) => vec![p],
                    None => {
                        let t =
                            t.ok_or_else(|| CoreError::NotAvailable("nothing is selected".into()))?;
                        let sel = t.selected_pathbufs();
                        if sel.is_empty() {
                            vec![t.location_path()]
                        } else {
                            sel
                        }
                    }
                };
                // Files in the selection are added at their own size.
                let folders: Vec<PathBuf> = paths
                    .into_iter()
                    .filter(|p| p.symlink_metadata().is_ok())
                    .collect();
                if folders.is_empty() {
                    return Err(CoreError::NotAvailable("nothing is selected".into()));
                }
                let tab_id = tab.or_else(|| self.active_tab().map(|t| t.id()));
                self.spawn_folder_size(folders, tab_id);
                Ok(Outcome::message("Working out the size…"))
            }
            _ => Err(CoreError::UnknownCommand(id.into())),
        }
    }

    /// Sizes folders on a background thread and toasts the answer.
    fn spawn_folder_size(&self, folders: Vec<PathBuf>, tab: Option<TabId>) {
        let shared = self.shared.clone();
        std::thread::Builder::new()
            .name("fstudio-folder-size".into())
            .spawn(move || {
                let mut sum = Totals::default();
                let mut names = Vec::new();
                for f in &folders {
                    names.push(fstudio_fs::display_name(f));
                    let t = match cached(f) {
                        Some(t) => t,
                        None if !f.symlink_metadata().is_ok_and(|m| m.is_dir()) => file_totals(f),
                        None => match fstudio_usage::folder_totals(f, &ScanControl::new()) {
                            Ok(t) => {
                                remember(f, &t);
                                t
                            }
                            Err(_) => Totals {
                                incomplete: true,
                                ..Totals::default()
                            },
                        },
                    };
                    sum.logical += t.logical;
                    sum.allocated += t.allocated;
                    sum.items += t.items;
                    sum.cloud += t.cloud;
                    sum.denied += t.denied;
                    sum.incomplete |= t.incomplete;
                }
                let style = if sum.denied > 0 || sum.incomplete {
                    ToastStyle::Warning
                } else {
                    ToastStyle::Info
                };
                shared.emit(CoreEvent::Shell {
                    tab,
                    request: ShellRequest::Toast {
                        text: folder_size_text(&names, &sum),
                        button: None,
                        command: None,
                        style,
                    },
                });
            })
            .ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_groups() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(1_149_084), "1,149,084");
    }

    #[test]
    fn toast_text() {
        let t = Totals {
            logical: 2_000_000,
            allocated: 2_100_000,
            items: 1234,
            cloud: 0,
            denied: 1,
            incomplete: false,
            elapsed_ms: 5,
        };
        assert_eq!(
            folder_size_text(&["Photos".into()], &t),
            "“Photos”: 2.1 MB on disk (2 MB) · 1,234 items · 1 folder couldn’t be read"
        );
        assert!(folder_size_text(&["a".into(), "b".into()], &t).starts_with("2 items:"));
    }
}

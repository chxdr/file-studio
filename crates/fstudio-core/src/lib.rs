#![warn(missing_docs)]
//! The File Studio core facade (F-001, F-002, F-005, F-006, F-010).
//!
//! One [`Engine`] per process owns the database, [`Settings`], the workspace
//! (windows → panes → [`Tab`]s), a shared folder watcher, the operation
//! queue, frecency and completion, and the keymap. Shells talk to it through
//! plain records, enums and `Arc` objects, and receive [`CoreEvent`]s through
//! an [`EventSink`]; there are no generics or lifetimes on this surface, so
//! UniFFI can export it in M2.
//!
//! ```no_run
//! use std::sync::Arc;
//! use fstudio_core::*;
//! let (tx, rx) = crossbeam_channel::unbounded();
//! let engine = Engine::new(
//!     EngineConfig { data_dir: "/tmp/fstudio-demo".into(), ..Default::default() },
//!     Arc::new(tx),
//! ).unwrap();
//! engine.restore_session();
//! let tab = engine.active_tab().unwrap();
//! tab.go("~/Downloads").unwrap();
//! while let Ok(ev) = rx.recv() {
//!     if let CoreEvent::ListingChanged { kind: ChangeKind::Sorted, .. } = ev {
//!         for row in tab.rows(0, 20) {
//!             println!("{}  {}  {}", row.name, row.size_text, row.modified_text);
//!         }
//!         break;
//!     }
//! }
//! ```
//!
//! Threads: no async runtime. Listing runs on short-lived lister threads,
//! watcher events on one router thread, events on one pump thread, session
//! saves on one saver thread, and file operations on the queue's threads.
//! No method blocks on disk except where its docs say so.

mod commands;
mod confirm;
mod drop;
mod engine;
mod error;
mod events;
mod favourites;
mod feedback;
mod grid;
mod ids;
mod listing;
mod listing_handle;
pub mod menus;
mod miller;
mod outcome;
mod preview;
mod probe;
mod protect;
// ── batch rename (M3) ──
mod rename;
mod settings;
mod shared;
mod sys;
mod system_integration;
mod tab;
mod usage;
mod view;
mod view_columns;
mod workspace;
mod workspaces;

pub use commands::{
    BindingSummary, CommandInvocation, CommandSummary, DropAction, Handling, InputContext,
    LATER_COMMANDS,
};
pub use drop::DropEffect;
pub use engine::{Engine, EngineConfig, FrecentDir};
pub use error::{CoreError, Result};
pub use events::{
    COALESCE_WINDOW, ChangeKind, CoreEvent, EventSink, HintInfo, NullEventSink, VolumeSummary,
};
pub use favourites::Favourite;

/// The name to show for a path everywhere (the volume's name for a volume
/// root, "Macintosh HD" for `/`, else the item's name as stored). One rule
/// for the sidebar, breadcrumbs, titles, toasts, questions and operation
/// titles; see [`fstudio_fs::display_name`].
pub fn display_name(path: &str) -> String {
    fstudio_fs::display_name(std::path::Path::new(path))
}
pub use grid::{
    GALLERY_PIXEL_STEPS, GridDir, GridLayout, GridSettings, ICON_SIZE_RANGE, ICON_SIZES,
    THUMB_PIXEL_STEPS, clamp_icon_size, gallery_order, gallery_pixels, grid_layout, grid_target,
    next_icon_size, thumb_pixels, thumbnail_order, wants_thumbnail,
};
pub use ids::{PaneId, TabId, WindowId, WorkspaceId};
pub use listing::{Listing, ListingState, RowInfo, RowKind};
pub use listing_handle::ListingHandle;
pub use menus::{
    MenuCommand, MenuContext, MenuEntryInfo, MenuItem, MenuKind, MenuOverride, MenuPlacement,
    MenuSettings, MenuTarget, SidebarItemKind, SystemMenu,
};
pub use outcome::{Outcome, ShellRequest, ToastStyle};
pub use preview::{PreviewService, PreviewSettings};
pub use probe::{FsCoreProbe, FsDirSource};
pub use protect::default_protected_folders;
pub use settings::{
    AppearanceSettings, FolderMemory, GeneralSettings, Motion, OperationsSettings, PathBarSettings,
    RowDensity, SETTINGS_VERSION, Settings, TerminalApp, ViewSettings,
};
pub use shared::MIGRATIONS;
pub use system_integration::{
    DEFAULT_VIEWER_EXPLAINED, FINDER_BUNDLE_ID, FolderOpensIn, IntegrationSettings, OpenItem,
    OpenStep, PanelKeyCheck, PanelKind, PanelProbe, ScreenRect, ViewerState, ViewerStatus,
    classify_panel, global_hotkey, missing_message, panel_button_origin, panel_button_title,
    panel_jump_text, panel_key_refusal, plan_system_open, viewer_status,
};
pub use tab::{CursorMove, ScrollAnchor, SelectionChange, SelectionInfo, Tab, TabState};
// M3 search.
pub use fstudio_search::{DateField, Kind as SearchKind, NameMode, SearchQuery};
pub use tab::search::{LeafCollation, SearchRequest, SearchScope, SearchState, SearchStatus};
pub use usage::{
    UsageChange, UsageMove, UsageNodeInfo, UsageObserver, UsageProgress, UsageRowInfo,
    UsageSizeMode, UsageState, UsageSummary, UsageTile, UsageTypeRow, UsageView,
};
pub use view::{SortBy, ViewState};
pub use view_columns::{
    ColumnId, ColumnSpec, MAX_COLUMN_WIDTH, MIN_COLUMN_WIDTH, default_columns, sanitize_columns,
};
pub use workspace::{
    DEFAULT_SIDEBAR_WIDTH, Orientation, PaneState, SIDEBAR_WIDTH_RANGE, WindowState,
    WorkspaceState, WorkspaceSummary,
};
pub use workspaces::{NewWorkspace, WORKSPACE_ICONS};

// ── batch rename (M3) ──
pub use fstudio_rename::{
    CaseKind, ExtChange, Pipeline, PipelineStep, Position, Preset, Step, TOKENS as RENAME_TOKENS,
    Target, TrimMode,
};
pub use rename::{RenameRow, RenameRowStatus, RenameSession, RenameSummary, default_pipeline};

// The types from other crates that appear on this crate's surface.
pub use fstudio_keymap::{
    CommandArg, Focus, HeldShortcuts, Mods, Profile, ShortcutRow, ViewMode, VimMode,
};
pub use fstudio_model::{Key, KeyChord, PathStyle, Via};
pub use fstudio_ops::{
    ConflictDecision, ConflictId, ConflictInfo, ConflictRules, EnqueueOptions, OpId, OpKind,
    OpQueue, OpReport, OpRequest, OpState, OpStatus, UndoState,
};
pub use fstudio_path::{
    Cleanup, Completion, CompletionKind, Completions, Crumb, CrumbKind, Location, ParsedLocation,
};
pub use fstudio_preview::{
    CancelToken, Category as PreviewCategory, FolderInfo, PhotoInfo, Preview, PreviewTag, Span,
    TextLine, TextPreview, TokenClass, VideoInfo,
};

/// The core's version, for the shells' About box and the CLI.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

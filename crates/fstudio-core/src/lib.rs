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
mod engine;
mod error;
mod events;
mod ids;
mod listing;
mod outcome;
mod probe;
mod settings;
mod shared;
mod sys;
mod tab;
mod view;
mod workspace;

pub use commands::{
    BindingSummary, CommandInvocation, CommandSummary, Handling, InputContext, LATER_COMMANDS,
};
pub use engine::{Engine, EngineConfig, FrecentDir};
pub use error::{CoreError, Result};
pub use events::{
    COALESCE_WINDOW, ChangeKind, CoreEvent, EventSink, HintInfo, NullEventSink, VolumeSummary,
};
pub use ids::{PaneId, TabId, WindowId, WorkspaceId};
pub use listing::{Listing, ListingState, RowInfo, RowKind};
pub use outcome::{Outcome, ShellRequest};
pub use probe::{FsCoreProbe, FsDirSource};
pub use settings::{
    FolderMemory, GeneralSettings, OperationsSettings, PathBarSettings, RowDensity,
    SETTINGS_VERSION, Settings, ViewSettings,
};
pub use shared::MIGRATIONS;
pub use tab::{CursorMove, ScrollAnchor, SelectionChange, SelectionInfo, Tab, TabState};
pub use view::{SortBy, ViewState};
pub use workspace::{Orientation, PaneState, WindowState, WorkspaceState, WorkspaceSummary};

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

/// The core's version, for the shells' About box and the CLI.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

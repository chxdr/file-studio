//! Plain numeric ids for windows, panes, tabs and saved workspaces.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

macro_rules! id_type {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub u64);

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

id_type!(
    /// One tab. Unique within a process run; restored sessions keep their ids.
    TabId
);
id_type!(
    /// One pane of a window (a window has one or two).
    PaneId
);
id_type!(
    /// One window.
    WindowId
);
id_type!(
    /// A saved workspace (its row id in the database).
    WorkspaceId
);

/// Hands out fresh ids for tabs, panes and windows. Restored ids bump it so
/// new ids never collide with restored ones.
#[derive(Debug, Default)]
pub(crate) struct IdGen(AtomicU64);

impl IdGen {
    pub(crate) fn next(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed) + 1
    }
    pub(crate) fn bump_past(&self, used: u64) {
        self.0.fetch_max(used, Ordering::Relaxed);
    }
}

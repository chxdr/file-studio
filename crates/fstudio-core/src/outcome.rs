//! What a command or navigation did, and what the shell must do for it.

use fstudio_ops::OpId;
use serde::{Deserialize, Serialize};

use crate::events::HintInfo;
use crate::ids::{PaneId, TabId, WindowId};

/// Work only the shell can do (Cocoa calls, dialogs, focus). The core has
/// already decided what the key or click means; the shell just carries it out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShellRequest {
    /// Open files with their default app (`NSWorkspace open`).
    Open {
        /// The files.
        paths: Vec<String>,
    },
    /// Show the "Open With" app chooser.
    OpenWith {
        /// The files.
        paths: Vec<String>,
    },
    /// Open in the user's editor.
    OpenInEditor {
        /// The files.
        paths: Vec<String>,
    },
    /// Quick Look.
    QuickLook {
        /// The items.
        paths: Vec<String>,
    },
    /// The Get Info panel.
    GetInfo {
        /// The items.
        paths: Vec<String>,
    },
    /// Reveal in Finder.
    RevealInFinder {
        /// The items.
        paths: Vec<String>,
    },
    /// Open Terminal in a folder.
    OpenTerminal {
        /// The folder.
        dir: String,
    },
    /// Put text on the pasteboard (Copy Path and friends).
    CopyText {
        /// The text.
        text: String,
    },
    /// Put file URLs on the pasteboard, so other apps can paste them.
    WriteFilePasteboard {
        /// The items.
        paths: Vec<String>,
    },
    /// Start editing a row's name in place; commit with `Tab::rename`.
    BeginRename {
        /// The tab.
        tab: TabId,
        /// The current name (the row may appear a moment later).
        name: String,
    },
    /// Show the "Rename or Move As…" dialog; commit with `Engine::rename_item`
    /// or a move.
    RenameDialog {
        /// The tab.
        tab: TabId,
        /// The items.
        paths: Vec<String>,
    },
    /// Offer "Create Folder" for a path that doesn't exist.
    OfferCreateFolder {
        /// The path that would be created.
        path: String,
        /// The deepest folder that does exist.
        nearest_existing: String,
    },
    /// Ask before moving to the Trash; on yes perform `file.trash` again with `Confirmed`.
    ConfirmTrash {
        /// The items.
        paths: Vec<String>,
    },
    /// Ask before deleting for good; on yes perform `file.delete_immediately` with `Confirmed`.
    ConfirmDeleteImmediately {
        /// The items.
        paths: Vec<String>,
    },
    /// Ask for a name pattern, then call `Tab::select_pattern`.
    AskPattern {
        /// Select (true) or deselect (false) matches.
        select: bool,
    },
    /// Ask for a workspace name, then call `Engine::save_workspace`.
    AskWorkspaceName,
    /// Show the saved-workspaces manager.
    ManageWorkspaces,
    /// Focus the address bar of a pane, optionally with text typed in.
    FocusAddressBar {
        /// The pane.
        pane: PaneId,
        /// Text to start with (`/`), or empty to select the whole path.
        text: String,
    },
    /// Focus the filter field.
    FocusFilter,
    /// Move focus back to the file list.
    FocusList,
    /// Apply a text-editing command to the focused field (address bar,
    /// palette, rename field): the command id says which.
    TextField {
        /// The command id (`addressbar.complete_next`…).
        command: String,
    },
    /// Show the command palette.
    ShowPalette,
    /// Show Quick Open.
    ShowQuickOpen,
    /// Show Settings, optionally at a section (`shortcuts`).
    ShowSettings {
        /// The section, or empty.
        section: String,
    },
    /// Show the power tour (`app.power_tour`, Help › File Studio Power
    /// Tour): a short, skippable sequence of cards about the power-user
    /// tools, with keys read from the current bindings.
    ShowPowerTour,
    /// Show or hide the sidebar.
    ToggleSidebar,
    /// Show or hide the inspector.
    ToggleInspector,
    /// Eject the volume mounted at `path` (`NSWorkspace
    /// unmountAndEjectDevice`). The shell reports a failure itself; the core
    /// re-reads volumes when the unmount notification arrives.
    Eject {
        /// The volume's mount point.
        path: String,
    },
    /// Show details about the volume mounted at `path`.
    ShowDriveInfo {
        /// The volume's mount point.
        path: String,
    },
    /// Ask for a new display name for a favourite, then call
    /// `Engine::rename_favourite`.
    AskFavouriteName {
        /// The favourite's path.
        path: String,
        /// Its current name, to start the field with.
        name: String,
    },
    /// Show the Finder tag editor for these items; apply changes with
    /// `tags.toggle`.
    EditTags {
        /// The items.
        paths: Vec<String>,
    },
    /// A window was created; open it.
    OpenWindow {
        /// The new window.
        window: WindowId,
    },
    /// A window was closed; close it.
    CloseWindow {
        /// The window.
        window: WindowId,
    },
}

/// The result of a command, key press or navigation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    /// The command that ran, if any.
    pub command: Option<String>,
    /// The tab it acted on.
    pub tab: Option<TabId>,
    /// Something only the shell can do.
    pub shell: Option<ShellRequest>,
    /// Operations queued (follow them with `OpProgress`/`OpFinished`).
    pub ops: Vec<OpId>,
    /// A short status message ("Copied path", "Nothing to undo").
    pub message: Option<String>,
    /// An unfinished key sequence to show (`g`).
    pub pending_keys: Option<String>,
    /// A "Did you know?" hint (also sent as an event).
    pub hint: Option<HintInfo>,
}

impl Outcome {
    /// Nothing visible happened.
    pub fn none() -> Self {
        Self::default()
    }
    pub(crate) fn shell(req: ShellRequest) -> Self {
        Self {
            shell: Some(req),
            ..Self::default()
        }
    }
    pub(crate) fn message(m: impl Into<String>) -> Self {
        Self {
            message: Some(m.into()),
            ..Self::default()
        }
    }
    pub(crate) fn op(id: OpId) -> Self {
        Self {
            ops: vec![id],
            ..Self::default()
        }
    }
    pub(crate) fn with_tab(mut self, t: TabId) -> Self {
        self.tab = Some(t);
        self
    }
}

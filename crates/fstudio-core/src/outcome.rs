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
    /// Ask before moving `paths` to the Trash; on yes call
    /// `Engine::confirm(token)`, on no `Engine::cancel_confirmation(token)`.
    /// Only these items are trashed, and only if they are unchanged (S1).
    ConfirmTrash {
        /// The one-shot token for this question.
        token: u64,
        /// The items.
        paths: Vec<String>,
    },
    /// Ask before deleting `paths` for good; on yes call
    /// `Engine::confirm(token)`, on no `Engine::cancel_confirmation(token)`.
    /// Only these items are deleted, and only if they are unchanged (S1).
    ConfirmDeleteImmediately {
        /// The one-shot token for this question.
        token: u64,
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
    /// Moving these items needs the user's yes (F-023: a protected folder is
    /// among them). Ask with Cancel as the default button, then call
    /// `Engine::confirm(token)` or `Engine::cancel_confirmation(token)`.
    ConfirmMove {
        /// The pending move.
        token: u64,
        /// "Move “Downloads” into “Import Studio”?"
        title: String,
        /// Why it is protected.
        detail: String,
        /// The items to move.
        paths: Vec<String>,
    },
    /// A move was queued: show `text` ("Moved “X” to “Y”") for about six
    /// seconds with an Undo button that performs `edit.undo` (F-023).
    Moved {
        /// What moved where.
        text: String,
    },
    /// A yes/no question; `no` is the default button (Return and Esc). On
    /// yes call `Engine::confirm(token)`, on no
    /// `Engine::cancel_confirmation(token)` (renames that hide an item or
    /// change its extension, putting an item back into a folder that is
    /// gone: safety audit S14).
    Confirm {
        /// The one-shot token.
        token: u64,
        /// The question.
        title: String,
        /// One or two sentences under it.
        detail: String,
        /// The button that goes ahead ("Use .md").
        yes: String,
        /// The default button that doesn't ("Keep .txt", "Cancel").
        no: String,
    },
    /// Show `text` as a toast for about six seconds (safety audit S11: Trash,
    /// Delete Immediately, Undo and Redo say what they did, and where when
    /// this window doesn't show it). With `button`, the toast has a button
    /// with that title that performs `command` (`edit.undo`, `edit.redo`).
    Toast {
        /// "Put back 3 items", "Redid: Move of “Downloads” in “Projects”".
        text: String,
        /// The button's title ("Undo", "Redo", "Details"), if any.
        button: Option<String>,
        /// The command the button performs.
        command: Option<String>,
        /// How it looks: a tick for success, a warning sign when something
        /// didn't happen.
        style: ToastStyle,
    },
    /// Show the operations queue, where every failed item is listed with
    /// its reason (`view.show_operations`, a failure toast's Details).
    ShowOperations,
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
    /// An interrupted copy's disks are back (Safe Transfer proposal §6.1):
    /// ask "Resume “title”?" with Resume, Later and Discard…. Resume calls
    /// `ops().resume(op)`; Later does nothing (the operations list keeps the
    /// row, and the question comes back on the next reconnect); Discard…
    /// asks `discard_question` first (Cancel is its default button), then
    /// calls `ops().cancel(op)`.
    ResumePrompt {
        /// The op.
        op: fstudio_ops::OpId,
        /// "Resume “Copying 800 items to “CLIENT_A””?"
        question: String,
        /// "312 files left (11.3 GB). Files already copied are kept, …"
        detail: String,
        /// "Discard the rest of this copy?"
        discard_question: String,
        /// "The finished files on “CLIENT_A” stay. …"
        discard_detail: String,
    },
    /// A copy stopped because source disks are missing: show `text`, with a
    /// button that shows the operations list.
    SourceMissing {
        /// The op.
        op: fstudio_ops::OpId,
        /// "212 files wait for “Untitled”. Insert it, then resume the copy."
        text: String,
    },
    /// Open the disk usage view for the folder or volume at `path`
    /// (`nav.disk_usage`); scan it with `Engine::usage_scan`.
    ShowDiskUsage {
        /// The folder or volume.
        path: String,
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

/// How a toast looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToastStyle {
    /// Neutral news.
    Info,
    /// Everything asked for happened (a green tick).
    Success,
    /// Some or all of it didn't happen, or it was refused (a warning sign).
    Warning,
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

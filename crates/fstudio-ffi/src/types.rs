//! Plain FFI mirror types: records and enums with `String` paths, plus their
//! conversions from and to the core types. No logic beyond translation.

use std::path::{Path, PathBuf};

use fstudio_core as core;
use fstudio_core::menus;

pub(crate) fn ps(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

// ───────────────────────── errors ─────────────────────────

/// Why a core call failed, in plain words (`msg`), with a rough class the
/// shell can use to pick how to show it (a beep, an alert, an inline note).
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiError {
    /// Nothing with that id or path.
    #[error("{msg}")]
    NotFound {
        /// The message.
        msg: String,
    },
    /// The command can't run right now (nothing selected, single pane…).
    #[error("{msg}")]
    NotAvailable {
        /// The message.
        msg: String,
    },
    /// A name the user typed can't be used.
    #[error("{msg}")]
    InvalidName {
        /// The message.
        msg: String,
    },
    /// A setting value was refused.
    #[error("{msg}")]
    InvalidSetting {
        /// The message.
        msg: String,
    },
    /// The command id is not registered.
    #[error("{msg}")]
    UnknownCommand {
        /// The message.
        msg: String,
    },
    /// Anything else (database, I/O, the queue).
    #[error("{msg}")]
    Failed {
        /// The message.
        msg: String,
    },
}

impl From<core::CoreError> for FfiError {
    fn from(e: core::CoreError) -> Self {
        let msg = e.to_string();
        match e {
            core::CoreError::NotFound(_) => FfiError::NotFound { msg },
            core::CoreError::NotAvailable(_) => FfiError::NotAvailable { msg },
            core::CoreError::InvalidName(_) => FfiError::InvalidName { msg },
            core::CoreError::InvalidSetting(_) => FfiError::InvalidSetting { msg },
            core::CoreError::UnknownCommand(_) => FfiError::UnknownCommand { msg },
            _ => FfiError::Failed { msg },
        }
    }
}

impl From<fstudio_ops::OpsError> for FfiError {
    fn from(e: fstudio_ops::OpsError) -> Self {
        FfiError::Failed { msg: e.to_string() }
    }
}

/// Result alias for FFI calls.
pub type R<T> = Result<T, FfiError>;

// ───────────────────────── engine config ─────────────────────────

/// How the engine is set up.
#[derive(uniffi::Record, Debug, Clone)]
pub struct FfiEngineConfig {
    /// The folder for the database.
    pub data_dir: String,
    /// The home folder; `None` uses `$HOME`.
    pub home: Option<String>,
    /// A folder to use as the Trash instead of the system Trash (tests).
    pub trash_dir: Option<String>,
    /// Poll folders instead of using FSEvents.
    pub poll_watcher: bool,
}

impl From<FfiEngineConfig> for core::EngineConfig {
    fn from(c: FfiEngineConfig) -> Self {
        core::EngineConfig {
            data_dir: c.data_dir,
            home: c.home,
            trash_dir: c.trash_dir,
            poll_watcher: c.poll_watcher,
        }
    }
}

// ───────────────────────── keys ─────────────────────────

/// A key, without modifiers.
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum FfiKey {
    /// A printable character as typed without Shift, lowercased (one character).
    Char {
        /// The character.
        value: String,
    },
    Left,
    Right,
    Up,
    Down,
    Space,
    Return,
    Enter,
    Escape,
    Tab,
    /// ⌫
    Delete,
    /// ⌦
    ForwardDelete,
    Home,
    End,
    PageUp,
    PageDown,
    /// F1–F20.
    F {
        /// The number.
        n: u8,
    },
}

/// Modifier keys.
#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FfiMods {
    pub shift: bool,
    pub cmd: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// What a drop onto a folder does (`Engine::drop_action`).
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiDropAction {
    None,
    Copy,
    Move,
}

impl From<core::DropAction> for FfiDropAction {
    fn from(a: core::DropAction) -> Self {
        match a {
            core::DropAction::None => Self::None,
            core::DropAction::Copy => Self::Copy,
            core::DropAction::Move => Self::Move,
        }
    }
}

/// A key press.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiKeyChord {
    pub key: FfiKey,
    pub mods: FfiMods,
}

impl From<FfiMods> for core::Mods {
    fn from(m: FfiMods) -> Self {
        core::Mods {
            shift: m.shift,
            cmd: m.cmd,
            alt: m.alt,
            ctrl: m.ctrl,
        }
    }
}

impl FfiKeyChord {
    pub(crate) fn to_core(&self) -> Option<core::KeyChord> {
        use core::Key as K;
        let key = match &self.key {
            FfiKey::Char { value } => K::Char(value.chars().next()?),
            FfiKey::Left => K::Left,
            FfiKey::Right => K::Right,
            FfiKey::Up => K::Up,
            FfiKey::Down => K::Down,
            FfiKey::Space => K::Space,
            FfiKey::Return => K::Return,
            FfiKey::Enter => K::Enter,
            FfiKey::Escape => K::Escape,
            FfiKey::Tab => K::Tab,
            FfiKey::Delete => K::Delete,
            FfiKey::ForwardDelete => K::ForwardDelete,
            FfiKey::Home => K::Home,
            FfiKey::End => K::End,
            FfiKey::PageUp => K::PageUp,
            FfiKey::PageDown => K::PageDown,
            FfiKey::F { n } => K::F(*n),
        };
        Some(core::KeyChord::new(key, self.mods.into()))
    }
}

/// How a command was issued.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiVia {
    Keyboard,
    Mouse,
    Menu,
    Palette,
    Script,
}

impl From<FfiVia> for core::Via {
    fn from(v: FfiVia) -> Self {
        match v {
            FfiVia::Keyboard => core::Via::Keyboard,
            FfiVia::Mouse => core::Via::Mouse,
            FfiVia::Menu => core::Via::Menu,
            FfiVia::Palette => core::Via::Palette,
            FfiVia::Script => core::Via::Script,
        }
    }
}

/// Which part of the UI has focus.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiFocus {
    List,
    AddressBar,
    Palette,
    Rename,
    Filter,
    Other,
}

impl From<FfiFocus> for core::Focus {
    fn from(f: FfiFocus) -> Self {
        match f {
            FfiFocus::List => core::Focus::List,
            FfiFocus::AddressBar => core::Focus::AddressBar,
            FfiFocus::Palette => core::Focus::Palette,
            FfiFocus::Rename => core::Focus::Rename,
            FfiFocus::Filter => core::Focus::Filter,
            FfiFocus::Other => core::Focus::Other,
        }
    }
}

/// Where a key press happened.
#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq, Eq)]
pub struct FfiInputContext {
    pub focus: FfiFocus,
    /// The tab whose pane has focus (the focused tab when `None`).
    pub tab: Option<u64>,
    /// This key-down is the system's auto-repeat (`NSEvent.isARepeat`).
    pub is_repeat: bool,
}

impl From<FfiInputContext> for core::InputContext {
    fn from(c: FfiInputContext) -> Self {
        core::InputContext {
            focus: c.focus.into(),
            tab: c.tab.map(core::TabId),
            is_repeat: c.is_repeat,
        }
    }
}

/// Vim's modal state.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiVimMode {
    Off,
    Normal,
    Visual,
}

impl From<core::VimMode> for FfiVimMode {
    fn from(v: core::VimMode) -> Self {
        match v {
            core::VimMode::Off => FfiVimMode::Off,
            core::VimMode::Normal => FfiVimMode::Normal,
            core::VimMode::Visual => FfiVimMode::Visual,
        }
    }
}

// ───────────────────────── commands ─────────────────────────

/// How a list shows its items.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiViewMode {
    Icons,
    List,
    Columns,
    Gallery,
}

impl From<core::ViewMode> for FfiViewMode {
    fn from(m: core::ViewMode) -> Self {
        match m {
            core::ViewMode::Icons => FfiViewMode::Icons,
            core::ViewMode::List => FfiViewMode::List,
            core::ViewMode::Columns => FfiViewMode::Columns,
            core::ViewMode::Gallery => FfiViewMode::Gallery,
        }
    }
}

impl From<FfiViewMode> for core::ViewMode {
    fn from(m: FfiViewMode) -> Self {
        match m {
            FfiViewMode::Icons => core::ViewMode::Icons,
            FfiViewMode::List => core::ViewMode::List,
            FfiViewMode::Columns => core::ViewMode::Columns,
            FfiViewMode::Gallery => core::ViewMode::Gallery,
        }
    }
}

/// A command's argument.
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum FfiCommandArg {
    Path {
        path: String,
    },
    Index {
        index: u32,
    },
    ViewMode {
        mode: FfiViewMode,
    },
    /// One character.
    Char {
        value: String,
    },
    Confirm,
    /// The user said yes to the core's confirmation question.
    Confirmed,
    Text {
        text: String,
    },
}

impl From<core::CommandArg> for FfiCommandArg {
    fn from(a: core::CommandArg) -> Self {
        match a {
            core::CommandArg::Path(p) => FfiCommandArg::Path { path: ps(&p) },
            core::CommandArg::Index(i) => FfiCommandArg::Index { index: i as u32 },
            core::CommandArg::ViewMode(m) => FfiCommandArg::ViewMode { mode: m.into() },
            core::CommandArg::Char(c) => FfiCommandArg::Char {
                value: c.to_string(),
            },
            core::CommandArg::Confirm => FfiCommandArg::Confirm,
            core::CommandArg::Confirmed => FfiCommandArg::Confirmed,
            core::CommandArg::Text(t) => FfiCommandArg::Text { text: t },
        }
    }
}

impl FfiCommandArg {
    pub(crate) fn to_core(&self) -> Option<core::CommandArg> {
        Some(match self {
            FfiCommandArg::Path { path } => core::CommandArg::Path(PathBuf::from(path)),
            FfiCommandArg::Index { index } => core::CommandArg::Index(*index as usize),
            FfiCommandArg::ViewMode { mode } => core::CommandArg::ViewMode((*mode).into()),
            FfiCommandArg::Char { value } => core::CommandArg::Char(value.chars().next()?),
            FfiCommandArg::Confirm => core::CommandArg::Confirm,
            FfiCommandArg::Confirmed => core::CommandArg::Confirmed,
            FfiCommandArg::Text { text } => core::CommandArg::Text(text.clone()),
        })
    }
}

/// A command to run.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiInvocation {
    /// The command id (`file.new_folder`).
    pub id: String,
    pub arg: Option<FfiCommandArg>,
    /// The tab it acts on (the focused tab when `None`).
    pub tab: Option<u64>,
}

impl From<core::CommandInvocation> for FfiInvocation {
    fn from(i: core::CommandInvocation) -> Self {
        FfiInvocation {
            id: i.id,
            arg: i.arg.map(Into::into),
            tab: i.tab.map(|t| t.0),
        }
    }
}

impl FfiInvocation {
    pub(crate) fn to_core(&self) -> core::CommandInvocation {
        core::CommandInvocation {
            id: self.id.clone(),
            arg: self.arg.as_ref().and_then(|a| a.to_core()),
            tab: self.tab.map(core::TabId),
        }
    }
}

/// A registered command.
#[derive(uniffi::Record, Debug, Clone)]
pub struct FfiCommandInfo {
    pub id: String,
    pub title: String,
    pub category: String,
    pub description: String,
    /// The primary shortcut as key-cap text.
    pub shortcut: Option<String>,
    pub shows_in_menu: bool,
    pub shows_in_palette: bool,
    pub customized: bool,
    /// Who carries it out.
    pub handling: FfiHandling,
}

/// Who carries a command out.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiHandling {
    /// The core.
    Core,
    /// The core decides, the shell does it (a shell request).
    Shell,
    /// A later milestone.
    Later,
}

impl From<core::Handling> for FfiHandling {
    fn from(h: core::Handling) -> Self {
        match h {
            core::Handling::Core => FfiHandling::Core,
            core::Handling::Shell => FfiHandling::Shell,
            core::Handling::Later => FfiHandling::Later,
        }
    }
}

/// A "Did you know?" hint.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiHint {
    pub command: String,
    pub keys: String,
    pub label: String,
}

impl From<core::HintInfo> for FfiHint {
    fn from(h: core::HintInfo) -> Self {
        FfiHint {
            command: h.command,
            keys: h.keys,
            label: h.label,
        }
    }
}

/// One overlay row.
#[derive(uniffi::Record, Debug, Clone)]
pub struct FfiShortcutRow {
    pub command: String,
    /// Key caps without modifiers.
    pub keys: String,
    pub label: String,
    pub category: String,
}

/// The held-modifier overlay.
#[derive(uniffi::Record, Debug, Clone)]
pub struct FfiHeldShortcuts {
    /// Badge text (`⌘ Command`).
    pub title: String,
    pub rows: Vec<FfiShortcutRow>,
}

impl From<core::HeldShortcuts> for FfiHeldShortcuts {
    fn from(h: core::HeldShortcuts) -> Self {
        FfiHeldShortcuts {
            title: h.title,
            rows: h
                .rows
                .into_iter()
                .map(|r| FfiShortcutRow {
                    command: r.id.as_str().to_string(),
                    keys: r.keys,
                    label: r.label,
                    category: r.category.title().to_string(),
                })
                .collect(),
        }
    }
}

/// How a toast looks. See `fstudio_core::ToastStyle`.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiToastStyle {
    Info,
    Success,
    Warning,
}

impl From<core::ToastStyle> for FfiToastStyle {
    fn from(s: core::ToastStyle) -> Self {
        match s {
            core::ToastStyle::Info => Self::Info,
            core::ToastStyle::Success => Self::Success,
            core::ToastStyle::Warning => Self::Warning,
        }
    }
}

/// Something only the shell can do. See `fstudio_core::ShellRequest`.
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum FfiShellRequest {
    Open {
        paths: Vec<String>,
    },
    OpenWith {
        paths: Vec<String>,
    },
    OpenInEditor {
        paths: Vec<String>,
    },
    QuickLook {
        paths: Vec<String>,
    },
    GetInfo {
        paths: Vec<String>,
    },
    RevealInFinder {
        paths: Vec<String>,
    },
    OpenTerminal {
        dir: String,
    },
    CopyText {
        text: String,
    },
    WriteFilePasteboard {
        paths: Vec<String>,
    },
    BeginRename {
        tab: u64,
        name: String,
    },
    RenameDialog {
        tab: u64,
        paths: Vec<String>,
    },
    OfferCreateFolder {
        path: String,
        nearest_existing: String,
    },
    ConfirmTrash {
        token: u64,
        paths: Vec<String>,
    },
    ConfirmDeleteImmediately {
        token: u64,
        paths: Vec<String>,
    },
    AskPattern {
        select: bool,
    },
    AskWorkspaceName,
    ManageWorkspaces,
    FocusAddressBar {
        pane: u64,
        text: String,
    },
    FocusFilter,
    /// M3: show the search bar with this text.
    ShowSearch {
        text: String,
    },
    FocusList,
    TextField {
        command: String,
    },
    ShowPalette,
    ShowQuickOpen,
    ShowSettings {
        section: String,
    },
    ConfirmMove {
        token: u64,
        title: String,
        detail: String,
        paths: Vec<String>,
    },
    Moved {
        text: String,
    },
    Toast {
        text: String,
        button: Option<String>,
        command: Option<String>,
        style: FfiToastStyle,
    },
    ShowOperations,
    Confirm {
        token: u64,
        title: String,
        detail: String,
        yes: String,
        no: String,
    },
    ShowPowerTour,
    ToggleSidebar,
    ToggleInspector,
    Eject {
        path: String,
    },
    ShowDriveInfo {
        path: String,
    },
    /// Disk usage (M3): open the usage view for `path`.
    ShowDiskUsage {
        path: String,
    },
    AskFavouriteName {
        path: String,
        name: String,
    },
    EditTags {
        paths: Vec<String>,
    },
    OpenWindow {
        window: u64,
    },
    CloseWindow {
        window: u64,
    },
    /// An interrupted copy's disks are back: Resume, Later, Discard….
    ResumePrompt {
        op: i64,
        question: String,
        detail: String,
        discard_question: String,
        discard_detail: String,
    },
    /// A copy waits for source disks.
    SourceMissing {
        op: i64,
        text: String,
    },
}

impl From<core::ShellRequest> for FfiShellRequest {
    fn from(r: core::ShellRequest) -> Self {
        use core::ShellRequest as S;
        match r {
            S::Open { paths } => Self::Open { paths },
            S::OpenWith { paths } => Self::OpenWith { paths },
            S::OpenInEditor { paths } => Self::OpenInEditor { paths },
            S::QuickLook { paths } => Self::QuickLook { paths },
            S::GetInfo { paths } => Self::GetInfo { paths },
            S::RevealInFinder { paths } => Self::RevealInFinder { paths },
            S::OpenTerminal { dir } => Self::OpenTerminal { dir },
            S::CopyText { text } => Self::CopyText { text },
            S::WriteFilePasteboard { paths } => Self::WriteFilePasteboard { paths },
            S::BeginRename { tab, name } => Self::BeginRename { tab: tab.0, name },
            S::RenameDialog { tab, paths } => Self::RenameDialog { tab: tab.0, paths },
            S::OfferCreateFolder {
                path,
                nearest_existing,
            } => Self::OfferCreateFolder {
                path,
                nearest_existing,
            },
            S::ConfirmTrash { token, paths } => Self::ConfirmTrash { token, paths },
            S::ConfirmDeleteImmediately { token, paths } => {
                Self::ConfirmDeleteImmediately { token, paths }
            }
            S::AskPattern { select } => Self::AskPattern { select },
            S::AskWorkspaceName => Self::AskWorkspaceName,
            S::ManageWorkspaces => Self::ManageWorkspaces,
            S::FocusAddressBar { pane, text } => Self::FocusAddressBar { pane: pane.0, text },
            S::FocusFilter => Self::FocusFilter,
            S::ShowSearch { text } => Self::ShowSearch { text },
            S::FocusList => Self::FocusList,
            S::TextField { command } => Self::TextField { command },
            S::ShowPalette => Self::ShowPalette,
            S::ShowQuickOpen => Self::ShowQuickOpen,
            S::ShowSettings { section } => Self::ShowSettings { section },
            S::ConfirmMove {
                token,
                title,
                detail,
                paths,
            } => Self::ConfirmMove {
                token,
                title,
                detail,
                paths,
            },
            S::Moved { text } => Self::Moved { text },
            S::Toast {
                text,
                button,
                command,
                style,
            } => Self::Toast {
                text,
                button,
                command,
                style: style.into(),
            },
            S::ShowOperations => Self::ShowOperations,
            S::Confirm {
                token,
                title,
                detail,
                yes,
                no,
            } => Self::Confirm {
                token,
                title,
                detail,
                yes,
                no,
            },
            S::ShowPowerTour => Self::ShowPowerTour,
            S::ToggleSidebar => Self::ToggleSidebar,
            S::ToggleInspector => Self::ToggleInspector,
            S::Eject { path } => Self::Eject { path },
            S::ShowDriveInfo { path } => Self::ShowDriveInfo { path },
            S::ShowDiskUsage { path } => Self::ShowDiskUsage { path },
            S::AskFavouriteName { path, name } => Self::AskFavouriteName { path, name },
            S::EditTags { paths } => Self::EditTags { paths },
            S::OpenWindow { window } => Self::OpenWindow { window: window.0 },
            S::CloseWindow { window } => Self::CloseWindow { window: window.0 },
            S::ResumePrompt {
                op,
                question,
                detail,
                discard_question,
                discard_detail,
            } => Self::ResumePrompt {
                op: op.0,
                question,
                detail,
                discard_question,
                discard_detail,
            },
            S::SourceMissing { op, text } => Self::SourceMissing { op: op.0, text },
        }
    }
}

/// What a command, key or navigation did.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq, Default)]
pub struct FfiOutcome {
    pub command: Option<String>,
    pub tab: Option<u64>,
    pub shell: Option<FfiShellRequest>,
    pub ops: Vec<i64>,
    pub message: Option<String>,
    pub pending_keys: Option<String>,
    pub hint: Option<FfiHint>,
}

impl From<core::Outcome> for FfiOutcome {
    fn from(o: core::Outcome) -> Self {
        FfiOutcome {
            command: o.command,
            tab: o.tab.map(|t| t.0),
            shell: o.shell.map(Into::into),
            ops: o.ops.into_iter().map(|i| i.0).collect(),
            message: o.message,
            pending_keys: o.pending_keys,
            hint: o.hint.map(Into::into),
        }
    }
}

// ───────────────────────── listing ─────────────────────────

/// What a row is.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiRowKind {
    Folder,
    Package,
    File,
    Symlink,
    Other,
}

impl From<core::RowKind> for FfiRowKind {
    fn from(k: core::RowKind) -> Self {
        match k {
            core::RowKind::Folder => FfiRowKind::Folder,
            core::RowKind::Package => FfiRowKind::Package,
            core::RowKind::File => FfiRowKind::File,
            core::RowKind::Symlink => FfiRowKind::Symlink,
            core::RowKind::Other => FfiRowKind::Other,
        }
    }
}

/// One row, ready to draw.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiRow {
    pub index: u32,
    /// Exactly as on disk.
    pub name: String,
    pub kind: FfiRowKind,
    /// `folder`, `package.app`, `file.png`, `file`, `symlink`, `other`.
    pub icon_key: String,
    pub kind_text: String,
    pub size: u64,
    pub size_text: String,
    pub modified_ns: i64,
    pub modified_text: String,
    pub is_hidden: bool,
    pub is_dataless: bool,
    pub is_package: bool,
    pub is_symlink: bool,
    pub is_selected: bool,
    pub is_cursor: bool,
    pub is_cut_pending: bool,
    // ui-list additions
    /// Creation time (the modification time when the filesystem has none).
    pub created_ns: i64,
    pub created_text: String,
    /// The extension as on disk; empty for folders.
    pub ext_text: String,
    // M3 search additions
    /// The name to draw (the item's own name in a search tab).
    pub display_name: String,
    /// The folder the item is in, in a search tab; else empty.
    pub location_text: String,
}

impl From<core::RowInfo> for FfiRow {
    fn from(r: core::RowInfo) -> Self {
        FfiRow {
            index: r.index,
            name: r.name,
            kind: r.kind.into(),
            icon_key: r.icon_key,
            kind_text: r.kind_text,
            size: r.size,
            size_text: r.size_text,
            modified_ns: r.modified_ns,
            modified_text: r.modified_text,
            is_hidden: r.is_hidden,
            is_dataless: r.is_dataless,
            is_package: r.is_package,
            is_symlink: r.is_symlink,
            is_selected: r.is_selected,
            is_cursor: r.is_cursor,
            is_cut_pending: r.is_cut_pending,
            created_ns: r.created_ns,
            created_text: r.created_text,
            ext_text: r.ext_text,
            display_name: r.display_name,
            location_text: r.location_text,
        }
    }
}

/// Where a listing is in its life.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiListingState {
    Loading,
    Complete,
    Error,
}

/// A tab's current listing, without its rows.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiListingInfo {
    pub version: u64,
    pub dir: String,
    pub state: FfiListingState,
    pub error: Option<String>,
    /// Visible rows.
    pub len: u32,
    /// Every entry, hidden and filtered-out included.
    pub total_len: u32,
}

impl From<&core::Listing> for FfiListingInfo {
    fn from(l: &core::Listing) -> Self {
        FfiListingInfo {
            version: l.version(),
            dir: l.dir(),
            state: match l.state() {
                core::ListingState::Loading => FfiListingState::Loading,
                core::ListingState::Complete => FfiListingState::Complete,
                core::ListingState::Error => FfiListingState::Error,
            },
            error: l.error(),
            len: l.len(),
            total_len: l.total_len(),
        }
    }
}

/// Why a listing changed.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiChangeKind {
    Partial,
    Sorted,
    Patched,
    Resorted,
    Error,
}

impl From<core::ChangeKind> for FfiChangeKind {
    fn from(k: core::ChangeKind) -> Self {
        match k {
            core::ChangeKind::Partial => FfiChangeKind::Partial,
            core::ChangeKind::Sorted => FfiChangeKind::Sorted,
            core::ChangeKind::Patched => FfiChangeKind::Patched,
            core::ChangeKind::Resorted => FfiChangeKind::Resorted,
            core::ChangeKind::Error => FfiChangeKind::Error,
        }
    }
}

// ───────────────────────── view, selection, tab state ─────────────────────────

/// What a listing is sorted by.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiSortBy {
    Name,
    Size,
    Modified,
    Created,
    Kind,
    Extension,
}

impl From<core::SortBy> for FfiSortBy {
    fn from(s: core::SortBy) -> Self {
        match s {
            core::SortBy::Name => FfiSortBy::Name,
            core::SortBy::Size => FfiSortBy::Size,
            core::SortBy::Modified => FfiSortBy::Modified,
            core::SortBy::Created => FfiSortBy::Created,
            core::SortBy::Kind => FfiSortBy::Kind,
            core::SortBy::Extension => FfiSortBy::Extension,
        }
    }
}

impl From<FfiSortBy> for core::SortBy {
    fn from(s: FfiSortBy) -> Self {
        match s {
            FfiSortBy::Name => core::SortBy::Name,
            FfiSortBy::Size => core::SortBy::Size,
            FfiSortBy::Modified => core::SortBy::Modified,
            FfiSortBy::Created => core::SortBy::Created,
            FfiSortBy::Kind => core::SortBy::Kind,
            FfiSortBy::Extension => core::SortBy::Extension,
        }
    }
}

/// How a tab shows its folder.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiViewState {
    pub mode: FfiViewMode,
    pub sort: FfiSortBy,
    pub ascending: bool,
    pub folders_first: bool,
    pub show_hidden: bool,
    pub packages_as_files: bool,
    // ui-list additions
    /// The list view's columns, in order, with widths.
    pub columns: Vec<FfiColumnSpec>,
}

impl From<core::ViewState> for FfiViewState {
    fn from(v: core::ViewState) -> Self {
        FfiViewState {
            mode: v.mode.into(),
            sort: v.sort.into(),
            ascending: v.ascending,
            folders_first: v.folders_first,
            show_hidden: v.show_hidden,
            packages_as_files: v.packages_as_files,
            columns: v.columns.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<FfiViewState> for core::ViewState {
    fn from(v: FfiViewState) -> Self {
        core::ViewState {
            mode: v.mode.into(),
            sort: v.sort.into(),
            ascending: v.ascending,
            folders_first: v.folders_first,
            show_hidden: v.show_hidden,
            packages_as_files: v.packages_as_files,
            columns: core::sanitize_columns(
                &v.columns.into_iter().map(Into::into).collect::<Vec<_>>(),
            ),
        }
    }
}

/// The selection.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiSelection {
    pub rows: Vec<u32>,
    pub names: Vec<String>,
    pub cursor: Option<u32>,
    pub anchor: Option<u32>,
    /// The listing version the rows refer to.
    pub version: u64,
}

impl From<core::SelectionInfo> for FfiSelection {
    fn from(s: core::SelectionInfo) -> Self {
        FfiSelection {
            rows: s.rows,
            names: s.names,
            cursor: s.cursor,
            anchor: s.anchor,
            version: s.version,
        }
    }
}

/// A cursor movement.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiCursorMove {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
}

impl From<FfiCursorMove> for core::CursorMove {
    fn from(m: FfiCursorMove) -> Self {
        match m {
            FfiCursorMove::Up => core::CursorMove::Up,
            FfiCursorMove::Down => core::CursorMove::Down,
            FfiCursorMove::PageUp => core::CursorMove::PageUp,
            FfiCursorMove::PageDown => core::CursorMove::PageDown,
            FfiCursorMove::Home => core::CursorMove::Home,
            FfiCursorMove::End => core::CursorMove::End,
        }
    }
}

/// A change to the selection.
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum FfiSelectionChange {
    /// Plain click.
    Only {
        row: u32,
    },
    /// ⌘-click.
    Toggle {
        row: u32,
    },
    /// ⇧-click.
    ExtendTo {
        row: u32,
    },
    All,
    None,
    Invert,
    /// A rubber band.
    Rows {
        rows: Vec<u32>,
    },
    Names {
        names: Vec<String>,
    },
    Move {
        by: FfiCursorMove,
        extend: bool,
    },
}

impl From<FfiSelectionChange> for core::SelectionChange {
    fn from(c: FfiSelectionChange) -> Self {
        use core::SelectionChange as S;
        match c {
            FfiSelectionChange::Only { row } => S::Only(row),
            FfiSelectionChange::Toggle { row } => S::Toggle(row),
            FfiSelectionChange::ExtendTo { row } => S::ExtendTo(row),
            FfiSelectionChange::All => S::All,
            FfiSelectionChange::None => S::None,
            FfiSelectionChange::Invert => S::Invert,
            FfiSelectionChange::Rows { rows } => S::Rows(rows),
            FfiSelectionChange::Names { names } => S::Names(names),
            FfiSelectionChange::Move { by, extend } => S::Move {
                by: by.into(),
                extend,
            },
        }
    }
}

/// Where the shell was scrolled to.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct FfiScrollAnchor {
    pub name: String,
    pub offset: f64,
}

/// A tab's persistent state.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct FfiTabState {
    pub id: u64,
    pub location: String,
    pub back: Vec<String>,
    pub forward: Vec<String>,
    pub view: FfiViewState,
    pub filter: String,
    pub selected: Vec<String>,
    pub cursor: Option<String>,
    pub scroll: Option<FfiScrollAnchor>,
}

impl From<core::TabState> for FfiTabState {
    fn from(t: core::TabState) -> Self {
        FfiTabState {
            id: t.id.0,
            location: t.location,
            back: t.back,
            forward: t.forward,
            view: t.view.into(),
            filter: t.filter,
            selected: t.selected,
            cursor: t.cursor,
            scroll: t.scroll.map(|s| FfiScrollAnchor {
                name: s.name,
                offset: s.offset,
            }),
        }
    }
}

// ───────────────────────── workspace ─────────────────────────

/// Side by side or stacked.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiOrientation {
    Horizontal,
    Vertical,
}

impl From<core::Orientation> for FfiOrientation {
    fn from(o: core::Orientation) -> Self {
        match o {
            core::Orientation::Horizontal => FfiOrientation::Horizontal,
            core::Orientation::Vertical => FfiOrientation::Vertical,
        }
    }
}

impl From<FfiOrientation> for core::Orientation {
    fn from(o: FfiOrientation) -> Self {
        match o {
            FfiOrientation::Horizontal => core::Orientation::Horizontal,
            FfiOrientation::Vertical => core::Orientation::Vertical,
        }
    }
}

/// A pane and its tabs.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct FfiPaneState {
    pub id: u64,
    pub tabs: Vec<FfiTabState>,
    pub active_tab: u32,
}

impl From<core::PaneState> for FfiPaneState {
    fn from(p: core::PaneState) -> Self {
        FfiPaneState {
            id: p.id.0,
            tabs: p.tabs.into_iter().map(Into::into).collect(),
            active_tab: p.active_tab,
        }
    }
}

/// A window and its panes.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct FfiWindowState {
    pub id: u64,
    pub panes: Vec<FfiPaneState>,
    pub active_pane: u32,
    pub orientation: FfiOrientation,
    pub split_ratio: f64,
    /// The sidebar's width in points, for this window in this workspace.
    pub sidebar_width: f64,
    /// Whether the sidebar is collapsed.
    pub sidebar_hidden: bool,
}

/// Every window.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct FfiWorkspaceState {
    pub windows: Vec<FfiWindowState>,
    pub active_window: Option<u64>,
}

impl From<core::WorkspaceState> for FfiWorkspaceState {
    fn from(w: core::WorkspaceState) -> Self {
        FfiWorkspaceState {
            windows: w
                .windows
                .into_iter()
                .map(|w| FfiWindowState {
                    id: w.id.0,
                    panes: w.panes.into_iter().map(Into::into).collect(),
                    active_pane: w.active_pane,
                    orientation: w.orientation.into(),
                    split_ratio: w.split_ratio,
                    sidebar_width: w.sidebar_width,
                    sidebar_hidden: w.sidebar_hidden,
                })
                .collect(),
            active_window: w.active_window.map(|w| w.0),
        }
    }
}

/// A workspace, for the switcher and the manager.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiWorkspaceSummary {
    pub id: u64,
    pub name: String,
    /// SF Symbol name.
    pub icon: String,
    /// Place in the switcher, from 0.
    pub position: u32,
    /// The one on screen now.
    pub active: bool,
    /// The key that switches to it (`⌃⌘1`).
    pub shortcut: Option<String>,
    pub windows: u32,
    pub tabs: u32,
    pub updated_ns: i64,
}

impl From<core::WorkspaceSummary> for FfiWorkspaceSummary {
    fn from(w: core::WorkspaceSummary) -> Self {
        FfiWorkspaceSummary {
            id: w.id.0,
            name: w.name,
            icon: w.icon,
            position: w.position,
            active: w.active,
            shortcut: w.shortcut,
            windows: w.windows,
            tabs: w.tabs,
            updated_ns: w.updated_ns,
        }
    }
}

// ───────────────────────── path bar ─────────────────────────

/// What a crumb is.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiCrumbKind {
    VolumeRoot,
    Home,
    Folder,
    Package,
}

/// One address-bar segment.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiCrumb {
    pub name: String,
    pub path: String,
    pub kind: FfiCrumbKind,
    pub is_current: bool,
}

impl From<core::Crumb> for FfiCrumb {
    fn from(c: core::Crumb) -> Self {
        FfiCrumb {
            name: c.name,
            path: ps(&c.path),
            kind: match c.kind {
                core::CrumbKind::VolumeRoot => FfiCrumbKind::VolumeRoot,
                core::CrumbKind::Home => FfiCrumbKind::Home,
                core::CrumbKind::Folder => FfiCrumbKind::Folder,
                core::CrumbKind::Package => FfiCrumbKind::Package,
            },
            is_current: c.is_current,
        }
    }
}

/// What a parsed location points at.
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum FfiLocation {
    Dir {
        path: String,
    },
    /// Show `dir` and select `name`.
    File {
        dir: String,
        name: String,
    },
    /// Offer "Create Folder".
    Missing {
        path: String,
        nearest_existing: String,
    },
    Remote {
        url: String,
    },
}

/// Parsed address-bar text.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiParsedLocation {
    pub location: FfiLocation,
    /// The cleaned text.
    pub cleaned: String,
    /// The "Cleaned up paste: …" chip text, if any.
    pub chip: Option<String>,
}

impl From<core::ParsedLocation> for FfiParsedLocation {
    fn from(p: core::ParsedLocation) -> Self {
        let chip = p.chip_text();
        FfiParsedLocation {
            location: match p.location {
                core::Location::Dir(d) => FfiLocation::Dir { path: ps(&d) },
                core::Location::File { dir, name } => FfiLocation::File {
                    dir: ps(&dir),
                    name,
                },
                core::Location::Missing {
                    path,
                    nearest_existing,
                } => FfiLocation::Missing {
                    path: ps(&path),
                    nearest_existing: ps(&nearest_existing),
                },
                core::Location::Remote(u) => FfiLocation::Remote { url: u },
            },
            cleaned: p.cleaned,
            chip,
        }
    }
}

/// A byte range to highlight.
#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq, Eq)]
pub struct FfiRange {
    pub start: u32,
    pub end: u32,
}

/// One completion row.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct FfiCompletion {
    pub display_name: String,
    pub full_path: String,
    /// The breadcrumb to show (`~/Pictures/2026`).
    pub breadcrumb_text: String,
    /// A frecent folder rather than a child of the typed parent.
    pub is_frecent: bool,
    /// UTF-8 byte ranges of `display_name` to highlight.
    pub match_ranges: Vec<FfiRange>,
    pub score: f64,
    /// The whole new input if chosen.
    pub insert_text: String,
    pub last_visit: Option<u64>,
}

/// What Tab does first.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiTabExtension {
    /// UTF-8 byte offset.
    pub replace_from: u32,
    pub text: String,
    pub new_input: String,
}

/// Completions for address-bar text.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct FfiCompletions {
    pub items: Vec<FfiCompletion>,
    pub common_prefix_extension: Option<FfiTabExtension>,
    pub ghost_text: String,
    /// A bare word (matched against frecent folders) rather than a path.
    pub is_bare_word: bool,
    pub parent: Option<String>,
    /// UTF-8 byte offset of the last segment.
    pub segment_start: u32,
    pub skipped_slow: bool,
}

impl From<core::Completions> for FfiCompletions {
    fn from(c: core::Completions) -> Self {
        FfiCompletions {
            items: c
                .items
                .into_iter()
                .map(|i| FfiCompletion {
                    display_name: i.display_name,
                    full_path: ps(&i.full_path),
                    breadcrumb_text: i.breadcrumb_text,
                    is_frecent: i.kind == core::CompletionKind::Frecent,
                    match_ranges: i
                        .match_ranges
                        .into_iter()
                        .map(|r| FfiRange {
                            start: r.start as u32,
                            end: r.end as u32,
                        })
                        .collect(),
                    score: i.score,
                    insert_text: i.insert_text,
                    last_visit: i.last_visit,
                })
                .collect(),
            common_prefix_extension: c.common_prefix_extension.map(|e| FfiTabExtension {
                replace_from: e.replace_from as u32,
                text: e.text,
                new_input: e.new_input,
            }),
            ghost_text: c.ghost_text,
            is_bare_word: c.kind == fstudio_path::InputKind::BareWord,
            parent: c.parent.map(|p| ps(&p)),
            segment_start: c.segment_start as u32,
            skipped_slow: c.skipped_slow,
        }
    }
}

/// How Copy Path writes a path.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiPathStyle {
    Posix,
    Escaped,
    HomeRelative,
    FileUrl,
    Quoted,
}

impl From<FfiPathStyle> for core::PathStyle {
    fn from(s: FfiPathStyle) -> Self {
        match s {
            FfiPathStyle::Posix => core::PathStyle::Posix,
            FfiPathStyle::Escaped => core::PathStyle::Escaped,
            FfiPathStyle::HomeRelative => core::PathStyle::HomeRelative,
            FfiPathStyle::FileUrl => core::PathStyle::FileUrl,
            FfiPathStyle::Quoted => core::PathStyle::Quoted,
        }
    }
}

/// A remembered folder.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct FfiFrecentDir {
    pub path: String,
    pub score: f64,
    pub last_accessed: u64,
}

// ───────────────────────── operations ─────────────────────────

/// What an operation does.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiOpKind {
    Copy,
    Move,
    Trash,
    DeleteImmediately,
    NewFolder,
    NewFile,
    Rename,
    Duplicate,
    MoveTo,
    /// Several steps as one op (New Folder with Selection, a chain of folders).
    Compound,
}

impl From<core::OpKind> for FfiOpKind {
    fn from(k: core::OpKind) -> Self {
        match k {
            core::OpKind::Copy => FfiOpKind::Copy,
            core::OpKind::Move => FfiOpKind::Move,
            core::OpKind::Trash => FfiOpKind::Trash,
            core::OpKind::DeleteImmediately => FfiOpKind::DeleteImmediately,
            core::OpKind::NewFolder => FfiOpKind::NewFolder,
            core::OpKind::NewFile => FfiOpKind::NewFile,
            core::OpKind::Rename => FfiOpKind::Rename,
            core::OpKind::Duplicate => FfiOpKind::Duplicate,
            core::OpKind::MoveTo => FfiOpKind::MoveTo,
            core::OpKind::Compound => FfiOpKind::Compound,
        }
    }
}

/// Where an operation is.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiOpState {
    Queued,
    Running,
    Paused,
    WaitingForDecision,
    Completed,
    CompletedWithErrors,
    Cancelled,
    Failed,
    Interrupted,
}

impl From<core::OpState> for FfiOpState {
    fn from(s: core::OpState) -> Self {
        use core::OpState as S;
        match s {
            S::Queued => FfiOpState::Queued,
            S::Running => FfiOpState::Running,
            S::Paused => FfiOpState::Paused,
            S::WaitingForDecision => FfiOpState::WaitingForDecision,
            S::Completed => FfiOpState::Completed,
            S::CompletedWithErrors => FfiOpState::CompletedWithErrors,
            S::Cancelled => FfiOpState::Cancelled,
            S::Failed => FfiOpState::Failed,
            S::Interrupted => FfiOpState::Interrupted,
        }
    }
}

/// One item's error.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiItemError {
    pub path: String,
    pub dest: Option<String>,
    pub message: String,
    pub errno: Option<i32>,
}

impl From<fstudio_ops::ItemError> for FfiItemError {
    fn from(e: fstudio_ops::ItemError) -> Self {
        FfiItemError {
            path: ps(&e.path),
            dest: e.dest.map(|d| ps(&d)),
            message: e.message,
            errno: e.errno,
        }
    }
}

/// The kind of a filesystem entry.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiEntryKind {
    File,
    Dir,
    Symlink,
    Other,
}

impl From<fstudio_model::EntryKind> for FfiEntryKind {
    fn from(k: fstudio_model::EntryKind) -> Self {
        match k {
            fstudio_model::EntryKind::File => FfiEntryKind::File,
            fstudio_model::EntryKind::Dir => FfiEntryKind::Dir,
            fstudio_model::EntryKind::Symlink => FfiEntryKind::Symlink,
            fstudio_model::EntryKind::Other => FfiEntryKind::Other,
        }
    }
}

/// A collision waiting for a decision.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiConflict {
    pub op: i64,
    pub id: i64,
    pub source: String,
    pub dest: String,
    pub source_kind: FfiEntryKind,
    pub dest_kind: FfiEntryKind,
    pub source_size: u64,
    pub dest_size: u64,
    pub source_mtime_ns: i64,
    pub dest_mtime_ns: i64,
    /// Both are folders, so Merge is offered.
    pub can_merge: bool,
}

impl From<core::ConflictInfo> for FfiConflict {
    fn from(c: core::ConflictInfo) -> Self {
        FfiConflict {
            op: c.op.0,
            id: c.id.0,
            source: ps(&c.source),
            dest: ps(&c.dest),
            source_kind: c.source_kind.into(),
            dest_kind: c.dest_kind.into(),
            source_size: c.source_size,
            dest_size: c.dest_size,
            source_mtime_ns: c.source_mtime_ns,
            dest_mtime_ns: c.dest_mtime_ns,
            can_merge: c.can_merge,
        }
    }
}

/// An answer to a conflict.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiConflictDecision {
    KeepBoth,
    /// The old item goes to the Trash.
    Replace,
    Skip,
    /// Folders only.
    Merge,
}

impl From<FfiConflictDecision> for core::ConflictDecision {
    fn from(d: FfiConflictDecision) -> Self {
        match d {
            FfiConflictDecision::KeepBoth => core::ConflictDecision::KeepBoth,
            FfiConflictDecision::Replace => core::ConflictDecision::Replace,
            FfiConflictDecision::Skip => core::ConflictDecision::Skip,
            FfiConflictDecision::Merge => core::ConflictDecision::Merge,
        }
    }
}

/// An operation's progress.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct FfiOpStatus {
    pub id: i64,
    pub kind: FfiOpKind,
    pub title: String,
    pub state: FfiOpState,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub current: Option<String>,
    pub speed_bps: f64,
    pub eta_secs: Option<f64>,
    pub errors: Vec<FfiItemError>,
    pub conflicts: Vec<FfiConflict>,
    /// The finished-row line ("Failed · 0 of 1 done · 1 failed").
    pub summary: String,
    /// Disks the op waits for, by name.
    pub waiting_for: Vec<String>,
    /// An interrupted op's Resume button is enabled.
    pub can_resume: bool,
    /// The line by it ("Insert “CLIENT_A” to resume").
    pub resume_note: Option<String>,
}

impl From<core::OpStatus> for FfiOpStatus {
    fn from(s: core::OpStatus) -> Self {
        let summary = s.summary();
        let (can_resume, resume_note) = s.resume_hint();
        FfiOpStatus {
            summary,
            can_resume,
            resume_note,
            waiting_for: s.waiting_for.clone(),
            id: s.id.0,
            kind: s.kind.into(),
            title: s.title,
            state: s.state.into(),
            files_done: s.files_done,
            files_total: s.files_total,
            bytes_done: s.bytes_done,
            bytes_total: s.bytes_total,
            current: s.current.map(|p| ps(&p)),
            speed_bps: s.speed_bps,
            eta_secs: s.eta_secs,
            errors: s.errors.into_iter().map(Into::into).collect(),
            conflicts: s.conflicts.into_iter().map(Into::into).collect(),
        }
    }
}

/// A finished operation's summary.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct FfiOpReport {
    pub id: i64,
    pub kind: FfiOpKind,
    pub state: FfiOpState,
    pub files_done: u64,
    pub files_failed: u64,
    pub files_skipped: u64,
    pub bytes_done: u64,
    pub errors: Vec<FfiItemError>,
    pub elapsed_ms: u64,
}

impl From<core::OpReport> for FfiOpReport {
    fn from(r: core::OpReport) -> Self {
        FfiOpReport {
            id: r.id.0,
            kind: r.kind.into(),
            state: r.state.into(),
            files_done: r.files_done,
            files_failed: r.files_failed,
            files_skipped: r.files_skipped,
            bytes_done: r.bytes_done,
            errors: r.errors.into_iter().map(Into::into).collect(),
            elapsed_ms: r.elapsed_ms,
        }
    }
}

/// Edit-menu undo/redo labels.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq, Default)]
pub struct FfiUndoState {
    pub undo: Option<String>,
    pub redo: Option<String>,
}

impl From<core::UndoState> for FfiUndoState {
    fn from(u: core::UndoState) -> Self {
        FfiUndoState {
            undo: u.undo,
            redo: u.redo,
        }
    }
}

/// A file operation to queue (drag and drop, scripted actions).
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum FfiOpRequest {
    Copy {
        sources: Vec<String>,
        dest_dir: String,
    },
    Move {
        sources: Vec<String>,
        dest_dir: String,
    },
    Trash {
        paths: Vec<String>,
    },
    NewFolder {
        parent: String,
        name: String,
    },
    NewFile {
        parent: String,
        name: String,
    },
    Rename {
        path: String,
        new_name: String,
    },
    Duplicate {
        paths: Vec<String>,
    },
}

fn pbs(v: Vec<String>) -> Vec<PathBuf> {
    v.into_iter().map(PathBuf::from).collect()
}

impl From<FfiOpRequest> for core::OpRequest {
    fn from(r: FfiOpRequest) -> Self {
        use core::OpRequest as O;
        match r {
            FfiOpRequest::Copy { sources, dest_dir } => O::Copy {
                sources: pbs(sources),
                dest_dir: dest_dir.into(),
            },
            FfiOpRequest::Move { sources, dest_dir } => O::Move {
                sources: pbs(sources),
                dest_dir: dest_dir.into(),
            },
            FfiOpRequest::Trash { paths } => O::Trash { paths: pbs(paths) },
            FfiOpRequest::NewFolder { parent, name } => O::NewFolder {
                parent: parent.into(),
                name,
            },
            FfiOpRequest::NewFile { parent, name } => O::NewFile {
                parent: parent.into(),
                name,
            },
            FfiOpRequest::Rename { path, new_name } => O::Rename {
                path: path.into(),
                new_name,
            },
            FfiOpRequest::Duplicate { paths } => O::Duplicate { paths: pbs(paths) },
        }
    }
}

// ───────────────────────── volumes, favourites ─────────────────────────

/// A mounted volume.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiVolume {
    pub name: String,
    pub mount_path: String,
    pub is_removable: bool,
    pub is_network: bool,
    pub is_read_only: bool,
    pub total_bytes: u64,
    pub free_bytes: u64,
}

impl From<core::VolumeSummary> for FfiVolume {
    fn from(v: core::VolumeSummary) -> Self {
        FfiVolume {
            name: v.name,
            mount_path: v.mount_path,
            is_removable: v.is_removable,
            is_network: v.is_network,
            is_read_only: v.is_read_only,
            total_bytes: v.total_bytes,
            free_bytes: v.free_bytes,
        }
    }
}

/// A sidebar favourite.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiFavourite {
    pub path: String,
    pub name: String,
    pub renamed: bool,
    /// The folder has gone: show the row dimmed and offer Remove.
    pub missing: bool,
}

impl From<core::Favourite> for FfiFavourite {
    fn from(f: core::Favourite) -> Self {
        FfiFavourite {
            path: f.path,
            name: f.name,
            renamed: f.renamed,
            missing: f.missing,
        }
    }
}

// ───────────────────────── context menus ─────────────────────────

/// What a sidebar row is.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiSidebarItemKind {
    Favourite,
    Volume,
}

/// What was right-clicked.
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum FfiMenuTarget {
    File {
        tab: u64,
        path: String,
    },
    Folder {
        tab: u64,
        path: String,
    },
    Package {
        tab: u64,
        path: String,
    },
    Multi {
        tab: u64,
        paths: Vec<String>,
    },
    EmptySpace {
        tab: u64,
    },
    Crumb {
        tab: u64,
        path: String,
    },
    SidebarItem {
        path: String,
        kind: FfiSidebarItemKind,
    },
    Tab {
        tab: u64,
    },
}

impl From<menus::MenuTarget> for FfiMenuTarget {
    fn from(t: menus::MenuTarget) -> Self {
        use menus::MenuTarget as M;
        match t {
            M::File { tab, path } => Self::File { tab: tab.0, path },
            M::Folder { tab, path } => Self::Folder { tab: tab.0, path },
            M::Package { tab, path } => Self::Package { tab: tab.0, path },
            M::Multi { tab, paths } => Self::Multi { tab: tab.0, paths },
            M::EmptySpace { tab } => Self::EmptySpace { tab: tab.0 },
            M::Crumb { tab, path } => Self::Crumb { tab: tab.0, path },
            M::SidebarItem { path, kind } => Self::SidebarItem {
                path,
                kind: match kind {
                    menus::SidebarItemKind::Favourite => FfiSidebarItemKind::Favourite,
                    menus::SidebarItemKind::Volume => FfiSidebarItemKind::Volume,
                },
            },
            M::Tab { tab } => Self::Tab { tab: tab.0 },
        }
    }
}

impl From<FfiMenuTarget> for menus::MenuTarget {
    fn from(t: FfiMenuTarget) -> Self {
        use core::TabId as T;
        use menus::MenuTarget as M;
        match t {
            FfiMenuTarget::File { tab, path } => M::File { tab: T(tab), path },
            FfiMenuTarget::Folder { tab, path } => M::Folder { tab: T(tab), path },
            FfiMenuTarget::Package { tab, path } => M::Package { tab: T(tab), path },
            FfiMenuTarget::Multi { tab, paths } => M::Multi { tab: T(tab), paths },
            FfiMenuTarget::EmptySpace { tab } => M::EmptySpace { tab: T(tab) },
            FfiMenuTarget::Crumb { tab, path } => M::Crumb { tab: T(tab), path },
            FfiMenuTarget::SidebarItem { path, kind } => M::SidebarItem {
                path,
                kind: match kind {
                    FfiSidebarItemKind::Favourite => menus::SidebarItemKind::Favourite,
                    FfiSidebarItemKind::Volume => menus::SidebarItemKind::Volume,
                },
            },
            FfiMenuTarget::Tab { tab } => M::Tab { tab: T(tab) },
        }
    }
}

/// The menu kinds (for Settings).
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiMenuKind {
    File,
    Folder,
    Package,
    Multi,
    EmptySpace,
    Crumb,
    SidebarItem,
    Tab,
}

impl From<FfiMenuKind> for menus::MenuKind {
    fn from(k: FfiMenuKind) -> Self {
        use menus::MenuKind as M;
        match k {
            FfiMenuKind::File => M::File,
            FfiMenuKind::Folder => M::Folder,
            FfiMenuKind::Package => M::Package,
            FfiMenuKind::Multi => M::Multi,
            FfiMenuKind::EmptySpace => M::EmptySpace,
            FfiMenuKind::Crumb => M::Crumb,
            FfiMenuKind::SidebarItem => M::SidebarItem,
            FfiMenuKind::Tab => M::Tab,
        }
    }
}

/// A system-provided submenu the shell fills.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiSystemMenu {
    OpenWith,
    Share,
    QuickActions,
    Services,
}

/// What a menu row is.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiMenuItemKind {
    Command,
    Separator,
    /// A submenu the core filled: see `children`.
    Submenu,
    /// A submenu the shell fills from the system: see `system`.
    System,
}

/// One menu row. Commands carry `invocation`; submenus carry `children`.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiMenuItem {
    pub kind: FfiMenuItemKind,
    /// The entry id (`file.duplicate`, `menu.copy_path`).
    pub entry: String,
    pub title: String,
    pub shortcut: Option<String>,
    pub enabled: bool,
    pub checked: bool,
    /// A Finder tag colour (1–7) to draw as a dot.
    pub tag_color: Option<u8>,
    /// What to perform when chosen (commands only).
    pub invocation: Option<FfiInvocation>,
    /// Which system submenu (system items only).
    pub system: Option<FfiSystemMenu>,
    /// The paths a system submenu is for.
    pub paths: Vec<String>,
    /// The submenu's rows.
    pub children: Vec<FfiMenuItem>,
}

impl FfiMenuItem {
    fn blank(kind: FfiMenuItemKind) -> Self {
        FfiMenuItem {
            kind,
            entry: String::new(),
            title: String::new(),
            shortcut: None,
            enabled: true,
            checked: false,
            tag_color: None,
            invocation: None,
            system: None,
            paths: Vec::new(),
            children: Vec::new(),
        }
    }
}

impl From<menus::MenuItem> for FfiMenuItem {
    fn from(i: menus::MenuItem) -> Self {
        match i {
            menus::MenuItem::Command(c) => FfiMenuItem {
                entry: c.entry,
                title: c.title,
                shortcut: c.shortcut,
                enabled: c.enabled,
                checked: c.checked,
                tag_color: c.tag_color,
                invocation: Some(c.invocation.into()),
                ..FfiMenuItem::blank(FfiMenuItemKind::Command)
            },
            menus::MenuItem::Separator => FfiMenuItem::blank(FfiMenuItemKind::Separator),
            menus::MenuItem::Submenu {
                entry,
                title,
                items,
            } => FfiMenuItem {
                entry,
                title,
                children: items.into_iter().map(Into::into).collect(),
                ..FfiMenuItem::blank(FfiMenuItemKind::Submenu)
            },
            menus::MenuItem::System { kind, title, paths } => FfiMenuItem {
                entry: format!("{kind:?}"),
                title,
                system: Some(match kind {
                    menus::SystemMenu::OpenWith => FfiSystemMenu::OpenWith,
                    menus::SystemMenu::Share => FfiSystemMenu::Share,
                    menus::SystemMenu::QuickActions => FfiSystemMenu::QuickActions,
                    menus::SystemMenu::Services => FfiSystemMenu::Services,
                }),
                paths,
                ..FfiMenuItem::blank(FfiMenuItemKind::System)
            },
        }
    }
}

/// What the shell knows at the moment of a right-click.
#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FfiMenuContext {
    /// The system pasteboard holds file URLs from another app.
    pub pasteboard_has_files: bool,
}

/// An entry of a menu, for the Settings editor.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiMenuEntry {
    /// `file.duplicate`, `menu.copy_path`, or `---` for a separator.
    pub entry: String,
    pub title: String,
}

impl From<menus::MenuEntryInfo> for FfiMenuEntry {
    fn from(e: menus::MenuEntryInfo) -> Self {
        FfiMenuEntry {
            entry: e.entry,
            title: e.title,
        }
    }
}

// ───────────────────────── events ─────────────────────────

/// Something the shell should react to. Delivered on one core thread, in
/// order; the Swift side hops to the main actor.
#[derive(uniffi::Enum, Debug, Clone, PartialEq)]
pub enum FfiEvent {
    ListingChanged {
        tab: u64,
        version: u64,
        kind: FfiChangeKind,
    },
    SelectionChanged {
        tab: u64,
        count: u32,
        cursor: Option<u32>,
    },
    /// Re-read the workspace state.
    TabsChanged,
    OpProgress {
        ops: Vec<FfiOpStatus>,
    },
    OpConflict {
        op: i64,
        conflict: FfiConflict,
    },
    OpFailed {
        op: i64,
        path: String,
        error: String,
    },
    OpFinished {
        op: i64,
        report: FfiOpReport,
    },
    UndoChanged {
        state: FfiUndoState,
    },
    VolumesChanged {
        volumes: Vec<FfiVolume>,
    },
    Hint {
        hint: FfiHint,
    },
    SettingsChanged,
    FavouritesChanged,
    /// Handle `request` as an outcome's shell request, from the window
    /// holding `tab` (the toast of a finished operation).
    Shell {
        tab: Option<u64>,
        request: FfiShellRequest,
    },
    /// Re-read the workspaces list.
    WorkspacesChanged,
}

impl From<core::CoreEvent> for FfiEvent {
    fn from(e: core::CoreEvent) -> Self {
        use core::CoreEvent as E;
        match e {
            E::ListingChanged { tab, version, kind } => FfiEvent::ListingChanged {
                tab: tab.0,
                version,
                kind: kind.into(),
            },
            E::SelectionChanged { tab, count, cursor } => FfiEvent::SelectionChanged {
                tab: tab.0,
                count,
                cursor,
            },
            E::TabsChanged => FfiEvent::TabsChanged,
            E::OpProgress { ops } => FfiEvent::OpProgress {
                ops: ops.into_iter().map(Into::into).collect(),
            },
            E::OpConflict { op, conflict } => FfiEvent::OpConflict {
                op: op.0,
                conflict: conflict.into(),
            },
            E::OpFailed { op, path, error } => FfiEvent::OpFailed {
                op: op.0,
                path,
                error,
            },
            E::OpFinished { op, report } => FfiEvent::OpFinished {
                op: op.0,
                report: report.into(),
            },
            E::UndoChanged { state } => FfiEvent::UndoChanged {
                state: state.into(),
            },
            E::VolumesChanged { volumes } => FfiEvent::VolumesChanged {
                volumes: volumes.into_iter().map(Into::into).collect(),
            },
            E::Hint { hint } => FfiEvent::Hint { hint: hint.into() },
            E::SettingsChanged => FfiEvent::SettingsChanged,
            E::FavouritesChanged => FfiEvent::FavouritesChanged,
            E::Shell { tab, request } => FfiEvent::Shell {
                tab: tab.map(|t| t.0),
                request: request.into(),
            },
            E::WorkspacesChanged => FfiEvent::WorkspacesChanged,
        }
    }
}

// ───────────────────────── ui-list additions ─────────────────────────

/// A list-view column.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FfiColumnId {
    Name,
    Size,
    Modified,
    Created,
    Kind,
    Extension,
}

impl From<core::ColumnId> for FfiColumnId {
    fn from(c: core::ColumnId) -> Self {
        match c {
            core::ColumnId::Name => FfiColumnId::Name,
            core::ColumnId::Size => FfiColumnId::Size,
            core::ColumnId::Modified => FfiColumnId::Modified,
            core::ColumnId::Created => FfiColumnId::Created,
            core::ColumnId::Kind => FfiColumnId::Kind,
            core::ColumnId::Extension => FfiColumnId::Extension,
        }
    }
}

impl From<FfiColumnId> for core::ColumnId {
    fn from(c: FfiColumnId) -> Self {
        match c {
            FfiColumnId::Name => core::ColumnId::Name,
            FfiColumnId::Size => core::ColumnId::Size,
            FfiColumnId::Modified => core::ColumnId::Modified,
            FfiColumnId::Created => core::ColumnId::Created,
            FfiColumnId::Kind => core::ColumnId::Kind,
            FfiColumnId::Extension => core::ColumnId::Extension,
        }
    }
}

/// One shown column and its width in points.
#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq)]
pub struct FfiColumnSpec {
    pub id: FfiColumnId,
    pub width: f64,
}

impl Eq for FfiColumnSpec {}

impl From<core::ColumnSpec> for FfiColumnSpec {
    fn from(c: core::ColumnSpec) -> Self {
        FfiColumnSpec {
            id: c.id.into(),
            width: c.width,
        }
    }
}

impl From<FfiColumnSpec> for core::ColumnSpec {
    fn from(c: FfiColumnSpec) -> Self {
        core::ColumnSpec {
            id: c.id.into(),
            width: c.width,
        }
    }
}

/// What a drop does.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiDropEffect {
    Copy,
    Move,
    None,
}

impl From<core::DropEffect> for FfiDropEffect {
    fn from(d: core::DropEffect) -> Self {
        match d {
            core::DropEffect::Copy => FfiDropEffect::Copy,
            core::DropEffect::Move => FfiDropEffect::Move,
            core::DropEffect::None => FfiDropEffect::None,
        }
    }
}

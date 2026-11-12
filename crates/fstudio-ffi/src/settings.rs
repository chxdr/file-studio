//! The settings a Settings window edits, as plain records. Keymap and menu
//! customisations are edited through their own calls, so `update_settings`
//! keeps them as they are.

use fstudio_core as core;

use crate::types::{FfiColumnSpec, FfiSortBy, FfiViewMode};

/// Where toasts and the overlay sit.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl From<fstudio_model::Corner> for FfiCorner {
    fn from(c: fstudio_model::Corner) -> Self {
        use fstudio_model::Corner as C;
        match c {
            C::TopLeft => FfiCorner::TopLeft,
            C::TopRight => FfiCorner::TopRight,
            C::BottomLeft => FfiCorner::BottomLeft,
            C::BottomRight => FfiCorner::BottomRight,
        }
    }
}

impl From<FfiCorner> for fstudio_model::Corner {
    fn from(c: FfiCorner) -> Self {
        use fstudio_model::Corner as C;
        match c {
            FfiCorner::TopLeft => C::TopLeft,
            FfiCorner::TopRight => C::TopRight,
            FfiCorner::BottomLeft => C::BottomLeft,
            FfiCorner::BottomRight => C::BottomRight,
        }
    }
}

/// The terminal Open in Terminal uses.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiTerminalApp {
    Terminal,
    ITerm,
    Ghostty,
}

/// Row height.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiRowDensity {
    Compact,
    Regular,
    Comfortable,
}

/// Which keymap is active.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiProfile {
    Standard,
    Vim,
}

/// How often hints appear.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiHintFrequency {
    Always,
    Sometimes,
    Rarely,
}

/// General behaviour.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiGeneralSettings {
    pub restore_session: bool,
    pub start_folder: String,
    pub dual_pane_by_default: bool,
    pub confirm_trash: bool,
    pub type_select_timeout_ms: u32,
    pub closed_tabs_kept: u32,
    pub terminal_app: FfiTerminalApp,
}

/// Default view options.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiViewSettings {
    pub mode: FfiViewMode,
    pub sort: FfiSortBy,
    pub ascending: bool,
    pub folders_first: bool,
    pub show_hidden: bool,
    pub packages_as_files: bool,
    /// Per-folder view memory ("smart") is on.
    pub folder_memory: bool,
    pub hour24: bool,
    pub density: FfiRowDensity,
    // ui-list additions
    /// The list view's default columns.
    pub columns: Vec<FfiColumnSpec>,
}

/// The address bar.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiPathBarSettings {
    pub completion_limit: u32,
    pub complete_hidden: bool,
    pub frecency_excluded: Vec<String>,
    pub max_crumbs: u32,
}

/// The operation queue.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiOperationsSettings {
    pub verify_copies: bool,
    pub fsync_copies: bool,
    pub allow_delete_immediately: bool,
    pub file_workers: u32,
    pub ops_per_volume: u32,
    pub undo_limit: u32,
}

/// Keyboard behaviour (bindings are edited with `set_command_key`).
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiKeyboardSettings {
    pub profile: FfiProfile,
    pub sequence_timeout_ms: u32,
    /// Printable keys filter instead of type-to-select.
    pub typing_filters: bool,
    /// Always deliver F1–F19 as F-keys.
    pub fn_keys_always: bool,
}

/// The held-modifier overlay.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct FfiOverlaySettings {
    pub enabled: bool,
    pub delay_ms: u32,
    pub corner: FfiCorner,
    pub list_plain_keys: bool,
}

/// "Did you know?" hints.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct FfiHintSettings {
    pub enabled: bool,
    pub frequency: FfiHintFrequency,
    pub seconds: f32,
    pub corner: FfiCorner,
    pub stop_when_learned: bool,
}

/// Every setting a Settings window edits.
#[derive(uniffi::Record, Debug, Clone, PartialEq)]
pub struct FfiSettings {
    pub general: FfiGeneralSettings,
    pub view: FfiViewSettings,
    pub path_bar: FfiPathBarSettings,
    pub operations: FfiOperationsSettings,
    pub keyboard: FfiKeyboardSettings,
    pub overlay: FfiOverlaySettings,
    pub hints: FfiHintSettings,
}

impl From<&core::Settings> for FfiSettings {
    fn from(s: &core::Settings) -> Self {
        let g = &s.general;
        let v = &s.view;
        let p = &s.path_bar;
        let o = &s.operations;
        let k = &s.keymap;
        FfiSettings {
            general: FfiGeneralSettings {
                restore_session: g.restore_session,
                start_folder: g.start_folder.clone(),
                dual_pane_by_default: g.dual_pane_by_default,
                confirm_trash: g.confirm_trash,
                type_select_timeout_ms: g.type_select_timeout_ms,
                closed_tabs_kept: g.closed_tabs_kept,
                terminal_app: match g.terminal_app {
                    core::TerminalApp::Terminal => FfiTerminalApp::Terminal,
                    core::TerminalApp::ITerm => FfiTerminalApp::ITerm,
                    core::TerminalApp::Ghostty => FfiTerminalApp::Ghostty,
                },
            },
            view: FfiViewSettings {
                mode: v.mode.into(),
                sort: v.sort.into(),
                ascending: v.ascending,
                folders_first: v.folders_first,
                show_hidden: v.show_hidden,
                packages_as_files: v.packages_as_files,
                folder_memory: v.folder_memory == core::FolderMemory::Smart,
                hour24: v.hour24,
                density: match v.density {
                    core::RowDensity::Compact => FfiRowDensity::Compact,
                    core::RowDensity::Regular => FfiRowDensity::Regular,
                    core::RowDensity::Comfortable => FfiRowDensity::Comfortable,
                },
                columns: v.columns.iter().map(|c| (*c).into()).collect(),
            },
            path_bar: FfiPathBarSettings {
                completion_limit: p.completion_limit,
                complete_hidden: p.complete_hidden,
                frecency_excluded: p.frecency_excluded.clone(),
                max_crumbs: p.max_crumbs,
            },
            operations: FfiOperationsSettings {
                verify_copies: o.verify_copies,
                fsync_copies: o.fsync_copies,
                allow_delete_immediately: o.allow_delete_immediately,
                file_workers: o.file_workers,
                ops_per_volume: o.ops_per_volume,
                undo_limit: o.undo_limit,
            },
            keyboard: FfiKeyboardSettings {
                profile: match k.profile {
                    core::Profile::Standard => FfiProfile::Standard,
                    core::Profile::Vim => FfiProfile::Vim,
                },
                sequence_timeout_ms: k.sequence_timeout_ms,
                typing_filters: k.typing_filters,
                fn_keys_always: k.fn_keys_mode == fstudio_keymap::FnKeysMode::Always,
            },
            overlay: FfiOverlaySettings {
                enabled: s.overlay.enabled,
                delay_ms: s.overlay.delay_ms,
                corner: s.overlay.corner.into(),
                list_plain_keys: s.overlay.list_plain_keys,
            },
            hints: FfiHintSettings {
                enabled: s.hints.enabled,
                frequency: match s.hints.frequency {
                    fstudio_keymap::HintFrequency::Always => FfiHintFrequency::Always,
                    fstudio_keymap::HintFrequency::Sometimes => FfiHintFrequency::Sometimes,
                    fstudio_keymap::HintFrequency::Rarely => FfiHintFrequency::Rarely,
                },
                seconds: s.hints.seconds,
                corner: s.hints.corner.into(),
                stop_when_learned: s.hints.stop_when_learned,
            },
        }
    }
}

/// Writes the record's values over `base`, keeping what it doesn't carry
/// (keymap overrides, menu overrides).
pub(crate) fn apply(base: &mut core::Settings, f: FfiSettings) {
    let g = f.general;
    base.general.restore_session = g.restore_session;
    base.general.start_folder = g.start_folder;
    base.general.dual_pane_by_default = g.dual_pane_by_default;
    base.general.confirm_trash = g.confirm_trash;
    base.general.type_select_timeout_ms = g.type_select_timeout_ms;
    base.general.closed_tabs_kept = g.closed_tabs_kept;
    base.general.terminal_app = match g.terminal_app {
        FfiTerminalApp::Terminal => core::TerminalApp::Terminal,
        FfiTerminalApp::ITerm => core::TerminalApp::ITerm,
        FfiTerminalApp::Ghostty => core::TerminalApp::Ghostty,
    };
    let v = f.view;
    base.view.mode = v.mode.into();
    base.view.sort = v.sort.into();
    base.view.ascending = v.ascending;
    base.view.folders_first = v.folders_first;
    base.view.show_hidden = v.show_hidden;
    base.view.packages_as_files = v.packages_as_files;
    base.view.folder_memory = if v.folder_memory {
        core::FolderMemory::Smart
    } else {
        core::FolderMemory::Off
    };
    base.view.hour24 = v.hour24;
    base.view.density = match v.density {
        FfiRowDensity::Compact => core::RowDensity::Compact,
        FfiRowDensity::Regular => core::RowDensity::Regular,
        FfiRowDensity::Comfortable => core::RowDensity::Comfortable,
    };
    base.view.columns =
        core::sanitize_columns(&v.columns.into_iter().map(Into::into).collect::<Vec<_>>());
    let p = f.path_bar;
    base.path_bar.completion_limit = p.completion_limit;
    base.path_bar.complete_hidden = p.complete_hidden;
    base.path_bar.frecency_excluded = p.frecency_excluded;
    base.path_bar.max_crumbs = p.max_crumbs;
    let o = f.operations;
    base.operations.verify_copies = o.verify_copies;
    base.operations.fsync_copies = o.fsync_copies;
    base.operations.allow_delete_immediately = o.allow_delete_immediately;
    base.operations.file_workers = o.file_workers;
    base.operations.ops_per_volume = o.ops_per_volume;
    base.operations.undo_limit = o.undo_limit;
    let k = f.keyboard;
    base.keymap.profile = match k.profile {
        FfiProfile::Standard => core::Profile::Standard,
        FfiProfile::Vim => core::Profile::Vim,
    };
    base.keymap.sequence_timeout_ms = k.sequence_timeout_ms;
    base.keymap.typing_filters = k.typing_filters;
    base.keymap.fn_keys_mode = if k.fn_keys_always {
        fstudio_keymap::FnKeysMode::Always
    } else {
        fstudio_keymap::FnKeysMode::System
    };
    base.overlay.enabled = f.overlay.enabled;
    base.overlay.delay_ms = f.overlay.delay_ms;
    base.overlay.corner = f.overlay.corner.into();
    base.overlay.list_plain_keys = f.overlay.list_plain_keys;
    base.hints.enabled = f.hints.enabled;
    base.hints.frequency = match f.hints.frequency {
        FfiHintFrequency::Always => fstudio_keymap::HintFrequency::Always,
        FfiHintFrequency::Sometimes => fstudio_keymap::HintFrequency::Sometimes,
        FfiHintFrequency::Rarely => fstudio_keymap::HintFrequency::Rarely,
    };
    base.hints.seconds = f.hints.seconds;
    base.hints.corner = f.hints.corner.into();
    base.hints.stop_when_learned = f.hints.stop_when_learned;
}

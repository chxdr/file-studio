//! Settings: one serde struct, assembled from each crate's settings plus the
//! core's own groups, stored as JSON in the key-value table with a version.

use fstudio_keymap::{KeymapSettings, ShortcutHints, ShortcutOverlay, ViewMode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::view::SortBy;

/// The key-value key the settings live under.
pub(crate) const SETTINGS_KEY: &str = "core.settings";
/// The settings format version written by this build.
pub const SETTINGS_VERSION: u32 = 1;

/// Every user setting.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Start-up and general behaviour.
    pub general: GeneralSettings,
    /// How folders are shown by default.
    pub view: ViewSettings,
    /// The address bar and completion.
    pub path_bar: PathBarSettings,
    /// The operation queue.
    pub operations: OperationsSettings,
    /// Keyboard profile, overrides and sequence timing.
    pub keymap: KeymapSettings,
    /// The held-modifier shortcut overlay.
    pub overlay: ShortcutOverlay,
    /// "Did you know?" hints.
    pub hints: ShortcutHints,
    /// Context-menu customisations (F-022).
    pub menus: crate::menus::MenuSettings,
}

/// Start-up and general behaviour.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralSettings {
    /// Reopen the last session's windows and tabs at launch.
    pub restore_session: bool,
    /// Where new windows and tabs start (`~` is the home folder).
    pub start_folder: String,
    /// New windows open with two panes.
    pub dual_pane_by_default: bool,
    /// Ask before moving items to the Trash.
    pub confirm_trash: bool,
    /// How long type-to-select keeps adding to the typed text (200–5000 ms).
    pub type_select_timeout_ms: u32,
    /// How many closed tabs "Reopen Closed Tab" remembers (1–100).
    pub closed_tabs_kept: u32,
    /// The terminal "Open in Terminal" uses.
    pub terminal_app: TerminalApp,
}

/// The terminal app "Open in Terminal" opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TerminalApp {
    /// Apple's Terminal.
    #[default]
    Terminal,
    /// iTerm2.
    ITerm,
    /// Ghostty.
    Ghostty,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            restore_session: true,
            start_folder: "~".into(),
            dual_pane_by_default: false,
            confirm_trash: false,
            type_select_timeout_ms: 1000,
            closed_tabs_kept: 20,
            terminal_app: TerminalApp::Terminal,
        }
    }
}

/// Whether a folder remembers its own view settings (F-012).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FolderMemory {
    /// Every folder uses the defaults.
    Off,
    /// A folder remembers sort and view mode once you change them there.
    #[default]
    Smart,
}

/// Row height.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RowDensity {
    /// 20 pt.
    Compact,
    /// 22 pt.
    #[default]
    Regular,
    /// 26 pt.
    Comfortable,
}

/// How folders are shown by default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewSettings {
    /// List, icons, columns or gallery.
    pub mode: ViewMode,
    /// The column sorted by.
    pub sort: SortBy,
    /// Smallest/oldest/A first.
    pub ascending: bool,
    /// Keep folders above files.
    pub folders_first: bool,
    /// Show dot-files and hidden items.
    pub show_hidden: bool,
    /// Treat packages (`.app`) as files when sorting.
    pub packages_as_files: bool,
    /// Per-folder view memory.
    pub folder_memory: FolderMemory,
    /// 24-hour clock in dates.
    pub hour24: bool,
    /// Row height.
    pub density: RowDensity,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            mode: ViewMode::List,
            sort: SortBy::Name,
            ascending: true,
            folders_first: true,
            show_hidden: false,
            packages_as_files: true,
            folder_memory: FolderMemory::Smart,
            hour24: true,
            density: RowDensity::Regular,
        }
    }
}

/// The address bar and completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PathBarSettings {
    /// Most completion rows (1–500).
    pub completion_limit: u32,
    /// Offer dot-folders even when the typed text doesn't start with a dot.
    pub complete_hidden: bool,
    /// Folders never remembered for frecency (globs).
    pub frecency_excluded: Vec<String>,
    /// Most crumbs shown before the middle collapses (2–30).
    pub max_crumbs: u32,
}

impl Default for PathBarSettings {
    fn default() -> Self {
        Self {
            completion_limit: 50,
            complete_hidden: false,
            frecency_excluded: vec!["/private/var/folders/*".into()],
            max_crumbs: 6,
        }
    }
}

/// The operation queue. Changes apply when no operation is running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OperationsSettings {
    /// Re-read and hash every copied file (clones skip it).
    pub verify_copies: bool,
    /// Full-sync every copied file (moves always sync).
    pub fsync_copies: bool,
    /// Enables Delete Immediately. Off by default (F-007).
    pub allow_delete_immediately: bool,
    /// Parallel file workers within one copy (1–16).
    pub file_workers: u32,
    /// Operations running at once per volume pair (1–8).
    pub ops_per_volume: u32,
    /// Undo steps kept (1–1000).
    pub undo_limit: u32,
}

impl Default for OperationsSettings {
    fn default() -> Self {
        Self {
            verify_copies: false,
            fsync_copies: false,
            allow_delete_immediately: false,
            file_workers: 4,
            ops_per_volume: 1,
            undo_limit: 100,
        }
    }
}

fn check(ok: bool, what: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(CoreError::InvalidSetting(what.into()))
    }
}

impl Settings {
    /// Checks every range and rule. Called by `update_settings`.
    pub fn validate(&self) -> Result<()> {
        let g = &self.general;
        check(
            !g.start_folder.trim().is_empty(),
            "general.start_folder is empty",
        )?;
        check(
            (200..=5000).contains(&g.type_select_timeout_ms),
            "general.type_select_timeout_ms must be 200–5000",
        )?;
        check(
            (1..=100).contains(&g.closed_tabs_kept),
            "general.closed_tabs_kept must be 1–100",
        )?;
        let p = &self.path_bar;
        check(
            (1..=500).contains(&p.completion_limit),
            "path_bar.completion_limit must be 1–500",
        )?;
        check(
            (2..=30).contains(&p.max_crumbs),
            "path_bar.max_crumbs must be 2–30",
        )?;
        check(
            p.frecency_excluded.iter().all(|g| !g.trim().is_empty()),
            "path_bar.frecency_excluded has an empty pattern",
        )?;
        let o = &self.operations;
        check(
            (1..=16).contains(&o.file_workers),
            "operations.file_workers must be 1–16",
        )?;
        check(
            (1..=8).contains(&o.ops_per_volume),
            "operations.ops_per_volume must be 1–8",
        )?;
        check(
            (1..=1000).contains(&o.undo_limit),
            "operations.undo_limit must be 1–1000",
        )?;
        check(
            (200..=5000).contains(&self.keymap.sequence_timeout_ms),
            "keymap.sequence_timeout_ms must be 200–5000",
        )?;
        check(
            self.overlay.delay_ms <= 2000,
            "overlay.delay_ms must be 0–2000",
        )?;
        check(
            (1.5..=15.0).contains(&self.hints.seconds),
            "hints.seconds must be 1.5–15",
        )?;
        for (id, o) in &self.keymap.overrides.commands {
            for b in &o.added {
                fstudio_keymap::validate_binding(b)
                    .map_err(|e| CoreError::InvalidSetting(format!("keymap {id}: {e}")))?;
            }
        }
        Ok(())
    }

    /// Encodes with the format version, for storage and export.
    pub fn to_json(&self) -> String {
        let doc = serde_json::json!({ "version": SETTINGS_VERSION, "settings": self });
        serde_json::to_string_pretty(&doc).unwrap_or_default()
    }

    /// Decodes what [`Settings::to_json`] wrote. Unknown keys are ignored and
    /// missing ones take their defaults; a newer format version is refused.
    pub fn from_json(text: &str) -> Result<Settings> {
        let v: Value = serde_json::from_str(text)?;
        let version = v.get("version").and_then(Value::as_u64).unwrap_or(1);
        if version > u64::from(SETTINGS_VERSION) {
            return Err(CoreError::InvalidSetting(format!(
                "settings version {version} is newer than this build supports"
            )));
        }
        let body = v.get("settings").cloned().unwrap_or(v);
        Ok(serde_json::from_value(body)?)
    }

    /// One value by dotted key (`view.show_hidden`), as JSON. `None` if there is no such key.
    pub fn get_key(&self, key: &str) -> Option<Value> {
        let mut v = serde_json::to_value(self).ok()?;
        for part in key.split('.').filter(|p| !p.is_empty()) {
            v = v.get(part)?.clone();
        }
        Some(v)
    }

    /// A copy with one value changed by dotted key. `value` is JSON (`true`,
    /// `12`, `"Vim"`); text that isn't JSON is taken as a string. The result is
    /// validated.
    pub fn with_key(&self, key: &str, value: &str) -> Result<Settings> {
        let parsed: Value =
            serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()));
        let mut root = serde_json::to_value(self)?;
        let parts: Vec<&str> = key.split('.').filter(|p| !p.is_empty()).collect();
        let Some((last, path)) = parts.split_last() else {
            return Err(CoreError::InvalidSetting("empty key".into()));
        };
        let mut cur = &mut root;
        for p in path {
            cur = cur
                .get_mut(*p)
                .ok_or_else(|| CoreError::InvalidSetting(format!("no setting '{key}'")))?;
        }
        let obj = cur
            .as_object_mut()
            .ok_or_else(|| CoreError::InvalidSetting(format!("no setting '{key}'")))?;
        if !obj.contains_key(*last) {
            return Err(CoreError::InvalidSetting(format!("no setting '{key}'")));
        }
        obj.insert((*last).to_string(), parsed);
        let s: Settings = serde_json::from_value(root)
            .map_err(|e| CoreError::InvalidSetting(format!("{key}: {e}")))?;
        s.validate()?;
        Ok(s)
    }

    /// The queue configuration these settings ask for.
    pub(crate) fn ops_config(&self) -> fstudio_ops::OpsConfig {
        let o = &self.operations;
        fstudio_ops::OpsConfig {
            allow_delete_immediately: o.allow_delete_immediately,
            verify: o.verify_copies,
            fsync_copies: o.fsync_copies,
            file_workers: o.file_workers as usize,
            ops_per_volume: o.ops_per_volume as usize,
            undo_limit: o.undo_limit as usize,
            ..fstudio_ops::OpsConfig::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate_and_roundtrip() {
        let s = Settings::default();
        s.validate().unwrap();
        assert_eq!(Settings::from_json(&s.to_json()).unwrap(), s);
        assert!(!s.operations.allow_delete_immediately);
    }

    #[test]
    fn dotted_keys() {
        let s = Settings::default();
        assert_eq!(s.get_key("view.show_hidden"), Some(Value::Bool(false)));
        let t = s.with_key("view.show_hidden", "true").unwrap();
        assert!(t.view.show_hidden);
        let t = s.with_key("keymap.profile", "Vim").unwrap();
        assert_eq!(t.keymap.profile, fstudio_keymap::Profile::Vim);
        assert!(s.with_key("view.nope", "1").is_err());
        assert!(s.with_key("operations.file_workers", "0").is_err());
        assert!(s.with_key("view.show_hidden", "\"yes\"").is_err());
    }

    #[test]
    fn missing_keys_take_defaults_and_newer_is_refused() {
        let s = Settings::from_json(r#"{"version":1,"settings":{"view":{"show_hidden":true}}}"#)
            .unwrap();
        assert!(s.view.show_hidden);
        assert_eq!(s.general, GeneralSettings::default());
        assert!(Settings::from_json(r#"{"version":99,"settings":{}}"#).is_err());
    }
}

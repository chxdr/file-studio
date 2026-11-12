//! Settings this crate owns: the keymap, the shortcut overlay and the hints.
//! The coordinator assembles them into the app's full `Settings`.

use std::collections::BTreeMap;

use fstudio_model::Corner;
use serde::{Deserialize, Serialize};

use crate::binding::Binding;
use crate::command::CommandId;
use crate::registry;
use crate::system::system_shortcut;

/// Which default keymap is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Profile {
    /// Finder-compatible, with F-key dual-pane additions.
    #[default]
    Standard,
    /// Modal, layered on top of Standard.
    Vim,
}

/// How the shell treats the F1–F19 row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum FnKeysMode {
    /// Leave F-keys to the system setting (they may be brightness or volume).
    #[default]
    System,
    /// Always deliver F1–F19 as F-keys: the shell turns media-key events back into chords.
    Always,
}

/// Where a binding in the effective keymap came from. Higher wins on equal keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    Standard,
    Vim,
    User,
}

/// One user change to a command's bindings, relative to the profile's defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CommandOverride {
    /// Default bindings the user took away.
    pub removed: Vec<Binding>,
    /// Bindings the user added, in order, after the surviving defaults.
    pub added: Vec<Binding>,
}

/// Which key slot of a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Primary,
    Secondary,
}

/// Why a binding was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BindError {
    #[error("unknown command '{0}'")]
    UnknownCommand(String),
    #[error("a binding needs at least one key")]
    Empty,
    #[error("{0} is a macOS shortcut and can't be used")]
    SystemShortcut(&'static str),
}

/// Checks a binding is allowed: not empty, and not a macOS shortcut.
pub fn validate_binding(b: &Binding) -> Result<(), BindError> {
    if b.keys.is_empty() {
        return Err(BindError::Empty);
    }
    if let Some(c) = b.first_chord()
        && let Some(name) = system_shortcut(&c)
    {
        return Err(BindError::SystemShortcut(name));
    }
    Ok(())
}

/// The default bindings of `id` in `profile`, with their tier. In the Vim
/// profile the Vim layer comes first, then Standard (so Vim falls back to it).
pub fn default_bindings(profile: Profile, id: &str) -> Vec<(Binding, Tier)> {
    let Some(info) = registry::get(id) else {
        return vec![];
    };
    let mut v = Vec::new();
    if profile == Profile::Vim {
        v.extend(info.vim.iter().cloned().map(|b| (b, Tier::Vim)));
    }
    v.extend(info.standard.iter().cloned().map(|b| (b, Tier::Standard)));
    v
}

/// All user overrides: a diff against the active profile's defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Overrides {
    pub commands: BTreeMap<CommandId, CommandOverride>,
}

impl Overrides {
    /// True when nothing is customised.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Has this command been customised?
    pub fn is_customized(&self, id: &CommandId) -> bool {
        self.commands.contains_key(id)
    }

    /// The bindings in force for `id`: surviving defaults, then the user's additions.
    /// The first is the primary key and the second the secondary.
    pub fn effective(&self, profile: Profile, id: &CommandId) -> Vec<(Binding, Tier)> {
        let mut v = default_bindings(profile, id.as_str());
        if let Some(o) = self.commands.get(id) {
            v.retain(|(b, _)| !o.removed.contains(b));
            for b in &o.added {
                if !v.iter().any(|(x, _)| x == b) {
                    v.push((b.clone(), Tier::User));
                }
            }
        }
        v
    }

    /// Stores the diff that turns the defaults into exactly `desired`.
    fn set_desired(&mut self, profile: Profile, id: &CommandId, desired: Vec<Binding>) {
        let defaults: Vec<Binding> = default_bindings(profile, id.as_str())
            .into_iter()
            .map(|(b, _)| b)
            .collect();
        let kept: Vec<Binding> = defaults
            .iter()
            .filter(|d| desired.contains(d))
            .cloned()
            .collect();
        let added: Vec<Binding> = desired
            .iter()
            .filter(|d| !defaults.contains(d))
            .cloned()
            .collect();
        let mut order = kept.clone();
        order.extend(added.iter().cloned());
        let ov = if order == desired {
            CommandOverride {
                removed: defaults.into_iter().filter(|d| !kept.contains(d)).collect(),
                added,
            }
        } else {
            // The order can't be written as "remove some, append some": replace all.
            CommandOverride {
                removed: defaults,
                added: desired,
            }
        };
        if ov == CommandOverride::default() {
            self.commands.remove(id);
        } else {
            self.commands.insert(id.clone(), ov);
        }
    }

    /// Sets (or with `None` clears) the primary or secondary key of a command.
    pub fn set_slot(
        &mut self,
        profile: Profile,
        id: &CommandId,
        slot: Slot,
        binding: Option<Binding>,
    ) -> Result<(), BindError> {
        if id.info().is_none() {
            return Err(BindError::UnknownCommand(id.to_string()));
        }
        let mut cur: Vec<Binding> = self
            .effective(profile, id)
            .into_iter()
            .map(|(b, _)| b)
            .collect();
        let idx = match slot {
            Slot::Primary => 0,
            Slot::Secondary => 1,
        };
        match binding {
            Some(b) => {
                validate_binding(&b)?;
                if let Some(j) = cur.iter().position(|x| x == &b) {
                    // Already bound here: move it into the slot.
                    cur.remove(j);
                    cur.insert(idx.min(cur.len()), b);
                } else if idx < cur.len() {
                    // Replace what sat in the slot.
                    cur[idx] = b;
                } else {
                    cur.push(b);
                }
            }
            None => {
                if idx < cur.len() {
                    cur.remove(idx);
                }
            }
        }
        self.set_desired(profile, id, cur);
        Ok(())
    }

    /// Adds another binding after the existing ones.
    pub fn add(&mut self, profile: Profile, id: &CommandId, b: Binding) -> Result<(), BindError> {
        if id.info().is_none() {
            return Err(BindError::UnknownCommand(id.to_string()));
        }
        validate_binding(&b)?;
        let mut cur: Vec<Binding> = self
            .effective(profile, id)
            .into_iter()
            .map(|(b, _)| b)
            .collect();
        if !cur.contains(&b) {
            cur.push(b);
        }
        self.set_desired(profile, id, cur);
        Ok(())
    }

    /// Removes one binding from a command.
    pub fn remove(&mut self, profile: Profile, id: &CommandId, b: &Binding) {
        let cur: Vec<Binding> = self
            .effective(profile, id)
            .into_iter()
            .map(|(b, _)| b)
            .filter(|x| x != b)
            .collect();
        self.set_desired(profile, id, cur);
    }

    /// Puts one command back to its defaults.
    pub fn reset_command(&mut self, id: &CommandId) {
        self.commands.remove(id);
    }

    /// Puts every command back to its defaults.
    pub fn reset_all(&mut self) {
        self.commands.clear();
    }
}

/// An exported set of overrides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportFile {
    /// Format version, currently 1.
    pub version: u32,
    /// The profile the diff was made against.
    pub profile: Profile,
    pub overrides: Overrides,
}

/// Why an import failed.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("not a File Studio keymap file: {0}")]
    Json(#[from] serde_json::Error),
    #[error("keymap file version {0} is newer than this build understands")]
    TooNew(u32),
}

/// The result of an import: what was kept and what was left out, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    pub profile: Profile,
    pub overrides: Overrides,
    pub warnings: Vec<String>,
}

/// Keymap settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeymapSettings {
    pub profile: Profile,
    pub overrides: Overrides,
    /// How long a multi-key sequence waits for its next key (200–5000 ms).
    pub sequence_timeout_ms: u32,
    /// Printable keys filter the list instead of selecting by name.
    pub typing_filters: bool,
    pub fn_keys_mode: FnKeysMode,
}

impl Default for KeymapSettings {
    fn default() -> Self {
        Self {
            profile: Profile::Standard,
            overrides: Overrides::default(),
            sequence_timeout_ms: 1000,
            typing_filters: false,
            fn_keys_mode: FnKeysMode::System,
        }
    }
}

impl KeymapSettings {
    /// The sequence timeout, clamped to its allowed range.
    pub fn sequence_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.sequence_timeout_ms.clamp(200, 5000) as u64)
    }

    /// Sets the primary or secondary key of a command in the active profile.
    pub fn set_slot(
        &mut self,
        id: &CommandId,
        slot: Slot,
        b: Option<Binding>,
    ) -> Result<(), BindError> {
        let p = self.profile;
        self.overrides.set_slot(p, id, slot, b)
    }

    /// Adds a binding to a command in the active profile.
    pub fn add_binding(&mut self, id: &CommandId, b: Binding) -> Result<(), BindError> {
        let p = self.profile;
        self.overrides.add(p, id, b)
    }

    /// Removes a binding from a command in the active profile.
    pub fn remove_binding(&mut self, id: &CommandId, b: &Binding) {
        let p = self.profile;
        self.overrides.remove(p, id, b);
    }

    /// Resets one command.
    pub fn reset_command(&mut self, id: &CommandId) {
        self.overrides.reset_command(id);
    }

    /// Resets all commands (keeps the other settings).
    pub fn reset_all(&mut self) {
        self.overrides.reset_all();
    }

    /// The overrides as pretty JSON, for export.
    pub fn export_json(&self) -> String {
        let f = ExportFile {
            version: 1,
            profile: self.profile,
            overrides: self.overrides.clone(),
        };
        serde_json::to_string_pretty(&f).unwrap_or_else(|_| "{}".into())
    }

    /// Reads an export. Unknown commands and macOS shortcuts are left out
    /// with a warning rather than failing the whole import.
    pub fn import_json(json: &str) -> Result<Imported, ImportError> {
        let f: ExportFile = serde_json::from_str(json)?;
        if f.version > 1 {
            return Err(ImportError::TooNew(f.version));
        }
        let mut warnings = Vec::new();
        let mut overrides = Overrides::default();
        for (id, o) in f.overrides.commands {
            if id.info().is_none() {
                warnings.push(format!("skipped unknown command '{id}'"));
                continue;
            }
            let mut keep = CommandOverride {
                removed: o.removed,
                added: vec![],
            };
            for b in o.added {
                match validate_binding(&b) {
                    Ok(()) => keep.added.push(b),
                    Err(e) => warnings.push(format!("{id}: skipped {} ({e})", b.text())),
                }
            }
            if keep != CommandOverride::default() {
                overrides.commands.insert(id, keep);
            }
        }
        Ok(Imported {
            profile: f.profile,
            overrides,
            warnings,
        })
    }
}

/// The overlay that lists shortcuts while a modifier key is held.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShortcutOverlay {
    pub enabled: bool,
    /// How long the modifier must be held before it appears (0–2000 ms).
    pub delay_ms: u32,
    pub corner: Corner,
    /// Also show the no-modifier list while holding Fn / Globe.
    pub list_plain_keys: bool,
}

impl Default for ShortcutOverlay {
    fn default() -> Self {
        Self {
            enabled: true,
            delay_ms: 400,
            corner: Corner::BottomRight,
            list_plain_keys: true,
        }
    }
}

/// How often "did you know?" hints appear.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HintFrequency {
    /// Every time (a few seconds apart).
    Always,
    /// Each shortcut at most every ten minutes, three times per launch.
    #[default]
    Sometimes,
    /// Each shortcut once per launch, and no more than one hint every two minutes.
    Rarely,
}

/// "Did you know?" hint settings.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShortcutHints {
    pub enabled: bool,
    pub frequency: HintFrequency,
    /// Seconds a hint stays up (1.5–15).
    pub seconds: f32,
    pub corner: Corner,
    /// Stop hinting a shortcut once it has been used from the keyboard a few times.
    pub stop_when_learned: bool,
}

impl Default for ShortcutHints {
    fn default() -> Self {
        Self {
            enabled: true,
            frequency: HintFrequency::Sometimes,
            seconds: 4.0,
            corner: Corner::BottomRight,
            stop_when_learned: true,
        }
    }
}

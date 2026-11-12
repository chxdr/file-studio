//! Command registry, key bindings, profiles (Standard, Vim), shortcut overlay and hints. See DECISIONS F-006, F-018.
//!
//! # Overview
//!
//! - [`registry`]: every command ([`CommandInfo`]), with default bindings per profile.
//! - [`Binding`]: a key sequence plus a [`When`] context; user changes are stored as a
//!   diff ([`Overrides`]) inside [`KeymapSettings`], which exports to and imports from JSON.
//! - [`Keymap`]: the effective keymap. [`Keymap::resolve`] turns a key press into a
//!   [`Resolution`]; [`Keymap::conflicts`] lists clashes; [`Keymap::held_shortcuts`] feeds
//!   the overlay; [`Keymap::hint_for`] feeds "Did you know?" hints.
//! - [`display`]: key-cap text (`⇧⌘N`, `F5`, `g g`).
//!
//! # For shells
//!
//! Translate native events into [`KeyChord`]s (shifted keys may be sent as the glyph
//! typed; they are normalised). Report the UI state as a [`Context`]. Run whatever
//! [`Resolution`] returns. For [`Resolution::None`] let the focused control handle the key.
//! Command ids are stable strings such as `"file.new_folder"`; match on
//! [`CommandId::as_str`].

mod binding;
mod command;
pub mod display;
mod hints;
mod keymap;
pub mod registry;
mod settings;
mod system;
mod when;

pub use binding::{Binding, KeySpec};
pub use command::{ArgKind, Category, CommandArg, CommandId, CommandInfo, ViewMode};
pub use fstudio_model::{Corner, Key, KeyChord, Mods, Via};
pub use hints::{Hint, LEARNED_AFTER, MIGRATIONS};
pub use keymap::{Conflict, ConflictKind, Entry, HeldShortcuts, Keymap, Resolution, ShortcutRow};
pub use settings::{
    BindError, CommandOverride, ExportFile, FnKeysMode, HintFrequency, ImportError, Imported,
    KeymapSettings, Overrides, Profile, ShortcutHints, ShortcutOverlay, Slot, Tier,
    default_bindings, validate_binding,
};
pub use system::system_shortcut;
pub use when::{Context, Focus, VimMode, When};

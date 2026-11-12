//! Command ids, metadata and arguments.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::binding::Binding;
use crate::when::When;

/// A stable, dotted command id such as `"file.new_folder"`.
///
/// Serialises as a plain string. Ids are part of the settings format, so a
/// shipped id is never renamed or reused (a snapshot test guards the list).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommandId(String);

impl CommandId {
    /// Wraps an id string. It need not be registered (settings written by a newer build may name unknown ids).
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The registry entry, if this id is registered.
    pub fn info(&self) -> Option<&'static CommandInfo> {
        crate::registry::get(self.as_str())
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for CommandId {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

/// Menu / palette / Settings grouping. Declaration order is display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Category {
    Navigation,
    Selection,
    File,
    Edit,
    View,
    Sort,
    Tabs,
    Panes,
    Workspaces,
    Preview,
    Path,
    Search,
    Vim,
    Settings,
    /// Keys that only act inside a text field or the palette. Never shown in menus.
    TextFields,
}

impl Category {
    /// Heading used in the overlay and Settings.
    pub fn title(self) -> &'static str {
        match self {
            Category::Navigation => "Navigation",
            Category::Selection => "Selection",
            Category::File => "File",
            Category::Edit => "Edit",
            Category::View => "View",
            Category::Sort => "Sort",
            Category::Tabs => "Tabs & Windows",
            Category::Panes => "Panes",
            Category::Workspaces => "Workspaces",
            Category::Preview => "Quick Look & Info",
            Category::Path => "Paths & Open In",
            Category::Search => "Palette & Filter",
            Category::Vim => "Vim",
            Category::Settings => "Settings",
            Category::TextFields => "Text Fields",
        }
    }
}

/// How a pane shows its items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ViewMode {
    Icons,
    List,
    Columns,
    Gallery,
}

/// What kind of argument a command takes (so the palette can ask for it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ArgKind {
    Path,
    Index,
    ViewMode,
    Char,
    /// A short text, such as a Finder tag name.
    Text,
}

/// An argument carried with a command.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CommandArg {
    /// A folder or file path.
    Path(PathBuf),
    /// A zero-based index (tab, workspace…).
    Index(usize),
    ViewMode(ViewMode),
    /// A character: the first key typed into a field, or a mark letter.
    Char(char),
    /// Ask before doing it (Vim `x` → Trash).
    Confirm,
    /// The user already said yes to the question the core asked: do it now.
    Confirmed,
    /// A short text, such as a Finder tag name (`tags.toggle`).
    Text(String),
}

/// Everything the registry knows about one command.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandInfo {
    pub id: CommandId,
    /// Menu / palette title.
    pub title: &'static str,
    pub category: Category,
    /// One plain sentence for Settings and the palette.
    pub description: &'static str,
    /// Standard-profile defaults, first is primary.
    pub standard: Vec<Binding>,
    /// Vim layer defaults. They sit on top of `standard` in the Vim profile.
    pub vim: Vec<Binding>,
    /// Where the command applies. Every default binding's `When` must imply one of these.
    pub contexts: Vec<When>,
    /// The argument it takes, if any.
    pub arg: Option<ArgKind>,
    pub shows_in_menu: bool,
    pub shows_in_palette: bool,
    /// May produce a "Did you know?" hint when used with the mouse.
    pub hint_eligible: bool,
    /// Runs again while its key is held (key auto-repeat). Only cursor
    /// movement, extending the selection, page keys and moving through a
    /// suggestion list repeat; holding any other key does it once (safety
    /// audit S6: a held ⌘⌫ must not walk down the list trashing each item).
    pub repeatable: bool,
}

impl CommandInfo {
    /// Rows in the overlay come from commands a person can see by name.
    pub fn shows_in_overlay(&self) -> bool {
        self.shows_in_menu || self.shows_in_palette
    }
}

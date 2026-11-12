//! Column view (Miller columns) behaviour that belongs in the core.
//!
//! In column view the tab's folder is the column with keyboard focus; the
//! columns to its left are ancestors shown through [`crate::ListingHandle`]s.
//! ← goes to the enclosing folder (selecting the one we came from) and → goes
//! into the selected folder. Packages count as files here, as in Finder.

use std::path::{Path, PathBuf};

use fstudio_keymap::{Focus, ViewMode};
use fstudio_model::{Key, KeyChord};

use crate::commands::InputContext;
use crate::engine::Engine;
use crate::error::Result;
use crate::ids::TabId;
use crate::outcome::Outcome;
use crate::tab::SelectionChange;

/// What ← and → do in column view, for a cursor row of this kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColumnStep {
    /// Go to the enclosing folder.
    Out,
    /// Go into the selected folder.
    In,
    /// Nothing (→ on a file, or no cursor): swallow the key.
    Stay,
}

/// The pure rule: which step a key makes. `None` when the key isn't a
/// column-view key (other keys, modifiers, other focus, other view modes).
pub(crate) fn column_step(
    chord: &KeyChord,
    focus: Focus,
    mode: ViewMode,
    cursor_is_folder: bool,
) -> Option<ColumnStep> {
    if focus != Focus::List || mode != ViewMode::Columns || chord.mods != Default::default() {
        return None;
    }
    match chord.key {
        Key::Left => Some(ColumnStep::Out),
        Key::Right if cursor_is_folder => Some(ColumnStep::In),
        Key::Right => Some(ColumnStep::Stay),
        _ => None,
    }
}

impl Engine {
    /// ←/→ in column view (called first by `handle_key`).
    pub(crate) fn column_key(&self, chord: &KeyChord, ctx: &InputContext) -> Option<Outcome> {
        if !matches!(chord.key, Key::Left | Key::Right) {
            return None;
        }
        let tab = self.target(ctx.tab).ok()?;
        let l = tab.listing();
        let sel = tab.selection();
        let cursor = sel.cursor.and_then(|c| l.entry(c));
        let is_folder = cursor.as_ref().is_some_and(|e| e.is_dir() && !e.is_package);
        let step = column_step(chord, ctx.focus, tab.view().mode, is_folder)?;
        let out = Outcome::none().with_tab(tab.id());
        Some(match step {
            ColumnStep::Out => {
                let cur = PathBuf::from(tab.location());
                if let Some(parent) = cur.parent() {
                    let name = cur.file_name().map(|n| n.to_string_lossy().into_owned());
                    let sel = name.map(|n| (vec![n.clone()], Some(n)));
                    tab.navigate_keeping_mode(parent.to_path_buf(), sel);
                }
                out
            }
            ColumnStep::In => {
                let dir = PathBuf::from(tab.location()).join(&cursor.expect("a folder").name);
                tab.navigate_keeping_mode(dir, None);
                out
            }
            ColumnStep::Stay => out,
        })
    }

    /// A click in another column of the column view: the tab goes to `dir`
    /// (an ancestor or the folder shown to the right) and selects `name`
    /// there, or nothing when `name` is `None`. In the tab's own folder it
    /// only changes the selection.
    pub fn column_select(&self, tab: TabId, dir: &str, name: Option<&str>) -> Result<Outcome> {
        let t = self.target(Some(tab))?;
        let names: Vec<String> = name.map(|n| vec![n.to_string()]).unwrap_or_default();
        if Path::new(&t.location()) == Path::new(dir) {
            t.set_selection(match name {
                Some(_) => SelectionChange::Names(names),
                None => SelectionChange::None,
            });
        } else {
            let cursor = names.first().cloned();
            t.navigate_keeping_mode(PathBuf::from(dir), Some((names, cursor)));
        }
        Ok(Outcome::none().with_tab(tab))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fstudio_model::Mods;

    fn k(key: Key) -> KeyChord {
        KeyChord {
            key,
            mods: Mods::NONE,
        }
    }

    #[test]
    fn steps() {
        let c = ViewMode::Columns;
        assert_eq!(
            column_step(&k(Key::Left), Focus::List, c, false),
            Some(ColumnStep::Out)
        );
        assert_eq!(
            column_step(&k(Key::Right), Focus::List, c, true),
            Some(ColumnStep::In)
        );
        assert_eq!(
            column_step(&k(Key::Right), Focus::List, c, false),
            Some(ColumnStep::Stay)
        );
        assert_eq!(
            column_step(&k(Key::Left), Focus::List, ViewMode::List, true),
            None
        );
        assert_eq!(column_step(&k(Key::Left), Focus::AddressBar, c, true), None);
        assert_eq!(column_step(&k(Key::Up), Focus::List, c, true), None);
        let shifted = KeyChord {
            key: Key::Left,
            mods: Mods {
                shift: true,
                ..Mods::NONE
            },
        };
        assert_eq!(column_step(&shifted, Focus::List, c, true), None);
    }
}

//! Where a binding applies: the UI state a key press happens in.

use serde::{Deserialize, Serialize};

/// Which part of the UI has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Focus {
    /// A file list (any pane).
    #[default]
    List,
    /// The address bar is being edited.
    AddressBar,
    /// The command palette is open.
    Palette,
    /// A name is being edited in place.
    Rename,
    /// The filter field is being edited.
    Filter,
    /// Anything else (sidebar, a dialog…).
    Other,
}

/// Vim modal state. `Off` means the Standard profile is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum VimMode {
    #[default]
    Off,
    Normal,
    Visual,
}

/// Everything the resolver needs to know about the UI at the moment of a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Context {
    pub focus: Focus,
    pub dual_pane: bool,
    pub vim: VimMode,
}

impl Context {
    /// File list focused, single pane, Standard profile.
    pub fn list() -> Self {
        Self::default()
    }

    /// Every possible context. Small enough to enumerate (6 × 2 × 3).
    pub fn all() -> Vec<Context> {
        let mut v = Vec::new();
        for focus in [
            Focus::List,
            Focus::AddressBar,
            Focus::Palette,
            Focus::Rename,
            Focus::Filter,
            Focus::Other,
        ] {
            for dual_pane in [false, true] {
                for vim in [VimMode::Off, VimMode::Normal, VimMode::Visual] {
                    v.push(Context {
                        focus,
                        dual_pane,
                        vim,
                    });
                }
            }
        }
        v
    }
}

/// A condition on [`Context`]; a binding is live only where its `When` holds.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum When {
    /// Everywhere.
    Always,
    ListFocused,
    AddressBarEditing,
    PaletteOpen,
    RenameEditing,
    FilterEditing,
    DualPane,
    SinglePane,
    VimNormal,
    VimVisual,
    Not(Box<When>),
    /// All of these hold.
    All(Vec<When>),
}

impl When {
    /// List focused in Vim normal mode.
    pub fn vim_normal() -> When {
        When::All(vec![When::ListFocused, When::VimNormal])
    }

    /// List focused in Vim visual mode.
    pub fn vim_visual() -> When {
        When::All(vec![When::ListFocused, When::VimVisual])
    }

    /// Does the predicate hold in `ctx`?
    pub fn matches(&self, ctx: &Context) -> bool {
        match self {
            When::Always => true,
            When::ListFocused => ctx.focus == Focus::List,
            When::AddressBarEditing => ctx.focus == Focus::AddressBar,
            When::PaletteOpen => ctx.focus == Focus::Palette,
            When::RenameEditing => ctx.focus == Focus::Rename,
            When::FilterEditing => ctx.focus == Focus::Filter,
            When::DualPane => ctx.dual_pane,
            When::SinglePane => !ctx.dual_pane,
            When::VimNormal => ctx.vim == VimMode::Normal,
            When::VimVisual => ctx.vim == VimMode::Visual,
            When::Not(w) => !w.matches(ctx),
            When::All(ws) => ws.iter().all(|w| w.matches(ctx)),
        }
    }

    /// Is there any context where both predicates hold? (Used for conflicts.)
    pub fn overlaps(&self, other: &When) -> bool {
        Context::all()
            .iter()
            .any(|c| self.matches(c) && other.matches(c))
    }

    /// Does every context that satisfies `self` also satisfy `other`?
    pub fn implies(&self, other: &When) -> bool {
        Context::all()
            .iter()
            .all(|c| !self.matches(c) || other.matches(c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_contexts_are_exclusive() {
        assert!(!When::ListFocused.overlaps(&When::AddressBarEditing));
        assert!(!When::RenameEditing.overlaps(&When::PaletteOpen));
        assert!(When::Always.overlaps(&When::PaletteOpen));
        assert!(!When::VimNormal.overlaps(&When::VimVisual));
        assert!(When::DualPane.overlaps(&When::ListFocused));
        assert!(!When::DualPane.overlaps(&When::SinglePane));
    }

    #[test]
    fn implication() {
        assert!(When::vim_normal().implies(&When::ListFocused));
        assert!(!When::ListFocused.implies(&When::vim_normal()));
        assert!(When::ListFocused.implies(&When::Always));
    }
}

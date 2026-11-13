//! The workspace model (F-002): windows → panes (one or two) → tabs.
//!
//! [`WorkspaceState`] is the plain, serialisable form used for session
//! restore, saved workspaces and the shell. The engine keeps a runtime form
//! ([`Model`]) whose pure operations are tested here without any I/O.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ids::{PaneId, TabId, WindowId, WorkspaceId};
use crate::tab::{Tab, TabState};

/// How two panes are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Orientation {
    /// Side by side.
    #[default]
    Horizontal,
    /// One above the other.
    Vertical,
}

/// A pane and its tabs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneState {
    /// The pane.
    pub id: PaneId,
    /// Its tabs, in order.
    pub tabs: Vec<TabState>,
    /// The index of the active tab.
    pub active_tab: u32,
}

/// A window and its panes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    /// The window.
    pub id: WindowId,
    /// One pane, or two when dual-pane is on.
    pub panes: Vec<PaneState>,
    /// The index of the focused pane.
    pub active_pane: u32,
    /// Side by side or stacked.
    pub orientation: Orientation,
    /// The first pane's share of the window (0.1–0.9).
    pub split_ratio: f64,
    /// The second pane while dual-pane is off, so it comes back as it was.
    #[serde(default)]
    pub hidden_pane: Option<PaneState>,
    /// The sidebar's width in points (F-019's 200 by default).
    #[serde(default = "default_sidebar_width")]
    pub sidebar_width: f64,
    /// Whether the sidebar is collapsed.
    #[serde(default)]
    pub sidebar_hidden: bool,
}

/// The sidebar width a new window starts with (F-019).
pub const DEFAULT_SIDEBAR_WIDTH: f64 = 200.0;
/// The narrowest and widest sidebar the shell allows.
pub const SIDEBAR_WIDTH_RANGE: (f64, f64) = (160.0, 320.0);

fn default_sidebar_width() -> f64 {
    DEFAULT_SIDEBAR_WIDTH
}

/// Keeps a sidebar width inside [`SIDEBAR_WIDTH_RANGE`] (NaN gives the default).
pub(crate) fn clamp_sidebar(w: f64) -> f64 {
    if w.is_finite() {
        w.clamp(SIDEBAR_WIDTH_RANGE.0, SIDEBAR_WIDTH_RANGE.1)
    } else {
        DEFAULT_SIDEBAR_WIDTH
    }
}

/// Every window, for session restore and saved workspaces.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceState {
    /// The windows, front to back.
    pub windows: Vec<WindowState>,
    /// The focused window.
    pub active_window: Option<WindowId>,
}

impl WorkspaceState {
    /// How many tabs, in all windows and panes (hidden panes not counted).
    pub fn tab_count(&self) -> usize {
        self.windows
            .iter()
            .flat_map(|w| &w.panes)
            .map(|p| p.tabs.len())
            .sum()
    }
}

/// A workspace, for the switcher and the manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSummary {
    /// Its id.
    pub id: WorkspaceId,
    /// Its name, exactly as the user typed it (trimmed).
    pub name: String,
    /// Its SF Symbol name (`camera`, `house`…).
    #[serde(default)]
    pub icon: String,
    /// Its place in the switcher, from 0.
    #[serde(default)]
    pub position: u32,
    /// Whether it is the one on screen now.
    #[serde(default)]
    pub active: bool,
    /// The key that switches to it (`⌃⌘1`), if it has one.
    #[serde(default)]
    pub shortcut: Option<String>,
    /// Windows in it.
    pub windows: u32,
    /// Tabs in it.
    pub tabs: u32,
    /// Last saved, ns since the epoch.
    pub updated_ns: i64,
}

// ───────────────────────── runtime model ─────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PaneRt {
    pub(crate) id: PaneId,
    pub(crate) tabs: Vec<TabId>,
    pub(crate) active: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WindowRt {
    pub(crate) id: WindowId,
    pub(crate) panes: Vec<PaneRt>,
    pub(crate) active_pane: usize,
    pub(crate) orientation: Orientation,
    pub(crate) split_ratio: f64,
    pub(crate) hidden: Option<PaneRt>,
    pub(crate) sidebar_width: f64,
    pub(crate) sidebar_hidden: bool,
}

/// A closed tab, for "Reopen Closed Tab".
#[derive(Debug, Clone)]
pub(crate) struct Closed {
    pub(crate) state: TabState,
    pub(crate) window: WindowId,
    pub(crate) pane: PaneId,
    pub(crate) index: usize,
}

/// Where a tab lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Place {
    pub(crate) window: usize,
    pub(crate) pane: usize,
    pub(crate) index: usize,
}

#[derive(Default)]
pub(crate) struct Model {
    pub(crate) windows: Vec<WindowRt>,
    pub(crate) active_window: Option<WindowId>,
    pub(crate) tabs: HashMap<TabId, Arc<Tab>>,
    pub(crate) closed: Vec<Closed>,
}

impl Model {
    pub(crate) fn find(&self, tab: TabId) -> Option<Place> {
        for (wi, w) in self.windows.iter().enumerate() {
            for (pi, p) in w.panes.iter().enumerate() {
                if let Some(i) = p.tabs.iter().position(|t| *t == tab) {
                    return Some(Place {
                        window: wi,
                        pane: pi,
                        index: i,
                    });
                }
            }
        }
        None
    }

    pub(crate) fn window_index(&self, id: WindowId) -> Option<usize> {
        self.windows.iter().position(|w| w.id == id)
    }

    pub(crate) fn pane_place(&self, id: PaneId) -> Option<(usize, usize)> {
        for (wi, w) in self.windows.iter().enumerate() {
            if let Some(pi) = w.panes.iter().position(|p| p.id == id) {
                return Some((wi, pi));
            }
        }
        None
    }

    pub(crate) fn active_window_index(&self) -> Option<usize> {
        self.active_window
            .and_then(|id| self.window_index(id))
            .or(if self.windows.is_empty() {
                None
            } else {
                Some(0)
            })
    }

    /// The focused tab of the focused pane of the focused window.
    pub(crate) fn active_tab(&self) -> Option<TabId> {
        let w = &self.windows[self.active_window_index()?];
        let p = w.panes.get(w.active_pane)?;
        p.tabs.get(p.active).copied()
    }

    /// The active tab of the other pane of the window holding `tab`.
    pub(crate) fn other_pane_tab(&self, tab: TabId) -> Option<TabId> {
        let pl = self.find(tab)?;
        let w = &self.windows[pl.window];
        let other = w.panes.get(1 - pl.pane.min(1))?;
        if w.panes.len() < 2 {
            return None;
        }
        other.tabs.get(other.active).copied()
    }

    /// Removes a tab from its pane. Returns where it was, and whether its
    /// window is now empty (and was removed).
    pub(crate) fn detach(&mut self, tab: TabId) -> Option<(Place, WindowId, PaneId, bool)> {
        let pl = self.find(tab)?;
        let w = &mut self.windows[pl.window];
        let wid = w.id;
        let pid = w.panes[pl.pane].id;
        let p = &mut w.panes[pl.pane];
        p.tabs.remove(pl.index);
        if p.active >= p.tabs.len() {
            p.active = p.tabs.len().saturating_sub(1);
        } else if pl.index < p.active {
            p.active -= 1;
        }
        if p.tabs.is_empty() {
            if w.panes.len() == 2 {
                w.panes.remove(pl.pane);
                w.active_pane = 0;
            } else {
                self.windows.remove(pl.window);
                if self.active_window == Some(wid) {
                    self.active_window = self.windows.first().map(|w| w.id);
                }
                return Some((pl, wid, pid, true));
            }
        }
        Some((pl, wid, pid, false))
    }

    /// Inserts a tab into a pane at `index` (clamped) and makes it active.
    pub(crate) fn attach(&mut self, tab: TabId, window: usize, pane: usize, index: usize) {
        let w = &mut self.windows[window];
        let p = &mut w.panes[pane];
        let i = index.min(p.tabs.len());
        p.tabs.insert(i, tab);
        p.active = i;
        w.active_pane = pane;
        self.active_window = Some(w.id);
    }

    /// Makes `tab` the focused tab (and its pane and window focused).
    pub(crate) fn activate(&mut self, tab: TabId) -> bool {
        let Some(pl) = self.find(tab) else {
            return false;
        };
        let w = &mut self.windows[pl.window];
        w.active_pane = pl.pane;
        w.panes[pl.pane].active = pl.index;
        self.active_window = Some(w.id);
        true
    }

    /// Moves a tab within or between panes.
    pub(crate) fn move_tab(
        &mut self,
        tab: TabId,
        window: usize,
        pane: usize,
        index: usize,
    ) -> bool {
        let Some(from) = self.find(tab) else {
            return false;
        };
        if from.window == window && from.pane == pane {
            let p = &mut self.windows[window].panes[pane];
            p.tabs.remove(from.index);
            let i = index.min(p.tabs.len());
            p.tabs.insert(i, tab);
            p.active = i;
            return true;
        }
        let target = self.windows[window].panes[pane].id;
        // The source pane may disappear; find the target again afterwards.
        if self.detach(tab).is_none() {
            return false;
        }
        let Some((wi, pi)) = self.pane_place(target) else {
            return false;
        };
        self.attach(tab, wi, pi, index);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> Model {
        Model {
            windows: vec![WindowRt {
                id: WindowId(1),
                panes: vec![
                    PaneRt {
                        id: PaneId(1),
                        tabs: vec![TabId(1), TabId(2), TabId(3)],
                        active: 1,
                    },
                    PaneRt {
                        id: PaneId(2),
                        tabs: vec![TabId(4)],
                        active: 0,
                    },
                ],
                active_pane: 0,
                orientation: Orientation::Horizontal,
                split_ratio: 0.5,
                hidden: None,
                sidebar_width: DEFAULT_SIDEBAR_WIDTH,
                sidebar_hidden: false,
            }],
            active_window: Some(WindowId(1)),
            ..Default::default()
        }
    }

    #[test]
    fn active_and_other() {
        let m = model();
        assert_eq!(m.active_tab(), Some(TabId(2)));
        assert_eq!(m.other_pane_tab(TabId(2)), Some(TabId(4)));
        assert_eq!(m.other_pane_tab(TabId(4)), Some(TabId(2)));
    }

    #[test]
    fn detach_adjusts_active_and_collapses_empty_pane() {
        let mut m = model();
        m.detach(TabId(1));
        assert_eq!(m.windows[0].panes[0].tabs, [TabId(2), TabId(3)]);
        assert_eq!(m.active_tab(), Some(TabId(2)));
        let (_, _, _, window_gone) = m.detach(TabId(4)).unwrap();
        assert!(!window_gone);
        assert_eq!(m.windows[0].panes.len(), 1);
        m.detach(TabId(2));
        let (_, _, _, window_gone) = m.detach(TabId(3)).unwrap();
        assert!(window_gone);
        assert!(m.windows.is_empty());
        assert_eq!(m.active_tab(), None);
    }

    #[test]
    fn reorder_and_move_between_panes() {
        let mut m = model();
        assert!(m.move_tab(TabId(1), 0, 0, 2));
        assert_eq!(m.windows[0].panes[0].tabs, [TabId(2), TabId(3), TabId(1)]);
        assert!(m.move_tab(TabId(4), 0, 0, 0));
        // The second pane emptied and collapsed.
        assert_eq!(m.windows[0].panes.len(), 1);
        assert_eq!(m.windows[0].panes[0].tabs[0], TabId(4));
        assert_eq!(m.active_tab(), Some(TabId(4)));
    }
}

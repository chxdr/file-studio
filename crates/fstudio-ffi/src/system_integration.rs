//! File Studio as the Mac's file explorer: the default-viewer state, folder
//! opens and reveals from other apps, and the Open/Save panel jump. Plain
//! mirrors of `fstudio_core::system_integration`; the decisions live there.

use fstudio_core as core;

use crate::{FfiEngine, FfiOutcome, R};

/// Who handles "Show in Finder" and folder opens.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiViewerState {
    Finder,
    ThisApp,
    OtherCopy,
    OtherApp,
}

/// The default-viewer switch, with text for Settings.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiViewerStatus {
    pub state: FfiViewerState,
    pub headline: String,
    pub detail: String,
    pub can_make_default: bool,
    pub can_restore_finder: bool,
    /// Reveal in Finder must send Finder its own reveal event.
    pub reveal_needs_finder_event: bool,
    /// What the switch changes, in plain words.
    pub explained: String,
}

/// Works out the default-viewer state from `NSFileViewer`, the
/// `public.folder` handler and this app's bundle id.
#[uniffi::export]
pub fn file_viewer_status(
    file_viewer: Option<String>,
    folder_handler: Option<String>,
    own_id: String,
) -> FfiViewerStatus {
    let s = core::viewer_status(file_viewer.as_deref(), folder_handler.as_deref(), &own_id);
    FfiViewerStatus {
        state: match s.state {
            core::ViewerState::Finder => FfiViewerState::Finder,
            core::ViewerState::ThisApp => FfiViewerState::ThisApp,
            core::ViewerState::OtherCopy => FfiViewerState::OtherCopy,
            core::ViewerState::OtherApp => FfiViewerState::OtherApp,
        },
        headline: s.headline,
        detail: s.detail,
        can_make_default: s.can_make_default,
        can_restore_finder: s.can_restore_finder,
        reveal_needs_finder_event: s.reveal_needs_finder_event,
        explained: core::DEFAULT_VIEWER_EXPLAINED.to_string(),
    }
}

/// An Open or Save panel.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiPanelKind {
    Open,
    Save,
}

/// What Accessibility says about a window or sheet.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiPanelProbe {
    pub role: String,
    pub subrole: String,
    pub identifier: String,
    pub buttons: Vec<String>,
    pub inner_roles: Vec<String>,
}

/// Is this window or sheet a system Open or Save panel?
#[uniffi::export]
pub fn classify_file_panel(probe: FfiPanelProbe) -> Option<FfiPanelKind> {
    core::classify_panel(&core::PanelProbe {
        role: probe.role,
        subrole: probe.subrole,
        identifier: probe.identifier,
        buttons: probe.buttons,
        inner_roles: probe.inner_roles,
    })
    .map(|k| match k {
        core::PanelKind::Open => FfiPanelKind::Open,
        core::PanelKind::Save => FfiPanelKind::Save,
    })
}

/// A rectangle in top-left-origin screen coordinates.
#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq)]
pub struct FfiScreenRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl From<FfiScreenRect> for core::ScreenRect {
    fn from(r: FfiScreenRect) -> Self {
        core::ScreenRect {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
        }
    }
}

/// Where the floating "Go to …" button goes for a panel (its top-left
/// corner, in the same coordinates).
#[uniffi::export]
pub fn panel_button_origin(
    panel: FfiScreenRect,
    button_w: f64,
    button_h: f64,
    screen: FfiScreenRect,
) -> FfiScreenRect {
    let (x, y) = core::panel_button_origin(panel.into(), button_w, button_h, screen.into());
    FfiScreenRect {
        x,
        y,
        w: button_w,
        h: button_h,
    }
}

/// The system-wide shortcut for a key-cap text (`⌃⌥⌘G`), or `None` when it
/// can't be one (a sequence, or no ⌘/⌃/⌥).
#[uniffi::export]
pub fn global_hotkey_chord(keys: String) -> Option<crate::FfiKeyChord> {
    use core::Key as K;
    let c = core::global_hotkey(&keys)?;
    let key = match c.key {
        K::Char(ch) => crate::FfiKey::Char {
            value: ch.to_string(),
        },
        K::Left => crate::FfiKey::Left,
        K::Right => crate::FfiKey::Right,
        K::Up => crate::FfiKey::Up,
        K::Down => crate::FfiKey::Down,
        K::Space => crate::FfiKey::Space,
        K::Return => crate::FfiKey::Return,
        K::Enter => crate::FfiKey::Enter,
        K::Escape => crate::FfiKey::Escape,
        K::Tab => crate::FfiKey::Tab,
        K::Delete => crate::FfiKey::Delete,
        K::ForwardDelete => crate::FfiKey::ForwardDelete,
        K::Home => crate::FfiKey::Home,
        K::End => crate::FfiKey::End,
        K::PageUp => crate::FfiKey::PageUp,
        K::PageDown => crate::FfiKey::PageDown,
        K::F(n) => crate::FfiKey::F { n },
    };
    Some(crate::FfiKeyChord {
        key,
        mods: crate::FfiMods {
            shift: c.mods.shift,
            cmd: c.mods.cmd,
            alt: c.mods.alt,
            ctrl: c.mods.ctrl,
        },
    })
}

/// The floating button's title for a folder.
#[uniffi::export]
pub fn panel_button_title(folder: String) -> String {
    core::panel_button_title(&folder)
}

/// What to type into a panel's Go to Folder field for a folder.
#[uniffi::export]
pub fn panel_jump_text(folder: String) -> String {
    core::panel_jump_text(&folder)
}

#[uniffi::export]
impl FfiEngine {
    /// Opens folders (`reveal` false) or reveals items (`reveal` true) sent by
    /// another app. Stats each path (quick, but it touches the disk).
    pub fn open_from_system(&self, paths: Vec<String>, reveal: bool) -> R<FfiOutcome> {
        Ok(self.inner.open_from_system(&paths, reveal)?.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_crosses_with_its_explanation() {
        let s = file_viewer_status(None, None, "app.filestudio.mac".into());
        assert_eq!(s.state, FfiViewerState::Finder);
        assert!(s.explained.contains("Show in Finder"));
    }

    #[test]
    fn panel_probe_crosses() {
        let p = FfiPanelProbe {
            role: "AXWindow".into(),
            subrole: "AXDialog".into(),
            identifier: "save-panel".into(),
            buttons: vec![],
            inner_roles: vec![],
        };
        assert_eq!(classify_file_panel(p), Some(FfiPanelKind::Save));
    }
}

//! File Studio as the Mac's file explorer: the default file viewer (folder
//! opens and "Show in Finder" requests from other apps) and the Open/Save
//! panel jump. The shell makes the OS calls (`NSWorkspace`, Apple Events,
//! Accessibility); everything it decides with lives here, so it can be tested
//! without the OS.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::ids::TabId;
use crate::outcome::Outcome;
use crate::tab::TabState;
use crate::view::ViewState;

/// Finder's bundle id.
pub const FINDER_BUNDLE_ID: &str = "com.apple.finder";

/// What the "default file viewer" switch changes, in plain words, for the
/// Settings pane.
pub const DEFAULT_VIEWER_EXPLAINED: &str = "Other apps' “Show in Finder” and “Reveal” \
buttons (Safari and browser downloads, Xcode, `open -R` in Terminal) will show the item in \
File Studio, selected in its folder. Finder keeps running for the desktop, the Trash and Get \
Info, and Open and Save panels stay the system's. macOS doesn't let another app take over \
folders that apps open directly (`open ~/Downloads`, a folder in the Dock), so those still \
open in Finder. You can switch back to Finder here at any time.";

/// Where folders that other apps open in File Studio appear.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FolderOpensIn {
    /// A new tab in the front window (a window if there is none).
    #[default]
    NewTab,
    /// A new window each time.
    NewWindow,
}

/// Settings for working as the Mac's file explorer (`integration.*`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IntegrationSettings {
    /// Where folders opened from other apps (and reveal requests) appear.
    pub folder_opens_in: FolderOpensIn,
    /// Send other apps' Open and Save panels to the active pane's folder with
    /// a global shortcut. Needs Accessibility permission, so it is off until
    /// the user turns it on.
    pub panel_jump: bool,
    /// While `panel_jump` is on, show a small button next to Open and Save
    /// panels that does the same.
    pub panel_jump_button: bool,
}

impl Default for IntegrationSettings {
    fn default() -> Self {
        Self {
            folder_opens_in: FolderOpensIn::NewTab,
            panel_jump: false,
            panel_jump_button: true,
        }
    }
}

// ───────────────────────── default file viewer ─────────────────────────

/// Who handles "Show in Finder" right now (the `NSFileViewer` app).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewerState {
    /// Finder does both (the macOS default).
    Finder,
    /// This copy of File Studio does both.
    ThisApp,
    /// Another copy of File Studio (Release vs Debug) does both.
    OtherCopy,
    /// Some other app does both.
    OtherApp,
}

/// The current state of the default-viewer switch, with text for Settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewerStatus {
    /// The state.
    pub state: ViewerState,
    /// One line: "File Studio is your default file viewer."
    pub headline: String,
    /// What that means, or what is half set.
    pub detail: String,
    /// The "make default" button should be offered (this app isn't fully it).
    pub can_make_default: bool,
    /// The "switch back to Finder" button should be offered.
    pub can_restore_finder: bool,
    /// Revealing in Finder must address Finder directly: the system reveal
    /// call (`activateFileViewerSelectingURLs`) now goes to another app.
    pub reveal_needs_finder_event: bool,
}

fn same_id(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn is_file_studio(id: &str) -> bool {
    let l = id.to_ascii_lowercase();
    l == "app.filestudio.mac" || l.starts_with("app.filestudio.mac.")
}

/// Works out the switch's state from the two system settings.
///
/// - `file_viewer`: the global `NSFileViewer` preference (`None` when unset,
///   which means Finder). It decides the state: it is where every app's
///   "Show in Finder" goes.
/// - `folder_handler`: the bundle id of the app that opens `public.folder`.
///   macOS (26 and later at least) refuses to give folders to any app but
///   Finder, so this only adds a note.
/// - `own_id`: this app's bundle id.
pub fn viewer_status(
    file_viewer: Option<&str>,
    folder_handler: Option<&str>,
    own_id: &str,
) -> ViewerStatus {
    let viewer = file_viewer
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(FINDER_BUNDLE_ID);
    let folders = folder_handler
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(FINDER_BUNDLE_ID);
    let finder_folders = same_id(folders, FINDER_BUNDLE_ID);
    let (state, headline, mut detail) = if same_id(viewer, FINDER_BUNDLE_ID) {
        (
            ViewerState::Finder,
            "Finder is your default file viewer.".to_string(),
            "“Show in Finder” in other apps goes to Finder.".to_string(),
        )
    } else if same_id(viewer, own_id) {
        (
            ViewerState::ThisApp,
            "File Studio is your default file viewer.".to_string(),
            "“Show in Finder” in other apps and `open -R` come here.".to_string(),
        )
    } else if is_file_studio(viewer) {
        (
            ViewerState::OtherCopy,
            format!("{} is your default file viewer.", other_copy_name(viewer)),
            "Click Make Default to use this copy instead; only one copy can be the viewer."
                .to_string(),
        )
    } else {
        (
            ViewerState::OtherApp,
            format!("{viewer} is your default file viewer."),
            "Another app handles “Show in Finder”.".to_string(),
        )
    };
    if same_id(folders, own_id) {
        detail.push_str(" Folders other apps open come here too.");
    } else if state != ViewerState::Finder && finder_folders {
        detail.push_str(
            " Folders other apps open directly still open in Finder: macOS keeps those for Finder.",
        );
    }
    ViewerStatus {
        can_make_default: state != ViewerState::ThisApp,
        can_restore_finder: state != ViewerState::Finder || !finder_folders,
        reveal_needs_finder_event: state != ViewerState::Finder,
        state,
        headline,
        detail,
    }
}

fn other_copy_name(id: &str) -> String {
    if id.to_ascii_lowercase().ends_with(".debug") {
        "File Studio Debug".into()
    } else if same_id(id, "app.filestudio.mac") {
        "File Studio (the installed copy)".into()
    } else {
        format!("Another File Studio ({id})")
    }
}

// ───────────────────────── folder opens and reveals ─────────────────────────

/// One path another app asked File Studio to open or reveal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenItem {
    /// The path, exactly as given.
    pub path: PathBuf,
    /// It is a folder (after following symlinks).
    pub is_dir: bool,
}

/// A folder to show, and the names to select in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenStep {
    /// The folder.
    pub folder: PathBuf,
    /// Names in it to select (empty: just open the folder).
    pub select: Vec<String>,
}

/// Turns an open or reveal request into folders to show.
///
/// - Opening a folder shows the folder itself.
/// - Revealing anything, or opening a file (File Studio only registers for
///   folders, so a file is shown, never launched), shows its parent folder
///   with the item selected.
/// - Items in the same folder share one step, in first-seen order; the root
///   folder has no parent, so it is opened.
pub fn plan_system_open(items: &[OpenItem], reveal: bool) -> Vec<OpenStep> {
    let mut steps: Vec<OpenStep> = Vec::new();
    for it in items {
        let (folder, name) = match (reveal || !it.is_dir, it.path.parent(), it.path.file_name()) {
            (true, Some(parent), Some(name)) => (
                parent.to_path_buf(),
                Some(name.to_string_lossy().into_owned()),
            ),
            _ => (it.path.clone(), None),
        };
        let folder = if folder.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            folder
        };
        let step = match steps.iter_mut().position(|s| s.folder == folder) {
            Some(i) => &mut steps[i],
            None => {
                steps.push(OpenStep {
                    folder,
                    select: Vec::new(),
                });
                steps.last_mut().expect("just pushed")
            }
        };
        if let Some(n) = name
            && !step.select.contains(&n)
        {
            step.select.push(n);
        }
    }
    steps
}

/// The text "Couldn't find …" for paths that don't exist, or `None`.
pub fn missing_message(missing: &[String]) -> Option<String> {
    match missing {
        [] => None,
        [one] => Some(format!(
            "Couldn't find “{}”",
            Path::new(one)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| one.clone())
        )),
        many => Some(format!("Couldn't find {} items", many.len())),
    }
}

impl Engine {
    /// Opens folders (`reveal == false`) or reveals items (`reveal == true`)
    /// that another app sent: a folder open, "Show in Finder", `open -R`.
    /// Each folder appears per `integration.folder_opens_in`, except that a
    /// folder the focused tab already shows is reused. Returns the last tab
    /// shown (the shell brings its window forward) and a message for paths
    /// that don't exist.
    pub fn open_from_system(&self, paths: &[String], reveal: bool) -> Result<Outcome> {
        let mut items = Vec::new();
        let mut missing = Vec::new();
        for p in paths {
            let path = PathBuf::from(p);
            match std::fs::metadata(&path) {
                Ok(md) => items.push(OpenItem {
                    path,
                    is_dir: md.is_dir(),
                }),
                // A dangling symlink can still be revealed in its folder.
                Err(_) if reveal && std::fs::symlink_metadata(&path).is_ok() => {
                    items.push(OpenItem {
                        path,
                        is_dir: false,
                    })
                }
                Err(_) => missing.push(p.clone()),
            }
        }
        let steps = plan_system_open(&items, reveal);
        if steps.is_empty() {
            return Err(CoreError::NotFound(
                missing_message(&missing).unwrap_or_else(|| "nothing to open".into()),
            ));
        }
        let target = self.settings_integration().folder_opens_in;
        let mut last: Option<TabId> = None;
        for s in steps {
            last = Some(self.show_step(s, target)?);
        }
        let mut out = Outcome::none();
        out.tab = last;
        out.message = missing_message(&missing);
        Ok(out)
    }

    fn settings_integration(&self) -> IntegrationSettings {
        self.settings().integration
    }

    fn show_step(&self, s: OpenStep, target: FolderOpensIn) -> Result<TabId> {
        if let Some(t) = self.active_tab()
            && t.location_path() == s.folder
        {
            if !s.select.is_empty() {
                t.select_when_present(s.select);
            }
            self.activate_tab(t.id())?;
            return Ok(t.id());
        }
        let state = self.exact_tab_state(&s.folder, s.select);
        let pane = {
            let m = self.model.lock();
            m.active_window_index().map(|wi| {
                let w = &m.windows[wi];
                (w.panes[w.active_pane].id, w.panes[w.active_pane].active + 1)
            })
        };
        match (target, pane) {
            (FolderOpensIn::NewTab, Some((pane, idx))) => {
                Ok(self.insert_tab(pane, state, Some(idx))?.id())
            }
            _ => {
                let w = self.new_window_with(state)?;
                let m = self.model.lock();
                let wi = m
                    .window_index(w)
                    .ok_or_else(|| CoreError::NotFound(format!("window {w}")))?;
                Ok(m.windows[wi].panes[0].tabs[0])
            }
        }
    }

    /// A tab at exactly `folder` (never parsed as typed text, so `$`, `~`
    /// and quotes in names stay literal) with `select` selected once listed.
    fn exact_tab_state(&self, folder: &Path, select: Vec<String>) -> TabState {
        let v = self.settings().view;
        TabState {
            id: TabId(0),
            location: folder.to_string_lossy().into_owned(),
            view: self
                .shared
                .folder_view(folder)
                .unwrap_or_else(|| ViewState::from_settings(&v)),
            cursor: select.first().cloned(),
            selected: select,
            ..TabState::default()
        }
    }
}

// ───────────────────────── Open/Save panel jump ─────────────────────────

/// What kind of system file panel an element is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PanelKind {
    /// An Open (or Choose, or Upload) panel.
    Open,
    /// A Save (or Export) panel.
    Save,
}

/// What the shell read about a window or sheet through Accessibility.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PanelProbe {
    /// `AXRole` (`AXWindow`, `AXSheet`).
    pub role: String,
    /// `AXSubrole` (`AXDialog`, `AXStandardWindow`).
    pub subrole: String,
    /// `AXIdentifier` (AppKit names its panels `open-panel` / `save-panel`).
    pub identifier: String,
    /// The titles of the buttons inside it.
    pub buttons: Vec<String>,
    /// The roles found inside it (a few levels deep): `AXOutline`,
    /// `AXBrowser`, `AXTextField`…
    pub inner_roles: Vec<String>,
}

fn has_role(p: &PanelProbe, role: &str) -> bool {
    p.inner_roles.iter().any(|r| r == role)
}

fn has_button(p: &PanelProbe, titles: &[&str]) -> bool {
    p.buttons
        .iter()
        .any(|b| titles.iter().any(|t| b.trim().eq_ignore_ascii_case(t)))
}

/// Decides whether a window or sheet is a system Open or Save panel.
///
/// AppKit's panels say so in `AXIdentifier`. Without one, a window or sheet
/// with a Cancel button and a file browser (an outline, browser or list
/// view) counts, which covers panels an app hosts in a sheet. A Save panel
/// also has a name field or a Save/Export button.
pub fn classify_panel(p: &PanelProbe) -> Option<PanelKind> {
    let id = p.identifier.to_ascii_lowercase();
    if id.contains("save-panel") || id == "nssavepanel" {
        return Some(PanelKind::Save);
    }
    if id.contains("open-panel") || id == "nsopenpanel" {
        return Some(PanelKind::Open);
    }
    if p.role != "AXWindow" && p.role != "AXSheet" {
        return None;
    }
    let browser = ["AXOutline", "AXBrowser"].iter().any(|r| has_role(p, r));
    if !browser || !has_button(p, &["Cancel"]) {
        return None;
    }
    if has_button(p, &["Save", "Export", "Save As", "Move", "Duplicate"]) {
        Some(PanelKind::Save)
    } else {
        Some(PanelKind::Open)
    }
}

/// A rectangle in global screen coordinates with the origin at the top left
/// of the main screen (as Accessibility reports frames).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct ScreenRect {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

/// Where the floating "Go to …" button sits for a panel: just above the
/// panel's top-right corner, or inside that corner when there is no room
/// above it on its screen. Returns the button's top-left corner.
pub fn panel_button_origin(
    panel: ScreenRect,
    button_w: f64,
    button_h: f64,
    screen: ScreenRect,
) -> (f64, f64) {
    const GAP: f64 = 6.0;
    let mut x = panel.x + panel.w - button_w;
    let mut y = panel.y - button_h - GAP;
    if y < screen.y {
        // No room above (a sheet under the menu bar): sit inside the corner.
        y = panel.y + GAP;
        x -= GAP * 2.0;
    }
    x = x.clamp(screen.x, (screen.x + screen.w - button_w).max(screen.x));
    y = y.clamp(screen.y, (screen.y + screen.h - button_h).max(screen.y));
    (x, y)
}

/// The system-wide shortcut for `panel.jump_here`, from its key-cap text
/// (`⌃⌥⌘G`). Only a single chord with ⌘, ⌃ or ⌥ qualifies: a sequence can't
/// be a system hotkey, and a plain or Shift-only key would take that key
/// away from typing in every app.
pub fn global_hotkey(keys: &str) -> Option<fstudio_model::KeyChord> {
    let b = fstudio_keymap::Binding::parse(keys.trim(), fstudio_keymap::When::Always)?;
    if b.keys.len() != 1 {
        return None;
    }
    let c = b.first_chord()?;
    (c.mods.cmd || c.mods.ctrl || c.mods.alt).then_some(c)
}

/// The floating button's title for the active pane's folder.
pub fn panel_button_title(folder: &str) -> String {
    let name = Path::new(folder)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| folder.to_string());
    format!("Go to “{name}”")
}

/// The text to type into the panel's "Go to Folder" field: the folder's full
/// path. A trailing slash makes the panel show the folder rather than select
/// it in its parent.
pub fn panel_jump_text(folder: &str) -> String {
    if folder.ends_with('/') {
        folder.to_string()
    } else {
        format!("{folder}/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN: &str = "app.filestudio.mac.debug";

    #[test]
    fn viewer_defaults_to_finder_when_unset() {
        let s = viewer_status(None, None, OWN);
        assert_eq!(s.state, ViewerState::Finder);
        assert!(s.can_make_default && !s.can_restore_finder);
        assert!(!s.reveal_needs_finder_event);
        let s = viewer_status(Some(""), Some("com.apple.Finder"), OWN);
        assert_eq!(s.state, ViewerState::Finder);
    }

    #[test]
    fn viewer_this_app_and_other_copy() {
        let s = viewer_status(Some(OWN), Some("App.FileStudio.Mac.Debug"), OWN);
        assert_eq!(s.state, ViewerState::ThisApp);
        assert!(!s.can_make_default && s.can_restore_finder);
        assert!(s.reveal_needs_finder_event);
        // The Release app is the viewer, seen from Debug: Debug must not
        // claim it is the viewer, and offers to take over.
        let s = viewer_status(Some("app.filestudio.mac"), Some("app.filestudio.mac"), OWN);
        assert_eq!(s.state, ViewerState::OtherCopy);
        assert!(s.can_make_default && s.can_restore_finder);
        assert!(s.headline.contains("installed copy"));
        let s = viewer_status(Some(OWN), Some(OWN), "app.filestudio.mac");
        assert_eq!(s.state, ViewerState::OtherCopy);
        assert!(s.headline.contains("File Studio Debug"));
    }

    #[test]
    fn viewer_other_app_and_folder_notes() {
        let s = viewer_status(
            Some("com.binarynights.ForkLift"),
            Some("com.binarynights.forklift"),
            OWN,
        );
        assert_eq!(s.state, ViewerState::OtherApp);
        // The viewer is this app but macOS kept folders for Finder: still
        // "this app", with a note, and Switch Back is offered.
        let s = viewer_status(Some(OWN), None, OWN);
        assert_eq!(s.state, ViewerState::ThisApp);
        assert!(s.detail.contains("still open in Finder"));
        assert!(!s.can_make_default && s.can_restore_finder);
        // Finder is the viewer but folders come here (an older macOS that
        // allowed it): Switch Back is still offered, to undo that half.
        let s = viewer_status(None, Some(OWN), OWN);
        assert_eq!(s.state, ViewerState::Finder);
        assert!(s.can_restore_finder && s.can_make_default);
        assert!(!s.reveal_needs_finder_event);
        assert!(s.detail.contains("come here too"));
    }

    fn item(p: &str, is_dir: bool) -> OpenItem {
        OpenItem {
            path: PathBuf::from(p),
            is_dir,
        }
    }

    #[test]
    fn plan_opens_folders_and_reveals_files() {
        let plan = plan_system_open(&[item("/a/b", true), item("/a/c.txt", false)], false);
        assert_eq!(
            plan,
            vec![
                OpenStep {
                    folder: "/a/b".into(),
                    select: vec![]
                },
                OpenStep {
                    folder: "/a".into(),
                    select: vec!["c.txt".into()]
                },
            ]
        );
    }

    #[test]
    fn plan_reveal_groups_by_folder_in_order() {
        let plan = plan_system_open(
            &[
                item("/x/one", false),
                item("/y/dir", true),
                item("/x/two", false),
                item("/x/one", false),
            ],
            true,
        );
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].folder, PathBuf::from("/x"));
        assert_eq!(plan[0].select, vec!["one", "two"]);
        assert_eq!(plan[1].folder, PathBuf::from("/y"));
        assert_eq!(plan[1].select, vec!["dir"]);
    }

    #[test]
    fn plan_keeps_names_exact() {
        // NFD "é" and a name that differs only by case stay as given (F-017).
        let nfd = "/p/Cafe\u{301}.txt";
        let plan = plan_system_open(&[item(nfd, false), item("/p/cafe\u{301}.TXT", false)], true);
        assert_eq!(plan[0].select, vec!["Cafe\u{301}.txt", "cafe\u{301}.TXT"]);
    }

    #[test]
    fn plan_root_and_trailing_slash() {
        let plan = plan_system_open(&[item("/", true)], true);
        assert_eq!(plan[0].folder, PathBuf::from("/"));
        assert!(plan[0].select.is_empty());
        let plan = plan_system_open(&[item("/a/b/", true)], true);
        assert_eq!(plan[0].folder, PathBuf::from("/a"));
        assert_eq!(plan[0].select, vec!["b"]);
    }

    #[test]
    fn missing_text() {
        assert_eq!(missing_message(&[]), None);
        assert_eq!(
            missing_message(&["/a/gone.txt".into()]).unwrap(),
            "Couldn't find “gone.txt”"
        );
        assert_eq!(
            missing_message(&["/a".into(), "/b".into()]).unwrap(),
            "Couldn't find 2 items"
        );
    }

    fn probe(role: &str, id: &str, buttons: &[&str], roles: &[&str]) -> PanelProbe {
        PanelProbe {
            role: role.into(),
            subrole: "AXDialog".into(),
            identifier: id.into(),
            buttons: buttons.iter().map(|s| s.to_string()).collect(),
            inner_roles: roles.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn panels_by_identifier() {
        assert_eq!(
            classify_panel(&probe("AXWindow", "open-panel", &[], &[])),
            Some(PanelKind::Open)
        );
        assert_eq!(
            classify_panel(&probe("AXSheet", "save-panel", &[], &[])),
            Some(PanelKind::Save)
        );
    }

    #[test]
    fn panels_by_shape() {
        let open = probe(
            "AXSheet",
            "",
            &["Cancel", "Open"],
            &["AXOutline", "AXButton"],
        );
        assert_eq!(classify_panel(&open), Some(PanelKind::Open));
        let upload = probe("AXWindow", "", &["Cancel", "Upload"], &["AXBrowser"]);
        assert_eq!(classify_panel(&upload), Some(PanelKind::Open));
        let save = probe(
            "AXWindow",
            "",
            &["Cancel", "Save"],
            &["AXOutline", "AXTextField"],
        );
        assert_eq!(classify_panel(&save), Some(PanelKind::Save));
        // An ordinary alert, a settings window, or a list without Cancel.
        let alert = probe("AXSheet", "", &["Cancel", "Delete"], &["AXStaticText"]);
        assert_eq!(classify_panel(&alert), None);
        let no_cancel = probe("AXWindow", "", &["Done"], &["AXOutline"]);
        assert_eq!(classify_panel(&no_cancel), None);
        let button = probe("AXButton", "", &["Cancel"], &["AXOutline"]);
        assert_eq!(classify_panel(&button), None);
    }

    #[test]
    fn button_sits_above_or_inside_the_panel() {
        let screen = ScreenRect {
            x: 0.0,
            y: 25.0,
            w: 1920.0,
            h: 1055.0,
        };
        let panel = ScreenRect {
            x: 400.0,
            y: 200.0,
            w: 800.0,
            h: 500.0,
        };
        assert_eq!(
            panel_button_origin(panel, 160.0, 28.0, screen),
            (1040.0, 166.0)
        );
        // Flush with the menu bar: inside the corner instead.
        let top = ScreenRect { y: 30.0, ..panel };
        assert_eq!(
            panel_button_origin(top, 160.0, 28.0, screen),
            (1028.0, 36.0)
        );
        // Off the right edge: pulled back onto the screen.
        let right = ScreenRect { x: 1800.0, ..panel };
        assert_eq!(panel_button_origin(right, 160.0, 28.0, screen).0, 1760.0);
    }

    #[test]
    fn hotkeys_need_a_command_modifier_and_one_chord() {
        let c = global_hotkey("⌃⌥⌘G").unwrap();
        assert!(c.mods.ctrl && c.mods.alt && c.mods.cmd && !c.mods.shift);
        assert_eq!(c.key, fstudio_model::Key::Char('g'));
        assert!(global_hotkey("⇧⌘F5").is_some());
        assert!(global_hotkey("G").is_none());
        assert!(global_hotkey("⇧G").is_none());
        assert!(global_hotkey("g g").is_none());
        assert!(global_hotkey("").is_none());
    }

    #[test]
    fn jump_text_and_title() {
        assert_eq!(
            panel_jump_text("/Users/me/Down loads"),
            "/Users/me/Down loads/"
        );
        assert_eq!(panel_jump_text("/"), "/");
        assert_eq!(
            panel_button_title("/Users/me/Downloads"),
            "Go to “Downloads”"
        );
        assert_eq!(panel_button_title("/"), "Go to “/”");
    }

    #[test]
    fn settings_default_off() {
        let s = IntegrationSettings::default();
        assert!(!s.panel_jump);
        assert!(s.panel_jump_button);
        assert_eq!(s.folder_opens_in, FolderOpensIn::NewTab);
    }
}

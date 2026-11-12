//! No silent moves of important folders (F-023, after the 2026-11-12
//! incident, when dragging sidebar favourites moved `~/Applications`,
//! `~/Downloads` and a project folder without anyone noticing).
//!
//! Every move the user starts (a drop, F6, cut/paste, Paste as Move, New
//! Folder with Selection) goes through [`Engine::guard_move`]. When any item
//! being moved is *protected*, nothing is queued: the core returns
//! [`ShellRequest::ConfirmMove`] and keeps the move pending under a token.
//! The shell asks (Cancel is the default) and calls
//! [`Engine::confirm_move`] or [`Engine::cancel_move`]. The confirmation
//! can't be turned off; only the list of protected folders is a setting
//! (`operations.protected_folders`).
//!
//! Protected are:
//! - the folders in `operations.protected_folders` (`~` is the home folder):
//!   by default the home folder, its standard folders and the system folders;
//! - the root of any volume;
//! - any folder an open tab shows, or that contains one.
//!
//! Every move that is queued also answers with [`ShellRequest::Moved`]: a
//! toast with Undo, so a move is never invisible.

use std::path::{Path, PathBuf};

use crate::confirm::{PendingAction, PendingOp};
use crate::engine::Engine;
use crate::error::Result;
use crate::outcome::{Outcome, ShellRequest};

/// The protected folders a new install starts with.
pub fn default_protected_folders() -> Vec<String> {
    [
        "~",
        "~/Desktop",
        "~/Documents",
        "~/Downloads",
        "~/Applications",
        "~/Library",
        "~/Movies",
        "~/Music",
        "~/Pictures",
        "~/Public",
        "/Applications",
        "/System",
        "/Library",
        "/Users",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Why a folder is protected, in words for the confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Why {
    Home,
    Standard,
    System,
    Listed,
    VolumeRoot,
    OpenInTab,
}

impl Why {
    fn text(self) -> &'static str {
        match self {
            Why::Home => "It’s your home folder.",
            Why::Standard => "It’s one of your standard folders.",
            Why::System => "It’s a system folder.",
            Why::Listed => "It’s on your list of protected folders.",
            Why::VolumeRoot => "It’s the top folder of a disk.",
            Why::OpenInTab => "A tab is showing it, or a folder inside it.",
        }
    }
}

/// `~` and `~/x` against `home`.
fn expand(s: &str, home: &Path) -> PathBuf {
    if s == "~" {
        home.to_path_buf()
    } else if let Some(rest) = s.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(s)
    }
}

/// The comparison key of a path: its folder resolved (not the item itself),
/// without a trailing `/`, folded (NFC + lower case, F-017).
fn key(p: &Path) -> String {
    let resolved = match (p.parent(), p.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => {
            std::fs::canonicalize(parent)
                .map(|c| c.join(name))
                .unwrap_or_else(|_| p.to_path_buf())
        }
        _ => p.to_path_buf(),
    };
    let s = resolved.to_string_lossy();
    let t = s.trim_end_matches('/');
    fstudio_path::keys::fold(if t.is_empty() { "/" } else { t })
}

/// Is `inner` the same as `outer` or inside it (both keys)?
fn same_or_inside(inner: &str, outer: &str) -> bool {
    inner == outer
        || (outer == "/" && inner.starts_with('/'))
        || inner.starts_with(&format!("{outer}/"))
}

/// The pure rule, given the keys of everything that protects.
pub(crate) fn why_protected(
    item: &str,
    listed: &[(String, Why)],
    volume_roots: &[String],
    open: &[String],
) -> Option<Why> {
    if let Some((_, w)) = listed.iter().find(|(k, _)| k == item) {
        return Some(*w);
    }
    if volume_roots.iter().any(|r| r == item) {
        return Some(Why::VolumeRoot);
    }
    if open.iter().any(|loc| same_or_inside(loc, item)) {
        return Some(Why::OpenInTab);
    }
    None
}

/// The name shown for a path.
fn name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}

/// "Moved “Downloads” to “Import Studio”", "Moved 3 items to “Archive”".
pub(crate) fn moved_text(sources: &[PathBuf], dest: &Path) -> String {
    match sources {
        [one] => format!("Moved “{}” to “{}”", name(one), name(dest)),
        _ => format!("Moved {} items to “{}”", sources.len(), name(dest)),
    }
}

impl Engine {
    fn protection_lists(&self) -> (Vec<(String, Why)>, Vec<String>, Vec<String>) {
        let home = self.shared.home.clone();
        let defaults = default_protected_folders();
        let listed = self
            .settings()
            .operations
            .protected_folders
            .iter()
            .map(|s| {
                let why = if s == "~" {
                    Why::Home
                } else if s.starts_with("~/") && defaults.contains(s) {
                    Why::Standard
                } else if defaults.contains(s) {
                    Why::System
                } else {
                    Why::Listed
                };
                (key(&expand(s, &home)), why)
            })
            .collect();
        let mut roots = vec!["/".to_string()];
        roots.extend(self.volumes().iter().map(|v| key(Path::new(&v.mount_path))));
        let open = self
            .model
            .lock()
            .tabs
            .values()
            .map(|t| key(&t.location_path()))
            .collect();
        (listed, roots, open)
    }

    /// Why moving `path` needs a yes, or `None` when it doesn't (F-023).
    /// Metadata only (`realpath` of its folder).
    pub fn move_protection(&self, path: &str) -> Option<String> {
        let (listed, roots, open) = self.protection_lists();
        why_protected(&key(Path::new(path)), &listed, &roots, &open).map(|w| w.text().to_string())
    }

    /// Checks a move before it is queued. `None`: go ahead. `Some`: an outcome
    /// asking the shell to confirm; the move waits under a one-shot token
    /// with the identity of every item (S1).
    pub(crate) fn guard_move(
        &self,
        sources: &[PathBuf],
        dest: &Path,
        pending: impl FnOnce() -> PendingAction,
    ) -> Option<Outcome> {
        let (listed, roots, open) = self.protection_lists();
        let hit = sources
            .iter()
            .find_map(|s| why_protected(&key(s), &listed, &roots, &open).map(|w| (s.clone(), w)))?;
        let (item, why) = hit;
        let title = match sources {
            [_] => format!("Move “{}” into “{}”?", name(&item), name(dest)),
            _ => format!(
                "Move {} items, including “{}”, into “{}”?",
                sources.len(),
                name(&item),
                name(dest)
            ),
        };
        let detail = format!(
            "{} Apps and other programs may stop finding it after a move.",
            why.text()
        );
        let action = pending();
        let tab = action.tab();
        let token = self.ask(action);
        let mut out = Outcome::shell(ShellRequest::ConfirmMove {
            token,
            title,
            detail,
            paths: sources
                .iter()
                .map(|s| s.to_string_lossy().into_owned())
                .collect(),
        });
        out.tab = tab;
        Some(out)
    }

    /// Runs a move now, or asks first when a protected folder is among
    /// `sources` (F-023). Either way only `op`'s exact request is queued.
    pub(crate) fn move_or_ask(
        &self,
        sources: &[PathBuf],
        dest: &Path,
        op: PendingOp,
    ) -> Result<Outcome> {
        let mut slot = Some(op);
        if let Some(ask) = self.guard_move(sources, dest, || {
            PendingAction::Op(slot.take().expect("taken once"))
        }) {
            return Ok(ask);
        }
        self.run_pending_op(slot.take().expect("not taken"), false)
    }

    /// The user said Move to a [`ShellRequest::ConfirmMove`]: queue it.
    /// The same as [`Engine::confirm`].
    pub fn confirm_move(&self, token: u64) -> Result<Outcome> {
        self.confirm(token)
    }

    /// The user cancelled a [`ShellRequest::ConfirmMove`]: forget it.
    pub fn cancel_move(&self, token: u64) {
        self.cancel_confirmation(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_covers_listed_roots_and_open_tabs() {
        let listed = vec![
            ("/users/me".to_string(), Why::Home),
            ("/users/me/downloads".to_string(), Why::Standard),
        ];
        let roots = vec!["/".to_string(), "/volumes/card".to_string()];
        let open = vec!["/users/me/projects/app/src".to_string()];
        assert_eq!(
            why_protected("/users/me", &listed, &roots, &open),
            Some(Why::Home)
        );
        assert_eq!(
            why_protected("/users/me/downloads", &listed, &roots, &open),
            Some(Why::Standard)
        );
        assert_eq!(
            why_protected("/volumes/card", &listed, &roots, &open),
            Some(Why::VolumeRoot)
        );
        // An ancestor of an open tab, and the open folder itself.
        assert_eq!(
            why_protected("/users/me/projects", &listed, &roots, &open),
            Some(Why::OpenInTab)
        );
        assert_eq!(
            why_protected("/users/me/projects/app/src", &listed, &roots, &open),
            Some(Why::OpenInTab)
        );
        // Not a prefix by name only.
        assert_eq!(
            why_protected("/users/me/proj", &listed, &roots, &open),
            None
        );
        // Inside a standard folder is fine.
        assert_eq!(
            why_protected("/users/me/downloads/x.zip", &listed, &roots, &open),
            None
        );
    }

    #[test]
    fn moved_text_names_items() {
        let d = Path::new("/a/Import Studio");
        assert_eq!(
            moved_text(&[PathBuf::from("/a/Downloads")], d),
            "Moved “Downloads” to “Import Studio”"
        );
        assert_eq!(
            moved_text(&[PathBuf::from("/a/x"), PathBuf::from("/a/y")], d),
            "Moved 2 items to “Import Studio”"
        );
    }

    #[test]
    fn keys_fold_and_resolve() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("Café")).unwrap();
        let nfd = d.path().join("Cafe\u{301}");
        let nfc = d.path().join("Caf\u{e9}");
        assert_eq!(key(&nfd), key(&nfc));
        assert_eq!(key(&d.path().join("x/")), key(&d.path().join("x")));
    }
}

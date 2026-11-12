//! What the user is told after a file operation (S11 of the safety audit):
//! every trash, undo and redo answers with a toast, so nothing changes on
//! disk unseen. Undo and redo are app-global (one journal stack), so their
//! toasts name the folder they touched when this window doesn't show it.

use std::path::{Path, PathBuf};

use fstudio_ops::OpRequest;

use crate::confirm::display_name;
use crate::engine::Engine;
use crate::ids::TabId;
use crate::outcome::ShellRequest;

fn quoted(p: &Path) -> String {
    format!("“{}”", display_name(p))
}

/// "“a.txt”" or "3 items".
fn what(paths: &[PathBuf]) -> String {
    match paths {
        [one] => quoted(one),
        _ => format!("{} items", paths.len()),
    }
}

/// "Moved “a.txt” to the Trash", "Moved 3 items to the Trash".
pub(crate) fn trashed_text(paths: &[PathBuf]) -> String {
    format!("Moved {} to the Trash", what(paths))
}

/// "Deleted “a.txt” immediately", "Deleted 3 items immediately".
pub(crate) fn deleted_text(paths: &[PathBuf]) -> String {
    format!("Deleted {} immediately", what(paths))
}

fn parent(p: &Path) -> Option<PathBuf> {
    p.parent().map(Path::to_path_buf)
}

/// What undoing `req` does, in words, and the folder it changes.
pub(crate) fn undo_text(req: &OpRequest, label: &str) -> (String, Option<PathBuf>) {
    match req {
        OpRequest::Trash { paths } => (
            format!("Put back {}", what(paths)),
            paths.first().and_then(|p| parent(p)),
        ),
        OpRequest::Move { sources, .. } => (
            format!("Moved {} back", what(sources)),
            sources.first().and_then(|p| parent(p)),
        ),
        OpRequest::Rename { path, new_name } => (
            format!("Renamed “{new_name}” back to {}", quoted(path)),
            parent(path),
        ),
        OpRequest::Copy { sources, dest_dir } => (
            match sources.as_slice() {
                [one] => format!("Moved the copy of {} to the Trash", quoted(one)),
                _ => format!("Moved {} copies to the Trash", sources.len()),
            },
            Some(dest_dir.clone()),
        ),
        OpRequest::Duplicate { paths } => (
            match paths.as_slice() {
                [one] => format!("Moved the duplicate of {} to the Trash", quoted(one)),
                _ => format!("Moved {} duplicates to the Trash", paths.len()),
            },
            paths.first().and_then(|p| parent(p)),
        ),
        OpRequest::NewFolder { parent: dir, name } | OpRequest::NewFile { parent: dir, name } => {
            (format!("Moved “{name}” to the Trash"), Some(dir.clone()))
        }
        _ => (format!("Undid {label}"), None),
    }
}

/// What redoing `req` does, in words ("Redid: Move of “Downloads”"), and
/// the folder it changes.
pub(crate) fn redo_text(req: &OpRequest, label: &str) -> (String, Option<PathBuf>) {
    let folder = match req {
        OpRequest::Move { dest_dir, .. } | OpRequest::Copy { dest_dir, .. } => {
            Some(dest_dir.clone())
        }
        OpRequest::Trash { paths } | OpRequest::Duplicate { paths } => {
            paths.first().and_then(|p| parent(p))
        }
        OpRequest::Rename { path, .. } => parent(path),
        OpRequest::NewFolder { parent: dir, .. } | OpRequest::NewFile { parent: dir, .. } => {
            Some(dir.clone())
        }
        _ => None,
    };
    (format!("Redid: {label}"), folder)
}

/// Where a folder is on screen, relative to the window the user is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Seen {
    /// A pane of this window shows it.
    Here,
    /// Only another window shows it.
    OtherWindow,
    /// No window shows it.
    Nowhere,
}

/// " in “Projects”", " in “Projects” (another window)", or nothing when
/// this window shows the folder.
pub(crate) fn where_suffix(folder: &Path, seen: Seen) -> String {
    match seen {
        Seen::Here => String::new(),
        Seen::OtherWindow => format!(" in {} (another window)", quoted(folder)),
        Seen::Nowhere => format!(" in {}", quoted(folder)),
    }
}

impl Engine {
    /// Whether the window holding `tab` (or the active window) shows
    /// `folder` in one of its panes, another window does, or none.
    pub(crate) fn seen(&self, folder: &Path, tab: Option<TabId>) -> Seen {
        let want = std::fs::canonicalize(folder).ok();
        let same =
            |a: &Path| a == folder || (want.is_some() && std::fs::canonicalize(a).ok() == want);
        let m = self.model.lock();
        let here = tab
            .and_then(|t| m.find(t).map(|p| p.window))
            .or_else(|| m.active_window.and_then(|w| m.window_index(w)));
        let mut seen = Seen::Nowhere;
        for (wi, w) in m.windows.iter().enumerate() {
            for p in &w.panes {
                let Some(t) = p.tabs.get(p.active).and_then(|id| m.tabs.get(id)) else {
                    continue;
                };
                if same(&t.location_path()) {
                    if Some(wi) == here {
                        return Seen::Here;
                    }
                    seen = Seen::OtherWindow;
                }
            }
        }
        seen
    }

    /// A toast with a button that runs `command`.
    pub(crate) fn toast(text: String, button: &str, command: &str) -> ShellRequest {
        ShellRequest::Toast {
            text,
            button: Some(button.to_string()),
            command: Some(command.to_string()),
        }
    }

    /// The toast for an undo or redo: the text, plus where it happened when
    /// this window doesn't show that folder.
    pub(crate) fn history_toast(
        &self,
        (text, folder): (String, Option<PathBuf>),
        tab: Option<TabId>,
        button: &str,
        command: &str,
    ) -> ShellRequest {
        let suffix = folder
            .map(|f| where_suffix(&f, self.seen(&f, tab)))
            .unwrap_or_default();
        Self::toast(format!("{text}{suffix}"), button, command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_name_what_and_where() {
        let a = PathBuf::from("/h/Downloads");
        let b = PathBuf::from("/h/b.txt");
        assert_eq!(
            trashed_text(&[a.clone(), b.clone(), b.clone()]),
            "Moved 3 items to the Trash"
        );
        assert_eq!(
            trashed_text(std::slice::from_ref(&b)),
            "Moved “b.txt” to the Trash"
        );
        let mv = OpRequest::Move {
            sources: vec![a.clone()],
            dest_dir: "/h/Projects".into(),
        };
        assert_eq!(
            redo_text(&mv, "Move of “Downloads”"),
            (
                "Redid: Move of “Downloads”".to_string(),
                Some(PathBuf::from("/h/Projects"))
            )
        );
        let tr = OpRequest::Trash {
            paths: vec![a, b.clone(), b],
        };
        assert_eq!(undo_text(&tr, "").0, "Put back 3 items");
        assert_eq!(
            where_suffix(Path::new("/h/Projects"), Seen::Nowhere),
            " in “Projects”"
        );
        assert_eq!(
            where_suffix(Path::new("/h/Projects"), Seen::OtherWindow),
            " in “Projects” (another window)"
        );
        assert_eq!(where_suffix(Path::new("/h/Projects"), Seen::Here), "");
    }
}

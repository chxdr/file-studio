//! Questions the core asks before it acts, and the one-shot tokens that
//! answer them (S1 of the safety audit).
//!
//! When a command needs the user's yes (Move to Trash with the setting on,
//! Vim `x`, Delete Immediately, a move of a protected folder…), the core
//! works out **exactly** what it would do, stores that under a token
//! together with the identity (device and inode) of every item the
//! question names, and returns the question to the shell. The shell shows
//! it and calls [`Engine::confirm`] with the token, or
//! [`Engine::cancel_confirmation`].
//!
//! On yes the core acts only on the stored items, never on whatever is
//! selected by then, and only if every item is still the object the
//! question named. If anything changed, nothing is done and the outcome
//! says why. A token works once.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use fstudio_ops::OpRequest;

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::ids::TabId;
use crate::outcome::{Outcome, ShellRequest};

/// What identifies an item for "is it still the same thing?": its device and inode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ObjId {
    pub dev: u64,
    pub ino: u64,
}

/// The identity of the item at `p` (`lstat`; a symlink is not followed).
pub(crate) fn obj_id(p: &Path) -> Option<ObjId> {
    std::fs::symlink_metadata(p).ok().map(|m| ObjId {
        dev: m.dev(),
        ino: m.ino(),
    })
}

/// The name shown for a path.
pub(crate) fn display_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}

/// Each item with its identity now, for a question about them.
pub(crate) fn snapshot(paths: &[PathBuf]) -> Vec<(PathBuf, Option<ObjId>)> {
    paths.iter().map(|p| (p.clone(), obj_id(p))).collect()
}

/// The first item that is no longer the object it was when the question
/// was asked, as a sentence; `None` when every item is unchanged. An item
/// is changed when it is gone or replaced by another object.
pub(crate) fn first_change(items: &[(PathBuf, Option<ObjId>)]) -> Option<String> {
    for (p, was) in items {
        let now = obj_id(p);
        let same = match (was, now) {
            (Some(a), Some(b)) => *a == b,
            _ => false,
        };
        if !same {
            return Some(format!(
                "“{}” changed after you were asked, so nothing was done",
                display_name(p)
            ));
        }
        // S5: the same object under another spelling is not what was named.
        if let (Ok(real), Some(n)) = (fstudio_fs::on_disk_name(p), p.file_name())
            && real != n.to_string_lossy()
        {
            return Some(format!(
                "“{}” is now called “{real}”, so nothing was done",
                display_name(p)
            ));
        }
    }
    None
}

/// What happens to the list after a confirmed op is queued.
#[derive(Debug, Clone, Default)]
pub(crate) struct After {
    /// Select what follows the removed items in this tab (Trash, F6).
    pub remove_from: Option<TabId>,
    /// Select these names in this tab once they appear (paste, F6 target).
    pub select: Option<(TabId, Vec<String>)>,
    /// A cut was used up by this paste.
    pub clear_cut: bool,
    /// Start renaming this item once it appears (New Folder with Selection).
    pub begin_rename: Option<(TabId, String)>,
}

/// A request waiting for the user's yes.
#[derive(Debug, Clone)]
pub(crate) struct PendingOp {
    /// Exactly what to queue.
    pub req: OpRequest,
    /// The items the question named, with their identity then.
    pub items: Vec<(PathBuf, Option<ObjId>)>,
    /// The tab it was asked from.
    pub tab: Option<TabId>,
    /// List updates once it is queued.
    pub after: After,
    /// The toast to show once it has finished, if every item succeeded
    /// (failures always get one, built from the outcome).
    pub toast: Option<ShellRequest>,
}

/// Something waiting for the user's yes.
#[derive(Debug, Clone)]
pub(crate) enum PendingAction {
    /// Queue a request.
    Op(Box<PendingOp>),
    /// Redo this op (a move of a protected folder), if it is still the top
    /// of the redo stack; redo checks the items itself.
    Redo(fstudio_ops::OpId, Option<TabId>),
    /// Undo this op (still the top of the undo stack), recreating the
    /// folders it puts items back into (S14).
    UndoRecreating(fstudio_ops::OpId, Option<TabId>),
}

impl PendingAction {
    /// The tab the question was asked from.
    pub(crate) fn tab(&self) -> Option<TabId> {
        match self {
            PendingAction::Op(op) => op.tab,
            PendingAction::Redo(_, tab) | PendingAction::UndoRecreating(_, tab) => *tab,
        }
    }
}

/// The open questions, by token.
#[derive(Debug, Default)]
pub(crate) struct Pending {
    next: u64,
    open: HashMap<u64, PendingAction>,
}

impl Engine {
    /// Stores an action under a fresh one-shot token.
    pub(crate) fn ask(&self, action: PendingAction) -> u64 {
        let mut p = self.pending.lock();
        p.next += 1;
        let token = p.next;
        p.open.insert(token, action);
        token
    }

    /// The user said yes to a question carrying `token` (Move to Trash,
    /// Delete Immediately, a protected move…). Acts only on the items the
    /// question named, after checking each is still the same object;
    /// otherwise nothing happens and the outcome's message says why. A
    /// token works once. Blocks on `lstat` of each item.
    pub fn confirm(&self, token: u64) -> Result<Outcome> {
        let action = self
            .pending
            .lock()
            .open
            .remove(&token)
            .ok_or_else(|| CoreError::NotFound(format!("question {token}")))?;
        match action {
            PendingAction::Op(op) => self.run_pending_op(*op, true),
            PendingAction::Redo(op, tab) => self.redo_op(Some(op), tab),
            PendingAction::UndoRecreating(op, tab) => self.undo_run(tab, Some(op)),
        }
    }

    /// The user said no (or closed the question): forget it.
    pub fn cancel_confirmation(&self, token: u64) {
        self.pending.lock().open.remove(&token);
    }

    /// Queues a request and applies its list updates. With `check` (a
    /// confirmed question), refuses unless every item is unchanged.
    pub(crate) fn run_pending_op(&self, op: PendingOp, check: bool) -> Result<Outcome> {
        if check && let Some(why) = first_change(&op.items) {
            let mut out = Outcome::message(why);
            out.tab = op.tab;
            return Ok(out);
        }
        let removed: Vec<PathBuf> = op.items.iter().map(|(p, _)| p.clone()).collect();
        let mut reselect = None;
        if let Some(t) = op.after.remove_from.and_then(|t| self.tab(t)) {
            t.select_after_removing(&removed);
            reselect = Some((t.id(), t.selection_generation()));
        }
        // The toast waits for the outcome: "Moved “locked” to the Trash"
        // only if it really went.
        let plan = crate::feedback::ToastPlan {
            subject: crate::feedback::Subject::Op(op.req.clone()),
            success: op.toast,
            reselect,
        };
        let id = self.enqueue_toasting(op.req, plan, op.tab)?;
        if op.after.clear_cut {
            self.clear_cut();
        }
        if let Some((t, names)) = op.after.select
            && let Some(t) = self.tab(t)
        {
            t.select_when_present(names);
        }
        let mut out = Outcome::op(id);
        out.tab = op.tab;
        if let Some((t, name)) = op.after.begin_rename
            && let Some(tab) = self.tab(t)
        {
            tab.select_when_present(vec![name.clone()]);
            out.shell = Some(ShellRequest::BeginRename { tab: t, name });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replaced_item_counts_as_changed() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.txt");
        std::fs::write(&p, b"1").unwrap();
        let snap = snapshot(std::slice::from_ref(&p));
        assert_eq!(first_change(&snap), None);
        // Another file at the same path is another object.
        std::fs::rename(&p, d.path().join("old.txt")).unwrap();
        std::fs::write(&p, b"2").unwrap();
        assert!(first_change(&snap).is_some());
        std::fs::remove_file(&p).unwrap();
        assert!(first_change(&snap).is_some());
    }
}

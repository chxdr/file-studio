//! Crash recovery: brings an interrupted op's items back to a state the
//! engine can continue from. Removes only our own temporary files: the path
//! must be recorded in the item's journal row *and* be named
//! `.<name>.fstudio-partial`.

use std::io;

use crate::journal::{ItemRow, ItemState, Journal, OpRow};
use crate::meta;
use crate::names::is_partial_name;
use crate::remove;
use crate::sys;
use crate::types::{OpKind, Result};

/// Recovers every unfinished item of `op` and saves the result.
pub fn recover(journal: &Journal, op: &OpRow) -> Result<()> {
    let kind = op.request.kind();
    for mut it in journal.load_items(op.id)? {
        if it.state.is_terminal() {
            continue;
        }
        let before = it.clone();
        recover_item(kind, &mut it)?;
        if it != before {
            journal.save_item(&it)?;
        }
    }
    Ok(())
}

/// Drops our temporary file for an item, if the journal recorded one.
fn drop_partial(it: &mut ItemRow) -> io::Result<()> {
    if let Some(p) = it.partial.take()
        && is_partial_name(&p)
    {
        remove::remove_partial(&p)?;
    }
    it.partial_ino = None;
    Ok(())
}

/// Is the recorded partial now sitting at the destination (the rename landed)?
fn rename_landed(it: &ItemRow) -> bool {
    match (&it.dest, it.partial_ino) {
        (Some(d), Some(ino)) => meta::lstat(d).is_ok_and(|m| meta::ident(&m).ino == ino),
        _ => false,
    }
}

fn placed(it: &mut ItemRow) {
    it.state = ItemState::Placed;
    it.created = true;
    it.dest_ident = it
        .dest
        .as_deref()
        .and_then(|d| meta::lstat(d).ok())
        .map(|m| meta::ident(&m));
    it.partial = None;
}

fn recover_item(kind: OpKind, it: &mut ItemRow) -> io::Result<()> {
    match it.state {
        ItemState::Placed => return Ok(()),
        ItemState::Pending | ItemState::Conflict => {
            drop_partial(it)?;
            it.state = ItemState::Pending;
            return Ok(());
        }
        _ => {}
    }
    // InProgress, Copied or Verified.
    match kind {
        OpKind::Trash | OpKind::DeleteImmediately => {
            it.state = if meta::exists(&it.source) {
                ItemState::Pending
            } else {
                ItemState::Done
            };
        }
        OpKind::NewFolder | OpKind::NewFile => {
            if rename_landed(it) {
                placed(it);
                it.state = ItemState::Done;
            } else {
                drop_partial(it)?;
                it.state = ItemState::Pending;
            }
        }
        _ => {
            // A case-only rename parks the item under a hidden name.
            if let Some(tmp) = it.partial.clone()
                && !is_partial_name(&tmp)
            {
                if meta::exists(&tmp) {
                    let to = match &it.dest {
                        Some(d) if !meta::exists(d) => d.clone(),
                        _ => it.source.clone(),
                    };
                    sys::rename_excl(&tmp, &to)?;
                    if Some(&to) == it.dest.as_ref() {
                        placed(it);
                        it.state = ItemState::Done;
                        return Ok(());
                    }
                }
                it.partial = None;
                it.state = ItemState::Pending;
                return Ok(());
            }
            if it.partial.is_none() && it.partial_ino.is_none() {
                // A same-volume move (one rename) or a Replace that was trashing.
                let src_there = meta::exists(&it.source);
                let dest_there = it.dest.as_deref().is_some_and(meta::exists);
                it.state = if !src_there && dest_there {
                    placed(it);
                    ItemState::Done
                } else {
                    ItemState::Pending
                };
                return Ok(());
            }
            if rename_landed(it) {
                // A placed folder that isn't expanded yet is expanded when it runs.
                placed(it);
            } else {
                drop_partial(it)?;
                it.state = ItemState::Pending;
            }
        }
    }
    Ok(())
}

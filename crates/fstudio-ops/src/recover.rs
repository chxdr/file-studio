//! Crash recovery: brings an interrupted op's items back to a state the
//! engine can continue from. Removes only our own temporary files: the path
//! must be recorded in the item's journal row *and* be named
//! `.<name>.fstudio-partial`.

use std::io;

use crate::journal::{ItemRow, ItemState, Journal, OpRow};
use crate::meta;
use crate::names::is_partial_name;
use crate::names::partial_path;
use crate::remove;
use crate::sys;
use crate::types::{OpKind, Result};

/// Recovers every unfinished item of `op` and saves the result.
pub fn recover(journal: &Journal, op: &OpRow) -> Result<()> {
    if matches!(op.request, crate::types::OpRequest::RenameMany { .. }) {
        return crate::batch_rename::recover(journal, op);
    }
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

/// Which volume the folder `p` is in, if it can be reached: a key derived
/// from the volume's UUID, which stays the same when a card is pulled out and
/// put back. `st_dev` doesn't: macOS gives a re-attached card a new number
/// and hands the old one to the next volume that mounts. Only volumes with no
/// UUID (some network shares) fall back to `st_dev`.
pub(crate) fn folder_dev(p: &std::path::Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    let d = p.parent()?;
    let m = meta::lstat(d).ok().filter(|m| m.is_dir())?;
    Some(match sys::volume_uuid(d) {
        Some(uuid) => volume_key(&uuid),
        None => m.dev(),
    })
}

/// A volume UUID as the `u64` the journal stores in `partial_dev`. The top
/// bit is set, so a key can never equal an `st_dev` fallback (those are far
/// below 2^63).
fn volume_key(uuid: &[u8; 16]) -> u64 {
    xxhash_rust::xxh3::xxh3_64(uuid) | (1 << 63)
}

/// Is `p` (one of our temporaries) known to be gone? Only if its folder is
/// there, on the volume it was written on (`dev`, when recorded), and it
/// isn't. A folder that can't be reached proves nothing (its volume was
/// pulled out), and neither does an empty mount-point folder left behind on
/// the startup disk, so the journal keeps the record.
pub(crate) fn partial_is_gone(p: &std::path::Path, dev: Option<u64>) -> bool {
    match meta::lstat(p) {
        Ok(_) => false,
        Err(e) if e.kind() == io::ErrorKind::NotFound => match (folder_dev(p), dev) {
            (Some(now), Some(then)) => now == then,
            (Some(_), None) => true,
            (None, _) => false,
        },
        Err(_) => false,
    }
}

/// Tries to remove a failed item's temporary file through the one removal
/// path, and says whether it is gone (see [`partial_is_gone`]).
pub(crate) fn drop_failed_partial(p: &std::path::Path, dev: Option<u64>) -> bool {
    if !is_partial_name(p) {
        return true;
    }
    let _ = remove::remove_partial(p);
    partial_is_gone(p, dev)
}

/// `._<name>` next to `p`: where macOS keeps `p`'s extended attributes on
/// volumes that have none of their own. For one of our temporaries it is
/// also one of our temporary names.
fn apple_double(p: &std::path::Path) -> Option<std::path::PathBuf> {
    let name = p.file_name()?.to_str()?;
    let c = p.with_file_name(format!("._{name}"));
    is_partial_name(&c).then_some(c)
}

/// A temporary file removed by [`sweep_leftovers`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweptPartial {
    /// Where it was.
    pub path: std::path::PathBuf,
    /// Its size in bytes.
    pub bytes: u64,
}

/// Removes temporary files that failed items of finished ops left behind
/// because their volume was gone at the time (a card pulled out at 74 %).
/// Called at launch and whenever volumes change. A file is removed only if
/// the journal records it for that item **and** its name is exactly the
/// item's own temporary name (`.<name>.<item>.fstudio-partial` next to the
/// item's destination); anything else is never touched. Records whose
/// folder still can't be reached are kept for next time.
pub fn sweep_leftovers(journal: &Journal) -> Result<Vec<SweptPartial>> {
    let mut swept = Vec::new();
    for mut it in journal.leftover_partials()? {
        let Some(p) = it.partial.clone() else {
            continue;
        };
        let ours = is_partial_name(&p)
            && it
                .dest
                .as_deref()
                .is_some_and(|d| partial_path(d, it.id) == p);
        if !ours {
            // A batch-rename item left under its temporary name (the user's
            // own item, never removed): put it back under a visible name if
            // it is still there; keep the record while it can't be.
            if crate::batch_rename::stranded(&it) {
                if let Some(at) = crate::batch_rename::rescue_stranded(&it) {
                    it.partial = None;
                    it.source = at;
                    journal.save_item(&it)?;
                }
                continue;
            }
            // Not one of ours by name (a case-only rename's parking name):
            // never removed here; forget it so it isn't looked at again.
            it.partial = None;
            journal.save_item(&it)?;
            continue;
        }
        let bytes = meta::lstat(&p).map(|m| m.len()).unwrap_or(0);
        // Only on the volume it was written on: a same-named file in an
        // empty mount-point folder is not ours.
        let here = folder_dev(&p);
        let same_volume = matches!((here, it.partial_dev), (Some(a), Some(b)) if a == b)
            || it.partial_dev.is_none();
        let existed = meta::exists(&p) && same_volume;
        if existed {
            let _ = remove::remove_partial(&p);
        }
        if partial_is_gone(&p, it.partial_dev) {
            // On volumes without native extended attributes (exFAT, FAT)
            // macOS keeps them in an AppleDouble file, `._<name>`; a card
            // pulled out mid-copy can leave ours behind on its own.
            let companion = apple_double(&p);
            let had_companion = companion
                .as_deref()
                .is_some_and(|c| meta::lstat(c).is_ok_and(|m| m.is_file()));
            if let Some(c) = companion.filter(|_| had_companion) {
                let _ = remove::remove_partial(&c);
            }
            if existed || had_companion {
                swept.push(SweptPartial { path: p, bytes });
            }
            it.partial = None;
            it.partial_ino = None;
            it.partial_dev = None;
            journal.save_item(&it)?;
        }
    }
    Ok(swept)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_s_volume_key_comes_from_its_uuid() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        let a = folder_dev(&d.path().join("x.fstudio-partial")).unwrap();
        let b = folder_dev(&d.path().join("sub/y.fstudio-partial")).unwrap();
        assert_eq!(a, b, "same volume, same key");
        // The temp folder is on the startup disk, which has a UUID.
        assert!(a & (1 << 63) != 0, "keyed by the volume UUID, not st_dev");
        assert!(folder_dev(&d.path().join("missing/z")).is_none());
    }

    #[test]
    fn volume_keys_never_look_like_device_numbers() {
        assert!(volume_key(&[0; 16]) >= 1 << 63);
        assert!(volume_key(&[0xff; 16]) >= 1 << 63);
    }
}

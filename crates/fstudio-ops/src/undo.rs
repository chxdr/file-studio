//! Undo: works out the inverse of a finished op from its journal, refusing
//! (with a reason) when the world changed since. The inverse never
//! overwrites anything: it is an ordinary journaled op with exclusive renames.

use std::collections::HashMap;
use std::path::PathBuf;

use fstudio_model::EntryKind;

use crate::engine::file_name;
use crate::journal::{ItemRow, ItemState, OpRow};
use crate::meta;
use crate::types::{OpKind, OpRequest};

fn q(p: &std::path::Path) -> String {
    format!("“{}”", file_name(p))
}

/// Items that are top-level results of the op: done, created by it, and not
/// inside another item it created (an item inside a merged folder counts).
fn tops(items: &[ItemRow]) -> Vec<&ItemRow> {
    let by_id: HashMap<i64, &ItemRow> = items.iter().map(|i| (i.id, i)).collect();
    items
        .iter()
        .filter(|i| i.state == ItemState::Done && i.created)
        .filter(|i| match i.parent.and_then(|p| by_id.get(&p)) {
            None => true,
            Some(p) => !p.created,
        })
        .collect()
}

/// The item must still be the one the op left at its destination.
fn check_dest(it: &ItemRow, verb: &str) -> Result<PathBuf, String> {
    let dest = it.dest.clone().ok_or("the journal has no destination")?;
    let md =
        meta::lstat(&dest).map_err(|_| format!("{} is no longer where it was {verb}", q(&dest)))?;
    let now = meta::ident(&md);
    if let Some(was) = it.dest_ident {
        let same = if it.kind == EntryKind::Dir {
            now.same_object(&was)
        } else {
            now.unchanged(&was)
        };
        if !same {
            return Err(format!("{} was changed after it was {verb}", q(&dest)));
        }
    }
    Ok(dest)
}

fn check_free(p: &std::path::Path) -> Result<(), String> {
    if meta::exists(p) {
        return Err(format!(
            "{} can't go back because its original location is taken",
            q(p)
        ));
    }
    Ok(())
}

/// Put back what "Replace" sent to the Trash.
fn replaced(items: &[ItemRow]) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let mut out = Vec::new();
    for it in items.iter().filter(|i| i.state == ItemState::Done) {
        if let (Some(t), Some(d)) = (&it.replaced_trash, &it.dest) {
            if !meta::exists(t) {
                return Err(format!(
                    "the replaced item {} is no longer in the Trash",
                    q(d)
                ));
            }
            out.push((t.clone(), d.clone()));
        }
    }
    Ok(out)
}

/// The requests that undo `op`, in order. `Err` explains why it can't be undone.
pub fn inverse(op: &OpRow, items: &[ItemRow]) -> Result<Vec<OpRequest>, String> {
    let mut out = Vec::new();
    match op.request.kind() {
        OpKind::Copy | OpKind::Duplicate => {
            let verb = if op.request.kind() == OpKind::Copy {
                "copied"
            } else {
                "duplicated"
            };
            let paths = tops(items)
                .into_iter()
                .map(|i| check_dest(i, verb))
                .collect::<Result<Vec<_>, _>>()?;
            let back = replaced(items)?;
            if !paths.is_empty() {
                out.push(OpRequest::Trash { paths });
            }
            if !back.is_empty() {
                out.push(OpRequest::MoveTo { pairs: back });
            }
        }
        OpKind::Move | OpKind::MoveTo | OpKind::Rename => {
            let mut pairs = Vec::new();
            for it in tops(items) {
                let dest = check_dest(it, "moved")?;
                // A case-only rename's "original" is the same item.
                let same_item = meta::lstat(&it.source)
                    .ok()
                    .zip(meta::lstat(&dest).ok())
                    .is_some_and(|(a, b)| meta::ident(&a).same_object(&meta::ident(&b)));
                if !same_item {
                    check_free(&it.source)?;
                }
                pairs.push((dest, it.source.clone()));
            }
            let back = replaced(items)?;
            if !pairs.is_empty() {
                out.push(OpRequest::MoveTo { pairs });
            }
            if !back.is_empty() {
                out.push(OpRequest::MoveTo { pairs: back });
            }
        }
        OpKind::Trash => {
            let mut pairs = Vec::new();
            for it in items.iter().filter(|i| i.state == ItemState::Done) {
                let Some(loc) = &it.trash_loc else {
                    return Err(format!("the Trash didn't say where {} went", q(&it.source)));
                };
                if !meta::exists(loc) {
                    return Err(format!("{} is no longer in the Trash", q(&it.source)));
                }
                check_free(&it.source)?;
                pairs.push((loc.clone(), it.source.clone()));
            }
            if !pairs.is_empty() {
                out.push(OpRequest::MoveTo { pairs });
            }
        }
        OpKind::NewFolder | OpKind::NewFile => {
            for it in items
                .iter()
                .filter(|i| i.state == ItemState::Done && i.created)
            {
                let dest = check_dest(it, "created")?;
                if it.kind == EntryKind::Dir {
                    let empty = std::fs::read_dir(&dest)
                        .map(|mut rd| rd.next().is_none())
                        .unwrap_or(false);
                    if !empty {
                        return Err(format!("{} isn't empty any more", q(&dest)));
                    }
                }
                out.push(OpRequest::Trash { paths: vec![dest] });
            }
        }
        OpKind::DeleteImmediately => {
            return Err("Delete Immediately can't be undone".into());
        }
    }
    if out.is_empty() {
        return Err("nothing was changed".into());
    }
    Ok(out)
}

/// The items a request acts on in place (moves, renames, trash) or reads
/// (copies). New folders and files have none.
pub fn sources(req: &OpRequest) -> Vec<PathBuf> {
    match req {
        OpRequest::NewFolder { .. } | OpRequest::NewFile { .. } => Vec::new(),
        _ => crate::plan::targets(req)
            .into_iter()
            .map(|(src, _, _)| src)
            .collect(),
    }
}

/// Whether redoing `op` would act on the same objects it acted on before
/// (S3 of the safety audit). Each source must still be the object the undo
/// put back there (`undo_items`: the items of the ops that undid it, with
/// their identity once placed), or, for a source the undo didn't touch (a
/// copy's original), the object the op first read. `Err` says what changed.
pub fn redo_check(op: &OpRow, items: &[ItemRow], undo_items: &[ItemRow]) -> Result<(), String> {
    for src in sources(&op.request) {
        let put_back = undo_items
            .iter()
            .filter(|i| i.state == ItemState::Done && i.dest.as_deref() == Some(src.as_path()))
            .find_map(|i| i.dest_ident);
        let first_read = || {
            items
                .iter()
                .filter(|i| i.parent.is_none() && i.source == src)
                .find_map(|i| i.src_ident)
        };
        let Some(want) = put_back.or_else(first_read) else {
            return Err(format!(
                "File Studio can’t tell whether {} changed",
                q(&src)
            ));
        };
        let Ok(md) = meta::lstat(&src) else {
            return Err(format!("{} is no longer where it was put back", q(&src)));
        };
        if !meta::ident(&md).same_object(&want) {
            return Err(format!("{} changed since it was put back", q(&src)));
        }
    }
    Ok(())
}

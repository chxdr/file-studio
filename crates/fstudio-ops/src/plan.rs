//! Dry runs: what a request would touch, found up front without changing anything.

use std::collections::{BTreeMap, HashSet};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use fstudio_model::EntryKind;

use crate::engine::{describe, file_name};
use crate::meta;
use crate::names::{self, is_within, name_key};
use crate::sys;
use crate::types::{
    CaseCollision, ItemError, OpPlan, OpRequest, OpsConfig, PlanConflict, PlanRoot,
};

/// Where each top-level item of a request goes: `(source, dest_dir, name)`.
/// Trash and Delete have no destination.
pub fn targets(req: &OpRequest) -> Vec<(PathBuf, Option<PathBuf>, Option<String>)> {
    match req {
        OpRequest::Copy { sources, dest_dir } | OpRequest::Move { sources, dest_dir } => sources
            .iter()
            .map(|s| (s.clone(), Some(dest_dir.clone()), None))
            .collect(),
        OpRequest::Trash { paths } | OpRequest::DeleteImmediately { paths } => {
            paths.iter().map(|p| (p.clone(), None, None)).collect()
        }
        OpRequest::Duplicate { paths } => paths
            .iter()
            .map(|p| (p.clone(), p.parent().map(Path::to_path_buf), None))
            .collect(),
        OpRequest::NewFolder { parent, name } | OpRequest::NewFile { parent, name } => {
            vec![(parent.join(name), Some(parent.clone()), Some(name.clone()))]
        }
        OpRequest::Rename { path, new_name } => vec![(
            path.clone(),
            path.parent().map(Path::to_path_buf),
            Some(new_name.clone()),
        )],
        OpRequest::Compound { .. } | OpRequest::UndoOf { .. } => Vec::new(),
        OpRequest::MoveTo { pairs } => pairs
            .iter()
            .map(|(from, to)| {
                (
                    from.clone(),
                    to.parent().map(Path::to_path_buf),
                    Some(file_name(to)),
                )
            })
            .collect(),
    }
}

/// Checks a request's shape (absolute paths, valid names) without touching the disk.
pub fn validate(req: &OpRequest) -> Result<(), String> {
    match req {
        OpRequest::Compound { steps, .. } => {
            if steps.is_empty() {
                return Err("nothing to do".into());
            }
            for s in steps {
                match s {
                    OpRequest::UndoOf { .. } => {}
                    OpRequest::Compound { .. } => return Err("a compound op can't nest".into()),
                    _ => validate(s)?,
                }
            }
            return Ok(());
        }
        OpRequest::UndoOf { .. } => {
            return Err("an undo step only runs inside a compound op".into());
        }
        _ => {}
    }
    let abs = |p: &Path| {
        if p.is_absolute() {
            Ok(())
        } else {
            Err(format!("{} is not an absolute path", p.display()))
        }
    };
    let ts = targets(req);
    if ts.is_empty() {
        return Err("nothing to do".into());
    }
    for (src, dir, name) in &ts {
        abs(src)?;
        if let Some(d) = dir {
            abs(d)?;
        }
        if src.file_name().is_none() {
            return Err(format!("{} has no name", src.display()));
        }
        if let Some(n) = name {
            names::validate_name(n)?;
        }
    }
    Ok(())
}

struct Walk<'a> {
    plan: &'a mut OpPlan,
    dest_cs: Option<bool>,
    visited: HashSet<(u64, u64)>,
}

impl Walk<'_> {
    /// Counts one source tree. Returns (files, dirs, bytes).
    fn walk(&mut self, src: &Path, dest: Option<&Path>) -> (u64, u64, u64) {
        let md = match meta::lstat(src) {
            Ok(m) => m,
            Err(e) => {
                self.plan.errors.push(ItemError {
                    path: src.to_path_buf(),
                    dest: dest.map(Path::to_path_buf),
                    message: describe(&e),
                    errno: e.raw_os_error(),
                });
                return (0, 0, 0);
            }
        };
        if meta::is_dataless(&md) {
            self.plan.dataless.push(src.to_path_buf());
        }
        if !md.is_dir() {
            return (1, 0, md.len());
        }
        if !self.visited.insert((md.dev(), md.ino())) {
            self.plan.errors.push(ItemError {
                path: src.to_path_buf(),
                dest: None,
                message: "this folder is inside itself (a folder loop)".into(),
                errno: None,
            });
            return (0, 0, 0);
        }
        let mut totals = (0, 1, 0);
        let rd = match std::fs::read_dir(src) {
            Ok(rd) => rd,
            Err(e) => {
                self.plan.errors.push(ItemError {
                    path: src.to_path_buf(),
                    dest: None,
                    message: describe(&e),
                    errno: e.raw_os_error(),
                });
                return totals;
            }
        };
        let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut children = Vec::new();
        for ent in rd.flatten() {
            let name = ent.file_name().to_string_lossy().into_owned();
            if let Some(cs) = self.dest_cs {
                groups
                    .entry(name_key(&name, cs))
                    .or_default()
                    .push(name.clone());
            }
            children.push(ent.path());
        }
        if let Some(d) = dest {
            for (_, names) in groups.into_iter().filter(|(_, v)| v.len() > 1) {
                self.plan.case_collisions.push(CaseCollision {
                    dest_dir: d.to_path_buf(),
                    names,
                });
            }
        }
        children.sort();
        for c in children {
            let cd = dest.map(|d| d.join(c.file_name().unwrap_or_default()));
            let (f, d, b) = self.walk(&c, cd.as_deref());
            totals.0 += f;
            totals.1 += d;
            totals.2 += b;
        }
        totals
    }
}

/// A dry run of `req`. Reads metadata only (never file content, so iCloud
/// placeholders are not downloaded). Blocks on disk: run it off the UI thread.
pub fn plan(req: &OpRequest, cfg: &OpsConfig) -> OpPlan {
    let mut plan = OpPlan::default();
    if let Err(e) = validate(req) {
        plan.refused = Some(e);
        return plan;
    }
    if matches!(req, OpRequest::DeleteImmediately { .. }) && !cfg.allow_delete_immediately {
        plan.refused = Some("Delete Immediately is turned off in Settings".into());
    }
    let ts = targets(req);
    let kind = req.kind();
    let creates = matches!(req, OpRequest::NewFolder { .. } | OpRequest::NewFile { .. });

    // Destination case sensitivity (F-017): one destination folder per request
    // in practice; MoveTo may have several, so take the first.
    let first_dir = ts.iter().find_map(|t| t.1.clone());
    plan.dest_case_sensitive = first_dir
        .as_deref()
        .filter(|_| kind != crate::types::OpKind::Trash)
        .map(|d| sys::case_sensitive(d).unwrap_or(false));
    let dest_cs = plan.dest_case_sensitive;

    // Existing names in each destination folder, by key, for collision checks.
    let mut existing: BTreeMap<PathBuf, BTreeMap<String, String>> = BTreeMap::new();
    let mut incoming: BTreeMap<(PathBuf, String), Vec<String>> = BTreeMap::new();

    let mut walk = Walk {
        plan: &mut plan,
        dest_cs,
        visited: HashSet::new(),
    };
    let mut roots = Vec::new();
    for (src, dir, name) in &ts {
        if creates {
            let dest = src.clone();
            if let Ok(dmd) = meta::lstat(&dest) {
                walk.plan.conflicts.push(PlanConflict {
                    source: dest.clone(),
                    dest: dest.clone(),
                    source_kind: if matches!(req, OpRequest::NewFolder { .. }) {
                        EntryKind::Dir
                    } else {
                        EntryKind::File
                    },
                    dest_kind: meta::kind_of(&dmd),
                });
            }
            roots.push(PlanRoot {
                source: dest.clone(),
                dest: Some(dest),
                kind: EntryKind::Dir,
                files: 0,
                dirs: 0,
                bytes: 0,
                cross_volume: false,
            });
            continue;
        }
        let smd = meta::lstat(src).ok();
        let dest_name = name.clone().unwrap_or_else(|| file_name(src));
        let dest = match (kind, dir) {
            (crate::types::OpKind::Duplicate, Some(d)) => {
                names::duplicate_names(&dest_name, smd.as_ref().is_some_and(|m| m.is_dir()))
                    .map(|n| d.join(n))
                    .find(|p| !meta::exists(p))
            }
            (_, Some(d)) => Some(d.join(&dest_name)),
            (_, None) => None,
        };
        let walk_dest = if kind.copies() { dest.as_deref() } else { None };
        let (files, dirs, bytes) = walk.walk(src, walk_dest);
        let cross = match (&smd, dir) {
            (Some(s), Some(d)) => meta::lstat(d)
                .map(|dm| dm.dev() != s.dev())
                .unwrap_or(false),
            _ => false,
        };
        if let (Some(s), Some(d), Some(dest)) = (&smd, dir, &dest) {
            if s.is_dir() && is_within(dest, src) {
                walk.plan.errors.push(ItemError {
                    path: src.clone(),
                    dest: Some(dest.clone()),
                    message: "a folder can't be copied or moved into itself".into(),
                    errno: None,
                });
            }
            if let Ok(dmd) = meta::lstat(dest) {
                let same = meta::ident(&dmd).same_object(&meta::ident(s));
                let case_only = same && kind != crate::types::OpKind::Copy;
                if !case_only {
                    walk.plan.conflicts.push(PlanConflict {
                        source: src.clone(),
                        dest: dest.clone(),
                        source_kind: meta::kind_of(s),
                        dest_kind: meta::kind_of(&dmd),
                    });
                }
            }
            if let Some(cs) = dest_cs {
                let names_here = existing.entry(d.clone()).or_insert_with(|| {
                    std::fs::read_dir(d)
                        .map(|rd| {
                            rd.flatten()
                                .map(|e| {
                                    let n = e.file_name().to_string_lossy().into_owned();
                                    (name_key(&n, cs), n)
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                });
                let dn = file_name(dest);
                let key = name_key(&dn, cs);
                if let Some(ex) = names_here.get(&key)
                    && *ex != dn
                    && Some(ex.as_str()) != src.file_name().and_then(|n| n.to_str())
                {
                    walk.plan.case_collisions.push(CaseCollision {
                        dest_dir: d.clone(),
                        names: vec![dn.clone(), ex.clone()],
                    });
                }
                incoming.entry((d.clone(), key)).or_default().push(dn);
            }
        }
        roots.push(PlanRoot {
            source: src.clone(),
            dest,
            kind: smd.as_ref().map(meta::kind_of).unwrap_or(EntryKind::Other),
            files,
            dirs,
            bytes,
            cross_volume: cross,
        });
    }
    for ((d, _), names) in incoming.into_iter().filter(|(_, v)| v.len() > 1) {
        plan.case_collisions
            .push(CaseCollision { dest_dir: d, names });
    }
    plan.cross_volume = roots.iter().any(|r| r.cross_volume);
    plan.files = roots.iter().map(|r| r.files).sum();
    plan.dirs = roots.iter().map(|r| r.dirs).sum();
    plan.bytes = roots.iter().map(|r| r.bytes).sum();
    plan.roots = roots;
    plan
}

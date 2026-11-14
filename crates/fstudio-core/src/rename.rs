//! Batch rename (M3): a [`RenameSession`] per "Batch Rename N Items…"
//! sheet, and the saved presets.
//!
//! The session reads each item's metadata and each folder's names once
//! when it opens, then previews in memory on every change
//! ([`RenameSession::set_pipeline`], a few milliseconds for 10,000 items).
//! The shell asks for the visible rows only ([`RenameSession::rows`]).
//! EXIF is read on a background thread the first time a step uses an
//! `{exif.*}` token; iCloud placeholders are never read (F-016). Applying
//! queues one `OpRequest::RenameMany`, which renames exclusively, handles
//! swaps and cycles, and undoes in one step (F-007).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use fstudio_rename::{
    Compiled, DateParts, Entry, Exif, Folder, ItemInfo, Pipeline, PipelineError, Preset, Presets,
    Preview, Status,
};
use parking_lot::Mutex;

use crate::confirm::{After, PendingOp, snapshot};
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::ids::TabId;
use crate::outcome::{Outcome, ShellRequest, ToastStyle};
use fstudio_ops::OpRequest;

/// The key-value key the presets live under.
const PRESETS_KEY: &str = "rename.presets";
/// The key-value key of the last pipeline applied (the next sheet starts with it).
const LAST_KEY: &str = "rename.last";

/// What happens to one row, for the preview's badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameRowStatus {
    /// It will be renamed.
    Ready,
    /// Its name doesn't change.
    Unchanged,
    /// It won't be renamed: see the row's message.
    Problem,
}

/// One row of the preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameRow {
    /// Position in the batch, from 0.
    pub index: u32,
    /// The current name, exactly as on disk.
    pub old_name: String,
    /// The new name.
    pub new_name: String,
    /// What happens.
    pub status: RenameRowStatus,
    /// The problem, in words ("an item with this name is already in the folder").
    pub message: String,
    /// A warning that doesn't stop the rename ("no EXIF camera"), or empty.
    pub warning: String,
    /// The folder the item is in (shown when the batch spans folders).
    pub folder: String,
}

/// The preview's counts and state, for the sheet's footer and button.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RenameSummary {
    /// Bumped on every recompute; reload the visible rows when it changes.
    pub version: u64,
    /// Items in the batch.
    pub total: u32,
    /// Items that will be renamed.
    pub ready: u32,
    /// Items whose name doesn't change.
    pub unchanged: u32,
    /// Items with a problem (left as they are).
    pub problems: u32,
    /// Items with a warning.
    pub warnings: u32,
    /// EXIF is being read; poll [`RenameSession::summary`] until it isn't.
    pub exif_loading: bool,
    /// EXIF files read so far, of `exif_total`.
    pub exif_done: u32,
    /// Files whose EXIF is wanted.
    pub exif_total: u32,
    /// Why the steps can't run (a bad regex or token), if they can't.
    pub error: Option<String>,
    /// Which step that is, from 0.
    pub error_step: Option<u32>,
    /// The batch spans more than one folder.
    pub many_folders: bool,
}

struct State {
    paths: Vec<PathBuf>,
    entries: Vec<Entry>,
    folders: Vec<Folder>,
    folder_paths: Vec<PathBuf>,
    pipeline: Pipeline,
    compiled: Option<Compiled>,
    error: Option<PipelineError>,
    preview: Preview,
    version: u64,
    exif_started: bool,
}

/// One open "Batch Rename" sheet. Cheap to query; every call is quick.
pub struct RenameSession {
    engine: Arc<Engine>,
    tab: Option<TabId>,
    snapshot: Vec<(PathBuf, Option<crate::confirm::ObjId>)>,
    st: Arc<Mutex<State>>,
    exif_loading: Arc<AtomicBool>,
    exif_done: Arc<AtomicU32>,
    exif_total: Arc<AtomicU32>,
}

impl std::fmt::Debug for RenameSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenameSession")
            .field("items", &self.st.lock().paths.len())
            .finish()
    }
}

fn local(t: std::io::Result<std::time::SystemTime>) -> Option<DateParts> {
    let t = t.ok()?;
    let ns = match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i64,
        Err(e) => -(e.duration().as_nanos() as i64),
    };
    let off = crate::sys::utc_offset_secs(ns.div_euclid(1_000_000_000));
    Some(DateParts::from_unix_ns(ns, off))
}

fn name_of(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl RenameSession {
    /// Opens a session for `paths` (in the order the sequence numbers
    /// follow). Reads each item's metadata (`lstat`) and lists each folder
    /// once: this blocks on disk, like a folder listing (F-010).
    pub fn open(engine: Arc<Engine>, tab: Option<TabId>, paths: Vec<String>) -> Result<Arc<Self>> {
        if paths.is_empty() {
            return Err(CoreError::NotAvailable("nothing is selected".into()));
        }
        let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        let mut folder_ix: HashMap<PathBuf, usize> = HashMap::new();
        let mut folder_paths = Vec::new();
        for p in &paths {
            if !p.is_absolute() || p.file_name().is_none() {
                return Err(CoreError::InvalidName(p.to_string_lossy().into_owned()));
            }
            let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
            folder_ix.entry(dir.clone()).or_insert_with(|| {
                folder_paths.push(dir);
                folder_paths.len() - 1
            });
        }
        let folders: Vec<Folder> = folder_paths
            .iter()
            .map(|d| {
                let cs = fstudio_ops::is_case_sensitive(d).unwrap_or(false);
                let names: Vec<String> = std::fs::read_dir(d)
                    .map(|rd| {
                        rd.flatten()
                            .map(|e| e.file_name().to_string_lossy().into_owned())
                            .collect()
                    })
                    .unwrap_or_default();
                Folder::with_names(cs, names.iter().map(String::as_str))
            })
            .collect();
        let entries: Vec<Entry> = paths
            .iter()
            .map(|p| {
                let md = std::fs::symlink_metadata(p).ok();
                let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
                let info = ItemInfo {
                    name: name_of(p),
                    is_dir: md.as_ref().is_some_and(|m| m.is_dir()),
                    parent_name: crate::display_name(&dir.to_string_lossy()),
                    modified: md.as_ref().and_then(|m| local(m.modified())),
                    created: md.as_ref().and_then(|m| local(m.created())),
                    exif: Exif::NotLoaded,
                };
                Entry::new(info, folder_ix[&dir], &folders)
            })
            .collect();
        let pipeline = engine
            .db
            .kv_get(LAST_KEY)
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str::<Pipeline>(&s).ok())
            .unwrap_or_else(default_pipeline);
        let snapshot = snapshot(&paths);
        let s = Arc::new(RenameSession {
            engine,
            tab,
            snapshot,
            st: Arc::new(Mutex::new(State {
                paths,
                entries,
                folders,
                folder_paths,
                pipeline: Pipeline::default(),
                compiled: None,
                error: None,
                preview: Preview::default(),
                version: 0,
                exif_started: false,
            })),
            exif_loading: Arc::new(AtomicBool::new(false)),
            exif_done: Arc::new(AtomicU32::new(0)),
            exif_total: Arc::new(AtomicU32::new(0)),
        });
        s.set_pipeline(pipeline);
        Ok(s)
    }

    /// The steps now.
    pub fn pipeline(&self) -> Pipeline {
        self.st.lock().pipeline.clone()
    }

    /// Replaces the steps and recomputes the preview. Fast enough for every
    /// keystroke; never touches the disk (EXIF, if a step needs it, is read
    /// on a background thread).
    pub fn set_pipeline(&self, p: Pipeline) -> RenameSummary {
        let mut st = self.st.lock();
        st.pipeline = p;
        match Compiled::new(&st.pipeline) {
            Ok(c) => {
                st.error = None;
                if c.uses_exif() && !st.exif_started {
                    st.exif_started = true;
                    self.start_exif(&st.paths);
                }
                st.compiled = Some(c);
            }
            Err(e) => {
                st.error = Some(e);
                st.compiled = None;
            }
        }
        recompute(&mut st);
        self.summary_of(&st)
    }

    /// The counts and state.
    pub fn summary(&self) -> RenameSummary {
        let st = self.st.lock();
        self.summary_of(&st)
    }

    /// Rows `start..start+count` of the preview.
    pub fn rows(&self, start: u32, count: u32) -> Vec<RenameRow> {
        let st = self.st.lock();
        let many = st.folders.len() > 1;
        let end = (start as usize + count as usize).min(st.entries.len());
        (start as usize..end)
            .map(|i| {
                let e = &st.entries[i];
                let r = st.preview.rows.get(i);
                let (new_name, status, message, warning) = match r {
                    Some(r) => {
                        let status = match r.status {
                            Status::Ready => RenameRowStatus::Ready,
                            Status::Unchanged => RenameRowStatus::Unchanged,
                            _ => RenameRowStatus::Problem,
                        };
                        let msg = if r.status.is_problem() {
                            r.status.message().to_string()
                        } else {
                            String::new()
                        };
                        (r.new_name.clone(), status, msg, warning_text(r))
                    }
                    None => (
                        e.info.name.clone(),
                        RenameRowStatus::Unchanged,
                        String::new(),
                        String::new(),
                    ),
                };
                RenameRow {
                    index: i as u32,
                    old_name: e.info.name.clone(),
                    new_name,
                    status,
                    message,
                    warning,
                    folder: if many {
                        crate::display_name(&st.folder_paths[e.folder].to_string_lossy())
                    } else {
                        String::new()
                    },
                }
            })
            .collect()
    }

    /// Renames every ready row as one queued operation (one Undo puts every
    /// name back). Rows with problems, and unchanged ones, are left alone.
    /// Refused, with a message, if an item changed since the sheet opened.
    pub fn apply(&self) -> Result<Outcome> {
        let (pairs, pipeline) = {
            let st = self.st.lock();
            if let Some(e) = &st.error {
                return Err(CoreError::NotAvailable(e.to_string()));
            }
            if st.preview.summary.exif_pending {
                return Err(CoreError::NotAvailable(
                    "EXIF is still being read; try again in a moment".into(),
                ));
            }
            let pairs: Vec<(PathBuf, String)> = st
                .preview
                .ready()
                .into_iter()
                .map(|(i, n)| (st.paths[i].clone(), n))
                .collect();
            (pairs, st.pipeline.clone())
        };
        if pairs.is_empty() {
            return Ok(Outcome::message("Nothing to rename"));
        }
        let _ = self.engine.db.kv_set(
            LAST_KEY,
            &serde_json::to_string(&pipeline).unwrap_or_default(),
        );
        let n = pairs.len();
        let names: Vec<String> = pairs.iter().map(|(_, n)| n.clone()).collect();
        let text = match pairs.as_slice() {
            [(p, new)] => format!("Renamed “{}” to “{new}”", name_of(p)),
            _ => format!("Renamed {n} items"),
        };
        let items: Vec<_> = self
            .snapshot
            .iter()
            .filter(|(p, _)| pairs.iter().any(|(q, _)| q == p))
            .cloned()
            .collect();
        let op = PendingOp {
            req: OpRequest::RenameMany { pairs },
            items,
            tab: self.tab,
            after: After {
                select: self.tab.map(|t| (t, names)),
                ..After::default()
            },
            toast: Some(ShellRequest::Toast {
                text,
                button: Some("Undo".into()),
                command: Some("edit.undo".into()),
                style: ToastStyle::Info,
            }),
        };
        self.engine.run_pending_op(op, true)
    }

    fn summary_of(&self, st: &State) -> RenameSummary {
        let s = st.preview.summary;
        RenameSummary {
            version: st.version,
            total: st.entries.len() as u32,
            ready: s.ready as u32,
            unchanged: s.unchanged as u32,
            problems: s.problems as u32,
            warnings: s.warnings as u32,
            exif_loading: self.exif_loading.load(Ordering::Acquire),
            exif_done: self.exif_done.load(Ordering::Relaxed),
            exif_total: self.exif_total.load(Ordering::Relaxed),
            error: st.error.as_ref().map(|e| e.message.clone()),
            error_step: st.error.as_ref().map(|e| e.step as u32),
            many_folders: st.folders.len() > 1,
        }
    }

    /// Reads EXIF for every file on a background thread, then recomputes.
    fn start_exif(&self, paths: &[PathBuf]) {
        let paths = paths.to_vec();
        let st = Arc::clone(&self.st);
        let (loading, done, total) = (
            Arc::clone(&self.exif_loading),
            Arc::clone(&self.exif_done),
            Arc::clone(&self.exif_total),
        );
        total.store(paths.len() as u32, Ordering::Relaxed);
        loading.store(true, Ordering::Release);
        let spawned = std::thread::Builder::new()
            .name("fstudio-rename-exif".into())
            .spawn(move || {
                use rayon::prelude::*;
                let exifs: Vec<Exif> = paths
                    .par_iter()
                    .map(|p| {
                        let e = fstudio_rename::exif::read(p);
                        done.fetch_add(1, Ordering::Relaxed);
                        e
                    })
                    .collect();
                let mut s = st.lock();
                for (e, x) in s.entries.iter_mut().zip(exifs) {
                    e.info.exif = x;
                }
                recompute(&mut s);
                loading.store(false, Ordering::Release);
            });
        if spawned.is_err() {
            self.exif_loading.store(false, Ordering::Release);
        }
    }
}

fn recompute(st: &mut State) {
    st.version += 1;
    st.preview = match &st.compiled {
        Some(c) => fstudio_rename::preview(&st.entries, &st.folders, c),
        // Steps that can't run: show every name unchanged.
        None => fstudio_rename::preview(
            &st.entries,
            &st.folders,
            &Compiled::new(&Pipeline::default()).expect("an empty pipeline compiles"),
        ),
    };
}

fn warning_text(r: &fstudio_rename::Row) -> String {
    let mut w = Vec::new();
    if r.notes.placeholder {
        w.push("in iCloud only, so its EXIF wasn’t read");
    } else if r.notes.exif_missing {
        w.push("no EXIF value");
    }
    if r.becomes_hidden {
        w.push("starts with “.”, so it will be hidden");
    }
    w.join("; ")
}

/// The steps a first sheet starts with.
pub fn default_pipeline() -> Pipeline {
    Pipeline::of(vec![fstudio_rename::Step::Template {
        template: "{name}".into(),
    }])
}

impl Engine {
    /// Opens a "Batch Rename" session for `paths` (see [`RenameSession::open`]).
    pub fn batch_rename(
        self: &Arc<Self>,
        tab: Option<TabId>,
        paths: Vec<String>,
    ) -> Result<Arc<RenameSession>> {
        RenameSession::open(Arc::clone(self), tab, paths)
    }

    fn presets_now(&self) -> Presets {
        match self.db.kv_get(PRESETS_KEY).ok().flatten() {
            Some(s) => Presets::from_json(&s).unwrap_or_else(|_| {
                // Keep unreadable text aside, so a newer format isn't lost.
                let _ = self.db.kv_set(&format!("{PRESETS_KEY}.unreadable"), &s);
                Presets::default()
            }),
            None => Presets::defaults(),
        }
    }

    /// The saved batch-rename presets, in the order they were saved.
    pub fn rename_presets(&self) -> Vec<Preset> {
        self.presets_now().presets
    }

    /// Saves `pipeline` as a preset called `name` (replacing one with the
    /// same name, ignoring case).
    pub fn save_rename_preset(&self, name: &str, pipeline: Pipeline) -> Result<()> {
        let mut ps = self.presets_now();
        ps.save(name, pipeline).map_err(CoreError::InvalidName)?;
        self.db.kv_set(PRESETS_KEY, &ps.to_json())?;
        Ok(())
    }

    /// Deletes the preset called `name`. Returns whether there was one.
    pub fn delete_rename_preset(&self, name: &str) -> Result<bool> {
        let mut ps = self.presets_now();
        let had = ps.delete(name);
        self.db.kv_set(PRESETS_KEY, &ps.to_json())?;
        Ok(had)
    }
}

//! What the user is told after a file operation (S11 of the safety audit):
//! every trash, undo and redo answers with a toast, so nothing changes on
//! disk unseen. Undo and redo are app-global (one journal stack), so their
//! toasts name the folder they touched when this window doesn't show it.
//!
//! A toast is built from what the operation really did, never from what was
//! asked for: the [`Toaster`] holds each operation's words until the queue
//! reports it finished, then sends the success toast only if every item
//! succeeded, a warning naming what didn't happen otherwise.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

use fstudio_ops::{ItemError, OpId, OpReport, OpRequest, OpState};
use parking_lot::Mutex;

use crate::confirm::display_name;
use crate::engine::Engine;
use crate::events::{CoreEvent, EventHub};
use crate::ids::TabId;
use crate::outcome::{ShellRequest, ToastStyle};

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

    /// A success toast with a button that runs `command`.
    pub(crate) fn toast(text: String, button: &str, command: &str) -> ShellRequest {
        ShellRequest::Toast {
            text,
            button: Some(button.to_string()),
            command: Some(command.to_string()),
            style: ToastStyle::Success,
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

// ───────────────────────── items that can't be opened ─────────────────────────

/// Why `p` can't be opened, in plain words, or `None` when it can be tried:
/// it is gone, or it is a symlink whose target doesn't exist (or loops, or
/// can't be reached). Opening such an item used to do nothing at all.
/// Does `lstat`, `stat` and `readlink` on `p` only.
pub(crate) fn unopenable(p: &Path) -> Option<String> {
    let name = quoted(p);
    let md = match std::fs::symlink_metadata(p) {
        Ok(md) => md,
        Err(_) => return Some(format!("{name} no longer exists")),
    };
    if !md.file_type().is_symlink() {
        return None;
    }
    let err = std::fs::metadata(p).err()?;
    let target = std::fs::read_link(p)
        .map(|t| t.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "another item".into());
    Some(match err.raw_os_error() {
        Some(libc::ELOOP) => {
            format!("{name} points to {target}, which leads back to itself (a symlink loop)")
        }
        Some(libc::EACCES) | Some(libc::EPERM) => {
            format!("{name} points to {target}, which you don’t have permission to open")
        }
        _ => format!("{name} points to {target}, which doesn’t exist"),
    })
}

/// A warning toast with no button (an open that can't happen).
pub(crate) fn warning_toast(text: String) -> ShellRequest {
    ShellRequest::Toast {
        text,
        button: None,
        command: None,
        style: ToastStyle::Warning,
    }
}

/// "Removed an unfinished copy of “bigfile” (1.6 GB) left behind when its
/// disk was disconnected", or `None` when nothing was removed.
pub(crate) fn swept_text(swept: &[fstudio_ops::SweptPartial]) -> Option<String> {
    let bytes: u64 = swept.iter().map(|s| s.bytes).sum();
    let size = fstudio_view::format::format_size(bytes, &Default::default());
    Some(match swept {
        [] => return None,
        [one] => {
            // `.<name>.<item>.fstudio-partial` → `<name>`.
            let n = one
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let name = n
                .strip_prefix('.')
                .and_then(|n| n.strip_suffix(".fstudio-partial"))
                .and_then(|n| n.rsplit_once('.').map(|(a, _)| a.to_string()))
                .unwrap_or(n);
            format!(
                "Removed an unfinished copy of “{name}” ({size}) left behind when its disk was disconnected"
            )
        }
        many => format!(
            "Removed {} unfinished copies ({size}) left behind when their disk was disconnected",
            many.len()
        ),
    })
}

// ───────────────────────── toasts from outcomes ─────────────────────────

/// What an operation is, for the words of its toast.
#[derive(Debug, Clone)]
pub(crate) enum Subject {
    /// A request the user made.
    Op(OpRequest),
    /// An undo of the op labelled this ("Move to Trash of 3 Items").
    Undo(String),
    /// A redo of the op labelled this.
    Redo(String),
}

/// What to say once an operation has finished.
#[derive(Debug, Clone)]
pub(crate) struct ToastPlan {
    /// What it was.
    pub subject: Subject,
    /// The toast when every item succeeded, if it has one (copies don't).
    pub success: Option<ShellRequest>,
    /// The tab whose selection moved off the items, and its selection
    /// generation then: items that failed are selected again there.
    pub reselect: Option<(TabId, u64)>,
}

impl ToastPlan {
    /// The words for a request, toasting only if something fails.
    pub(crate) fn failures_only(req: &OpRequest) -> Self {
        Self {
            subject: Subject::Op(req.clone()),
            success: None,
            reselect: None,
        }
    }
}

/// How a request is described: its top-level items and verb forms.
struct Words {
    roots: Vec<PathBuf>,
    /// "Moved"
    past: &'static str,
    /// "move"
    verb: &'static str,
    /// "moved"
    participle: &'static str,
    /// " to “Backup”", " to the Trash"
    tail: String,
}

fn words(req: &OpRequest) -> Option<Words> {
    let w = |roots: Vec<PathBuf>, past, verb, participle, tail: String| Words {
        roots,
        past,
        verb,
        participle,
        tail,
    };
    let to = |d: &Path| format!(" to {}", quoted(d));
    Some(match req {
        OpRequest::Trash { paths } => w(
            paths.clone(),
            "Moved",
            "move",
            "moved",
            " to the Trash".into(),
        ),
        OpRequest::DeleteImmediately { paths } => {
            w(paths.clone(), "Deleted", "delete", "deleted", String::new())
        }
        OpRequest::Move { sources, dest_dir } => {
            w(sources.clone(), "Moved", "move", "moved", to(dest_dir))
        }
        OpRequest::Copy { sources, dest_dir } => {
            w(sources.clone(), "Copied", "copy", "copied", to(dest_dir))
        }
        OpRequest::Duplicate { paths } => w(
            paths.clone(),
            "Duplicated",
            "duplicate",
            "duplicated",
            String::new(),
        ),
        OpRequest::MoveTo { pairs } => w(
            pairs.iter().map(|(f, _)| f.clone()).collect(),
            "Moved",
            "move",
            "moved",
            String::new(),
        ),
        OpRequest::Rename { path, new_name } => w(
            vec![path.clone()],
            "Renamed",
            "rename",
            "renamed",
            format!(" to “{new_name}”"),
        ),
        OpRequest::NewFolder { parent, name } | OpRequest::NewFile { parent, name } => w(
            vec![parent.join(name)],
            "Created",
            "create",
            "created",
            String::new(),
        ),
        OpRequest::Compound { .. } | OpRequest::UndoOf { .. } => return None,
    })
}

/// "1 item" / "3 items".
fn items(n: usize) -> String {
    if n == 1 {
        "1 item".into()
    } else {
        format!("{n} items")
    }
}

/// The first reason, in plain words ("permission denied").
fn reason(errors: &[&ItemError]) -> String {
    errors
        .iter()
        .map(|e| e.message.trim())
        .find(|m| !m.is_empty())
        .unwrap_or("something went wrong")
        .to_string()
}

fn warning(text: String) -> ShellRequest {
    ShellRequest::Toast {
        text,
        button: Some("Details".into()),
        command: Some("view.show_operations".into()),
        style: ToastStyle::Warning,
    }
}

/// The toast for a finished operation (or the ops of one undo), from what
/// really happened:
/// - every item done: the plan's success toast (with Undo), as before;
/// - some failed: a warning, "Moved 3 items to the Trash · 1 couldn't be
///   moved", whose Details button opens the queue with each reason;
/// - none done: "Couldn't move “locked” to the Trash: permission denied",
///   with no Undo (there is nothing to undo).
///
/// A cancelled op with nothing done says nothing (the user stopped it).
pub(crate) fn outcome_toast(plan: &ToastPlan, reports: &[OpReport]) -> Option<ShellRequest> {
    let errors: Vec<&ItemError> = reports.iter().flat_map(|r| r.errors.iter()).collect();
    let done: u64 = reports.iter().map(|r| r.files_done).sum();
    let whole_failed = reports.iter().any(|r| r.state == OpState::Failed);
    let cancelled = reports.iter().any(|r| r.state == OpState::Cancelled);
    let failed = (reports.iter().map(|r| r.files_failed).sum::<u64>() as usize)
        .max(errors.len())
        .max(usize::from(whole_failed));
    if failed == 0 {
        if cancelled {
            return (done > 0).then(|| ShellRequest::Toast {
                text: format!("Stopped · {} already done", items(done as usize)),
                button: Some("Undo".into()),
                command: Some("edit.undo".into()),
                style: ToastStyle::Info,
            });
        }
        return plan.success.clone();
    }
    let none_done = done == 0;
    let text = match &plan.subject {
        Subject::Undo(label) if none_done => {
            format!("Couldn’t undo {label}: {}", reason(&errors))
        }
        Subject::Undo(label) => format!(
            "Undid part of {label} · {} couldn’t be undone",
            items(failed)
        ),
        Subject::Redo(label) if none_done => {
            format!("Couldn’t redo {label}: {}", reason(&errors))
        }
        Subject::Redo(label) => format!(
            "Redid part of {label} · {} couldn’t be redone",
            items(failed)
        ),
        Subject::Op(req) => match words(req) {
            None if none_done => format!("Couldn’t finish the operation: {}", reason(&errors)),
            None => format!(
                "The operation finished with problems · {} failed",
                items(failed)
            ),
            Some(w) => {
                // Top-level items with no error in or under them.
                let ok: Vec<PathBuf> = w
                    .roots
                    .iter()
                    .filter(|r| {
                        !whole_failed
                            && !errors.iter().any(|e| {
                                e.path.as_os_str().is_empty() || e.path.starts_with(r.as_path())
                            })
                    })
                    .cloned()
                    .collect();
                if none_done {
                    format!(
                        "Couldn’t {} {}{}: {}",
                        w.verb,
                        what(&w.roots),
                        w.tail,
                        reason(&errors)
                    )
                } else {
                    let head = if ok.is_empty() {
                        format!("{} part of {}{}", w.past, what(&w.roots), w.tail)
                    } else {
                        format!("{} {}{}", w.past, what(&ok), w.tail)
                    };
                    format!("{head} · {failed} couldn’t be {}", w.participle)
                }
            }
        },
    };
    Some(warning(text))
}

/// Ops waiting to be toasted, grouped (one undo can queue several ops).
struct Group {
    plan: ToastPlan,
    tab: Option<TabId>,
    waiting: HashSet<OpId>,
    reports: Vec<OpReport>,
}

#[derive(Default)]
struct ToastState {
    groups: Vec<Group>,
    /// Reports of ops that finished before anyone asked about them (an op
    /// can finish before `enqueue` returns its id), newest last.
    unclaimed: VecDeque<OpReport>,
}

/// How many unclaimed reports are kept (parts of compound ops, resumed ops).
const UNCLAIMED: usize = 256;

/// Turns finished operations into toasts (see the module docs).
pub(crate) struct Toaster {
    hub: EventHub,
    state: Mutex<ToastState>,
    /// For selecting failed items again.
    engine: std::sync::OnceLock<std::sync::Weak<Engine>>,
}

/// The top-level items of `req` that an error is in or under.
fn failed_roots(req: &OpRequest, reports: &[OpReport]) -> Vec<PathBuf> {
    let Some(w) = words(req) else {
        return Vec::new();
    };
    w.roots
        .into_iter()
        .filter(|r| {
            reports
                .iter()
                .flat_map(|x| x.errors.iter())
                .any(|e| !e.path.as_os_str().is_empty() && e.path.starts_with(r))
        })
        .collect()
}

impl Toaster {
    pub(crate) fn new(hub: EventHub) -> Self {
        Self {
            hub,
            state: Mutex::new(ToastState::default()),
            engine: std::sync::OnceLock::new(),
        }
    }

    /// Connects the engine (once it exists).
    pub(crate) fn attach(&self, engine: std::sync::Weak<Engine>) {
        let _ = self.engine.set(engine);
    }

    /// Toasts `plan` once every op in `ids` has finished.
    pub(crate) fn watch(&self, ids: &[OpId], plan: ToastPlan, tab: Option<TabId>) {
        if ids.is_empty() {
            return;
        }
        let mut st = self.state.lock();
        let mut g = Group {
            plan,
            tab,
            waiting: ids.iter().copied().collect(),
            reports: Vec::new(),
        };
        st.unclaimed.retain(|r| {
            if g.waiting.remove(&r.id) {
                g.reports.push(r.clone());
                false
            } else {
                true
            }
        });
        if g.waiting.is_empty() {
            drop(st);
            self.send(g);
        } else {
            st.groups.push(g);
        }
    }

    /// The queue says `report.id` finished.
    pub(crate) fn finished(&self, report: &OpReport) {
        let mut st = self.state.lock();
        let Some(i) = st
            .groups
            .iter()
            .position(|g| g.waiting.contains(&report.id))
        else {
            st.unclaimed.push_back(report.clone());
            if st.unclaimed.len() > UNCLAIMED {
                st.unclaimed.pop_front();
            }
            return;
        };
        let g = &mut st.groups[i];
        g.waiting.remove(&report.id);
        g.reports.push(report.clone());
        if g.waiting.is_empty() {
            let g = st.groups.remove(i);
            drop(st);
            self.send(g);
        }
    }

    fn send(&self, g: Group) {
        if let (Some((tab, generation)), Subject::Op(req)) = (g.plan.reselect, &g.plan.subject)
            && let Some(engine) = self.engine.get().and_then(std::sync::Weak::upgrade)
            && let Some(t) = engine.tab(tab)
        {
            let names: Vec<String> = failed_roots(req, &g.reports)
                .iter()
                .filter_map(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .collect();
            t.reselect_if_untouched(generation, &names);
        }
        if let Some(request) = outcome_toast(&g.plan, &g.reports) {
            self.hub.emit(CoreEvent::Shell {
                tab: g.tab,
                request,
            });
        }
    }
}

/// A file's extension as Finder sees it ("txt" in "a.txt"); none for a
/// folder, a name with no dot, or a name that only starts with one.
fn extension(name: &str) -> Option<&str> {
    let i = name.rfind('.')?;
    (i > 0 && i + 1 < name.len()).then(|| &name[i + 1..])
}

/// The question to ask before renaming `old` to `new` (S14), or `None`:
/// `(title, detail, yes, no)`. A name that starts with a dot hides the item;
/// a changed extension may make a file open in another app.
pub(crate) fn rename_warning(
    old: &str,
    new: &str,
    is_dir: bool,
    warn_hidden: bool,
    warn_extension: bool,
) -> Option<(String, String, String, String)> {
    if warn_hidden && new.starts_with('.') && !old.starts_with('.') {
        return Some((
            format!("Use a name that begins with a dot, “{new}”?"),
            "Names that begin with a dot are hidden, so the item will disappear from the list \
             (View › Show Hidden Files shows it)."
                .into(),
            "Use “.”".into(),
            "Cancel".into(),
        ));
    }
    if !warn_extension || is_dir {
        return None;
    }
    let (a, b) = (extension(old), extension(new));
    let same = match (a, b) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        (None, None) => true,
        _ => false,
    };
    if same {
        return None;
    }
    let detail = "If you make this change, the file may open in a different app.".to_string();
    Some(match (a, b) {
        (Some(a), Some(b)) => (
            format!("Change the extension from “.{a}” to “.{b}”?"),
            detail,
            format!("Use .{b}"),
            format!("Keep .{a}"),
        ),
        (Some(a), None) => (
            format!("Remove the extension “.{a}”?"),
            detail,
            "Remove".into(),
            format!("Keep .{a}"),
        ),
        (None, Some(b)) => (
            format!("Add the extension “.{b}” to the end of the name?"),
            detail,
            format!("Add .{b}"),
            "Cancel".into(),
        ),
        (None, None) => unreachable!("handled above"),
    })
}

/// "“A”", "“A” and “B”", "“A”, “B” and “C”".
fn names_list(names: &[String]) -> String {
    let q: Vec<String> = names.iter().map(|n| format!("“{n}”")).collect();
    match q.as_slice() {
        [] => "a disk".into(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The question when an interrupted copy's disks are back (Safe Transfer
/// proposal §6.1).
pub(crate) fn resume_prompt(
    op: fstudio_ops::OpId,
    title: &str,
    volume: &str,
    files_left: u64,
    bytes_left: u64,
) -> ShellRequest {
    let files = match files_left {
        1 => "1 file left".to_string(),
        n => format!("{n} files left"),
    };
    let size = fstudio_view::format_size(bytes_left, &fstudio_view::SizeFormat::default());
    ShellRequest::ResumePrompt {
        op,
        question: format!("Resume “{title}”?"),
        detail: format!(
            "{files} ({size}). Files already copied are kept, and so is anything \
             partly copied that still checks out."
        ),
        discard_question: "Discard the rest of this copy?".into(),
        discard_detail: format!(
            "The finished files on “{volume}” stay. The unfinished hidden copies are \
             removed. Your originals aren't touched."
        ),
    }
}

/// What to say when a copy stops for missing disks.
pub(crate) fn waiting(op: fstudio_ops::OpId, volumes: &[String], sources: bool) -> ShellRequest {
    let them = names_list(volumes);
    if sources {
        ShellRequest::SourceMissing {
            op,
            text: format!(
                "Some files wait for {them}. Insert it, then resume the copy from the \
                 operations list."
            ),
        }
    } else {
        ShellRequest::Toast {
            text: format!("{them} was disconnected. The copy stopped safely; insert it to resume."),
            button: Some("Show".into()),
            command: Some("view.show_operations".into()),
            style: ToastStyle::Warning,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rename_warnings_follow_finder() {
        use super::rename_warning as w;
        assert!(w("a.txt", "b.txt", false, true, true).is_none());
        assert!(w("a.txt", "a.TXT", false, true, true).is_none());
        let (t, _, yes, no) = w("a.txt", "a.md", false, true, true).unwrap();
        assert_eq!(t, "Change the extension from “.txt” to “.md”?");
        assert_eq!((yes.as_str(), no.as_str()), ("Use .md", "Keep .txt"));
        assert!(
            w("a.txt", "a", false, true, true)
                .unwrap()
                .0
                .starts_with("Remove")
        );
        assert!(
            w("a", "a.md", false, true, true)
                .unwrap()
                .0
                .starts_with("Add")
        );
        // Folders keep dots freely; the settings turn each warning off.
        assert!(w("Site", "Site.old", true, true, true).is_none());
        assert!(w("a.txt", "a.md", false, true, false).is_none());
        assert!(
            w("a.txt", ".a.txt", false, true, true)
                .unwrap()
                .0
                .contains("dot")
        );
        assert!(w("a.txt", ".a.txt", false, false, true).is_none());
        assert!(w(".env", ".env2", false, true, true).is_none());
    }

    use super::*;

    #[test]
    fn the_resume_prompt_says_what_is_left_and_what_discard_keeps() {
        let r = resume_prompt(
            fstudio_ops::OpId(7),
            "Copying 800 items to “CLIENT_A”",
            "CLIENT_A",
            312,
            11_300_000_000,
        );
        let ShellRequest::ResumePrompt {
            op,
            question,
            detail,
            discard_detail,
            ..
        } = r
        else {
            panic!("{r:?}")
        };
        assert_eq!(op, fstudio_ops::OpId(7));
        assert_eq!(question, "Resume “Copying 800 items to “CLIENT_A””?");
        assert!(detail.starts_with("312 files left (11.3 GB)."), "{detail}");
        assert!(
            discard_detail.contains("on “CLIENT_A” stay"),
            "{discard_detail}"
        );
        assert!(matches!(
            waiting(fstudio_ops::OpId(7), &["Untitled".into()], true),
            ShellRequest::SourceMissing { text, .. } if text.contains("“Untitled”")
        ));
        assert!(matches!(
            waiting(fstudio_ops::OpId(7), &["CLIENT_A".into()], false),
            ShellRequest::Toast { text, .. } if text.starts_with("“CLIENT_A” was disconnected")
        ));
    }

    #[test]
    fn cleanup_after_a_pulled_card_is_worded_plainly() {
        let one = fstudio_ops::SweptPartial {
            path: "/Volumes/SD/.bigfile.1.fstudio-partial".into(),
            bytes: 1_600_000_000,
        };
        assert_eq!(swept_text(&[]), None);
        assert_eq!(
            swept_text(std::slice::from_ref(&one)).unwrap(),
            "Removed an unfinished copy of “bigfile” (1.6 GB) left behind when its disk was disconnected"
        );
        assert!(
            swept_text(&[one.clone(), one])
                .unwrap()
                .starts_with("Removed 2 unfinished copies (3.2 GB)")
        );
    }

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

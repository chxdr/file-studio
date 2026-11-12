//! Events from the core to the shell, and the coalescing pump that delivers them.
//!
//! Every event goes through one pump thread, so a shell sees them in order and
//! on one thread. State-like events ([`CoreEvent::ListingChanged`],
//! [`CoreEvent::SelectionChanged`], [`CoreEvent::TabsChanged`],
//! [`CoreEvent::OpProgress`], [`CoreEvent::UndoChanged`],
//! [`CoreEvent::VolumesChanged`], [`CoreEvent::SettingsChanged`]) are coalesced
//! to at most one per kind per tab every [`COALESCE_WINDOW`]: the first one in a
//! quiet period goes out at once, later ones within the window are merged and
//! the newest goes out when the window ends. A listing that reaches a final
//! state (`Sorted`, `Resorted`, `Error`) is delivered at once, replacing any
//! held update, so the shell never waits to see a finished listing. Discrete
//! events (conflicts, failures, finished ops, hints) are never dropped; any
//! held state events are flushed before them so order is kept.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use fstudio_ops::{ConflictInfo, OpEvent, OpId, OpReport, OpStatus, UndoState};
use serde::{Deserialize, Serialize};

use crate::ids::TabId;

/// How long state events of one kind are held and merged.
pub const COALESCE_WINDOW: Duration = Duration::from_millis(16);

/// Why a tab's listing changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChangeKind {
    /// The first rows of a folder that is still being read.
    Partial,
    /// The folder is fully read and sorted.
    Sorted,
    /// Watcher changes were applied to the existing snapshot.
    Patched,
    /// The same entries under new view options (sort, hidden files, filter).
    Resorted,
    /// The folder could not be read; see [`crate::Listing::error`].
    Error,
}

impl ChangeKind {
    fn is_final(self) -> bool {
        matches!(
            self,
            ChangeKind::Sorted | ChangeKind::Resorted | ChangeKind::Error
        )
    }
}

/// A mounted volume, for the sidebar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeSummary {
    /// The name Finder shows.
    pub name: String,
    /// Where it is mounted.
    pub mount_path: String,
    /// Can be ejected (a card, a USB drive, a disk image).
    pub is_removable: bool,
    /// A network share.
    pub is_network: bool,
    /// Mounted read-only.
    pub is_read_only: bool,
    /// Capacity in bytes.
    pub total_bytes: u64,
    /// Free space in bytes.
    pub free_bytes: u64,
}

/// A "Did you know?" hint to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HintInfo {
    /// The command id.
    pub command: String,
    /// Its shortcut, as key-cap text.
    pub keys: String,
    /// What it does.
    pub label: String,
}

/// Something the shell should react to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CoreEvent {
    /// A tab has a new listing snapshot. Fetch rows with [`crate::Tab::rows`].
    ListingChanged {
        /// The tab.
        tab: TabId,
        /// The tab's listing version (monotonic per tab).
        version: u64,
        /// Why it changed.
        kind: ChangeKind,
    },
    /// A tab's selection or cursor changed.
    SelectionChanged {
        /// The tab.
        tab: TabId,
        /// How many rows are selected.
        count: u32,
        /// The cursor row, if any.
        cursor: Option<u32>,
    },
    /// Windows, panes or tabs were added, closed, moved or activated, or a tab's
    /// location changed. Re-read [`crate::Engine::workspace_state`].
    TabsChanged,
    /// Progress of running operations (newest status of each).
    OpProgress {
        /// One status per op that changed.
        ops: Vec<OpStatus>,
    },
    /// An operation needs a conflict decision.
    OpConflict {
        /// The op.
        op: OpId,
        /// What collides.
        conflict: ConflictInfo,
    },
    /// One item of an operation failed; the operation goes on.
    OpFailed {
        /// The op.
        op: OpId,
        /// The item.
        path: String,
        /// What went wrong, in plain words.
        error: String,
    },
    /// An operation finished.
    OpFinished {
        /// The op.
        op: OpId,
        /// Its summary.
        report: OpReport,
    },
    /// The Edit menu's undo/redo labels changed.
    UndoChanged {
        /// The new labels.
        state: UndoState,
    },
    /// Volumes were mounted or unmounted.
    VolumesChanged {
        /// Every mounted volume.
        volumes: Vec<VolumeSummary>,
    },
    /// Show a "Did you know?" hint.
    Hint {
        /// The hint.
        hint: HintInfo,
    },
    /// Settings changed; re-read [`crate::Engine::settings`].
    SettingsChanged,
    /// The sidebar favourites changed; re-read [`crate::Engine::favourites`].
    FavouritesChanged,
    /// Something for the shell to do that wasn't the direct answer to a
    /// command: the toast for a finished operation, built from what really
    /// happened (all done, some failed, none done). Handle it as an
    /// [`crate::Outcome`]'s `shell` request from the window holding `tab`.
    Shell {
        /// The tab the operation was started from, if any.
        tab: Option<TabId>,
        /// What to do.
        request: crate::outcome::ShellRequest,
    },
}

/// Receives core events. Implemented by the shell. Called on one pump
/// thread, in order; keep it quick (hop to the main thread and return).
pub trait EventSink: Send + Sync {
    /// Handles one event.
    fn on_event(&self, event: CoreEvent);
}

impl EventSink for Sender<CoreEvent> {
    fn on_event(&self, event: CoreEvent) {
        let _ = self.send(event);
    }
}

/// An event sink that drops everything.
pub struct NullEventSink;

impl EventSink for NullEventSink {
    fn on_event(&self, _event: CoreEvent) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Listing(TabId),
    Selection(TabId),
    Tabs,
    Progress,
    Undo,
    Volumes,
    Settings,
    Favourites,
}

fn key_of(e: &CoreEvent) -> Option<Key> {
    Some(match e {
        CoreEvent::ListingChanged { tab, .. } => Key::Listing(*tab),
        CoreEvent::SelectionChanged { tab, .. } => Key::Selection(*tab),
        CoreEvent::TabsChanged => Key::Tabs,
        CoreEvent::OpProgress { .. } => Key::Progress,
        CoreEvent::UndoChanged { .. } => Key::Undo,
        CoreEvent::VolumesChanged { .. } => Key::Volumes,
        CoreEvent::SettingsChanged => Key::Settings,
        CoreEvent::FavouritesChanged => Key::Favourites,
        _ => return None,
    })
}

enum Msg {
    Event(CoreEvent),
    Flush(Sender<()>),
}

/// The pure coalescing state, separate from the thread so it can be tested.
pub(crate) struct Coalescer {
    window: Duration,
    last_sent: HashMap<Key, Instant>,
    pending: Vec<(Key, Instant, CoreEvent)>,
}

impl Coalescer {
    pub(crate) fn new(window: Duration) -> Self {
        Self {
            window,
            last_sent: HashMap::new(),
            pending: Vec::new(),
        }
    }

    /// Takes one event; returns what to deliver now, in order.
    pub(crate) fn push(&mut self, e: CoreEvent, now: Instant) -> Vec<CoreEvent> {
        let Some(key) = key_of(&e) else {
            let mut out = self.drain_all();
            out.push(e);
            return out;
        };
        let is_final = matches!(&e, CoreEvent::ListingChanged { kind, .. } if kind.is_final());
        if is_final {
            self.pending.retain(|p| p.0 != key);
            self.last_sent.insert(key, now);
            return vec![e];
        }
        if let Some(p) = self.pending.iter_mut().find(|p| p.0 == key) {
            merge(&mut p.2, e);
            return Vec::new();
        }
        match self.last_sent.get(&key) {
            Some(t) if now.saturating_duration_since(*t) < self.window => {
                self.pending.push((key, *t + self.window, e));
                Vec::new()
            }
            _ => {
                self.last_sent.insert(key, now);
                vec![e]
            }
        }
    }

    /// The next time something held is due.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.pending.iter().map(|p| p.1).min()
    }

    /// Everything whose window has ended.
    pub(crate) fn due(&mut self, now: Instant) -> Vec<CoreEvent> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.pending.len() {
            if self.pending[i].1 <= now {
                let (k, _, e) = self.pending.remove(i);
                self.last_sent.insert(k, now);
                out.push(e);
            } else {
                i += 1;
            }
        }
        out
    }

    /// Everything held, at once.
    pub(crate) fn drain_all(&mut self) -> Vec<CoreEvent> {
        let now = Instant::now();
        let out: Vec<CoreEvent> = self.pending.drain(..).map(|p| p.2).collect();
        for e in &out {
            if let Some(k) = key_of(e) {
                self.last_sent.insert(k, now);
            }
        }
        out
    }
}

fn merge(held: &mut CoreEvent, new: CoreEvent) {
    match (held, new) {
        (CoreEvent::OpProgress { ops }, CoreEvent::OpProgress { ops: newer }) => {
            for s in newer {
                match ops.iter_mut().find(|o| o.id == s.id) {
                    Some(o) => *o = s,
                    None => ops.push(s),
                }
            }
        }
        (held, new) => *held = new,
    }
}

/// The sending side of the pump. Cheap to clone.
#[derive(Clone)]
pub(crate) struct EventHub {
    tx: Sender<Msg>,
}

impl EventHub {
    /// Starts the pump thread delivering to `sink`.
    pub(crate) fn start(sink: Arc<dyn EventSink>) -> Self {
        let (tx, rx) = unbounded();
        std::thread::Builder::new()
            .name("fstudio-events".into())
            .spawn(move || pump(rx, sink))
            .expect("spawn event pump");
        Self { tx }
    }

    /// Queues an event.
    pub(crate) fn emit(&self, e: CoreEvent) {
        let _ = self.tx.send(Msg::Event(e));
    }

    /// Delivers everything held and waits until the sink has seen it.
    pub(crate) fn flush(&self) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        if self.tx.send(Msg::Flush(tx)).is_ok() {
            let _ = rx.recv_timeout(Duration::from_secs(5));
        }
    }
}

fn pump(rx: Receiver<Msg>, sink: Arc<dyn EventSink>) {
    let mut c = Coalescer::new(COALESCE_WINDOW);
    loop {
        let msg = match c.next_deadline() {
            Some(d) => match rx.recv_deadline(d) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            },
            None => match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => break,
            },
        };
        let now = Instant::now();
        let out = match msg {
            Some(Msg::Event(e)) => {
                let mut out = c.due(now);
                out.extend(c.push(e, now));
                out
            }
            Some(Msg::Flush(ack)) => {
                for e in c.drain_all() {
                    sink.on_event(e);
                }
                let _ = ack.send(());
                continue;
            }
            None => c.due(now),
        };
        for e in out {
            sink.on_event(e);
        }
    }
    for e in c.drain_all() {
        sink.on_event(e);
    }
}

/// Forwards queue events into the hub as core events.
pub(crate) struct OpsBridge {
    pub(crate) hub: EventHub,
    /// Turns finished ops into toasts.
    pub(crate) toaster: Arc<crate::feedback::Toaster>,
}

impl fstudio_ops::EventSink for OpsBridge {
    fn on_event(&self, e: OpEvent) {
        if let OpEvent::OpFinished { report, .. } = &e {
            // The queue's own event first, then the toast built from it.
            self.hub.emit(CoreEvent::OpFinished {
                op: report.id,
                report: report.clone(),
            });
            self.toaster.finished(report);
            return;
        }
        self.hub.emit(match e {
            OpEvent::OpProgress(s) => CoreEvent::OpProgress { ops: vec![s] },
            OpEvent::OpConflict { op, conflict } => CoreEvent::OpConflict { op, conflict },
            OpEvent::OpItemFailed { op, error } => CoreEvent::OpFailed {
                op,
                path: error.path.to_string_lossy().into_owned(),
                error: error.message,
            },
            OpEvent::OpFinished { op, report } => CoreEvent::OpFinished { op, report },
            OpEvent::UndoChanged(state) => CoreEvent::UndoChanged { state },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(tab: u64, kind: ChangeKind, version: u64) -> CoreEvent {
        CoreEvent::ListingChanged {
            tab: TabId(tab),
            version,
            kind,
        }
    }

    #[test]
    fn first_goes_out_then_held_then_newest_wins() {
        let mut c = Coalescer::new(Duration::from_millis(16));
        let t0 = Instant::now();
        assert_eq!(c.push(listing(1, ChangeKind::Patched, 1), t0).len(), 1);
        assert!(c.push(listing(1, ChangeKind::Patched, 2), t0).is_empty());
        assert!(c.push(listing(1, ChangeKind::Patched, 3), t0).is_empty());
        // Another tab is independent.
        assert_eq!(c.push(listing(2, ChangeKind::Patched, 1), t0).len(), 1);
        assert!(c.due(t0 + Duration::from_millis(5)).is_empty());
        let out = c.due(t0 + Duration::from_millis(17));
        assert_eq!(out, vec![listing(1, ChangeKind::Patched, 3)]);
    }

    #[test]
    fn final_listing_is_immediate_and_replaces_held() {
        let mut c = Coalescer::new(Duration::from_millis(16));
        let t0 = Instant::now();
        c.push(listing(1, ChangeKind::Partial, 1), t0);
        assert!(c.push(listing(1, ChangeKind::Partial, 2), t0).is_empty());
        let out = c.push(listing(1, ChangeKind::Sorted, 3), t0);
        assert_eq!(out, vec![listing(1, ChangeKind::Sorted, 3)]);
        assert!(c.next_deadline().is_none());
    }

    #[test]
    fn discrete_events_flush_held_ones_first() {
        let mut c = Coalescer::new(Duration::from_millis(16));
        let t0 = Instant::now();
        c.push(CoreEvent::TabsChanged, t0);
        c.push(CoreEvent::TabsChanged, t0);
        let h = CoreEvent::Hint {
            hint: HintInfo {
                command: "x".into(),
                keys: "⌘X".into(),
                label: "X".into(),
            },
        };
        let out = c.push(h.clone(), t0);
        assert_eq!(out, vec![CoreEvent::TabsChanged, h]);
    }
}

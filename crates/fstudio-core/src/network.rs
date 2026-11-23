//! Connect to Server (SMB) and the sidebar's Network section.
//!
//! - **Connecting** (`⌘K`, a typed `smb://` address, a click on a server in
//!   the sidebar) parses the address ([`fstudio_net::parse_server`]), adds it
//!   to the recent servers, and mounts the share with macOS's NetFS, which
//!   shows the system's own sign-in window and share picker. When the mount
//!   finishes, the tab that asked goes to the share. A share that is already
//!   mounted opens at once, without asking NetFS again.
//! - **Saved servers** are addresses the user pinned to the sidebar; a click
//!   connects again. **Recent servers** are the last ones connected to, for
//!   the Connect to Server sheet. Both store the URL only, never a password
//!   (a password typed into an address is dropped before anything is stored).
//! - **Nearby computers** come from Bonjour (`_smb._tcp`), once the sidebar
//!   asks for them ([`Engine::start_network_browsing`]).
//! - **Shares** are the mounted network volumes, each with an eject button.
//!
//! Every change is announced with [`CoreEvent::NetworkChanged`].

use std::path::PathBuf;
use std::sync::{Arc, Weak};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use fstudio_net::{BrowseEvent, Browser, MountError, ServerAddress};

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::events::{CoreEvent, VolumeSummary};
use crate::ids::TabId;
use crate::outcome::{Outcome, ShellRequest, ToastStyle};
use crate::shared::Shared;
use crate::tab::Tab;

/// The key-value key for saved servers.
pub(crate) const SAVED_KEY: &str = "core.network.saved";
/// The key-value key for recent servers.
pub(crate) const RECENT_KEY: &str = "core.network.recent";
/// How many recent servers are kept.
pub const RECENT_SERVERS_MAX: usize = 10;

/// Where a server in the Network section comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerKind {
    /// Found on the local network through Bonjour.
    Nearby,
    /// Saved by the user.
    Saved,
    /// Connected to recently.
    Recent,
}

/// A server or share address, for the sidebar and the Connect sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerEntry {
    /// The canonical address (`smb://archbox/media`), never with a password.
    pub url: String,
    /// The name to show ("media on archbox", "Arch Box").
    pub name: String,
    /// Where it comes from.
    pub kind: ServerKind,
    /// A connection to it is in progress.
    #[serde(default, skip_serializing)]
    pub connecting: bool,
    /// The share is mounted here (for addresses with a share).
    #[serde(default, skip_serializing)]
    pub mounted_at: Option<String>,
    /// Also in the saved servers (for nearby and recent entries).
    #[serde(default, skip_serializing)]
    pub saved: bool,
}

/// Everything the sidebar's Network section shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NetworkSidebar {
    /// Nearby SMB computers (Bonjour), sorted by name.
    pub computers: Vec<ServerEntry>,
    /// Saved servers, in the order they were saved.
    pub saved: Vec<ServerEntry>,
    /// Mounted network volumes, each ejectable.
    pub shares: Vec<VolumeSummary>,
    /// Why nearby computers can't be listed (local network access refused),
    /// for a hint under the section. `None` when browsing works or hasn't
    /// started.
    pub browse_problem: Option<String>,
}

/// One stored address (saved or recent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Stored {
    url: String,
    name: String,
}

/// A connection in progress: its mount, once NetFS has started it.
type MountSlot = Arc<Mutex<Option<fstudio_net::MountHandle>>>;

/// The network state the engine and its tabs share.
#[derive(Debug, Default)]
pub(crate) struct Network {
    computers: Mutex<Vec<fstudio_net::Computer>>,
    browse_problem: Mutex<Option<String>>,
    browser: Mutex<Option<Browser>>,
    /// Share keys being connected, with the mount to cancel.
    connecting: Mutex<Vec<(String, MountSlot)>>,
}

fn load(db: &fstudio_db::Db, key: &str) -> Vec<Stored> {
    db.kv_get(key)
        .ok()
        .flatten()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn store(db: &fstudio_db::Db, key: &str, list: &[Stored]) -> Result<()> {
    db.kv_set(key, &serde_json::to_string(list)?)?;
    Ok(())
}

/// Parses a typed address into the core's error type.
pub(crate) fn parse(text: &str) -> Result<ServerAddress> {
    fstudio_net::parse_server(text).map_err(|e| CoreError::InvalidName(e.to_string()))
}

/// Puts `entry` first in `list` (dropping any other entry for the same
/// address) and keeps at most `max`. Pure.
fn push_front(list: &mut Vec<Stored>, entry: Stored, max: usize) {
    list.retain(|s| s.url != entry.url);
    list.insert(0, entry);
    list.truncate(max);
}

/// Where to go once `a` is mounted at one of `mounts`: the first mount
/// point, plus the folders the address names inside the share when they
/// exist (else the share itself). Pure apart from one `is_dir` check.
fn destination(a: &ServerAddress, mounts: &[PathBuf]) -> Option<PathBuf> {
    let root = mounts.first()?;
    let inner = a.path.iter().fold(root.clone(), |p, f| p.join(f));
    Some(if inner.is_dir() { inner } else { root.clone() })
}

impl Network {
    fn mounted(key: &str) -> Option<PathBuf> {
        fstudio_net::network_mounts()
            .ok()?
            .into_iter()
            .find(|m| m.key == key)
            .map(|m| m.mount_path)
    }

    fn is_connecting(&self, key: &str) -> bool {
        self.connecting.lock().iter().any(|(k, _)| k == key)
    }

    /// Connects to the typed address and, when mounted, sends `target` there.
    /// Returns at once; the outcome says what is happening.
    pub(crate) fn connect(
        &self,
        shared: &Arc<Shared>,
        text: &str,
        target: Weak<Tab>,
        tab: Option<TabId>,
    ) -> Result<Outcome> {
        let a = parse(text)?;
        let what = a.display_name();
        self.remember_recent(shared, &a)?;
        let note = if a.dropped_password {
            " The password in the address wasn’t used; macOS asks for it instead."
        } else {
            ""
        };
        // Already mounted: just go there.
        if a.share.is_some()
            && let Some(m) = Self::mounted(&a.key())
            && let Some(dest) = destination(&a, &[m])
            && let Some(t) = target.upgrade()
        {
            let mut out = t.go(&dest.to_string_lossy())?;
            if !note.is_empty() {
                out.message = Some(note.trim().to_string());
            }
            return Ok(out);
        }
        let key = a.key();
        if self.is_connecting(&key) {
            return Ok(
                Outcome::message(format!("Already connecting to “{what}”…")).with_tab_opt(tab)
            );
        }
        let slot: MountSlot = Arc::default();
        self.connecting.lock().push((key.clone(), slot.clone()));
        shared.emit(CoreEvent::NetworkChanged);
        let weak_shared = Arc::downgrade(shared);
        let done_key = key.clone();
        let addr = a.clone();
        let url = if a.share.is_some() {
            a.share_url()
        } else {
            a.server_url()
        };
        let started = fstudio_net::mount_url(&url, move |r| {
            let Some(shared) = weak_shared.upgrade() else {
                return;
            };
            shared
                .network
                .finish(&shared, &done_key, &addr, r, target, tab);
        });
        match started {
            Ok(h) => {
                *slot.lock() = Some(h);
                Ok(Outcome::message(format!("Connecting to “{what}”…{note}")).with_tab_opt(tab))
            }
            Err(e) => {
                self.connecting.lock().retain(|(k, _)| *k != key);
                shared.emit(CoreEvent::NetworkChanged);
                Err(CoreError::NotAvailable(
                    e.message(&what).unwrap_or_else(|| "Cancelled".into()),
                ))
            }
        }
    }

    /// A mount finished (on NetFS's dispatch queue).
    fn finish(
        &self,
        shared: &Arc<Shared>,
        key: &str,
        a: &ServerAddress,
        r: std::result::Result<Vec<PathBuf>, MountError>,
        target: Weak<Tab>,
        tab: Option<TabId>,
    ) {
        self.connecting.lock().retain(|(k, _)| k != key);
        shared.emit(CoreEvent::NetworkChanged);
        let what = a.display_name();
        let mounts = match r {
            // NetFS said "already mounted" without saying where: look it up.
            Ok(m) if m.is_empty() => Self::mounted(key).into_iter().collect(),
            Ok(m) => m,
            Err(e) => {
                if let Some(text) = e.message(&what) {
                    shared.emit(CoreEvent::Shell {
                        tab,
                        request: crate::feedback::warning_toast(text),
                    });
                }
                return;
            }
        };
        let Some(dest) = destination(a, &mounts) else {
            return;
        };
        match target.upgrade() {
            Some(t) => {
                let _ = t.go(&dest.to_string_lossy());
            }
            None => shared.emit(CoreEvent::Shell {
                tab: None,
                request: ShellRequest::Toast {
                    text: format!("Connected to “{what}”"),
                    button: None,
                    command: None,
                    style: ToastStyle::Success,
                },
            }),
        }
    }

    /// Stops a connection in progress (the system's sign-in window closes).
    fn cancel(&self, key: &str) -> bool {
        let mut list = self.connecting.lock();
        let Some(i) = list.iter().position(|(k, _)| k == key) else {
            return false;
        };
        let (_, slot) = list.remove(i);
        if let Some(h) = slot.lock().take() {
            h.cancel();
        }
        true
    }

    fn remember_recent(&self, shared: &Shared, a: &ServerAddress) -> Result<()> {
        let mut list = load(&shared.db, RECENT_KEY);
        push_front(
            &mut list,
            Stored {
                url: a.url(),
                name: a.display_name(),
            },
            RECENT_SERVERS_MAX,
        );
        store(&shared.db, RECENT_KEY, &list)?;
        shared.emit(CoreEvent::NetworkChanged);
        Ok(())
    }

    fn entry(&self, s: Stored, kind: ServerKind, saved: &[Stored]) -> ServerEntry {
        let a = fstudio_net::parse_server(&s.url).ok();
        let key = a.as_ref().map(|a| a.key());
        ServerEntry {
            connecting: key.as_deref().is_some_and(|k| self.is_connecting(k)),
            mounted_at: a
                .as_ref()
                .filter(|a| a.share.is_some())
                .and_then(|a| Self::mounted(&a.key()))
                .map(|p| p.to_string_lossy().into_owned()),
            saved: kind == ServerKind::Saved || saved.iter().any(|x| x.url == s.url),
            url: s.url,
            name: s.name,
            kind,
        }
    }

    fn start_browsing(&self, shared: &Arc<Shared>) {
        let mut b = self.browser.lock();
        if b.is_some() {
            return;
        }
        let weak = Arc::downgrade(shared);
        let started = Browser::smb(move |ev| {
            let Some(shared) = weak.upgrade() else {
                return;
            };
            match ev {
                BrowseEvent::Changed(list) => {
                    *shared.network.computers.lock() = list;
                    *shared.network.browse_problem.lock() = None;
                }
                BrowseEvent::Failed(code) => {
                    let text = browse_problem_text(code);
                    let mut p = shared.network.browse_problem.lock();
                    if p.as_deref() == Some(text.as_str()) {
                        return;
                    }
                    *p = Some(text);
                }
            }
            shared.emit(CoreEvent::NetworkChanged);
        });
        if let Ok(br) = started {
            *b = Some(br);
        }
    }
}

/// A sentence for a Bonjour error: refused local network access gets advice.
fn browse_problem_text(code: i32) -> String {
    // kDNSServiceErr_PolicyDenied
    if code == -65570 {
        "Nearby computers can’t be shown: allow File Studio in System Settings › Privacy & Security › Local Network.".into()
    } else {
        format!("Nearby computers can’t be shown right now (Bonjour error {code}).")
    }
}

impl Outcome {
    fn with_tab_opt(self, tab: Option<TabId>) -> Self {
        Self { tab, ..self }
    }
}

impl Engine {
    /// Connects to a server (Connect to Server…, a typed `smb://` address, a
    /// click on a server in the sidebar): `archbox/media`, `smb://archbox`,
    /// `\\archbox\media`. The share is mounted by the system (which asks for
    /// a password when one is needed), then `tab` (or the focused tab) goes
    /// to it. An address without a share lets the system ask which shares to
    /// mount. A share that is already mounted opens at once.
    ///
    /// Returns at once with "Connecting to …"; a failure arrives later as a
    /// warning toast ([`CoreEvent::Shell`]). Never blocks on the network.
    pub fn connect_server(&self, address: &str, tab: Option<TabId>) -> Result<Outcome> {
        let t = self.target(tab)?;
        let id = t.id();
        self.shared
            .network
            .connect(&self.shared, address, Arc::downgrade(&t), Some(id))
    }

    /// Stops connecting to `address` (the system's sign-in window closes).
    /// `false` when no connection to it was in progress.
    pub fn cancel_connect(&self, address: &str) -> Result<bool> {
        let a = parse(address)?;
        let stopped = self.shared.network.cancel(&a.key());
        if stopped {
            self.shared.emit(CoreEvent::NetworkChanged);
        }
        Ok(stopped)
    }

    /// The canonical form of a typed address (`smb://archbox/media`), or why
    /// it can't be used. The Connect sheet checks as the user types.
    pub fn check_server_address(&self, address: &str) -> Result<String> {
        Ok(parse(address)?.url())
    }

    /// The servers connected to recently, newest first (at most
    /// [`RECENT_SERVERS_MAX`]).
    pub fn recent_servers(&self) -> Vec<ServerEntry> {
        let saved = load(&self.db, SAVED_KEY);
        load(&self.db, RECENT_KEY)
            .into_iter()
            .map(|s| self.shared.network.entry(s, ServerKind::Recent, &saved))
            .collect()
    }

    /// Forgets the recent servers.
    pub fn clear_recent_servers(&self) -> Result<()> {
        store(&self.db, RECENT_KEY, &[])?;
        self.shared.emit(CoreEvent::NetworkChanged);
        Ok(())
    }

    /// Saves a server address in the sidebar's Network section (at the end;
    /// saving one that is there already does nothing). Only the address is
    /// stored; a password in it is dropped.
    pub fn save_server(&self, address: &str) -> Result<()> {
        let a = parse(address)?;
        let url = a.url();
        let mut list = load(&self.db, SAVED_KEY);
        if list.iter().any(|s| s.url == url) {
            return Ok(());
        }
        list.push(Stored {
            url,
            name: a.display_name(),
        });
        store(&self.db, SAVED_KEY, &list)?;
        self.shared.emit(CoreEvent::NetworkChanged);
        Ok(())
    }

    /// Removes a saved server (by its address, in any spelling). The share
    /// stays mounted if it is.
    pub fn remove_saved_server(&self, address: &str) -> Result<()> {
        let url = parse(address)?.url();
        let mut list = load(&self.db, SAVED_KEY);
        let before = list.len();
        list.retain(|s| s.url != url);
        if list.len() == before {
            return Err(CoreError::NotFound(format!("saved server {url}")));
        }
        store(&self.db, SAVED_KEY, &list)?;
        self.shared.emit(CoreEvent::NetworkChanged);
        Ok(())
    }

    /// Starts looking for SMB computers on the local network (Bonjour). The
    /// sidebar calls this when it first shows; macOS may ask the user for
    /// local network access. Calling it again does nothing.
    pub fn start_network_browsing(&self) {
        self.shared.network.start_browsing(&self.shared);
    }

    /// Everything the sidebar's Network section shows. Reads the mount table
    /// (`getfsstat` without waiting on servers); never touches the network.
    pub fn network_sidebar(&self) -> NetworkSidebar {
        let n = &self.shared.network;
        let saved_list = load(&self.db, SAVED_KEY);
        let computers = n
            .computers
            .lock()
            .iter()
            .map(|c| {
                n.entry(
                    Stored {
                        url: c.url.clone(),
                        name: c.name.clone(),
                    },
                    ServerKind::Nearby,
                    &saved_list,
                )
            })
            .collect();
        let saved = saved_list
            .iter()
            .cloned()
            .map(|s| n.entry(s, ServerKind::Saved, &saved_list))
            .collect();
        NetworkSidebar {
            computers,
            saved,
            shares: self
                .volumes()
                .into_iter()
                .filter(|v| v.is_network)
                .collect(),
            browse_problem: n.browse_problem.lock().clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(url: &str) -> Stored {
        Stored {
            url: url.into(),
            name: url.into(),
        }
    }

    #[test]
    fn recents_are_newest_first_without_repeats_and_capped() {
        let mut l = Vec::new();
        for i in 0..15 {
            push_front(&mut l, st(&format!("smb://h/{i}")), RECENT_SERVERS_MAX);
        }
        assert_eq!(l.len(), RECENT_SERVERS_MAX);
        assert_eq!(l[0].url, "smb://h/14");
        push_front(&mut l, st("smb://h/10"), RECENT_SERVERS_MAX);
        assert_eq!(l[0].url, "smb://h/10");
        assert_eq!(l.iter().filter(|s| s.url == "smb://h/10").count(), 1);
    }

    #[test]
    fn the_destination_is_the_folder_inside_the_share_when_it_exists() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("Photos/2026")).unwrap();
        let a = parse("archbox/media/Photos/2026").unwrap();
        assert_eq!(
            destination(&a, &[d.path().to_path_buf()]),
            Some(d.path().join("Photos/2026"))
        );
        let gone = parse("archbox/media/Nope").unwrap();
        assert_eq!(
            destination(&gone, &[d.path().to_path_buf()]),
            Some(d.path().to_path_buf())
        );
        assert_eq!(destination(&a, &[]), None);
    }

    #[test]
    fn refused_local_network_access_says_where_to_allow_it() {
        assert!(browse_problem_text(-65570).contains("Local Network"));
        assert!(browse_problem_text(-1).contains("-1"));
    }
}

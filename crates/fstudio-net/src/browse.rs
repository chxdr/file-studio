//! SMB computers on the local network, found through Bonjour (`_smb._tcp`).
//!
//! Macs with File Sharing on, NAS boxes and Linux machines running Samba
//! with Avahi announce themselves this way. A computer seen on two network
//! interfaces (Wi-Fi and Ethernet) is listed once, and stays listed until
//! it has gone from both.
//!
//! The app needs local network access for this (macOS asks once; the app's
//! Info.plist names `_smb._tcp` in `NSBonjourServices`). Refused access is
//! reported as [`BrowseEvent::Failed`], and the list stays empty.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::address::ServerAddress;
use crate::sys::{BrowseReply, DnsBrowse};

/// The Bonjour service type of SMB file servers.
pub const SMB_SERVICE: &str = "_smb._tcp";

/// How long one wait for answers lasts; also how quickly a dropped
/// [`Browser`] stops.
const POLL_MS: i32 = 250;

/// A computer that offers SMB shares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Computer {
    /// Its name as announced ("Arch Box").
    pub name: String,
    /// The Bonjour domain ("local.").
    pub domain: String,
    /// The address to connect to: `smb://Arch%20Box._smb._tcp.local`, which
    /// macOS resolves through Bonjour (so it works without DNS).
    pub url: String,
}

impl Computer {
    fn new(name: &str, domain: &str) -> Self {
        let d = domain.trim_end_matches('.');
        let d = if d.is_empty() { "local" } else { d };
        let a = ServerAddress {
            host: format!("{name}.{SMB_SERVICE}.{d}"),
            port: None,
            user: None,
            share: None,
            path: Vec::new(),
            dropped_password: false,
        };
        Computer {
            name: name.to_string(),
            domain: domain.to_string(),
            url: a.url(),
        }
    }
}

/// What the browser saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowseEvent {
    /// The list of computers changed; here is all of it, sorted by name.
    Changed(Vec<Computer>),
    /// Browsing failed with a dns_sd error (`-65570`: local network access
    /// was refused). The browser tries again a little later.
    Failed(i32),
}

/// The computers seen so far, by (name, domain), with the interfaces each
/// was seen on. Pure, so it can be tested without a network.
#[derive(Debug, Default)]
pub(crate) struct Seen {
    by: BTreeMap<(String, String), BTreeSet<u32>>,
}

impl Seen {
    /// Applies one reply; `true` when the list of computers changed.
    pub(crate) fn apply(&mut self, r: &BrowseReply) -> bool {
        let k = (r.name.clone(), r.domain.clone());
        if r.added {
            let set = self.by.entry(k).or_default();
            let was_empty = set.is_empty();
            set.insert(r.interface);
            was_empty
        } else {
            let Some(set) = self.by.get_mut(&k) else {
                return false;
            };
            set.remove(&r.interface);
            if set.is_empty() {
                self.by.remove(&k);
                true
            } else {
                false
            }
        }
    }

    /// The computers, sorted by name ignoring case.
    pub(crate) fn computers(&self) -> Vec<Computer> {
        let mut v: Vec<Computer> = self.by.keys().map(|(n, d)| Computer::new(n, d)).collect();
        v.sort_by_key(|c| c.name.to_lowercase());
        v
    }
}

/// A running Bonjour browse on its own thread. Dropping it stops the
/// thread within a quarter of a second.
#[derive(Debug)]
pub struct Browser {
    stop: Arc<AtomicBool>,
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Browser {
    /// Starts looking for SMB computers. `on_event` runs on the browser's
    /// thread each time the list changes (and when browsing fails).
    pub fn smb(on_event: impl FnMut(BrowseEvent) + Send + 'static) -> std::io::Result<Browser> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::Builder::new()
            .name("fstudio-bonjour".into())
            .spawn(move || run(SMB_SERVICE, &flag, on_event))?;
        Ok(Browser { stop })
    }
}

fn run(service: &str, stop: &AtomicBool, mut on_event: impl FnMut(BrowseEvent)) {
    let mut seen = Seen::default();
    while !stop.load(Ordering::Relaxed) {
        let mut b = match DnsBrowse::start(service) {
            Ok(b) => b,
            Err(e) => {
                on_event(BrowseEvent::Failed(e));
                pause(stop, Duration::from_secs(10));
                continue;
            }
        };
        while !stop.load(Ordering::Relaxed) {
            match b.poll(POLL_MS) {
                Ok(replies) => {
                    let mut changed = false;
                    for r in &replies {
                        changed |= seen.apply(r);
                    }
                    if changed {
                        on_event(BrowseEvent::Changed(seen.computers()));
                    }
                }
                Err(e) => {
                    // The daemon restarted or the browse broke: start over.
                    on_event(BrowseEvent::Failed(e));
                    if !seen.computers().is_empty() {
                        seen = Seen::default();
                        on_event(BrowseEvent::Changed(Vec::new()));
                    }
                    pause(stop, Duration::from_secs(2));
                    break;
                }
            }
        }
    }
}

/// Sleeps up to `d`, waking early when `stop` is set.
fn pause(stop: &AtomicBool, d: Duration) {
    let step = Duration::from_millis(POLL_MS as u64);
    let mut left = d;
    while !stop.load(Ordering::Relaxed) && !left.is_zero() {
        let s = step.min(left);
        std::thread::sleep(s);
        left -= s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(added: bool, name: &str, interface: u32) -> BrowseReply {
        BrowseReply {
            added,
            name: name.into(),
            domain: "local.".into(),
            interface,
        }
    }

    #[test]
    fn a_computer_on_two_interfaces_is_listed_once_until_it_leaves_both() {
        let mut s = Seen::default();
        assert!(s.apply(&r(true, "Arch Box", 4)));
        assert!(!s.apply(&r(true, "Arch Box", 9)));
        assert_eq!(s.computers().len(), 1);
        assert!(!s.apply(&r(false, "Arch Box", 4)));
        assert_eq!(s.computers().len(), 1);
        assert!(s.apply(&r(false, "Arch Box", 9)));
        assert!(s.computers().is_empty());
        // A goodbye for something never seen changes nothing.
        assert!(!s.apply(&r(false, "Ghost", 1)));
    }

    #[test]
    fn computers_are_sorted_and_have_bonjour_urls() {
        let mut s = Seen::default();
        s.apply(&r(true, "nas", 1));
        s.apply(&r(true, "Arch Box", 1));
        let c = s.computers();
        assert_eq!(c[0].name, "Arch Box");
        assert_eq!(c[0].url, "smb://Arch%20Box._smb._tcp.local");
        assert_eq!(c[1].url, "smb://nas._smb._tcp.local");
        let a = crate::parse_server(&c[0].url).unwrap();
        assert_eq!(a.host_name(), "Arch Box");
    }

    #[test]
    fn a_dropped_browser_stops() {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let b = Browser::smb(move |_| {
            let _ = tx.send(());
        })
        .unwrap();
        drop(b);
        // The thread ends, dropping the sender: recv sees a disconnect.
        let end = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                _ if std::time::Instant::now() > end => panic!("browser thread didn't stop"),
                _ => {}
            }
        }
    }
}

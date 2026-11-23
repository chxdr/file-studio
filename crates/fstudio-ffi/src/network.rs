//! Connect to Server (SMB) and the sidebar's Network section. Plain mirrors
//! of `fstudio_core`'s network API; the decisions live there.

use fstudio_core as core;

use crate::types::FfiVolume;
use crate::{FfiEngine, FfiOutcome, R};

/// Where a server in the Network section comes from.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiServerKind {
    Nearby,
    Saved,
    Recent,
}

/// A server or share address.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiServer {
    /// The canonical address (`smb://archbox/media`), never with a password.
    pub url: String,
    pub name: String,
    pub kind: FfiServerKind,
    /// A connection is in progress.
    pub connecting: bool,
    /// Where the share is mounted, when it is.
    pub mounted_at: Option<String>,
    /// Also a saved server.
    pub saved: bool,
}

impl From<core::ServerEntry> for FfiServer {
    fn from(s: core::ServerEntry) -> Self {
        FfiServer {
            url: s.url,
            name: s.name,
            kind: match s.kind {
                core::ServerKind::Nearby => FfiServerKind::Nearby,
                core::ServerKind::Saved => FfiServerKind::Saved,
                core::ServerKind::Recent => FfiServerKind::Recent,
            },
            connecting: s.connecting,
            mounted_at: s.mounted_at,
            saved: s.saved,
        }
    }
}

/// Everything the sidebar's Network section shows.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct FfiNetworkSidebar {
    pub computers: Vec<FfiServer>,
    pub saved: Vec<FfiServer>,
    pub shares: Vec<FfiVolume>,
    /// Why nearby computers can't be listed, for a hint.
    pub browse_problem: Option<String>,
}

#[uniffi::export]
impl FfiEngine {
    /// Connects to a server address (`archbox/media`, `smb://archbox`) and
    /// opens the share in `tab` (or the focused tab) once mounted.
    pub fn connect_server(&self, address: String, tab: Option<u64>) -> R<FfiOutcome> {
        Ok(self
            .inner
            .connect_server(&address, tab.map(core::TabId))?
            .into())
    }
    /// Stops connecting to `address`; false when nothing was in progress.
    pub fn cancel_connect(&self, address: String) -> R<bool> {
        Ok(self.inner.cancel_connect(&address)?)
    }
    /// The canonical form of a typed address, or why it can't be used.
    pub fn check_server_address(&self, address: String) -> R<String> {
        Ok(self.inner.check_server_address(&address)?)
    }
    pub fn recent_servers(&self) -> Vec<FfiServer> {
        self.inner
            .recent_servers()
            .into_iter()
            .map(Into::into)
            .collect()
    }
    pub fn clear_recent_servers(&self) -> R<()> {
        Ok(self.inner.clear_recent_servers()?)
    }
    pub fn save_server(&self, address: String) -> R<()> {
        Ok(self.inner.save_server(&address)?)
    }
    pub fn remove_saved_server(&self, address: String) -> R<()> {
        Ok(self.inner.remove_saved_server(&address)?)
    }
    /// Starts Bonjour browsing for SMB computers (once).
    pub fn start_network_browsing(&self) {
        self.inner.start_network_browsing()
    }
    pub fn network_sidebar(&self) -> FfiNetworkSidebar {
        let n = self.inner.network_sidebar();
        FfiNetworkSidebar {
            computers: n.computers.into_iter().map(Into::into).collect(),
            saved: n.saved.into_iter().map(Into::into).collect(),
            shares: n.shares.into_iter().map(Into::into).collect(),
            browse_problem: n.browse_problem,
        }
    }
}

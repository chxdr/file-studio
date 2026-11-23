//! Which mounted network volume is which share.
//!
//! A network volume has no volume UUID, and its `st_dev` is handed out anew
//! at every mount, so neither says "this is the `media` share on `archbox`
//! again". What the volume was mounted from (`f_mntfromname`, like
//! `//milky@archbox/media`) does, and it is the same every time the share is
//! mounted from the same address. [`mount_key`] turns it into a key that
//! ignores the account and case; [`network_mounts`] lists the mounted
//! network volumes with their keys.

use std::io;
use std::path::PathBuf;

/// A mounted network volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkMount {
    /// Where it is mounted (`/Volumes/media`).
    pub mount_path: PathBuf,
    /// The filesystem (`smbfs`, `nfs`, `afpfs`, `webdav`).
    pub fs_type: String,
    /// What it was mounted from (`//milky@archbox/media`).
    pub mount_from: String,
    /// The share's key (see [`mount_key`]); equal to
    /// [`crate::ServerAddress::key`] for an address of the same share.
    pub key: String,
}

/// The key for host + share: host in lower case without a trailing dot,
/// share in lower case (SMB share names ignore case).
pub(crate) fn share_key(host: &str, share: Option<&str>) -> String {
    let h = host.trim_end_matches('.').to_lowercase();
    match share {
        Some(s) => format!("{h}/{}", s.to_lowercase()),
        None => h,
    }
}

/// The key of a mounted network volume, from its filesystem type and what
/// it was mounted from. For SMB (`smbfs`) it is the same as the key of the
/// address it was mounted from ([`crate::ServerAddress::key`]): host and
/// share, without the account. Other network filesystems get their type
/// and source, lower case. `None` for an empty source.
pub fn mount_key(fs_type: &str, mount_from: &str) -> Option<String> {
    if mount_from.is_empty() {
        return None;
    }
    if fs_type == "smbfs" {
        // `//user@host/share`; a `GUEST:` user has an empty password field.
        let a = crate::address::parse_server(&format!("smb:{mount_from}")).ok()?;
        return Some(a.key());
    }
    Some(format!("{fs_type}:{}", mount_from.to_lowercase()))
}

/// Every mounted network volume, without waiting on unresponsive ones
/// (`getfsstat(MNT_NOWAIT)`).
pub fn network_mounts() -> io::Result<Vec<NetworkMount>> {
    Ok(crate::sys::mounts()?
        .into_iter()
        .filter(|m| !m.local)
        .filter_map(|m| {
            Some(NetworkMount {
                key: mount_key(&m.fs_type, &m.mount_from)?,
                mount_path: m.mount_on,
                fs_type: m.fs_type,
                mount_from: m.mount_from,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_server;

    #[test]
    fn a_mounted_share_matches_the_address_it_came_from() {
        let typed = parse_server("smb://archbox/Media").unwrap();
        for from in [
            "//milky@archbox/media",
            "//GUEST:@ArchBox/MEDIA",
            "//archbox./media",
        ] {
            assert_eq!(
                mount_key("smbfs", from).as_deref(),
                Some(typed.key().as_str()),
                "{from}"
            );
        }
        // Bonjour names are percent-encoded in the mount source.
        let b = parse_server("smb://Arch Box._smb._tcp.local/media").unwrap();
        assert_eq!(
            mount_key("smbfs", "//milky@Arch%20Box._smb._tcp.local/media"),
            Some(b.key())
        );
        assert_ne!(mount_key("smbfs", "//archbox/photos"), Some(typed.key()));
    }

    #[test]
    fn other_network_filesystems_keep_their_source() {
        assert_eq!(
            mount_key("nfs", "NAS:/export/Media").as_deref(),
            Some("nfs:nas:/export/media")
        );
        assert_eq!(mount_key("smbfs", ""), None);
    }

    /// The listing works without any network volume mounted, and every
    /// entry it returns is a network one.
    #[test]
    fn network_mounts_lists_only_network_volumes() {
        let list = network_mounts().unwrap();
        assert!(
            list.iter()
                .all(|m| m.mount_path != std::path::Path::new("/"))
        );
    }
}

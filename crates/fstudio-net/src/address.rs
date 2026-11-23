//! Server addresses: what the user typed, as one canonical `smb://` URL.
//!
//! Accepted spellings, all meaning the same share:
//! - `smb://archbox/media`, `SMB://archbox/media/`, `cifs://archbox/media`
//! - `archbox/media`, `//archbox/media`
//! - `\\archbox\media` (a Windows UNC path, handy when the other computer
//!   is a PC)
//!
//! A user name (`smb://milky@archbox/media`) is kept, because it says which
//! account to sign in as. A password (`smb://milky:secret@archbox`) is
//! **dropped**, never stored or passed on: macOS asks for it in its own
//! sign-in window and can keep it in the Keychain.
//!
//! Only SMB is supported here; `afp://`, `nfs://`, `sftp://` and the rest
//! are refused with a clear message (the big version is M5).

use std::fmt;

/// A parsed server address. Text fields are decoded (no `%20`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerAddress {
    /// The computer: a name (`archbox`, `archbox.local`), a Bonjour service
    /// name (`Arch Box._smb._tcp.local`) or an IP address (IPv6 without
    /// brackets).
    pub host: String,
    /// A port other than SMB's own.
    pub port: Option<u16>,
    /// The account to sign in as, when the address names one.
    pub user: Option<String>,
    /// The share (`media`), or `None` to ask the server which shares to mount.
    pub share: Option<String>,
    /// Folders inside the share to open after mounting.
    pub path: Vec<String>,
    /// The typed address had a password in it, which was dropped.
    pub dropped_password: bool,
}

/// Why an address can't be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressError {
    /// Nothing was typed.
    Empty,
    /// Another protocol (`afp`, `nfs`, `sftp`, …).
    Unsupported(String),
    /// The address has no computer name, or one that can't be right.
    BadHost(String),
}

impl fmt::Display for AddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AddressError::Empty => write!(f, "Type a server address, like smb://archbox/media"),
            AddressError::Unsupported(s) => write!(
                f,
                "{s}:// servers aren’t supported yet; only SMB (smb://) is"
            ),
            AddressError::BadHost(h) if h.is_empty() => {
                write!(f, "The address has no server name")
            }
            AddressError::BadHost(h) => write!(f, "“{h}” isn’t a server name"),
        }
    }
}

impl std::error::Error for AddressError {}

/// Parses a typed server address. See the module docs for what is accepted.
pub fn parse_server(text: &str) -> Result<ServerAddress, AddressError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(AddressError::Empty);
    }
    let (rest, had_scheme) = match text.find("://") {
        Some(i) => {
            let scheme = text[..i].to_ascii_lowercase();
            if scheme != "smb" && scheme != "cifs" {
                return Err(AddressError::Unsupported(scheme));
            }
            (&text[i + 3..], true)
        }
        None => (text, false),
    };
    // `smb:///share` has no computer in it.
    if had_scheme && rest.starts_with('/') {
        return Err(AddressError::BadHost(String::new()));
    }
    // A UNC path (`\\host\share`) is the same with the other slash.
    let unc;
    let rest = if rest.starts_with("\\\\") {
        unc = rest.replace('\\', "/");
        unc.as_str()
    } else {
        rest
    };
    let rest = rest.trim_start_matches('/');
    let (authority, remainder) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, ""),
    };
    // `user[:password]@host[:port]`: the last `@` ends the user part.
    let (userinfo, hostport) = match authority.rfind('@') {
        Some(i) => (Some(&authority[..i]), &authority[i + 1..]),
        None => (None, authority),
    };
    let mut dropped_password = false;
    let user = userinfo
        .map(|u| match u.find(':') {
            Some(i) => {
                dropped_password = true;
                &u[..i]
            }
            None => u,
        })
        .map(decode)
        .filter(|u| !u.is_empty());
    let (host, port) = split_port(hostport);
    let host = decode(host);
    if host.is_empty()
        || host
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | '@'))
    {
        return Err(AddressError::BadHost(host));
    }
    let mut parts = remainder
        .split('/')
        .filter(|p| !p.is_empty())
        .map(decode)
        .collect::<Vec<_>>()
        .into_iter();
    let share = parts.next();
    Ok(ServerAddress {
        host,
        port,
        user,
        share,
        path: parts.collect(),
        dropped_password,
    })
}

/// `host:port`, `[v6]:port`, `[v6]` or `host`.
fn split_port(hp: &str) -> (&str, Option<u16>) {
    if let Some(inner) = hp.strip_prefix('[') {
        return match inner.find(']') {
            Some(i) => {
                let port = inner[i + 1..]
                    .strip_prefix(':')
                    .and_then(|p| p.parse().ok());
                (&inner[..i], port)
            }
            None => (inner, None),
        };
    }
    match hp.rfind(':') {
        // One colon followed by digits: a port. More colons: a bare IPv6.
        Some(i) if hp.matches(':').count() == 1 => match hp[i + 1..].parse::<u16>() {
            Ok(p) => (&hp[..i], Some(p)),
            Err(_) => (hp, None),
        },
        _ => (hp, None),
    }
}

impl ServerAddress {
    /// The canonical URL to mount: `smb://[user@]host[:port][/share[/path…]]`,
    /// percent-encoded. Never has a password in it.
    pub fn url(&self) -> String {
        let mut s = self.server_url();
        if let Some(share) = &self.share {
            s.push('/');
            s.push_str(&encode(share));
            for p in &self.path {
                s.push('/');
                s.push_str(&encode(p));
            }
        }
        s
    }

    /// The URL of the computer alone (no share), for "pick a share".
    pub fn server_url(&self) -> String {
        let mut s = String::from("smb://");
        if let Some(u) = &self.user {
            s.push_str(&encode(u));
            s.push('@');
        }
        if self.host.contains(':') {
            s.push('[');
            s.push_str(&self.host);
            s.push(']');
        } else {
            s.push_str(&encode(&self.host));
        }
        if let Some(p) = self.port {
            s.push_str(&format!(":{p}"));
        }
        s
    }

    /// The URL of the share alone (no folders inside it): what is mounted.
    pub fn share_url(&self) -> String {
        Self {
            path: Vec::new(),
            ..self.clone()
        }
        .url()
    }

    /// The computer's name for people: `Arch Box` for a Bonjour service
    /// name (`Arch Box._smb._tcp.local`), else the host as typed.
    pub fn host_name(&self) -> &str {
        self.host
            .strip_suffix("._smb._tcp.local")
            .or_else(|| self.host.strip_suffix("._smb._tcp.local."))
            .unwrap_or(&self.host)
    }

    /// A short name for the sidebar and messages: `media on archbox`, or the
    /// computer's name when no share is given.
    pub fn display_name(&self) -> String {
        match &self.share {
            Some(s) => format!("{s} on {}", self.host_name()),
            None => self.host_name().to_string(),
        }
    }

    /// What two addresses for the same share have in common: the host
    /// (lower case, no trailing dot) and the share (lower case; SMB share
    /// names don't care about case). User, port and folders don't count.
    pub fn key(&self) -> String {
        crate::identity::share_key(&self.host, self.share.as_deref())
    }
}

/// Characters left as they are in a URL part (RFC 3986 "unreserved").
fn unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
}

/// Percent-encodes one URL part (a host, user, share or folder name).
pub(crate) fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if unreserved(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Decodes `%xx` escapes. Broken escapes are kept as typed.
pub(crate) fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> ServerAddress {
        parse_server(s).unwrap()
    }

    #[test]
    fn every_spelling_of_one_share_is_the_same_url() {
        for s in [
            "smb://archbox/media",
            "SMB://archbox/media/",
            "cifs://archbox/media",
            "archbox/media",
            "//archbox/media",
            "\\\\archbox\\media",
            "  smb://archbox//media  ",
        ] {
            assert_eq!(p(s).url(), "smb://archbox/media", "{s}");
        }
    }

    #[test]
    fn a_bare_name_is_the_computer() {
        let a = p("archbox");
        assert_eq!(a.share, None);
        assert_eq!(a.url(), "smb://archbox");
        assert_eq!(a.display_name(), "archbox");
    }

    #[test]
    fn passwords_are_dropped_and_users_kept() {
        let a = p("smb://milky:hunter2@archbox/media");
        assert!(a.dropped_password);
        assert_eq!(a.user.as_deref(), Some("milky"));
        assert_eq!(a.url(), "smb://milky@archbox/media");
        assert!(!a.url().contains("hunter2"));
        // An empty password is still a password field.
        assert!(p("smb://milky:@archbox").dropped_password);
        assert!(!p("smb://milky@archbox").dropped_password);
    }

    #[test]
    fn spaces_and_unicode_are_encoded_and_round_trip() {
        // NFD "é" stays NFD (F-017: names as the system returns them).
        let nfd = "Cafe\u{301} Photos";
        let a = p(&format!("smb://Arch Box._smb._tcp.local/{nfd}/2026"));
        assert_eq!(a.share.as_deref(), Some(nfd));
        assert_eq!(a.path, vec!["2026".to_string()]);
        let url = a.url();
        assert!(url.starts_with("smb://Arch%20Box._smb._tcp.local/Cafe%CC%81%20Photos/2026"));
        assert_eq!(p(&url), ServerAddress { ..a.clone() });
        assert_eq!(a.host_name(), "Arch Box");
        assert_eq!(a.display_name(), format!("{nfd} on Arch Box"));
        assert_eq!(
            a.share_url(),
            "smb://Arch%20Box._smb._tcp.local/Cafe%CC%81%20Photos"
        );
    }

    #[test]
    fn ports_and_ipv6() {
        let a = p("smb://192.168.1.20:4445/share");
        assert_eq!(a.host, "192.168.1.20");
        assert_eq!(a.port, Some(4445));
        assert_eq!(a.url(), "smb://192.168.1.20:4445/share");
        let a = p("smb://[fe80::1]:445/share");
        assert_eq!(a.host, "fe80::1");
        assert_eq!(a.port, Some(445));
        assert_eq!(a.url(), "smb://[fe80::1]:445/share");
        let a = p("smb://fe80::1/share");
        assert_eq!(a.host, "fe80::1");
        assert_eq!(a.port, None);
    }

    #[test]
    fn other_protocols_and_nonsense_are_refused() {
        assert_eq!(
            parse_server("afp://old-mac/share"),
            Err(AddressError::Unsupported("afp".into()))
        );
        assert_eq!(
            parse_server("sftp://box"),
            Err(AddressError::Unsupported("sftp".into()))
        );
        assert_eq!(parse_server("   "), Err(AddressError::Empty));
        assert!(matches!(
            parse_server("smb:///share"),
            Err(AddressError::BadHost(_))
        ));
        assert!(matches!(
            parse_server("smb://milky@/share"),
            Err(AddressError::BadHost(_))
        ));
        assert!(
            AddressError::Unsupported("nfs".into())
                .to_string()
                .contains("nfs://")
        );
    }

    #[test]
    fn keys_ignore_case_user_port_and_folders() {
        assert_eq!(
            p("smb://milky@ArchBox:445/Media/x").key(),
            p("archbox/media").key()
        );
        assert_ne!(p("archbox/media").key(), p("archbox/photos").key());
        assert_ne!(p("archbox").key(), p("archbox/media").key());
    }

    #[test]
    fn broken_escapes_stay_as_typed() {
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("a%zzb"), "a%zzb");
        assert_eq!(decode("a%2"), "a%2");
        assert_eq!(decode("a%20b"), "a b");
    }
}

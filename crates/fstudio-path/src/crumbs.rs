//! Breadcrumbs for the resting address bar, overflow collapsing and the chevron menus.

use std::path::{Path, PathBuf};

use crate::source::{DirSource, is_package_name};

/// What a crumb represents (the shell picks its icon from this).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CrumbKind {
    /// The root of a volume; the crumb shows the volume's name.
    VolumeRoot,
    /// The user's home folder.
    Home,
    /// An ordinary folder.
    Folder,
    /// A package or bundle (`.app`...).
    Package,
}

/// One segment of the bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crumb {
    /// The label: the folder name, or the volume name for a volume root.
    pub name: String,
    /// The full path this crumb navigates to.
    pub path: PathBuf,
    /// What it is.
    pub kind: CrumbKind,
    /// True for the last crumb (the folder being shown).
    pub is_current: bool,
}

/// A mounted volume, as the shell reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    /// The name Finder shows (`Macintosh HD`).
    pub name: String,
    /// Where it is mounted (`/` or `/Volumes/Card`).
    pub mount: PathBuf,
}

/// Splits `path` into crumbs, starting at the volume root (Finder style): the
/// root crumb is named after the volume that contains the path (the longest
/// matching mount; `/` if none match), the home folder is marked `Home`, and `.app`
/// style folders are `Package`.
pub fn breadcrumbs(path: &Path, home: &Path, volumes: &[Volume]) -> Vec<Crumb> {
    let comps: Vec<String> = path
        .to_string_lossy()
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .map(str::to_owned)
        .collect();

    // The volume whose mount is the longest prefix of the path.
    let mut root_idx = 0; // number of leading components consumed by the volume mount
    let mut root_name = volumes
        .iter()
        .find(|v| v.mount == Path::new("/"))
        .map(|v| v.name.clone())
        .unwrap_or_else(|| "/".to_owned());
    let mut best = 0usize;
    for v in volumes {
        let mc: Vec<&str> = v
            .mount
            .to_str()
            .unwrap_or("")
            .split('/')
            .filter(|c| !c.is_empty())
            .collect();
        if mc.len() > best
            && mc.len() <= comps.len()
            && mc.iter().zip(&comps).all(|(a, b)| *a == b.as_str())
        {
            best = mc.len();
            root_idx = mc.len();
            root_name = v.name.clone();
        }
    }

    let mut out = Vec::with_capacity(comps.len() + 1);
    let mut cur = PathBuf::from("/");
    for c in comps.iter().take(root_idx) {
        cur.push(c);
    }
    out.push(Crumb {
        name: root_name,
        path: cur.clone(),
        kind: CrumbKind::VolumeRoot,
        is_current: false,
    });
    for c in comps.iter().skip(root_idx) {
        cur.push(c);
        let kind = if !home.as_os_str().is_empty() && cur == home {
            CrumbKind::Home
        } else if is_package_name(c) {
            CrumbKind::Package
        } else {
            CrumbKind::Folder
        };
        out.push(Crumb {
            name: c.clone(),
            path: cur.clone(),
            kind,
            is_current: false,
        });
    }
    if let Some(last) = out.last_mut() {
        last.is_current = true;
    }
    out
}

/// Collapses a long crumb list into `(head, hidden, tail)`.
///
/// Keeps the first crumb and the last `max_visible - 1`; the middle goes into
/// `hidden` (the shell shows it behind a `…` button listing the names). If
/// everything fits, `hidden` and `tail` are empty and `head` holds all crumbs.
/// `max_visible` is clamped to at least 2 (first and current).
pub fn collapse(crumbs: &[Crumb], max_visible: usize) -> (Vec<Crumb>, Vec<Crumb>, Vec<Crumb>) {
    let max_visible = max_visible.max(2);
    if crumbs.len() <= max_visible {
        return (crumbs.to_vec(), Vec::new(), Vec::new());
    }
    let tail_len = max_visible - 1;
    let split = crumbs.len() - tail_len;
    (
        crumbs[..1].to_vec(),
        crumbs[1..split].to_vec(),
        crumbs[split..].to_vec(),
    )
}

/// The sub-folders of `path` for a crumb's chevron menu, sorted by name. Hidden
/// folders are left out. Returns nothing for slow (network, cloud) folders or if
/// the folder cannot be read.
pub fn subfolders(path: &Path, source: &dyn DirSource) -> Vec<Crumb> {
    if source.may_be_slow(path) {
        return Vec::new();
    }
    let Ok(listing) = source.list_dirs(path) else {
        return Vec::new();
    };
    listing
        .children
        .iter()
        .filter(|c| !c.hidden)
        .map(|c| Crumb {
            name: c.name.clone(),
            path: path.join(&c.name),
            kind: if c.is_package {
                CrumbKind::Package
            } else {
                CrumbKind::Folder
            },
            is_current: false,
        })
        .collect()
}

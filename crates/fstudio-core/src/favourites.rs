//! Sidebar favourites: the folders the user pinned, in their order, with an
//! optional display name. Stored as JSON in the key-value table.
//!
//! Mounted volumes are not favourites; they come from [`Engine::volumes`].

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::events::CoreEvent;

/// The key-value key favourites live under.
pub(crate) const FAVOURITES_KEY: &str = "core.favourites";

/// One sidebar favourite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Favourite {
    /// The folder (or file) it points at.
    pub path: String,
    /// The name shown in the sidebar: the user's name for it, or the
    /// folder's own name.
    pub name: String,
    /// The user renamed it (so `name` is not just the folder's name).
    pub renamed: bool,
}

impl Favourite {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_string_lossy().into_owned(),
            name: display_name(path),
            renamed: false,
        }
    }
}

fn display_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}

/// The favourites a new install starts with (those that exist).
pub(crate) fn default_favourites(home: &Path) -> Vec<Favourite> {
    [
        PathBuf::from("/Applications"),
        home.join("Desktop"),
        home.join("Documents"),
        home.join("Downloads"),
        home.to_path_buf(),
    ]
    .iter()
    .filter(|p| p.is_dir())
    .map(|p| Favourite::new(p))
    .collect()
}

/// Loads the stored list, or the defaults when there is none yet.
pub(crate) fn load(db: &fstudio_db::Db, home: &Path) -> Vec<Favourite> {
    db.kv_get(FAVOURITES_KEY)
        .ok()
        .flatten()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| default_favourites(home))
}

/// Where `path` is in `list`, comparing exact strings.
fn position(list: &[Favourite], path: &str) -> Option<usize> {
    list.iter().position(|f| f.path == path)
}

impl Engine {
    /// The sidebar favourites, in order.
    pub fn favourites(&self) -> Vec<Favourite> {
        self.favourites.lock().clone()
    }

    /// Is `path` a favourite?
    pub fn is_favourite(&self, path: &str) -> bool {
        position(&self.favourites.lock(), path).is_some()
    }

    fn save_favourites(&self, list: Vec<Favourite>) -> Result<()> {
        self.db
            .kv_set(FAVOURITES_KEY, &serde_json::to_string(&list)?)?;
        *self.favourites.lock() = list;
        self.shared.emit(CoreEvent::FavouritesChanged);
        Ok(())
    }

    /// Adds `path` to the favourites at `index` (the end when `None` or past
    /// it). Adding one that is already there moves nothing and is not an error.
    pub fn add_favourite(&self, path: &str, index: Option<u32>) -> Result<()> {
        let mut list = self.favourites();
        if position(&list, path).is_some() {
            return Ok(());
        }
        let at = index
            .map(|i| i as usize)
            .unwrap_or(list.len())
            .min(list.len());
        list.insert(at, Favourite::new(Path::new(path)));
        self.save_favourites(list)
    }

    /// Removes a favourite. The folder itself is not touched.
    pub fn remove_favourite(&self, path: &str) -> Result<()> {
        let mut list = self.favourites();
        let i = position(&list, path)
            .ok_or_else(|| CoreError::NotFound(format!("favourite {path}")))?;
        list.remove(i);
        self.save_favourites(list)
    }

    /// Gives a favourite a display name. An empty name goes back to the
    /// folder's own name. The folder itself is not renamed.
    pub fn rename_favourite(&self, path: &str, name: &str) -> Result<()> {
        let mut list = self.favourites();
        let i = position(&list, path)
            .ok_or_else(|| CoreError::NotFound(format!("favourite {path}")))?;
        let name = name.trim();
        if name.is_empty() {
            list[i].name = display_name(Path::new(path));
            list[i].renamed = false;
        } else {
            list[i].name = name.to_string();
            list[i].renamed = true;
        }
        self.save_favourites(list)
    }

    /// Moves a favourite to `index` (clamped).
    pub fn move_favourite(&self, path: &str, index: u32) -> Result<()> {
        let mut list = self.favourites();
        let i = position(&list, path)
            .ok_or_else(|| CoreError::NotFound(format!("favourite {path}")))?;
        let f = list.remove(i);
        let at = (index as usize).min(list.len());
        list.insert(at, f);
        self.save_favourites(list)
    }

    /// Puts back the default favourites.
    pub fn reset_favourites(&self) -> Result<()> {
        self.save_favourites(default_favourites(&self.shared.home))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_only_list_existing_folders() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("Desktop")).unwrap();
        let f = default_favourites(d.path());
        let names: Vec<_> = f.iter().map(|f| f.path.clone()).collect();
        assert!(names.contains(&d.path().join("Desktop").to_string_lossy().into_owned()));
        assert!(!names.contains(&d.path().join("Documents").to_string_lossy().into_owned()));
    }
}

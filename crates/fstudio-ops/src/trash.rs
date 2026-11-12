//! Where trashed items go. The queue never deletes for "Move to Trash" or
//! "Replace": it calls a [`Trasher`], which reports the item's new location
//! so undo can put it back.

use std::io;
use std::path::{Path, PathBuf};

use crate::names;
use crate::sys;

/// Moves items to a trash and says where they went.
pub trait Trasher: Send + Sync {
    /// Moves `path` (a file, folder or symlink, never followed) to the trash and
    /// returns its location there.
    fn trash(&self, path: &Path) -> io::Result<PathBuf>;
}

/// The macOS Trash, through `NSFileManager trashItemAtURL:resultingItemURL:error:`,
/// so Finder's Put Back works. (F-008 had the shell do this; doing it in Rust
/// lets the CLI trash too.)
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemTrasher;

impl Trasher for SystemTrasher {
    fn trash(&self, path: &Path) -> io::Result<PathBuf> {
        sys::trash::trash_item(path)
    }
}

/// A trash that is just a folder, for tests and headless tools. Items are
/// renamed into it (never copied), so it must be on the same volume as what
/// it trashes; clashing names get " 2", " 3"… as in the real Trash.
#[derive(Debug, Clone)]
pub struct DirTrasher {
    dir: PathBuf,
}

impl DirTrasher {
    /// Uses (and creates) `dir` as the trash.
    pub fn new(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }
    /// The trash folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Trasher for DirTrasher {
    fn trash(&self, path: &Path) -> io::Result<PathBuf> {
        let md = std::fs::symlink_metadata(path)?;
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?
            .to_string_lossy()
            .into_owned();
        let first = std::iter::once(name.clone());
        for candidate in first.chain(names::keep_both_names(&name, md.is_dir())) {
            let to = self.dir.join(&candidate);
            match sys::rename_excl(path, &to) {
                Ok(_) => return Ok(to),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        unreachable!("keep_both_names is infinite")
    }
}

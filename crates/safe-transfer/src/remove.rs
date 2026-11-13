//! **The only place in Safe Transfer that removes anything** (F-007). A test
//! (`tests/safety_grep.rs`) fails if `remove_file`, `remove_dir`,
//! `remove_dir_all`, `unlink` or `rmdir` appear anywhere else in the crate.
//!
//! There is exactly one way to remove something: [`remove_partial`], for a
//! partial file Safe Transfer itself wrote. Safe Transfer never removes a
//! user's file; a move sends the original to the Trash instead.

use std::io;
use std::path::{Path, PathBuf};

use crate::names::{JobId, is_partial_name, partial_path};
use crate::platform::Platform;

/// Which partial a removal is for: file `n` of job `job`, whose real name
/// would be `final_path`. Only Safe Transfer's own pipeline creates one.
#[derive(Debug, Clone)]
pub struct PartialOwner {
    pub(crate) final_path: PathBuf,
    pub(crate) job: JobId,
    pub(crate) n: u32,
}

impl PartialOwner {
    /// The partial's path.
    pub fn path(&self) -> PathBuf {
        partial_path(&self.final_path, self.job, self.n)
    }
}

/// Removes the partial of `owner`, and its `._` AppleDouble companion on
/// exFAT and FAT. Refuses any path that isn't exactly the partial derived
/// from the owner, so it can never remove a user's file. ACLs and immutable
/// flags are cleared first (a copied `deny delete` would block removal).
/// A missing partial is not an error.
pub fn remove_partial(platform: &Platform, path: &Path, owner: &PartialOwner) -> io::Result<()> {
    if path != owner.path() || !is_partial_name(path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "refusing to remove {}: it isn't this file's partial",
                path.display()
            ),
        ));
    }
    match std::fs::symlink_metadata(path) {
        Ok(md) => {
            if !md.file_type().is_symlink() {
                let _ = platform.meta.clear_security(path);
            }
            std::fs::remove_file(path)?;
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        let companion = path.with_file_name(format!("._{name}"));
        if std::fs::symlink_metadata(&companion).is_ok_and(|m| m.is_file()) {
            std::fs::remove_file(&companion)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(dir: &Path) -> PartialOwner {
        PartialOwner {
            final_path: dir.join("photo.ARW"),
            job: JobId::from_random([1, 2, 3, 4, 5]),
            n: 0,
        }
    }

    #[test]
    fn removes_only_its_own_partial() {
        let d = tempfile::tempdir().unwrap();
        let p = Platform::native();
        let o = owner(d.path());
        let part = o.path();
        std::fs::write(&part, b"x").unwrap();
        let user = d.path().join("photo.ARW");
        std::fs::write(&user, b"mine").unwrap();
        // A decoy with the right shape but the wrong job.
        let decoy = d.path().join(".photo.ARW.zzzzzzzz-0.st-partial");
        std::fs::write(&decoy, b"not ours").unwrap();

        assert!(remove_partial(&p, &user, &o).is_err());
        assert!(remove_partial(&p, &decoy, &o).is_err());
        remove_partial(&p, &part, &o).unwrap();
        assert!(!part.exists() && user.exists() && decoy.exists());
        // Gone already: fine.
        remove_partial(&p, &part, &o).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn clears_a_deny_delete_acl_first() {
        let d = tempfile::tempdir().unwrap();
        let o = owner(d.path());
        let part = o.path();
        std::fs::write(&part, b"x").unwrap();
        assert!(
            std::process::Command::new("chmod")
                .args(["+a", "everyone deny delete"])
                .arg(&part)
                .status()
                .unwrap()
                .success()
        );
        remove_partial(&Platform::native(), &part, &o).unwrap();
        assert!(!part.exists());
    }
}

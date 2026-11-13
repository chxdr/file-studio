//! Shared test helpers: fixtures, hashing, and hdiutil disk images.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use safe_transfer::{Control, CopyRequest, Failure, FileReport, NoProgress, Options, Platform};

/// Deterministic pseudo-random bytes.
pub fn bytes(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

/// Options for the byte-copy path (no instant clone on the same volume).
pub fn no_clone() -> Options {
    Options {
        clone_same_volume: false,
        ..Options::default()
    }
}

/// Copies `src` into `dir` with `opts`.
pub fn copy(src: &Path, dir: &Path, opts: &Options) -> Result<FileReport, Failure> {
    let req = CopyRequest::into_dir(src, dir).unwrap();
    safe_transfer::copy_file(
        &Platform::native(),
        &req,
        opts,
        &Control::new(),
        &NoProgress,
    )
}

/// Names in `dir`, sorted.
pub fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// No `.st-partial` (or its `._` companion) left in `dir`.
pub fn no_partials(dir: &Path) -> bool {
    !names(dir).iter().any(|n| n.ends_with(".st-partial"))
}

/// Runs a command, panicking with its output on failure.
pub fn sh(cmd: &str, args: &[&str]) {
    let out = Command::new(cmd).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{cmd} {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A small disk image mounted at a private mount point, standing in for a
/// card or external drive. Detached and deleted on drop.
pub struct Image {
    _dir: tempfile::TempDir,
    pub img: PathBuf,
    pub mount: PathBuf,
}

impl Image {
    /// `fs` is an hdiutil filesystem name: "APFS", "ExFAT", "MS-DOS".
    pub fn new(fs: &str, megabytes: u32) -> Image {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("vol.dmg");
        let mount = dir.path().join("mnt");
        std::fs::create_dir(&mount).unwrap();
        sh(
            "hdiutil",
            &[
                "create",
                "-quiet",
                "-megabytes",
                &megabytes.to_string(),
                "-fs",
                fs,
                "-volname",
                "STTEST",
                img.to_str().unwrap(),
            ],
        );
        let im = Image {
            _dir: dir,
            img,
            mount,
        };
        im.put_back();
        im
    }

    /// Pulls the "card" out, as a yank would: `hdiutil detach -force`.
    pub fn pull_out(&self) {
        let _ = Command::new("hdiutil")
            .args(["detach", "-force", "-quiet"])
            .arg(&self.mount)
            .status();
    }

    /// Puts it back at the same mount point.
    pub fn put_back(&self) {
        sh(
            "hdiutil",
            &[
                "attach",
                "-quiet",
                "-nobrowse",
                "-mountpoint",
                self.mount.to_str().unwrap(),
                self.img.to_str().unwrap(),
            ],
        );
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        self.pull_out();
    }
}

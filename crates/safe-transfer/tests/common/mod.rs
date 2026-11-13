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

    /// Pulls the "card" out the way a yank or power cut would: see [`yank`].
    pub fn pull_out(&self) {
        yank(&self.img, &self.mount);
    }

    /// Puts it back at the same mount point.
    pub fn put_back(&self) {
        for attempt in 0..5 {
            let ok = Command::new("hdiutil")
                .args(["attach", "-quiet", "-nobrowse", "-mountpoint"])
                .arg(&self.mount)
                .arg(&self.img)
                .status()
                .is_ok_and(|s| s.success());
            if ok {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(500 * (attempt + 1)));
        }
        panic!("couldn't attach {} again", self.img.display());
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        let _ = Command::new("hdiutil")
            .args(["detach", "-force", "-quiet"])
            .arg(&self.mount)
            .status();
    }
}

/// The `diskimages-helper` process serving `img` (it plays the drive).
pub fn helper_pid(img: &Path) -> Option<u32> {
    let want = std::fs::canonicalize(img).ok()?;
    let out = Command::new("hdiutil").arg("info").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut current = false;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("image-path") {
            let p = p.trim_start_matches([' ', ':']).trim();
            current = std::fs::canonicalize(p).is_ok_and(|p| p == want);
        } else if current && let Some(pid) = line.strip_prefix("process ID") {
            return pid.trim_start_matches([' ', ':']).trim().parse().ok();
        }
    }
    None
}

/// A yank: `kill -9` the image's helper, so the "drive" vanishes with
/// whatever it hadn't been told to flush, then clear the dead mount.
///
/// `hdiutil detach -force` alone is **not** a yank: it writes cached data
/// back first, so even files that were never flushed survive it (measured:
/// 2,000 of 2,000 unflushed files intact). Killing the helper loses them
/// (APFS: all 2,000; exFAT: 46–70 missing and one short), so it can tell
/// flushed from unflushed writes.
pub fn yank(img: &Path, mount: &Path) {
    if let Some(pid) = helper_pid(img) {
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    let _ = Command::new("hdiutil")
        .args(["detach", "-force", "-quiet"])
        .arg(mount)
        .status();
    let _ = Command::new("diskutil")
        .args(["unmount", "force"])
        .arg(mount)
        .output();
}

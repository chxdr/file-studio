//! Shared test helpers: fixtures, hashing, and hdiutil disk images.
#![allow(dead_code)]

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

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

/// The longest any single `hdiutil` (or `diskutil`, `ps`, `kill`) call may
/// take. A disk-image service that stops answering fails the test with a
/// message instead of hanging it.
pub const TOOL_LIMIT: Duration = Duration::from_secs(60);

/// How long a killed helper may take to exit, and a yanked image to leave
/// `hdiutil info`.
const GONE_LIMIT: Duration = Duration::from_secs(30);

/// Runs `cmd` with a time limit, feeding it `stdin`. Returns its output
/// whatever the exit status, or a readable error if it couldn't start or
/// didn't finish in time (it is then killed).
pub fn run_limited<S: AsRef<OsStr>>(
    cmd: &str,
    args: &[S],
    stdin: Option<&[u8]>,
    limit: Duration,
) -> Result<Output, String> {
    let shown = || {
        let a: Vec<_> = args
            .iter()
            .map(|a| a.as_ref().to_string_lossy().into_owned())
            .collect();
        format!("{cmd} {}", a.join(" "))
    };
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't start `{}`: {e}", shown()))?;
    // Feed and drain on threads so a full pipe can't stall the child.
    let feeder = stdin.map(|b| {
        let mut w = child.stdin.take().unwrap();
        let b = b.to_vec();
        std::thread::spawn(move || {
            let _ = w.write_all(&b);
        })
    });
    let drain = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut v = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut v);
            }
            v
        })
    };
    let out = drain(child.stdout.take().map(|r| Box::new(r) as _));
    let err = drain(child.stderr.take().map(|r| Box::new(r) as _));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() < limit => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "`{}` didn't finish in {} s and was stopped. The disk-image \
                     service may be stuck; check `hdiutil info`, and restart the \
                     Mac if images won't attach or detach.",
                    shown(),
                    limit.as_secs()
                ));
            }
            Err(e) => return Err(format!("waiting for `{}`: {e}", shown())),
        }
    };
    if let Some(f) = feeder {
        let _ = f.join();
    }
    Ok(Output {
        status,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}

/// Runs a command (time-limited), panicking with its output on failure.
pub fn sh(cmd: &str, args: &[&str]) {
    let out = run_limited(cmd, args, None, TOOL_LIMIT).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        out.status.success(),
        "{cmd} {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A small disk image mounted at a private mount point, standing in for a
/// card or external drive. Detached and deleted on drop, including when the
/// test panics.
pub struct Image {
    _dir: tempfile::TempDir,
    pub img: PathBuf,
    pub mount: PathBuf,
    /// Where it is attached now (`put_back_at` can move it).
    now: Mutex<PathBuf>,
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
        // From here on, a panic drops `im`, which detaches it.
        let im = Image {
            _dir: dir,
            img,
            now: Mutex::new(mount.clone()),
            mount,
        };
        im.put_back();
        im
    }

    /// Pulls the "card" out the way a yank or power cut would: see [`yank`].
    /// Panics if the yank didn't take.
    pub fn pull_out(&self) {
        let now = self.now.lock().unwrap().clone();
        if let Err(e) = try_yank(&self.img, &now) {
            panic!("{e}");
        }
    }

    /// Puts it back at the same mount point.
    pub fn put_back(&self) {
        self.put_back_at(&self.mount.clone());
    }

    /// Puts it back somewhere else, as macOS does when a card comes back
    /// as "Untitled 1".
    ///
    /// Panics, before attaching, if an earlier yank failed or the image is
    /// still attached: attaching an image whose old helper is still serving
    /// it waits forever.
    pub fn put_back_at(&self, mount: &Path) {
        if let Some(e) = take_yank_failure() {
            panic!("an earlier yank failed, so the card was never really pulled: {e}");
        }
        match attachments(&self.img) {
            Ok(a) if a.is_empty() => {}
            Ok(a) => panic!(
                "{} is still attached (helper {:?}), so attaching it again would \
                 hang; the yank didn't take",
                self.img.display(),
                a
            ),
            Err(e) => panic!("{e}"),
        }
        std::fs::create_dir_all(mount).unwrap();
        *self.now.lock().unwrap() = mount.to_path_buf();
        let mut last = String::new();
        for attempt in 0..5 {
            let out = run_limited(
                "hdiutil",
                &[
                    OsStr::new("attach"),
                    OsStr::new("-quiet"),
                    OsStr::new("-nobrowse"),
                    OsStr::new("-mountpoint"),
                    mount.as_os_str(),
                    self.img.as_os_str(),
                ],
                None,
                TOOL_LIMIT,
            )
            .unwrap_or_else(|e| panic!("{e}"));
            if out.status.success() {
                return;
            }
            last = String::from_utf8_lossy(&out.stderr).into_owned();
            std::thread::sleep(Duration::from_millis(500 * (attempt + 1)));
        }
        panic!("couldn't attach {} again: {last}", self.img.display());
    }

    /// Detaches the image if it is attached, killing its own helper if a
    /// plain forced detach doesn't work. Never panics; problems are printed.
    fn clean_up(&self) {
        let now = self.now.lock().map(|m| m.clone()).unwrap_or_default();
        match attachments(&self.img) {
            Ok(a) if a.is_empty() => return,
            Ok(_) => {}
            Err(e) => eprintln!("cleaning up {}: {e}", self.img.display()),
        }
        let _ = run_limited(
            "hdiutil",
            &[
                OsStr::new("detach"),
                OsStr::new("-force"),
                OsStr::new("-quiet"),
                now.as_os_str(),
            ],
            None,
            TOOL_LIMIT,
        );
        if attachments(&self.img).is_ok_and(|a| a.is_empty()) {
            return;
        }
        if let Err(e) = try_yank(&self.img, &now) {
            eprintln!(
                "WARNING: couldn't detach {}: {e}. Check `hdiutil info`.",
                self.img.display()
            );
        }
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        self.clean_up();
        // A failure recorded by a hook that no `put_back` saw is dropped
        // here so it can't leak into the next test's image.
        if let Some(e) = take_yank_failure()
            && !std::thread::panicking()
        {
            eprintln!("WARNING: a yank failed and nothing checked it: {e}");
        }
    }
}

/// One `hdiutil info` entry for an image: the PID of the process serving it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attached {
    /// `hdid-pid` (`process ID` in the text output), if listed.
    pub pid: Option<u32>,
}

/// `hdiutil info -plist` as JSON (time-limited).
fn hdiutil_info() -> Result<serde_json::Value, String> {
    let plist = run_limited("hdiutil", &["info", "-plist"], None, TOOL_LIMIT)?;
    if !plist.status.success() {
        return Err(format!(
            "hdiutil info failed: {}",
            String::from_utf8_lossy(&plist.stderr)
        ));
    }
    let json = run_limited(
        "plutil",
        &["-convert", "json", "-o", "-", "--", "-"],
        Some(&plist.stdout),
        TOOL_LIMIT,
    )?;
    serde_json::from_slice(&json.stdout).map_err(|e| {
        format!(
            "couldn't read hdiutil info: {e}: {}",
            String::from_utf8_lossy(&json.stderr)
        )
    })
}

/// The PIDs listed for each image in `info`, keyed by its image path.
fn listed(info: &serde_json::Value) -> Vec<(PathBuf, Option<u32>)> {
    let pid = |im: &serde_json::Value| {
        ["hdid-pid", "process ID"]
            .iter()
            .find_map(|k| im.get(*k)?.as_u64())
            .and_then(|p| u32::try_from(p).ok())
    };
    info.get("images")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|im| Some((PathBuf::from(im.get("image-path")?.as_str()?), pid(im))))
        .collect()
}

/// Whether two image paths name the same file (`/var` vs `/private/var`).
fn same_image(a: &Path, b: &Path) -> bool {
    a == b
        || match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
}

/// Every `hdiutil info` entry for `img`, matched on its image path. Empty
/// when it isn't attached.
pub fn attachments(img: &Path) -> Result<Vec<Attached>, String> {
    Ok(listed(&hdiutil_info()?)
        .into_iter()
        .filter(|(p, _)| same_image(p, img))
        .map(|(_, pid)| Attached { pid })
        .collect())
}

/// The `diskimages-helper` serving `img`, and only `img`, run by the same
/// user who owns `img`. Anything else is refused: a shared daemon such as
/// `diskimagesiod` serves other images too and must never be killed.
pub fn helper_pid(img: &Path) -> Result<u32, String> {
    let info = hdiutil_info()?;
    let all = listed(&info);
    let mine: Vec<_> = all.iter().filter(|(p, _)| same_image(p, img)).collect();
    let pid = match mine.as_slice() {
        [] => return Err(format!("{} isn't attached", img.display())),
        [(_, Some(pid))] => *pid,
        [(_, None)] => {
            return Err(format!(
                "hdiutil info lists no process for {}",
                img.display()
            ));
        }
        _ => {
            return Err(format!(
                "{} is attached {} times",
                img.display(),
                mine.len()
            ));
        }
    };
    let shared = all
        .iter()
        .filter(|(p, q)| *q == Some(pid) && !same_image(p, img))
        .count();
    if shared > 0 {
        return Err(format!(
            "process {pid} also serves {shared} other image(s); refusing to kill it"
        ));
    }
    let uid = std::fs::metadata(img)
        .map_err(|e| format!("{}: {e}", img.display()))?
        .uid();
    let ps = run_limited(
        "ps",
        &["-o", "uid=,comm=", "-p", &pid.to_string()],
        None,
        TOOL_LIMIT,
    )?;
    let line = String::from_utf8_lossy(&ps.stdout).trim().to_owned();
    let (who, comm) = line.split_once(char::is_whitespace).unwrap_or((&line, ""));
    if who.trim().parse::<u32>() != Ok(uid) || !comm.trim().ends_with("diskimages-helper") {
        return Err(format!(
            "process {pid} ({line:?}) isn't this image's own diskimages-helper run by \
             uid {uid}; refusing to kill it. macOS may be serving the image through a \
             shared daemon, in which case this Mac can't simulate a yank."
        ));
    }
    Ok(pid)
}

/// Whether process `pid` still exists.
fn alive(pid: u32) -> bool {
    // `kill -0` fails once the process is gone. If `kill` can't even run,
    // assume it's alive, so the caller times out with a message.
    run_limited("kill", &["-0", &pid.to_string()], None, TOOL_LIMIT)
        .map_or(true, |o| o.status.success())
}

/// Polls `done` every 100 ms for up to `limit`.
fn wait_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        if done() {
            return true;
        }
        if start.elapsed() >= limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Failures from yanks fired inside fault hooks, which run on engine threads
/// and mustn't panic there. The next `put_back` reports them.
static YANK_FAILURES: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn take_yank_failure() -> Option<String> {
    let mut v = YANK_FAILURES.lock().unwrap_or_else(|p| p.into_inner());
    if v.is_empty() {
        None
    } else {
        Some(std::mem::take(&mut *v).join("; "))
    }
}

/// A yank: `kill -9` the image's own helper, so the "drive" vanishes with
/// whatever it hadn't been told to flush, then clear the dead mount.
///
/// `hdiutil detach -force` alone is **not** a yank: it writes cached data
/// back first, so even files that were never flushed survive it (measured:
/// 2,000 of 2,000 unflushed files intact). Killing the helper loses them
/// (APFS: all 2,000; exFAT: 46–70 missing and one short), so it can tell
/// flushed from unflushed writes.
///
/// Safe to call from a fault hook: it never panics. A failure is recorded
/// and the next [`Image::put_back_at`] fails the test with it.
pub fn yank(img: &Path, mount: &Path) {
    if let Err(e) = try_yank(img, mount) {
        YANK_FAILURES
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(e);
    }
}

/// [`yank`], returning what went wrong. On success the helper has exited and
/// the image is no longer listed by `hdiutil info`.
pub fn try_yank(img: &Path, mount: &Path) -> Result<(), String> {
    try_yank_stale(img, mount, Duration::ZERO, || {})
}

/// [`try_yank`] as macOS really does it: the drive is gone (its helper is
/// killed), `dead` runs, and the dead mount stays listed for `stale` before
/// it is cleared. I/O on it fails while `statfs` still finds the volume,
/// which is the window where a copy must not mistake a pulled card for a
/// broken file.
pub fn try_yank_stale(
    img: &Path,
    mount: &Path,
    stale: Duration,
    dead: impl FnOnce(),
) -> Result<(), String> {
    let pid = helper_pid(img).map_err(|e| format!("yank of {}: {e}", img.display()))?;
    let _ = run_limited("kill", &["-9", &pid.to_string()], None, TOOL_LIMIT)?;
    if !wait_until(GONE_LIMIT, || !alive(pid)) {
        return Err(format!(
            "yank of {}: helper {pid} was sent kill -9 but is still running after {} s",
            img.display(),
            GONE_LIMIT.as_secs()
        ));
    }
    dead();
    std::thread::sleep(stale);
    // Clear the dead mount. Failures here are expected (the device is gone);
    // what matters is the check below.
    let _ = run_limited(
        "hdiutil",
        &[
            OsStr::new("detach"),
            OsStr::new("-force"),
            OsStr::new("-quiet"),
            mount.as_os_str(),
        ],
        None,
        TOOL_LIMIT,
    );
    let _ = run_limited(
        "diskutil",
        &[
            OsStr::new("unmount"),
            OsStr::new("force"),
            mount.as_os_str(),
        ],
        None,
        TOOL_LIMIT,
    );
    let mut last = Ok(());
    let gone = wait_until(GONE_LIMIT, || match attachments(img) {
        Ok(a) => {
            last = Ok(());
            a.is_empty()
        }
        Err(e) => {
            last = Err(e);
            false
        }
    });
    if gone {
        Ok(())
    } else {
        Err(format!(
            "yank of {}: helper {pid} is gone but the image is still listed by \
             hdiutil info after {} s{}",
            img.display(),
            GONE_LIMIT.as_secs(),
            last.err().map(|e| format!(" ({e})")).unwrap_or_default()
        ))
    }
}

//! The per-job journal on the Mac (proposal §3): a folder per job in the
//! host's journal directory, holding
//!
//! - `manifest.json` (+ `manifest.bak.json`): the plan, written once;
//! - `head.{a,b}.json`: the job's state, two alternating slots;
//! - `seg-NNNN.{a,b}.json`: per-file state for 1,024 files each;
//! - `report.json` when finished; `lock` while a process runs the job.
//!
//! Every file is two lines: the JSON body, then `{"len":…,"xxh3":"…"}`. A
//! torn, truncated or bit-flipped file fails that check and is ignored; the
//! other slot (or the backup) is used. Every write goes to `<name>.tmp`, is
//! flushed (`F_FULLFSYNC`), renamed over the older slot, and the folder is
//! flushed. The journal is only ever a list of claims: everything it says
//! is checked against the disk again before it is trusted.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::xxh3_64;

use crate::platform::Flush;

/// The journal format version this build writes and reads.
pub const VERSION: u32 = 1;
/// Files per state segment.
pub const SEGMENT_FILES: usize = 1024;

/// A volume as the journal remembers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolRef {
    /// Volume UUID (`None` for volumes without one; identity is then weak).
    pub uuid: Option<String>,
    /// Filesystem type ("apfs", "exfat", "msdos", "hfs"…).
    pub fs: String,
    /// Name, for messages only.
    pub name: String,
    /// Size in bytes (a cross-check on the UUID).
    pub capacity: u64,
    /// Where it was mounted when the job was made.
    pub mount: PathBuf,
}

/// What a manifest entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A folder.
    Dir,
    /// A symlink, copied as a link.
    Link,
    /// A pipe, socket or device: never copied.
    Other,
}

/// One item of a job, in the order it is copied (folders before their contents).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Index in the job (part of the partial's name).
    pub n: u32,
    /// What it is.
    pub kind: EntryKind,
    /// The original (`None` for folders made only to hold `Exact` items).
    pub src: Option<PathBuf>,
    /// Index into the manifest's `sources`.
    pub src_vol: u32,
    /// Path under the destination folder.
    pub rel: PathBuf,
    /// Size in bytes when planned.
    pub size: u64,
    /// Modification time when planned.
    pub mtime_ns: i64,
    /// Inode when planned.
    pub ino: u64,
    /// An iCloud placeholder when planned.
    #[serde(default)]
    pub dataless: bool,
    /// An earlier entry of this job that takes the same name on this disk
    /// ("File" and "file" onto a case-insensitive card). This one waits for
    /// it, then is decided like any taken name: asked, kept both, skipped.
    /// Never an overwrite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<u32>,
}

/// The options a job was made with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobOptions {
    /// Chunk size in bytes.
    pub chunk_size: u64,
    /// Flush after each chunk of big files.
    pub flush_each_chunk: bool,
    /// Read originals twice (§4.6).
    pub reread_source: bool,
    /// Copy what symlinks point to.
    pub follow_symlinks: bool,
    /// Copy iCloud placeholders (downloads them).
    pub allow_dataless: bool,
    /// Instant clones on the same APFS volume.
    pub clone_same_volume: bool,
    /// Default conflict answers.
    #[serde(default)]
    pub on_conflict: crate::options::ConflictPolicy,
    /// What happens to moved originals.
    #[serde(default)]
    pub disposal: crate::options::Disposal,
    /// Files worked on at once (`None`: automatic).
    #[serde(default)]
    pub workers: Option<usize>,
}

/// Where a job writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dest {
    /// The destination folder.
    pub path: PathBuf,
    /// Its volume.
    pub volume: VolRef,
}

/// The job's plan, written once when it is created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Always "safe-transfer/manifest".
    pub format: String,
    /// [`VERSION`].
    pub version: u32,
    /// The job id.
    pub job: String,
    /// Seconds since the epoch.
    pub created: u64,
    /// For the resume prompt: "Smith wedding → CLIENT_A".
    pub label: String,
    /// The host's own reference, returned in events, never interpreted.
    pub host_ref: Option<String>,
    /// "copy" (moves come in M3).
    pub mode: String,
    /// Options.
    pub options: JobOptions,
    /// The destination.
    pub dest: Dest,
    /// Source volumes.
    pub sources: Vec<VolRef>,
    /// Every item, in copy order.
    pub files: Vec<Entry>,
}

/// A job's lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Made, not started.
    Planned,
    /// Copying.
    Running,
    /// Paused by the user.
    Paused,
    /// Some files wait for a source card that isn't connected.
    WaitingForSource,
    /// The destination isn't connected (pulled out mid-job, for example).
    WaitingForDestination,
    /// The destination filled up; free some space and resume.
    DestinationFull,
    /// A process stopped mid-job (crash, quit, power cut).
    Interrupted,
    /// Another process (the app or the CLI) is running this job right now.
    RunningElsewhere,
    /// Everything else is done; some names are taken and wait for an answer.
    WaitingForAnswer,
    /// Cancelled: the destination is as it was, and the job can be resumed.
    Cancelled,
    /// Every file is done, skipped or failed.
    Finished,
    /// Closed without finishing; its partials were removed.
    Closed,
}

impl JobState {
    /// A short lowercase name.
    pub fn as_str(self) -> &'static str {
        match self {
            JobState::Planned => "planned",
            JobState::Running => "running",
            JobState::Paused => "paused",
            JobState::WaitingForSource => "waiting_for_source",
            JobState::WaitingForDestination => "waiting_for_destination",
            JobState::DestinationFull => "destination_full",
            JobState::Interrupted => "interrupted",
            JobState::RunningElsewhere => "running_elsewhere",
            JobState::WaitingForAnswer => "waiting_for_answer",
            JobState::Cancelled => "cancelled",
            JobState::Finished => "finished",
            JobState::Closed => "closed",
        }
    }

    /// Whether the job stopped before finishing and waits for a resume:
    /// interrupted, waiting for a disk, full, or cancelled (resumable).
    pub fn needs_resume(self) -> bool {
        matches!(
            self,
            JobState::Interrupted
                | JobState::WaitingForDestination
                | JobState::WaitingForSource
                | JobState::DestinationFull
                | JobState::Cancelled
                | JobState::WaitingForAnswer
        )
    }

    /// Whether the job can't do anything more.
    pub fn is_final(self) -> bool {
        matches!(self, JobState::Finished | JobState::Closed)
    }
}

/// The job's state file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Head {
    /// Always "safe-transfer/head".
    pub format: String,
    /// [`VERSION`].
    pub version: u32,
    /// The job id.
    pub job: String,
    /// Generation: one more on every write; the higher valid slot wins.
    #[serde(rename = "gen")]
    pub generation: u64,
    /// The state.
    pub state: JobState,
    /// Seconds since the epoch.
    pub updated: u64,
    /// "Apply to all" answers given so far.
    #[serde(default)]
    pub rules: Rules,
}

/// Conflict answers given with "apply to all".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Rules {
    /// For files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<crate::options::Decision>,
    /// For folders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folders: Option<crate::options::Decision>,
}

/// Where one file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileSt {
    /// Not started.
    #[default]
    Pending,
    /// The partial exists; `chunks` lists the chunks flushed so far.
    Copying,
    /// A small file's partial is complete and `fsync`ed, waiting for its group's drive flush.
    Written,
    /// The partial is complete and on the drive.
    Flushed,
    /// It has its real name (the folder flush may still be due).
    Placed,
    /// Finished.
    Done,
    /// Not copied, on purpose (see `reason`).
    Skipped,
    /// Not copied because of an error (see `reason`).
    Failed,
    /// Its source card isn't connected.
    WaitingSource,
    /// The destination name is taken; waits for an answer.
    Conflict,
}

impl FileSt {
    /// Nothing more will happen to this file.
    pub fn is_terminal(self) -> bool {
        matches!(self, FileSt::Done | FileSt::Skipped | FileSt::Failed)
    }
}

/// One file's state.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FileRec {
    /// State.
    pub st: FileSt,
    /// xxh3-64 of each flushed chunk, as 16 hex digits.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chunks: Vec<String>,
    /// xxh3-128 of the whole file, as 32 hex digits, once written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// The original's size, mtime and inode when its copy started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src_ident: Option<(u64, i64, u64)>,
    /// "verified", "verified_from_memory", "cloned", "link", "folder".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// Why it was skipped or failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Why it had to start again (the original changed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restarted: Option<String>,
    /// It continued from an earlier, interrupted attempt.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub resumed: bool,
    /// Chunks kept from that attempt.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub chunks_reused: u32,
    /// The original was read twice and both reads agreed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reread: bool,
    /// Metadata the destination didn't keep.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub meta_lost: Vec<String>,
    /// The flush only reached OS level (the disk refused a drive flush).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub os_flush_only: bool,
    /// Something was already at the destination name when the job was planned.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub existed: bool,
    /// The answer for a taken name (given, from a rule, or the policy).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<crate::options::Decision>,
    /// The new name chosen for "keep both".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dest_name: Option<String>,
    /// A folder being replaced: its new contents go into a hidden partial
    /// folder that replaces the old one once everything in it is done.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replacing: bool,
    /// Where "replace" sent the old item (the Trash).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_trash: Option<PathBuf>,
    /// Where a move sent the original (the Trash).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trashed_to: Option<PathBuf>,
    /// Why a moved original was kept where it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_kept: Option<String>,
    /// Moved by renaming (with its folder, for items inside a moved folder).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub moved: bool,
    /// A folder whose contents are all done and whose metadata is applied.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub finished: bool,
    /// What was found at the taken name, for the question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub found: Option<Found>,
}

/// What is at a taken destination name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Found {
    /// A folder (not a file).
    pub dir: bool,
    /// Its size in bytes.
    pub size: u64,
    /// Its modification time.
    pub mtime_ns: i64,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl FileRec {
    /// The chunk hashes as numbers (unparseable entries end the list).
    pub fn chunk_hashes(&self) -> Vec<u64> {
        self.chunks
            .iter()
            .map_while(|h| u64::from_str_radix(h, 16).ok())
            .collect()
    }
}

/// Per-file state for one segment of 1,024 files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    /// Always "safe-transfer/segment".
    pub format: String,
    /// [`VERSION`].
    pub version: u32,
    /// The job id.
    pub job: String,
    /// Segment number.
    pub seg: u32,
    /// Generation.
    #[serde(rename = "gen")]
    pub generation: u64,
    /// File index → state (files not listed are pending).
    pub files: BTreeMap<u32, FileRec>,
}

/// Encodes `value` as the two-line checked format.
pub fn encode<T: Serialize>(value: &T) -> Vec<u8> {
    let mut body = serde_json::to_vec(value).expect("journal types always serialise");
    let trailer = format!(
        "{{\"len\":{},\"xxh3\":\"{:016x}\"}}\n",
        body.len(),
        xxh3_64(&body)
    );
    body.push(b'\n');
    body.extend_from_slice(trailer.as_bytes());
    body
}

#[derive(Deserialize)]
struct Trailer {
    len: usize,
    xxh3: String,
}

/// Decodes the two-line format; `None` if anything doesn't check out.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    let nl = bytes.iter().position(|b| *b == b'\n')?;
    let (body, rest) = (&bytes[..nl], &bytes[nl + 1..]);
    let t: Trailer = serde_json::from_slice(rest.strip_suffix(b"\n").unwrap_or(rest)).ok()?;
    if t.len != body.len() || t.xxh3 != format!("{:016x}", xxh3_64(body)) {
        return None;
    }
    serde_json::from_slice(body).ok()
}

/// Something stored in two alternating slots.
pub trait Slotted {
    /// Its generation.
    fn generation(&self) -> u64;
    /// Its job id (a slot from another job is ignored).
    fn job(&self) -> &str;
}

impl Slotted for Head {
    fn generation(&self) -> u64 {
        self.generation
    }
    fn job(&self) -> &str {
        &self.job
    }
}

impl Slotted for Segment {
    fn generation(&self) -> u64 {
        self.generation
    }
    fn job(&self) -> &str {
        &self.job
    }
}

/// The files of one job's journal folder.
#[derive(Clone)]
pub struct JobFiles {
    /// The job's folder.
    pub dir: PathBuf,
    flush: Arc<dyn Flush>,
    #[cfg(feature = "fault-injection")]
    pub(crate) faults: Option<Arc<dyn crate::fault::FaultHook>>,
}

impl std::fmt::Debug for JobFiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JobFiles({})", self.dir.display())
    }
}

/// The name of segment `seg`'s files.
pub fn segment_name(seg: usize) -> String {
    format!("seg-{seg:04}")
}

impl JobFiles {
    /// The journal of the job in `dir` (created if missing).
    pub fn new(dir: PathBuf, flush: Arc<dyn Flush>) -> JobFiles {
        JobFiles {
            dir,
            flush,
            #[cfg(feature = "fault-injection")]
            faults: None,
        }
    }

    fn fault(&self, _point: FaultArg) -> io::Result<()> {
        #[cfg(feature = "fault-injection")]
        if let Some(h) = &self.faults {
            let p = match _point {
                FaultArg::Mid => crate::fault::FaultPoint::MidJournalWrite,
                FaultArg::BeforeRename => crate::fault::FaultPoint::BeforeJournalRename,
            };
            h.at(p, &self.dir)?;
        }
        Ok(())
    }

    /// Writes `name` atomically: temp file, drive flush, rename, folder flush.
    pub fn write_file(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        self.write_files(&[(name.to_string(), bytes.to_vec())])
    }

    /// Writes several journal files with one drive flush: each goes to
    /// `<name>.tmp` and is `fsync`ed, then each is renamed over its old
    /// copy, then one `F_FULLFSYNC` on the folder covers them all (the same
    /// group commit as small files, §4.7). A crash at any point leaves each
    /// file either old or new, and every file checks itself.
    pub fn write_files(&self, files: &[(String, Vec<u8>)]) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        for (i, (name, bytes)) in files.iter().enumerate() {
            let tmp = self.dir.join(format!("{name}.tmp"));
            let mut f = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp)?;
            let half = bytes.len() / 2;
            f.write_all(&bytes[..half])?;
            if i == 0 {
                self.fault(FaultArg::Mid)?;
            }
            f.write_all(&bytes[half..])?;
            self.flush.os_flush(&f)?;
        }
        self.fault(FaultArg::BeforeRename)?;
        for (name, _) in files {
            // Replaces our own older copy of this journal file, never user data.
            std::fs::rename(self.dir.join(format!("{name}.tmp")), self.dir.join(name))?;
        }
        self.flush.flush_dir(&self.dir)?;
        Ok(())
    }

    /// The file name a slotted value goes to: even generations → a, odd → b.
    pub fn slot_name<T: Slotted>(base: &str, value: &T) -> String {
        let slot = if value.generation().is_multiple_of(2) {
            "a"
        } else {
            "b"
        };
        format!("{base}.{slot}.json")
    }

    /// Writes `value` into the slot its generation picks (even → a, odd → b),
    /// so the other slot keeps the previous generation.
    pub fn write_slotted<T: Serialize + Slotted>(&self, base: &str, value: &T) -> io::Result<()> {
        let slot = if value.generation().is_multiple_of(2) {
            "a"
        } else {
            "b"
        };
        self.write_file(&format!("{base}.{slot}.json"), &encode(value))
    }

    /// Reads both slots of `base` and returns the newest valid one for `job`.
    pub fn read_slotted<T: DeserializeOwned + Slotted>(&self, base: &str, job: &str) -> Option<T> {
        ["a", "b"]
            .iter()
            .filter_map(|s| std::fs::read(self.dir.join(format!("{base}.{s}.json"))).ok())
            .filter_map(|b| decode::<T>(&b))
            .filter(|v| v.job() == job)
            .max_by_key(|v| v.generation())
    }

    /// Writes the manifest and its backup.
    pub fn write_manifest(&self, m: &Manifest) -> io::Result<()> {
        // Two separate commits, so a crash can't catch both copies mid-write.
        let b = encode(m);
        self.write_file("manifest.json", &b)?;
        self.write_file("manifest.bak.json", &b)
    }

    /// The manifest, or its backup if the main copy is damaged.
    pub fn read_manifest(&self) -> Option<Manifest> {
        ["manifest.json", "manifest.bak.json"]
            .iter()
            .filter_map(|n| std::fs::read(self.dir.join(n)).ok())
            .filter_map(|b| decode::<Manifest>(&b))
            .find(|m| m.format == "safe-transfer/manifest" && m.version <= VERSION)
    }

    /// Opens (creating) the lock file and tries to lock it. `Ok(None)` means
    /// another process holds it.
    pub fn lock(&self) -> io::Result<Option<File>> {
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.dir.join("lock"))?;
        Ok(crate::sys::try_lock(&f)?.then_some(f))
    }
}

#[derive(Clone, Copy)]
#[cfg_attr(not(feature = "fault-injection"), allow(dead_code))]
enum FaultArg {
    Mid,
    BeforeRename,
}

/// Seconds since the epoch.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Whether `path` is inside `dir` (lexically).
pub fn under(path: &Path, dir: &Path) -> bool {
    path.starts_with(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(generation: u64) -> Head {
        Head {
            format: "safe-transfer/head".into(),
            version: VERSION,
            job: "k3f9q2xm".into(),
            generation,
            state: JobState::Running,
            updated: 1,
            rules: Rules::default(),
        }
    }

    fn files(d: &Path) -> JobFiles {
        JobFiles::new(d.to_path_buf(), Arc::new(crate::platform::Native))
    }

    #[test]
    fn round_trip_and_every_damage_is_caught() {
        let b = encode(&head(7));
        assert_eq!(decode::<Head>(&b), Some(head(7)));
        // Any cut loses data and is caught, except dropping only the final
        // newline, which leaves the complete, checked content.
        for cut in 0..b.len() - 1 {
            assert_eq!(decode::<Head>(&b[..cut]), None, "truncated at {cut}");
        }
        assert_eq!(decode::<Head>(&b[..b.len() - 1]), Some(head(7)));
        for i in 0..b.len() {
            for bit in [0x01u8, 0x20, 0x80] {
                let mut c = b.clone();
                c[i] ^= bit;
                if let Some(h) = decode::<Head>(&c) {
                    // Only a flip that leaves the JSON meaning identical may pass
                    // (none exist here, but be exact about the claim).
                    assert_eq!(h, head(7), "byte {i} bit {bit:#x}");
                }
            }
        }
    }

    #[test]
    fn slots_alternate_and_the_newest_valid_one_wins() {
        let d = tempfile::tempdir().unwrap();
        let j = files(d.path());
        for g in 1..=4 {
            j.write_slotted("head", &head(g)).unwrap();
        }
        assert_eq!(
            j.read_slotted::<Head>("head", "k3f9q2xm")
                .unwrap()
                .generation,
            4
        );
        // Damage the newest slot (generation 4 lives in a): generation 3 from b is used.
        std::fs::write(d.path().join("head.a.json"), b"{\"torn").unwrap();
        assert_eq!(
            j.read_slotted::<Head>("head", "k3f9q2xm")
                .unwrap()
                .generation,
            3
        );
        // Both damaged: nothing.
        std::fs::write(d.path().join("head.b.json"), b"").unwrap();
        assert!(j.read_slotted::<Head>("head", "k3f9q2xm").is_none());
        // Another job's slot is never taken for this one.
        j.write_slotted("head", &head(9)).unwrap();
        assert!(j.read_slotted::<Head>("head", "zzzzzzzz").is_none());
    }

    #[test]
    fn a_left_over_temp_file_changes_nothing() {
        let d = tempfile::tempdir().unwrap();
        let j = files(d.path());
        j.write_slotted("head", &head(2)).unwrap();
        std::fs::write(d.path().join("head.b.json.tmp"), b"half a write").unwrap();
        assert_eq!(
            j.read_slotted::<Head>("head", "k3f9q2xm")
                .unwrap()
                .generation,
            2
        );
        j.write_slotted("head", &head(3)).unwrap();
        assert_eq!(
            j.read_slotted::<Head>("head", "k3f9q2xm")
                .unwrap()
                .generation,
            3
        );
    }

    #[test]
    fn records_skip_defaults_and_parse_chunks() {
        let r = FileRec {
            st: FileSt::Copying,
            chunks: vec!["00000000000000ff".into(), "zz".into(), "01".into()],
            ..Default::default()
        };
        assert_eq!(r.chunk_hashes(), vec![255]);
        let s = serde_json::to_string(&FileRec::default()).unwrap();
        assert_eq!(s, r#"{"st":"pending"}"#);
    }

    #[test]
    fn the_manifest_backup_is_used_when_the_main_copy_is_damaged() {
        let d = tempfile::tempdir().unwrap();
        let j = files(d.path());
        let m = Manifest {
            format: "safe-transfer/manifest".into(),
            version: VERSION,
            job: "k3f9q2xm".into(),
            created: 1,
            label: "test".into(),
            host_ref: None,
            mode: "copy".into(),
            options: JobOptions {
                chunk_size: 64 << 20,
                flush_each_chunk: true,
                reread_source: false,
                follow_symlinks: false,
                allow_dataless: false,
                clone_same_volume: true,
                on_conflict: Default::default(),
                disposal: Default::default(),
                workers: None,
            },
            dest: Dest {
                path: "/d".into(),
                volume: VolRef {
                    uuid: None,
                    fs: "apfs".into(),
                    name: "x".into(),
                    capacity: 1,
                    mount: "/".into(),
                },
            },
            sources: vec![],
            files: vec![],
        };
        j.write_manifest(&m).unwrap();
        std::fs::write(d.path().join("manifest.json"), b"garbage").unwrap();
        assert_eq!(j.read_manifest(), Some(m));
        let mut newer = j.read_manifest().unwrap();
        newer.version = VERSION + 1;
        j.write_manifest(&newer).unwrap();
        assert_eq!(
            j.read_manifest(),
            None,
            "a newer format is never guessed at"
        );
    }
}

# Safe Transfer: design proposal (pre-code)

Status: **approved 2026-11-13** with every §14 recommendation, plus an opt-in "re-read originals" mode that first-run setup explains (§4.6). The measurement probe is in `spikes/safe-transfer/` (`st_probe.c`, `run.sh`), and its numbers are in §12. M1 is under way in `crates/safe-transfer` and `crates/safe-transfer-cli`.

**Inputs:**
- your brief (`safe-transfer-prompt.md`)
- `DECISIONS.md` (F-007, F-015, F-016, F-017, F-020, F-023)
- `docs/PROPOSAL.md`, `docs/SPIKES.md` §3, `bench/history.md` (including the M1 real-card run)
- all of `crates/fstudio-ops`, and the edge-fixes branch's leftover-partial sweep
- Import Studio's D-011 and `istudio-ingest/src/copy.rs`

---

## Summary (one page)

Safe Transfer is a separate Rust library, plus a small `safe-transfer` command-line tool, that copies and moves files so that **an interruption can cost you time, but never a client file.** File Studio will use it for every copy and move that writes data. Import Studio can use it later, unchanged.

### The promises

1. **A file with its real name is always complete and checked.** Data is written under a hidden temporary name. It only gets its real name after three steps: every byte is flushed to the drive, the copy is read back from the drive (not from memory) and matched against the original, and the rename itself is made durable. Pull the card, cut the power or crash at any moment, and you'll see either the real file, complete and checked, or no file at all, plus a hidden temporary file that Safe Transfer cleans up.
2. **The original is never touched during a copy.** It is only read. Before Safe Transfer counts a file as done, it checks that the original didn't change while it was being copied.
3. **A move never leaves a file missing from both places.** The original goes to the Trash only after its copy has passed every check above. Safe Transfer never permanently deletes anything of yours.
4. **Nothing is overwritten by surprise.** "Replace" writes and checks the new file first. Only then does the old file go to the Trash, where Finder's Put Back works.
5. **An interrupted job can be resumed.** The record of the job lives on the Mac, not on the card, and it recognises the card by its volume ID, not its name. When you resume, every piece already copied is read back and checked again before it is trusted.
6. **Nothing is lost quietly.** Each file ends as copied, verified, skipped, failed (with the reason) or resumed. If a disk can't hold some metadata (exFAT can't keep permissions, for example), the end-of-job report says so.

### The honest limits

- **A drive that lies.** `F_FULLFSYNC` asks the drive to empty its own write cache onto the storage. A few cheap USB card readers and enclosures report success without doing it. If one of those loses power within a second or so of a file finishing, that file can be damaged even though the check passed. No software can fully defend against that. The only defence is the read-back after reconnecting, which Safe Transfer's resume does.
- **A bad read of the original.** The original is read once. Its bytes are hashed in memory, and that hash is what the copy is checked against. If the card or reader returned wrong bytes during that one read (very rare, and nothing would flag it), the copy faithfully matches the wrong bytes. The opt-in "re-read originals" mode (§4.6) closes this gap by reading the original a second time, at the cost of roughly twice the reading time from the source.
- **exFAT's rename isn't strictly exclusive.** macOS can't do a "rename only if the name is free" on exFAT. Safe Transfer checks that the name is free and then renames, which leaves a window of a few microseconds. Another app writing the exact same name into the same folder in that window could be overwritten. File Studio has the same limit today.
- **exFAT has no journal.** If power is lost during the instant of the rename itself, the card's folder entry can be damaged. Disk Utility's First Aid repairs it. The file's data is already safe on the card at that point, because the rename only happens after the flush and the check.
- **Checking costs time.** Copying from a card to the Mac costs an estimated 5–15 % more. Copying from the Mac to a card costs an estimated 60–85 % more, because the copy has to be read back from the slow card. See §12.

### What was decided (2026-11-13; details in §14)

1. Chunk size: **64 MB**.
2. Verify: **always, except instant clones**, with no off switch in v1.
3. Journal: **JSON**.
4. Same-volume copies: **keep the instant APFS clone**.
5. The fstudio-ops flush gap (§0.2) is being fixed separately, now, by the coordinator.
6. The smaller ones as recommended. In addition, **"re-read originals"** is an opt-in mode (off by default), explained in first-run setup (§4.6).

---

## 0. Where we start

### 0.1 What fstudio-ops already does well, and what Safe Transfer keeps

| Piece (in `crates/fstudio-ops`) | What Safe Transfer does with it |
|---|---|
| Hidden partial file, then `renamex_np(RENAME_EXCL)`, with a check-then-rename fallback on exFAT (`sys/mod.rs` `rename_excl`) | **Extracted** as is, behind the `AtomicRename` trait |
| `F_FULLFSYNC` with an `fsync` fallback (`full_sync`, `sync_dir`) | **Extracted.** Safe Transfer also reports which flush level actually happened (§4.1) |
| `F_NOCACHE` on both ends, with the source hashed while it's being copied (`copy_loop`) | **Extracted and extended** into a chunked loop with per-chunk hashes, plus cache eviction before the read-back (§4.2) |
| `SEEK_DATA`/`SEEK_HOLE` hole handling, then `copyfile(XATTR\|STAT)` | **Extracted** (F-015) |
| ACLs and immutable flags cleared on the partial and re-applied after the rename (SPIKES S2) | **Extracted** |
| The `copyfile` status callback that stops on `COPYFILE_ERR` (the full-disk hang fix, F-020) | **Extracted** for the clone path |
| `remove.rs` as the only file that removes anything, enforced by `tests/safety_grep.rs` | **Copied as a pattern.** Safe Transfer gets its own `remove.rs` with one entry point, `remove_partial`, and its own grep test. It deliberately has no delete-immediately function (§7) |
| `SystemTrasher` (`NSFileManager trashItemAtURL`), so Put Back works | **Extracted** behind the `Trash` trait |
| Name rules: truncation to 255 bytes, Finder's "name 2" for Keep Both, NFC and case-folded keys (`names.rs`) | **Extracted** (pure functions) |
| `Ident` (dev, ino, size, mtime) and the "the original changed" check | **Extracted.** On resume, the dev number is replaced by the volume UUID (§3.4) |
| Fault-injection hook (`FaultStep`) | **Extended** into named stop points that can also pause, so a test can pull the disk at that exact moment (§11) |
| The SQLite op journal, the queue, undo, conflict UX, F-023 protected moves, toasts, the Delete Immediately gate | **Stay in fstudio-ops.** Safe Transfer doesn't know about any of them (§0.3) |

### 0.2 What's new, and a gap it closes

**New in Safe Transfer:**
- a chunked journal, with an xxh3 hash per 64 MB chunk, recorded only after the chunk is flushed
- resume at the chunk level: re-hash what's on the card, keep what matches, continue from there
- volume-UUID keying for the source and destination volumes, which works on exFAT and FAT too (measured, §3.4)
- a standalone journal folder per job, on the Mac, written by temp-then-rename with two alternating slots and a checksum
- reconnect detection: "this card has an unfinished job"
- the cache-eviction step that makes the read-back provably come from the drive (§4.2)
- an end-of-job report that includes metadata that couldn't be kept

**A gap in fstudio-ops today.** While reading `engine.rs` I found that the default settings are `verify = false` and `fsync_copies = false`. With those, a plain copy is renamed to its real name **without being flushed first** (`finish_partial`: `must_sync` is only true for moves or `fsync_copies`). After a power cut, or a card pulled within a few seconds of a file "finishing", the card can end up with a file under its real name whose data never fully reached the card. That breaks your rule #1.

Two related details:
- A verified copy is read back without an `F_FULLFSYNC` first. Import Studio's D-011 does flush first.
- The re-read can be served from RAM if anything else (Spotlight, Quick Look) has read the file in the meantime (§4.2).

Your real yank test passed because a yank usually cuts the rename off too. That isn't guaranteed, and power loss is worse. Safe Transfer closes this by design. If you want the gap closed in File Studio before Safe Transfer is wired in (its M4), the fix is small: always `F_FULLFSYNC` before the rename, and flush before verifying. That would be its own short task, not part of this one (decision 5).

One more finding, for the edge-fixes work: the new leftover sweep identifies a card by its `st_dev` number. I measured that this number **changes when a card is re-attached** and is **handed to the next volume that mounts** (§3.4). The sweep is still safe, because it only removes its own exactly-named files. But a card that comes back with a new number is never swept. The volume UUID fixes that. I've noted this for the coordinator; I haven't changed anything.

### 0.3 How fstudio-ops sits on top afterwards

```
 Swift shell ── shows progress, buttons, the resume prompt (no logic, F-001)
     │ UniFFI (fstudio-ffi mirrors the few Safe Transfer types it shows)
 fstudio-core ── owns one safe_transfer::Engine (journal folder in App Support)
     │
 fstudio-ops ── queue, SQLite op history, undo, conflict rules and dialogs,
     │          F-023 protected-move confirmation, toasts, Trash/Put Back,
     │          the gated Delete Immediately, same-volume renames, trash, new folder
     │          ─ for every copy and every cross-volume move: one Safe Transfer job
     ▼
 safe-transfer ── the bytes: partial → chunks + hashes → flush → evict → read back
                  → rename → flush folder → (move) Trash the original; its own journal
```

- **One ops operation equals one Safe Transfer job.** Ops keeps its SQLite row per item for history and undo, and records the Safe Transfer job id and file index next to it.
- **Conflicts:**
  - Ops still decides through its rules and dialogs, and gives Safe Transfer an exact decision per file (or "ask").
  - Safe Transfer does the safe part: for Replace, the new file is verified before the old one goes to the Trash.
  - This also fixes an ordering in ops today: its `resolve()` trashes the old file *before* copying the new one.
- **Undo** stays in ops. It reads the paths and Trash locations from Safe Transfer's report.
- **Same-volume moves** stay a single rename. That is a few lines either way; Safe Transfer offers it too, so the CLI and Import Studio have it.

### 0.4 Migration without weakening anything

| Step | What changes | Guarantee during the step |
|---|---|---|
| A (ST M1–M3) | `crates/safe-transfer` is built and tested on its own. **fstudio-ops isn't touched.** The extracted code is *copied*, with its tests, not moved. | File Studio behaves exactly as today. |
| B (ST M4, part 1) | Ops gains a backend switch (`copy_backend = ops \| safe_transfer`, default `ops`). With `safe_transfer`, ops' `copy_file` hands the item to a Safe Transfer job. | All of ops' existing tests run against both backends. These include resume, faults, conflicts, leftovers and the safety grep. The new backend is stricter: it always flushes and verifies. |
| C (ST M4, part 2) | The default flips to `safe_transfer` after the yank and `kill -9` suites pass through the File Studio app path, and after you sign off. | The old backend stays available for one release as a fallback. |
| D (ST M5) | Ops' own copy loop and copyfile data path are deleted. Ops' `sys` and `names` call Safe Transfer's versions. `remove.rs` keeps its four entry points. | The safety grep tests in both crates still pass. Nothing that removes data moves out of `remove.rs`. |

If you approve the interim fix in decision 5, it lands on the ops backend before step A ends. Then even the "old" path flushes before renaming.

---

## 1. Crate layout

```
crates/safe-transfer/                 # library, no File Studio types, no UI, no fstudio-db
├── Cargo.toml                         # GPL-3.0-or-later; deps below
├── src/
│   ├── lib.rs                         # Engine, re-exports; #![deny(unsafe_code)] via workspace lints
│   ├── job.rs                         # JobSpec → Plan → running job; pause/resume/cancel state machine
│   ├── plan.rs                        # stat-only planning: expand folders, conflicts, free space (pure where possible)
│   ├── pipeline.rs                    # one file: chunks → flush → evict → verify → place → (move) dispose
│   ├── chunk.rs                       # pure: chunk boundaries, hole maps, hash bookkeeping
│   ├── journal/                       # on-disk format (§3): manifest, state segments, A/B slots, checksums
│   │   ├── mod.rs  format.rs  recover.rs
│   ├── resume.rs                      # re-hash partials, source-changed rules, waiting-for-source
│   ├── conflict.rs                    # Skip / Replace / KeepBoth / Merge / Ask + apply-to-all
│   ├── names.rs                       # partial names, 255-byte limit, Keep Both names (pure)
│   ├── meta.rs                        # what to preserve, what was lost (report), per filesystem
│   ├── space.rs                       # free-space need with cluster rounding and margin (pure)
│   ├── events.rs                      # Event, Progress, Phase, JobReport; coalescing (16 ms)
│   ├── remove.rs                      # THE ONLY removal: remove_partial (journal-matched names only)
│   ├── platform.rs                    # the traits below + `Platform` bundle
│   └── sys/                           # ALL unsafe code; #![allow(unsafe_code)], // SAFETY: on every block
│       ├── mod.rs
│       ├── macos.rs                   # fcntl, renamex_np, getattrlist(ATTR_VOL_UUID), mincore/msync,
│       │                              # copyfile, acl_*, lchflags, statfs, getentropy
│       ├── macos_trash.rs             # NSFileManager trashItemAtURL (objc2-foundation)
│       ├── linux.rs                   # stub: fdatasync, posix_fadvise(DONTNEED), renameat2(NOREPLACE),
│       │                              # /dev/disk/by-uuid → Unsupported until a Linux shell exists
│       └── windows.rs                 # stub: FlushFileBuffers, FILE_FLAG_NO_BUFFERING,
│                                      # MoveFileExW (no REPLACE_EXISTING), GetVolumeInformationW serial
└── tests/                             # unit + integration + yank/kill suites (§11)

crates/safe-transfer-cli/              # binary `safe-transfer` (clap), so the library has no CLI deps
```

**The platform traits.** These are small and separate, as the brief asks, and bundled into one `Platform` struct for convenience:

```rust
/// Makes written data durable.
pub trait Flush: Send + Sync {
    /// Flushes the file's data and asks the drive to empty its own write cache.
    /// Says how far it got: `Device` (F_FULLFSYNC accepted) or `OsOnly` (only fsync worked).
    fn flush_file(&self, f: &File) -> io::Result<FlushLevel>;
    /// The same for a folder, so a rename inside it survives a power cut.
    fn flush_dir(&self, dir: &Path) -> io::Result<FlushLevel>;
}

/// Reads and writes that don't go through (or stay in) the OS cache.
pub trait Uncached: Send + Sync {
    fn set_uncached(&self, f: &File) -> io::Result<()>;        // F_NOCACHE
    /// Drops any cached pages of `path` and says whether any are left.
    fn evict(&self, path: &Path) -> io::Result<Residency>;      // msync(MS_INVALIDATE) + mincore
}

/// Puts a file in place without ever replacing something.
pub trait AtomicRename: Send + Sync {
    fn rename_exclusive(&self, from: &Path, to: &Path) -> io::Result<RenameMode>; // Exclusive | CheckThenRename
}

/// Recognises a volume across unplugging and remounting.
pub trait VolumeIdentity: Send + Sync {
    fn volume_of(&self, path: &Path) -> io::Result<VolumeInfo>;
    fn mounted(&self) -> io::Result<Vec<VolumeInfo>>;
}

/// The user's Trash, with Put Back.
pub trait Trash: Send + Sync {
    fn trash(&self, path: &Path) -> io::Result<PathBuf>;        // where it went
}
```

Two more traits sit in the same bundle, because copying needs them: `CloneFile` (APFS `copyfile(CLONE_FORCE)`) and `Metadata` (copy with `copyfile(XATTR|STAT)`, then ACLs and flags, then read back what landed).

**Tests** plug in fakes. One example is a `Flush` that reports `OsOnly`, which exercises the report.

**Rules carried over:**
- No async runtime (F-010): plain threads and crossbeam channels.
- Nothing in the API blocks the caller on disk, except `plan()`, which is documented as stat-only.
- All unsafe code is in `src/sys/`.

**Dependencies.** All are already in the workspace or `DEPENDENCIES.md`, and all are GPLv3-compatible. **No new licences.**

| Crate | Licence | Used for |
|---|---|---|
| libc | MIT/Apache-2.0 | syscalls |
| xxhash-rust (xxh3) | BSL-1.0 | chunk hashes (xxh3-64) and file hashes (xxh3-128, the same as Import Studio and ops) |
| serde, serde_json | MIT/Apache-2.0 | the journal |
| thiserror | MIT/Apache-2.0 | errors |
| crossbeam-channel, parking_lot | MIT/Apache-2.0 | threads |
| unicode-normalization | MIT/Apache-2.0 | name keys (F-017) |
| objc2, objc2-foundation (macOS) | MIT | Trash, and `volumeAvailableCapacityForImportantUsage` |
| clap (CLI only) | MIT/Apache-2.0 | arguments |
| tempfile (dev) | MIT/Apache-2.0 | tests |

The random job id comes from `getentropy` in `sys`, so no `rand` crate is needed.

**Workspace notes for the coordinator:**
- The root `members = ["crates/*"]` picks the new crates up automatically.
- Ops (in M4) needs a `safe-transfer = { path = … }` line in the root `[workspace.dependencies]`. That file is the coordinator's.
- The name breaks F-003's `fstudio-` prefix on purpose, because the crate isn't File Studio's (decision 6a).

---

## 2. Public API

### 2.1 The engine and jobs

```rust
pub struct Engine { /* Arc-shared; cheap to clone */ }

impl Engine {
    /// Opens the engine. `journal_dir` is supplied by the host: File Studio passes
    /// ~/Library/Application Support/File Studio/Safe Transfer/, Import Studio its own.
    /// Loads unfinished jobs (never starts them by itself unless `auto_resume`).
    pub fn open(cfg: EngineConfig, platform: Platform, sink: Arc<dyn EventSink>) -> Result<Engine>;

    /// Dry run: expands folders (stat only, never reads data or iCloud placeholders),
    /// predicts conflicts and checks free space. Blocking, metadata only.
    pub fn plan(&self, spec: &JobSpec) -> Result<Plan>;

    /// Plans, writes the job's manifest durably, and returns its id. Nothing is written
    /// at the destination yet. Fails up front if it won't fit (with the numbers).
    pub fn create(&self, spec: JobSpec) -> Result<JobId>;

    pub fn start(&self, job: JobId) -> Result<()>;
    pub fn pause(&self, job: JobId) -> Result<()>;   // finishes the current chunk's write, then waits
    pub fn resume(&self, job: JobId) -> Result<()>;  // paused, interrupted (crash/quit) or waiting
    /// Always safe: stops, removes this job's own partial files (journal-matched),
    /// leaves every finished file and every original where it is, and keeps the journal
    /// so the job can still be resumed (`Cancel::KeepResumable`, the default) or closes it
    /// (`Cancel::Close`, partials removed and the report kept).
    pub fn cancel(&self, job: JobId, how: Cancel) -> Result<()>;

    pub fn resolve_conflict(&self, job: JobId, c: ConflictId, d: Decision, apply_to_all: bool) -> Result<()>;

    pub fn job(&self, job: JobId) -> Option<JobInfo>;
    pub fn jobs(&self) -> Vec<JobInfo>;            // running, paused, waiting, interrupted
    pub fn report(&self, job: JobId) -> Option<JobReport>;

    /// The host calls this when volumes mount or unmount (File Studio already watches them;
    /// the CLI polls every 2 s). Emits `ResumeAvailable` / `SourceBack` / `DestinationGone`.
    pub fn volumes_changed(&self);
}
```

```rust
pub struct JobSpec {
    pub items: Vec<Item>,
    pub dest: PathBuf,                // a folder
    pub mode: Mode,                   // Copy | Move
    pub options: Options,
    pub label: String,                // "Smith wedding → CLIENT_A", for the resume prompt
    pub host_ref: Option<String>,     // e.g. ops' op id; returned in events, never interpreted
}

pub enum Item {
    /// A file, symlink or folder (copied as a tree), keeping its name.
    Path(PathBuf),
    /// One file to an exact relative destination path (Import Studio's renamed imports).
    Exact { source: PathBuf, dest_rel: PathBuf },
}

pub struct Options {
    pub chunk_size: u64,              // 64 MiB (§14 Q1); 16 MiB..=256 MiB
    pub verify: Verify,               // Always (default; clones excepted) | SkipInternalToInternal
    pub reread_source: bool,          // false; true = read the original a second time, separately (§4.6)
    pub flush_each_chunk: bool,       // true for files over one chunk (§3.6)
    pub small_file_batch: Batch,      // group commit for files < 8 MiB: ≤ 256 files, 64 MiB or 1 s per drive flush (§4.7)
    pub on_conflict: ConflictPolicy,  // per class (files / folders): Ask | Skip | Replace | KeepBoth | Merge(folders)
    pub symlinks: Symlinks,           // CopyLink (default) | Follow
    pub allow_dataless: bool,         // false: iCloud placeholders are skipped, never downloaded (F-016)
    pub disposal: Disposal,           // for Move: Trash (default) | Keep. No hard delete (§7)
    pub space_margin: Margin,         // max(100 MB, 1 % of the job) (§10)
    pub workers: Workers,             // Auto (§12)
    pub auto_resume: bool,            // false (default)
}
```

### 2.2 Events, progress and the report

`EventSink` is a trait with `fn on_event(&self, e: Event)`, implemented for a crossbeam `Sender`, as in ops. Progress events are coalesced to at most one every 16 ms per job (F-010).

```rust
pub enum Event {
    JobState    { job: JobId, state: JobState },          // Planned, Running, Paused, WaitingForSource,
                                                         // WaitingForDestination, DestinationFull, Interrupted, Finished
    Progress    (Progress),
    FileDone    { job: JobId, file: FileRef, outcome: FileOutcome },
    Conflict    { job: JobId, info: ConflictInfo },       // the file waits; the rest of the job goes on
    WaitingForSource { job: JobId, missing: Vec<MissingSource> },  // which files wait on which card
    ResumeAvailable  { job: JobId, volume: VolumeInfo, label: String },
    JobFinished { job: JobId, report: JobReport },
}

pub struct Progress {
    pub job: JobId,
    pub phase: Phase,                 // Copying | Flushing | Verifying | Renaming | Metadata | Trashing
    pub file: Option<FileRef>,        // current file (name, index, size)
    pub file_done: u64, pub file_total: u64,   // bytes in the current phase of this file
    pub done: u64, pub total: u64,    // job "work bytes": write + read-back, so the bar doesn't stall at 100 %
    pub files_done: u32, pub files_total: u32,
    pub bytes_per_sec: u64,           // smoothed over 5 s
    pub eta_secs: Option<u32>,        // computed in Rust from both rates; Swift only shows it
}
```

**Phases, in order for one file:**
1. Copying (with chunk hashing)
2. Flushing
3. Verifying (the read-back)
4. Renaming (into place, plus the folder flush)
5. Metadata (ACLs and flags after the rename)
6. Trashing (moves only)

With parallel workers several files are in flight, and `phase` is that of the file the bar names.

```rust
pub struct JobReport {
    pub copied_verified: u32,
    pub cloned: u32,                  // instant APFS clones (no bytes copied, nothing to verify, §4.4)
    pub renamed: u32,                 // same-volume moves
    pub resumed: u32,                 // files continued from a journal; with chunks reused
    pub restarted: Vec<(FileRef, String)>, // "the original changed (52.4 MB → 53.0 MB)"
    pub skipped: Vec<(FileRef, String)>,   // "already there", "iCloud placeholder", user choice
    pub failed: Vec<(FileRef, String)>,    // per-file reason
    pub metadata_lost: Vec<MetaLoss>, // grouped: "exFAT can't store permissions: 812 files"
    pub flush_level: FlushLevel,      // Device everywhere, or which volume only managed OsOnly
    pub source_reread: u32,           // files whose original was read twice and matched (§4.6)
    pub sources_trashed: Vec<(FileRef, PathBuf)>,   // for undo
    pub replaced_trashed: Vec<(FileRef, PathBuf)>,  // old files sent to the Trash by Replace
    pub bytes: u64, pub duration: Duration, pub verify_duration: Duration,
}
```

### 2.3 Through UniFFI (File Studio)

- Safe Transfer itself has **no UniFFI dependency**.
- `fstudio-core` owns the `Engine` and gives it to `OpQueue`.
- Ops turns Safe Transfer events into its existing `OpEvent`s. `OpStatus` gains `phase`, `eta` and `waiting_for: Vec<…>`.
- `fstudio-ffi` mirrors these the way it mirrors `OpEvent` today.
- Two new `ShellRequest`s (the full manual-resume UX is §6.1):
  - `ResumePrompt { job, label, volume_name, files_left, bytes_left }`, with Resume, Not now and Discard buttons
  - `SourceMissing { job, files_by_source }`
- The shell only draws these and calls `resume` / `cancel` back.
- The auto-resume setting lives in File Studio's settings and is passed in as `EngineConfig.auto_resume`.

### 2.4 Import Studio, later

Import Studio would:
1. call `Engine::open` with its own journal folder
2. build `Item::Exact` entries from its RAW+JPEG naming plan (D-012)
3. take the per-file `xxh3-128` from `FileDone`, which is the same hash it uses today for dedupe
4. use `JobFinished` for its "safe to eject" rule (D-013)

Its single-read, multi-destination fan-out is the one thing Safe Transfer doesn't do in v1. A job has one destination. Two destinations would mean two jobs, reading the card twice. Adding `dests: Vec<…>` with one shared source read is a natural v2, and the chunk pipeline is designed so that a chunk buffer can be written to several destinations. I'd do it when Import Studio needs it, not now.

### 2.5 The CLI

```
safe-transfer copy  <src>… <dest-dir> [--chunk 64M] [--on-conflict ask|skip|replace|keep-both]
                                      [--no-verify-internal] [--reread-source] [--json]
safe-transfer move  <src>… <dest-dir> [--keep-originals] …
safe-transfer plan  <src>… <dest-dir>          # what would happen, conflicts, space; writes nothing
safe-transfer jobs                              # unfinished jobs, by volume
safe-transfer resume <job> | pause <job> | cancel <job> [--close]
safe-transfer report <job> [--json]
safe-transfer volumes                           # mounted volumes with their UUIDs
safe-transfer compare <src> <dest>              # hash both trees (F_NOCACHE), list differences
```

- The journal folder defaults to `~/Library/Application Support/Safe Transfer/` and can be set with `--journal-dir`.
- `--json` writes one event per line, for scripts.
- Exit codes: 0 means everything was verified; 1 means some files failed or were skipped (the report says which); 2 means the job could not start.

---

## 3. The journal

### 3.1 Where it lives

The journal lives in `<journal_dir>/jobs/<job-id>/`, on the Mac. The host chooses `journal_dir`. Nothing about the journal is written to the destination, except the partial files themselves.

```
jobs/k3f9q2xm/
├── lock                 # flock: the app and the CLI can never run the same job at once
├── manifest.json        # written once at create; never changes
├── manifest.bak.json    # identical copy (written after manifest.json is durable)
├── head.a.json          # job state + generation; two slots, written alternately
├── head.b.json
├── seg-0000.a.json      # per-file state for files 0..1023; two slots
├── seg-0000.b.json
└── report.json          # when finished
```

A job of 600–1,000 raws is one segment. Segments exist so that a 100k-file folder copy doesn't rewrite megabytes of journal on every update; only the segments whose files changed are written.

### 3.2 Format (JSON, version 1)

Every file is **two lines**: the JSON body, then a trailer holding the body's length and xxh3-64 hash. A file that is cut short, has a bit flipped or is half written fails the check and is ignored. You can still open it in a text editor.

`manifest.json`:

```json
{"format":"safe-transfer/manifest","version":1,"job":"k3f9q2xm","created":"2026-11-13T10:42:07Z",
 "label":"Smith wedding → CLIENT_A","host_ref":"fstudio-op:1234","mode":"copy",
 "options":{"chunk_size":67108864,"verify":"always","flush_each_chunk":true,"symlinks":"copy_link",
            "on_conflict":{"files":"ask","folders":"merge"},"disposal":"trash","allow_dataless":false,"reread_source":false},
 "dest":{"path":"/Volumes/CLIENT_A/2026-11-13 Smith","volume":{"uuid":"15F49193-C24C-395C-999A-615DCE8F315F",
         "fs":"exfat","name":"CLIENT_A","capacity":128043712512,"case_sensitive":false}},
 "sources":[{"volume":{"uuid":"…","fs":"exfat","name":"Untitled","capacity":…},"root":"/Volumes/Untitled/DCIM"}],
 "files":[
  {"n":0,"kind":"dir","src":"/Volumes/Untitled/DCIM/100MSDCF","rel":"100MSDCF"},
  {"n":1,"kind":"file","src":"/Volumes/Untitled/DCIM/100MSDCF/DSC01234.ARW","src_vol":0,"rel":"100MSDCF/DSC01234.ARW",
   "size":52428800,"mtime_ns":1763030527120000000,"birth_ns":1763030527120000000,"ino":null,"sparse":false}
 ]}
{"len":1234,"xxh3":"9c1f0e2a7b3d4c55"}
```

`seg-0000.a.json`:

```json
{"format":"safe-transfer/segment","version":1,"job":"k3f9q2xm","seg":0,"gen":418,
 "files":{
  "1":{"st":"done","outcome":"verified","hash":"<xxh3-128>","chunks":["<xxh3-64>"],"reread":"match","meta_lost":["acl","owner"]},
  "2":{"st":"copying","chunks":["<xxh3-64>","<xxh3-64>"],"partial_vol":"15F49193-…"},
  "3":{"st":"conflict","conflict":{"existing_size":52428800,"existing_mtime_ns":…}}}}
{"len":512,"xxh3":"…"}
```

**Per-file states:**
- `pending`
- `copying` (the partial exists; `chunks` lists the flushed chunks)
- `flushed`
- `verifying`
- `verified`
- `placed` (real name, folder flushed)
- `trashing_source` (moves)
- `done`

Plus the side states `conflict`, `waiting_source`, `skipped` and `failed`. Unknown fields are ignored, so newer files stay readable.

**Versioning:**
- `version` is an integer.
- A newer engine reads older versions and upgrades the job on its next write.
- An older engine refuses a newer job with a clear message ("this job was made by a newer Safe Transfer") and never touches its partials.

**Paths** are stored as UTF-8 strings. macOS paths always are. On Linux, a non-UTF-8 path would be stored as `{"b64": "…"}`.

### 3.3 Writing it, and recovering from damage

**Each write:**
1. Serialise into `<slot>.tmp`.
2. `F_FULLFSYNC` the temporary file.
3. `rename` it over the *older* slot (A and B alternate).
4. `F_FULLFSYNC` the folder.

The highest valid `gen` wins on load. A crash at any point leaves at least the previous good slot.

**Recovery, from mild to worst:**
- **One slot is damaged or torn:** the other slot is used. The job loses at most one update, which only means a little work is redone.
- **Both slots of a segment are damaged:** those files are treated as "unknown" and rebuilt from the disk using the manifest:
  - Real name present: hash it against the original. A match means done; otherwise it's a conflict.
  - Partial present (its name is known from the manifest, §5): re-hash it chunk by chunk against the original, which is read again.
  - Neither: pending.
- **Manifest damaged:** the `.bak` copy is used.
- **Both manifests damaged:** the job is listed as "damaged". Safe Transfer won't resume it and won't delete any of its partials, because nothing can be matched safely. The report names the folder so a person can look.

**Why a damaged journal can never hurt a file.** The journal is a list of claims, and every claim is checked again against the disk before anything is trusted. Resume re-hashes chunks; files with real names were verified before they got those names. A journal that is behind the disk means repeated work. A journal that is ahead of the disk, or wrong, is caught by the re-hash. The journal never authorises deleting anything except a partial whose exact name it holds (§5).

### 3.4 Recognising volumes (measured)

The probe (`st_probe volid`) results on disk images under macOS 27.2:

| Filesystem | `getattrlist(ATTR_VOL_UUID)` | Same as `diskutil`'s Volume UUID | After detach and re-attach |
|---|---|---|---|
| APFS | `A3088E8F-47F1-44C9-…` | yes | – |
| exFAT (FSKit driver) | `15F49193-C24C-395C-…` | yes | **same UUID** |
| FAT16 | `D7E958B7-3AC6-323B-…` | yes | – |
| `st_dev` (any) | – | – | **changed (16777248 → 16777250), and the old number went to the FAT image mounted next** |

The exFAT and FAT UUIDs are version-3 (name-based) UUIDs that macOS derives from the card's **volume serial number**, which is written when the card is formatted. A camera writes a new one on every format. So:

- **Two cards both called "Untitled"** have different UUIDs, and Safe Transfer keeps them apart. The name is stored only so the prompt can say which card to put back.
- **The key** is the UUID plus `fs` and `capacity` as a cross-check. A mismatch in either means "not the same card".
- **Two cards with the same UUID** can happen with a card cloned sector by sector, or very rarely by coincidence. Safe Transfer would take the wrong card for the right one, but that still can't produce a bad file:
  - every reused chunk is re-hashed against the journal first
  - a missing partial just means the file starts again
  - cleanup only removes files whose exact job-specific name it holds, and a different card won't have those names unless it is a clone of this one
- **Volumes without a UUID** (some network shares) fall back to the server and share plus capacity. Their identity is marked "weak", and the prompt asks you to confirm before resuming.

No DiskArbitration dependency is needed: `getattrlist` returns the same UUID.

### 3.5 Reconnect detection

The host tells the engine when volumes change (`volumes_changed()`). File Studio already does this for the leftover sweep, and the CLI polls `getmntinfo`. The engine then compares the mounted UUIDs with its unfinished jobs:

- **Destination back, sources present:**
  - With `auto_resume` off (the default): `ResumeAvailable` → File Studio shows "CLIENT_A has an unfinished copy: Smith wedding, 212 of 640 files left (11.3 GB). Resume / Not now / Discard".
  - With it on, the job resumes and a toast says so.
- **Destination back, a source missing:** `WaitingForSource`, listing the files per source card, for example "212 files wait on the card 'Untitled' (64 GB, exFAT)".
- **Destination pulled during a job:** that job's state becomes `WaitingForDestination`. The current file stops, its partial stays on the card, and no file is marked failed. This is the "don't give 600 failures for one pulled card" rule.

### 3.6 How often the journal is written, and why 64 MB chunks

- A chunk is marked complete **only after it is flushed** to the destination (your rule).
- The journal is written at most **twice a second** while anything changed. It is also written immediately when the job starts, pauses, is cancelled or finishes, and when it waits for a conflict.
- Each write is a few KB to a few hundred KB, on the internal SSD.

Recording a chunk late is safe (§3.3), so the write frequency only decides how much work a crash can repeat: at most about 0.5 s of copying, plus the current chunk.

**The chunk size trade-off:**

| | 16 MB | **64 MB** | 256 MB |
|---|---|---|---|
| Flushes per GiB (big files) | 64 | **16** | 4 |
| Measured flush cost per GiB, APFS image (§12) | 590 ms | **145 ms** | ≈ 40 ms (est.) |
| Work lost per interruption, worst case | 16 MB | **64 MB** (≈ 0.5 s on a card) | 256 MB (≈ 2 s) |
| A 25–80 MB ARW | 2–5 chunks | **1–2 chunks** | 1 chunk |
| Journal entries for 1,000 ARWs | ≈ 3,000 hashes | **≈ 1,300** | 1,000 |

- At 64 MB, a typical raw is one chunk. It costs exactly one flush, the same as a plain verified copy.
- Chunking only adds flushes for big files, such as video or PSB.
- Memory doesn't grow with the chunk size: I/O runs in 4 MiB buffers (F-015 found no gain beyond 256 KiB). A "chunk" is only the unit for hashing, flushing and journaling.

---

## 4. Flushing and verifying

### 4.1 What `F_FULLFSYNC` does and doesn't promise

- **`fsync` on macOS** sends the file's data to the drive, but the drive may keep it in its own volatile cache.
- **`F_FULLFSYNC`** also sends the drive a "flush your cache" command, which is SCSI SYNCHRONIZE CACHE over USB bridges, or NVMe Flush. That command is drive-wide: it makes every earlier write durable, not just this file's.
- **On APFS and HFS+**, the filesystem's metadata (sizes, allocation, the rename) is committed in order with it.
- **On exFAT (the FSKit driver in macOS 26+) and FAT:** I measured that `F_FULLFSYNC` is **accepted** (returns 0, on files and on folders). There's no journal on these filesystems, so ordering is our job, which is why each step waits for the flush before the next one starts.

**What it can't promise:**
- A drive or USB card reader that acknowledges the flush without doing it. Cheap readers have been known to.
- Data damaged inside the card after it is written (wear). That is what the read-back check is for, at the time of the copy.

**Where Safe Transfer flushes:**
1. After each chunk (files bigger than one chunk).
2. At the end of each file.
3. On the folder, after the rename.

If `F_FULLFSYNC` is refused and only `fsync` works (some network filesystems), the report says "this disk only confirmed an OS-level flush" instead of claiming more.

**Small files (group commit)** are a first-class part of the pipeline: see §4.7, which has the measurements, the exact ordering and how it is proved.

### 4.2 Making the read-back really come from the drive (measured)

There are three ways the "check" could secretly read RAM instead of the card. The probe shows two of them happening and the cure for each:

| Written | Read back with | Pages in RAM before the read (mincore) | Read speed (APFS image) | Proves anything? |
|---|---|---|---|---|
| through the cache, then `F_FULLFSYNC` | fresh fd + `F_NOCACHE` | **100 %** | **19.6 GB/s** (RAM) | **No.** `F_NOCACHE` doesn't evict pages that are already cached |
| with `F_NOCACHE` | fresh fd + `F_NOCACHE` | 0 % | 5.8 GB/s (below the cache) | yes |
| with `F_NOCACHE`, then *anyone* reads it normally (Spotlight, Quick Look, a thumbnailer) | fresh fd + `F_NOCACHE` | **100 %** | **18.5 GB/s** (RAM) | **No** |
| any of the above, then `msync(MS_INVALIDATE)` on a read-only mapping | fresh fd + `F_NOCACHE` | **0 %** (the eviction worked, no admin rights needed) | 5.1 GB/s | yes |

The same pattern held on the exFAT image.

**Safe Transfer's verify step:**
1. Write the partial with `F_NOCACHE`, so nothing of ours stays in the cache.
2. `F_FULLFSYNC`.
3. **Evict**: map the partial read-only, `msync(MS_INVALIDATE)`, unmap.
4. **Check** with `mincore` that 0 % of its pages are resident. If any are still resident, evict once more, and if that fails, report the file as "verified from memory only" instead of calling it verified.
5. Open a **fresh** descriptor with `F_NOCACHE` and read the whole partial in 4 MiB reads.
6. Compare each chunk's xxh3-64 and the whole file's xxh3-128 with the hashes taken from the source bytes during the copy.

So the Mac's own cache is ruled out. What's left is the drive's internal cache: after a successful `F_FULLFSYNC` it holds the same data as the storage, and §4.1 covers drives that lie about that.

A note on the measurements: a disk image's own "device" is a file on the internal SSD, which the host can cache. That's why the image read-backs run at 5–6 GB/s. The residency results are valid; the speeds aren't card speeds (§12).

**On a mismatch:** Safe Transfer re-reads the source chunk and the partial's chunk to find out which side is wrong.
- **The copy is wrong:** the partial is removed and the file is copied once more.
- **The source reads differently now:** that's a failing card or reader. The file fails with "the original read differently twice. The card or reader may be failing", and Safe Transfer never retries in a loop.

### 4.3 Hashing while writing

- Each 4 MiB read from the source is fed to both the chunk's xxh3-64 and the file's xxh3-128 before it is written. xxh3 runs at about 11.5 GB/s on one core (SPIKES §3), which is far above any card or USB disk.
- The reading and the writing overlap: two 4 MiB buffers in flight per file, so hashing is hidden.
- When resuming, the file-level xxh3-128 is rebuilt as the re-hash streams through the existing chunks, so it stays a hash of the whole file.

### 4.4 Same-volume APFS clones

- **The fast path stays.** A copy on the same APFS volume is `copyfile(CLONE_FORCE)` to the partial name, then metadata, then the rename. It's instant and keeps ACLs (F-015).
- **Why clones aren't verified:** a clone writes no data. It is a second name for the same blocks on the same disk. Reading it back would read exactly the blocks the original reads from, so a "check" would compare the original with itself and prove nothing. A clone can't be "half written": APFS commits it as one transaction.
- **What clones do get:** the rename and the folder flush, as for every other file.
- **In the report**, clones show as "cloned (shares storage with the original until either changes)", not as "verified", so the report never claims a check that didn't happen.
- If cloning fails (the volume can't clone, or `EXDEV` through firmlinks), the file goes down the normal chunked path and **is** verified.

### 4.5 Sparse files (F-015)

- The chunk loop keeps holes using `SEEK_DATA` and `SEEK_HOLE`. A hole counts as zeros in the hashes and is skipped on the destination.
- The copy never goes through `copyfile(ALL)`'s data path, which fills holes in.
- exFAT and FAT can't store holes, so the zeros are written out. The report says "written in full: this disk can't store sparse files". The space check (§10) counts the full logical size for those disks.
- Sparse files are rare in photo work (they are mostly VM disks and some databases), but this keeps a 32 KB file from silently becoming 1 GB.

### 4.6 Re-read originals (opt-in, off by default)

**What it protects against.** Normally the original is read once. Its bytes are hashed in memory, written to the destination, and the copy is checked against that hash. If the card or the card reader returned a wrong byte during that one read, the copy is a faithful copy of the wrong byte, and the check passes. With this mode on, the original is read a **second time, separately**, and the two reads must agree before the copy gets its real name.

**How it works.**
1. Before the first read, any cached pages of the original are evicted (§4.2). This is read-only for the original: `msync` with `MS_SYNC|MS_INVALIDATE` writes back anything another app has pending before dropping it, so nothing of the original's content can be lost.
2. The copy is made, flushed and read back as usual.
3. The original's cache is evicted again, and the original is read a second time with `F_NOCACHE`. Its xxh3-128 must equal the first read's.
4. **If they agree**, the file is placed, and the report counts it under `source_reread`.
5. **If they don't**, the partial is removed and the file fails with "the original read differently twice; the card or reader may be failing". It is never retried in a loop, and the original is never touched.

**The cost.** Roughly twice the reading time from the source. Copying from a card means reading the card twice: a 30 GB job at 161 MB/s takes about 3 minutes longer. From the internal SSD the cost is small.

**Where it shows up.**
- `Options.reread_source`
- the manifest's `options.reread_source`
- the per-file `"reread": "match"` in the journal
- `JobReport.source_reread`
- the CLI's `--reread-source`

**Settings key (File Studio):** `operations.reread_originals`, a bool, default `false`, under Settings › Operations › Copying. Import Studio would keep its own.

**First-run setup text** (spec for M4; the page is core-driven, and Swift only shows it):

> **Read originals twice?**
>
> File Studio always checks every copy against the original before giving it its real name. That catches a copy that went wrong on the way to the disk.
>
> It can't catch a copy that was wrong from the start: if a failing card or card reader hands over a bad byte while the original is being read, the copy matches that bad byte perfectly.
>
> **Turn this on to read each original a second time** and make sure both reads agree. If they don't, the file is flagged and your original is left untouched.
>
> **Who it's for:**
> - copying straight off camera cards that have shown errors
> - ageing or unreliable card readers
> - archives and client deliveries, where a second read is worth the time
>
> **The cost:** copies take longer, about twice the reading time from the original's disk. From a card, a 30 GB job takes about 3 minutes more.
>
> [ Read originals twice ]   [ Not now ]   You can change this any time in Settings › Operations.

### 4.7 Small files: one drive flush per group (M2, first-class)

**Why it matters.** A drive flush (`F_FULLFSYNC`) costs about the same whether it covers one byte or a thousand files:
- ≈ 7.7 ms on the internal SSD
- ≈ 20 ms on an APFS disk image
- ≈ 16 ms on an exFAT disk image

fstudio-ops now flushes every copied file before its rename, and that took 5,000 small files from 1.2 s to 48 s (≈ 9.3 ms per flush, the coordinator's measurement). The probe below measured the same pattern, per-file barriers, at 39–101 s. Photo jobs have plenty of small files: XMP sidecars, JPEG previews, `.THM`, catalogue folders.

**The rule stays exact:** nothing gets its real name before a drive flush that covers its data, and nothing is reported done before a drive flush that covers its rename. A group only shares the flushes. Each file is still written, hashed, read back from the drive and compared on its own.

**Which files are grouped.**
- **Eligible:** files smaller than 8 MiB, which is one-eighth of a chunk.
- **A group closes** at the first of: 256 files, 64 MiB of data, 1 second of writing (so a slow card never holds many unnamed files), or the end of the job.
- **Not grouped:** larger files keep the per-chunk pipeline of §4.1–4.2.

**The order, for one group:**
1. **Write each file.** For every file: write its partial with `F_NOCACHE`, hash it while writing, copy its xattrs and dates, clear immutable flags and ACLs, then **`fsync(fd)`** and close.
   - `fsync` has no drive barrier, so it is cheap (0.27 s for 5,000 files on the internal SSD), but it is documented to send that file's data *and attributes* to the drive.
   - The journal records `written`.
2. **One drive barrier:** `F_FULLFSYNC` on the last file's descriptor. It is documented to "ask the drive to flush all buffered data to permanent storage". The drive flush command carries no file scope (SCSI SYNCHRONIZE CACHE over the whole device, NVMe Flush per namespace), so it covers every write the drive has already received, from any descriptor.
   - The journal marks the group `flushed`. This is when its chunks count as complete.
3. **Check and name each file:** evict it from the cache, confirm with `mincore` that nothing is left, read it back uncached and compare the hashes. In re-read mode the original is read again here too. Then `renamex_np(RENAME_EXCL)`.
   - A file that fails its check is **not** renamed. It goes back for one retry in the next group, and the rest of the group carries on.
4. **One folder barrier:** `fsync` on each destination folder touched by the group (usually one), then one `F_FULLFSYNC`. Only now are the group's files `done`.
5. **ACLs and immutable flags**, then the metadata check, as for any file.

**If the barrier fails** (EIO, or the card was pulled), none of the group is renamed. The partials stay, and the job waits for the destination (§3.5).

**Why `fsync` each file, rather than relying on `F_NOCACHE` writes.** An uncached write sends the *data*, but the file's size and its directory entry stay in the filesystem's memory. In the harness below, files written uncached with no flush were **all lost** on APFS. Experimentally, a single `F_FULLFSYNC` alone covered all 2,000 files even without the per-file `fsync`. That relies on undocumented filesystem behaviour, though, and the per-file `fsync` costs almost nothing. So the design uses both: per-file `fsync` plus a group `F_FULLFSYNC`. That is the fallback you suggested, adopted as the primary design.

**How it is proved:**
1. **By documentation.** `fsync(2)` moves each file's data and attributes to the drive. `F_FULLFSYNC` then moves everything the drive holds to permanent storage. Together they cover every file in the group, whichever descriptor wrote it.
2. **By a yank harness that can tell the difference (measured today).**
   - `hdiutil detach -force` is **not** a yank: it writes cached data back first. Even files that were never flushed survive it (2,000 of 2,000 intact on both filesystems), so it can't prove anything about flushing.
   - The harness instead does `kill -9` on the image's own `diskimages-helper`, the process that plays the drive. That makes the drive vanish with whatever it wasn't told to flush.
   - Results for 2,000 small files written then yanked (two runs each; the five modes are `st_probe batchwrite` modes):

     | Mode | APFS image | exFAT image |
     |---|---|---|
     | cached writes, no flush | **2,000 missing** | **46–70 missing, 1 short file under its real name** |
     | `F_NOCACHE` writes, no flush | **2,000 missing** | 46–56 missing |
     | `fsync` each file | all intact | all intact |
     | cached writes + **one** group `F_FULLFSYNC` | all intact | all intact |
     | `F_NOCACHE` + one group `F_FULLFSYNC` | all intact | all intact |

   - The negative controls lose data, so the harness is real. The group barrier covered files written through other, already-closed descriptors on both filesystems.
3. **In the M2 test suite.**
   - New stop points: `BeforeGroupBarrier`, `AfterGroupBarrier`, `MidGroupRename(k)` and `BeforeFolderBarrier`.
   - At each one, the helper-kill yank runs on APFS, exFAT, FAT and HFS+ images. Every file of the group is checked: it must be absent, or complete under its real name, never short.
   - Each filesystem also gets a negative-control test that must lose data without the barrier. The M1 suite already has one for APFS, `the_yank_harness_loses_unflushed_writes`.
   - The M1 yank tests have been switched to this harness and still pass.
4. **On real hardware (M5):** your card and reader, pulling the reader's USB cable during a 5,000-file copy, ten times.

**What it doesn't prove:** a disk-image helper behaves like a drive that honours flushes. A reader that acknowledges a flush without doing it isn't modelled (§4.1). Only the real-card runs in M5 can catch that.

**Cost, measured with `st_probe batch`.** 5,000 files of 1–64 KiB (≈ 160 MB). Every row includes the uncached read-back of every file, the exclusive rename and the folder flush:

| Destination | No flush (unsafe, for reference) | Flush each file | Groups of 64 | **Groups of 256** | Groups of 1,024 |
|---|---|---|---|---|---|
| Internal SSD (APFS) | 0.91 s | 38.6 s | 2.09 s | **1.53 s** | 1.32 s |
| APFS disk image | 1.50 s | 100.9 s | 3.13 s | **1.82 s** | 1.43 s |
| exFAT disk image | 13.5 s | 78.5 s | 15.8 s | **14.2 s** | 14.8 s |

exFAT is slow at creating small files whether or not anything is flushed (12.8 s with no flush and no verify), so batching adds about 5 % there. Going from 256 to 1,024 files per group saves little and leaves four times as many files waiting unnamed, so the group stays at 256.

**Benchmark targets (M2 gate, `fstudio-bench` rows):**
- **5,000 small files, internal SSD → APFS disk image, verified: ≤ 2.4 s.** That is 2× the old 1.2 s unflushed ops number. The probe does 1.82 s, which leaves room for the engine's own overhead (journal writes, events).
- **The same onto an exFAT image: ≤ 1.25× the unflushed exFAT time.**
- **On your card (M5): within 1.3× of today's 73.9 s,** which was measured without flushes, so ≤ 96 s.

---

## 5. Partial names and cleanup

**The name:** `.<name>.<job>-<n>.st-partial`, for example `.DSC01234.ARW.k3f9q2xm-417.st-partial`. This reconciles the brief with F-020:

- **The brief:** it's hidden and ends in `.st-partial`.
- **F-020:**
  - it contains the job id (8 random base-32 characters) and the file's index in the manifest
  - two files whose names differ only by case or normalisation (`File`, `file`) never share a temporary name
  - two jobs writing into the same folder never collide
- **The name is fully derivable from the manifest,** so a partial is "ours" even when the journal write that would have recorded it never happened.
- **No mix-ups between engines:** ops' `.fstudio-partial` files and Safe Transfer's `.st-partial` files are never confused. Each engine only removes its own suffix.

**The 255-byte limit:**
- APFS allows 255 bytes per name. exFAT and FAT allow 255 UTF-16 units, which is never fewer, so 255 bytes is the safe bound.
- The original name is shortened on a character boundary so that the whole partial name, **plus 2 bytes for the `._` prefix** of the AppleDouble companion that exFAT creates, fits in 255.
- The real name is never shortened. A final name that a disk refuses fails that file with "the name is too long for this disk".

**Folders** are created as `.<name>.<job>-<n>.st-partial` and renamed into place at once (empty), as ops does. Their metadata is applied when their contents are done.

**Cleanup.** Only Safe Transfer removes `.st-partial` files, and only through `remove.rs::remove_partial`, which refuses a path unless all of these hold:
1. its name is exactly the derived partial name of a file in a job manifest it holds, for that job's destination folder
2. the volume it sits on has that job's destination UUID
3. the file's state isn't `placed` or `done`

Its `._` companion is removed with it. Cleanup runs when a job is cancelled with Close, when you choose Discard on a resume prompt, when a file is restarted because its original changed, and when a file fails. If the card is gone at that moment, the record is kept and swept when the card returns (by UUID, §3.4).

A grep test, as in ops, fails if any other file in the crate removes anything. `.st-partial` files from a job whose journal is gone (both manifests damaged) are left alone and listed; Safe Transfer never removes a file just because its name looks like a partial.

---

## 6. Resume

For every file that isn't `done`, in manifest order:

1. **Is the original there?** Its source volume (by UUID) must be mounted and the file must exist.
   - If the card is missing, the file goes to `waiting_source`, and the job keeps going with the files it can do.
   - When nothing else is left, the job is `WaitingForSource`, listing exactly which files wait on which card.
2. **Did the original change?**
   - The size and modification time are compared with the manifest, and the inode number too on APFS and HFS+. exFAT and FAT invent inode numbers when mounting, so they aren't used there.
   - If anything differs, the partial is removed (§5) and the file starts over. The report says "restarted: the original changed (52.4 MB → 53.0 MB, modified 10:51)".
   - FAT stores times to 2 seconds and exFAT to 10 ms, so comparisons use the source filesystem's precision.
3. **Is the real name already there?**
   - If the state was `placed` or later, Safe Transfer checks that it's our file (the same inode on APFS/HFS+; elsewhere the size plus a hash) and finishes the remaining steps (metadata, trashing the source for a move).
   - If the real name is there but the state is earlier, someone else put a file there. That is a conflict.
4. **Is there a partial?**
   - Evict it from the cache (§4.2), then re-hash it chunk by chunk with `F_NOCACHE`, comparing with the journal's chunk hashes.
   - The longest run of matching chunks from the start is kept. The partial is truncated there and copying continues from that offset.
   - Bytes beyond the last journaled chunk are always redone.
5. **A journal segment was damaged?** Same as step 4, but the chunks are compared with freshly read source chunks (§3.3).

**When chunk-level resume pays off:**
- **Card → SSD** (re-hashing the SSD partial at GB/s instead of re-reading the card): big win.
- **Mac → card:** re-hashing 1.6 GB on the card (about 10 s) versus re-copying it (about 13 s) is only a small win.
- **Resume per file** (skip the 600 files already done, with no re-reading) is where most of the time is saved in either direction. Files with real names are trusted after a size check, because they were verified before they were named. A "re-verify finished files" option exists for the cautious.

**Auto-resume** is off by default (your brief). When it's on, a job resumes as soon as its destination and sources are present, and a toast says so.

### 6.1 Resuming by hand (auto-resume off)

Your words: *"if it's disabled but you trip it in the software there's like a resume transfer for that data."* With auto-resume off, an unfinished job never just waits out of sight. It can be resumed from the app at any time, from three places, and every manual resume ends with the same per-file report as an automatic one.

**Which jobs count:** interrupted (a crash, quit or power cut), waiting for the destination (pulled mid-job), waiting for a source card, destination full, and cancelled (Cancel keeps a job resumable; only Discard ends it).

**1. A prompt when the disk comes back** (`ResumeAvailable`, shown once per reconnect):

> **Resume “Smith wedding → CLIENT_A”?**
> 312 of 800 files left (11.3 GB). Files already copied are kept, and so is anything partly copied that still checks out.
>
> [ Resume ]   [ Later ]   [ Discard… ]

- **Later** closes the prompt and changes nothing: the job stays in the operations list.
- **Discard…** always asks first:

  > **Discard the rest of this copy?** The 488 finished files on CLIENT_A stay. The 3 unfinished hidden copies are removed. Your originals aren't touched.
  >
  > [ Cancel ] [ Discard ]

  It removes only this job's own partials (§5), and only with the destination connected. The engine refuses otherwise ("insert CLIENT_A first").
- Cancelled jobs aren't prompted on reconnect, because you stopped them yourself. They stay in the list.

**2. The operations list:**
- The job stays as a row labelled **Interrupted** (or "Waiting for Untitled", or "CLIENT_A is full"), with the counts and a **Resume Transfer** button.
- A **badge** on the toolbar's operations icon shows how many jobs are waiting.
- The button's state follows the disks:
  - destination not connected: **"Insert CLIENT_A to resume"**, disabled, and it enables itself as soon as that volume appears
  - destination there but some source cards missing: Resume is enabled and copies what it can; the row says "212 files wait for Untitled"
  - destination full: "Free space on CLIENT_A, then resume"
  - running in the other app or terminal: disabled, "Running in another window"

**3. The sidebar:** a card or drive with unfinished jobs gets **Resume Transfer** in its context menu (and **Resume All Transfers** when it has more than one). This matches jobs by volume UUID, as either the destination or a source, so it works even when two cards share a name.

**Commands** (registry, F-006): `transfer.resume` (the selected row or sidebar disk) and `transfer.resume_all` (every job whose disks are connected). They are listed in the palette, the menus and the Settings shortcut editor.

**CLI:**
- `safe-transfer resume <job>`
- `safe-transfer resume --all`, which also lists what the others still need ("insert CLIENT_A to resume…")
- `safe-transfer jobs`, which shows each job's state and what it needs

**What the engine provides (built in M2):**
- `JobInfo.resume`, a `Resumability`: one of `NotNeeded`, `Ready { still_missing }`, `NeedsVolumes([names])` or `InUseElsewhere`, computed from the mounted volumes.
- `JobInfo.dest_uuid`.
- `Engine::resumable()` for the list and the badge, `Engine::jobs_on_volume(uuid)` for the sidebar, and `Engine::resume_all()`.
- `cancel(Close)` refuses with `NeedsVolume` while the destination is away.
- Tests: `a_stopped_job_stays_listed_and_can_be_resumed_by_hand`, and the CLI's `resume_all_finishes_a_crashed_copy`.

**What M4 builds:**
- In `fstudio-core`: the prompt as a `ShellRequest`, the operations-list row, the badge count and the sidebar menu items. The core recomputes them on every volume change (the host already calls `volumes_changed`).
- In the Swift shell: drawing them, with no logic of its own (F-001).

**The journal is unchanged:** the job's state already lives in its head file, and resumability is worked out from the mounted volumes each time. "Later" isn't stored. The prompt comes back on the next reconnect or launch, and the list row is always there.

---

## 7. Moves

- **Same volume:** one exclusive rename, the whole folder at once. Nothing is copied, so nothing needs checking. On exFAT this uses the check-then-rename fallback.
  - The journal records the intended rename first.
  - Recovery looks at which side exists: source only means not done; destination only means done; both means a conflict appeared and is reported.
- **Across volumes**, per file:
  1. Copy → flush → evict → verify → rename → flush the folder.
  2. Check the original is unchanged since it was copied (as ops' `remove_moved_source` does).
  3. Send the original to the **Trash**, through the `Trash` trait, so Put Back works.
  4. Folders are removed only once empty (`rmdir`, also in `remove.rs`).
- **No hard delete in Safe Transfer.** `Disposal` is `Trash` or `Keep`. If you ask File Studio for a move that permanently deletes, ops does that removal itself, after Safe Transfer reports the file verified, through its existing gated `delete_immediately` (F-007). That keeps one gated delete path in the whole app.
- **Disks with no Trash** (some FAT and network volumes): the original is **kept**, and the report says "copied and verified; the original was kept because this disk has no Trash". A file is never missing from both places.
- **Moving off a card:** macOS puts the card's trashed originals in the card's own `.Trashes` folder. They stop using space only when the Trash is emptied or the card is formatted. The report says so the first time.
- **F-023 (protected folders):** the decision and the confirmation stay in ops and core, before the job exists. Safe Transfer's `plan()` lists the top-level folders a job would move, so ops can ask. Toasts with Undo stay in ops.
- **Undo** stays in ops. It uses the report's `sources_trashed` and `replaced_trashed` locations and the destination paths, exactly as ops' undo uses its own journal today.
- **Crash recovery for moves:**
  - If the destination is placed and verified and the original is still in its place, the original is checked once more and then trashed.
  - If the original is gone, it's either in the Trash already or the user moved it. Either way the destination copy is complete.

---

## 8. Conflicts

**When they're found:**
- **At plan time,** conflicts are predicted using NFC and case-folded keys on case-insensitive volumes (F-017), so File Studio can ask once, up front: "312 of 640 files already exist on CLIENT_A".
- **The authoritative check is the exclusive rename** (`EEXIST`), which also catches files that appear while the job runs.

**The decisions:**
- Skip, Replace, Keep Both, Merge (folders only), or Ask.
- Each can be set separately for files and for folders, with "apply to all" ("apply to the rest of this job").
- A file waiting for an answer doesn't hold up the others.

**Replace never overwrites:**
1. The new file is written, flushed and verified as a partial.
2. The old file goes to the Trash (Put Back works; the report records where).
3. The partial is renamed to the real name exclusively, and the folder is flushed.

If anything fails before step 2, the old file is untouched. A crash between steps 2 and 3 leaves the old file in the Trash and the verified new one as a partial. Resume finishes step 3. The name is empty for a few milliseconds, but it never holds an incomplete file.

I considered `renamex_np(RENAME_SWAP)`, which swaps the two in one step, and rejected it: the old file would end up in the Trash under the hidden partial name, which breaks Put Back.

**Keep Both** uses Finder's "name 2", "name 3" numbering (ops' `keep_both_names`). The name is chosen at rename time and retried if it gets taken.

**Suggested extra decision: "Skip if identical"**
- Same size, plus a hash of the existing file: skip it as "already there, identical".
- Useful when you re-run an import onto a drive that has half of it.
- It costs reading the existing files, so it's offered rather than the default (§14).

---

## 9. Metadata

Applied in this order:
1. **Data.**
2. **Extended attributes and stat info** (`copyfile(XATTR|STAT)`): mode, dates, Finder tags, colour labels, comments, the resource fork.
3. **The rename.**
4. **ACLs and the immutable flags**, after the rename (SPIKES S2: a `deny delete` ACL would block the rename).
5. **Read back:** Safe Transfer reads the destination's metadata and compares it with the source per category. Anything that didn't survive goes into `metadata_lost`.

**What each filesystem can hold** (✓ kept; ✗ can't be stored, reported; "AD" means stored in a hidden `._` AppleDouble file):

| | APFS / HFS+ | exFAT | FAT32 |
|---|---|---|---|
| Data | ✓ | ✓ | ✓, **but files ≥ 4 GiB can't exist**: refused at plan time |
| Modified date | ✓ (HFS+: 1 s) | ✓ (10 ms) | ✓ (2 s; reported as "rounded") |
| Created date | ✓ | ✓ | ✓ |
| Permissions, owner | ✓ | ✗ (mounted "noowners") | ✗ |
| Finder tags, colour labels, comments, other xattrs | ✓ | AD | AD |
| Resource fork | ✓ | AD | AD |
| ACLs | ✓ | ✗ | ✗ |
| Locked / immutable flag | ✓ | ✗ (to be checked in M3) | ✗ (to be checked in M3) |
| Hidden flag | ✓ | checked in M3 | checked in M3 |
| Sparse holes | ✓ | ✗ (written in full) | ✗ |

**AppleDouble on cards.** On exFAT and FAT, Finder tags survive in `._DSC01234.ARW` files, as they do when Finder copies. Cameras, Windows and some card tools delete those files, which takes the tags with them. The report says this once per job, not once per file:

> "Kept Finder tags on 214 files in hidden ._ files. Cameras and Windows may remove them."

Also on exFAT:
- **The AppleDouble name can be too long.** A real name of 254–255 bytes leaves no room for the `._` companion, so that file's tags can't be stored. It is reported for that file.
- **Some characters aren't allowed** (`" * : < > ? \ |`), and macOS refuses names containing them. That file fails with "this name can't exist on exFAT". It is never silently renamed. Plan time warns ahead.

**Where the `._` companion is written is decided in M3.** The edge-fixes work found that an exFAT copy can leave the `._` companion of a partial behind on its own. There are two options:
- apply xattrs only after the rename on these disks, or
- keep them on the partial and rename the companion too.

M3 tests both and keeps whichever leaves nothing behind when the card is yanked. Either way the data is already flushed and verified.

---

## 10. Free space

**The calculation at plan time:**
1. Each file's size, rounded up to the destination's allocation unit. On a 128 GB exFAT card that is usually 128 KB.
2. On exFAT and FAT, one more allocation unit per file that has xattrs (its `._` file).
3. Replaced files count in full. Their old copies go to the Trash on the same volume, so no space is freed during the job.
4. Clones count as zero.

**The free space** comes from `volumeAvailableCapacityForImportantUsage` on APFS, which includes purgeable space and matches what Finder shows, and from `statfs` elsewhere.

**The margin** is the larger of 100 MB and 1 % of the job, so a card is never filled to the last byte, where some cameras and filesystems misbehave.

**If it won't fit,** the job is refused up front with the numbers:

> "Needs 48.2 GB (including a 0.5 GB safety margin); CLIENT_A has 47.9 GB free."

**If the disk fills mid-job anyway** (another app wrote to it): the current file's partial is removed and the job pauses as `DestinationFull`. It doesn't fail every remaining file. Free some space and resume.

---

## 11. Test plan

### 11.1 Unit tests (pure, no I/O)

- **Chunking:** boundaries at 0, 1, chunk − 1, chunk, chunk + 1 and 2 GiB + 1 bytes; hole maps.
- **Hashes:** a chunk run's hash equals a whole-file hash; the hash is rebuilt correctly on resume.
- **Journal encode and decode:** the trailer check; every single-byte flip and every truncation length of a sample file is rejected; the A/B choice by generation; an upgrade from a fake "version 0".
- **Recovery:** each combination of state × which files exist on disk.
- **Resume rules:** source changed by size, by mtime at the source's precision, or by inode; prefix-match truncation.
- **Partial names:** 255-byte and `._` bounds with emoji and NFD names; derivable from the manifest; `File`/`file` and NFC/NFD pairs never collide.
- **Free space:** cluster rounding and margins.
- **Conflict rules and apply-to-all.**

### 11.2 Fault injection (`--features fault-injection`, never in release builds)

Named stop points are checked in the pipeline:

| Point | Where |
|---|---|
| `MidChunk(file, byte)` | inside the 4 MiB write loop, at an exact byte |
| `BeforeFlush` / `AfterFlush` | around `F_FULLFSYNC` (per chunk and per file) |
| `MidVerify(file, byte)` | inside the read-back |
| `BeforeRename` / `AfterRename` (before the folder flush) | around placing |
| `MidJournalWrite(Tmp \| BeforeRename)` | the journal's temp file half written / written but not yet renamed |
| `BeforeTrashSource` | moves |

Each point can do one of three things: return an error, **pause** (the engine writes `REACHED <point>` to a pipe and waits until the test says go), or abort the process. Pausing is what makes the yank and kill tests deterministic: the test waits for `REACHED MidVerify(3, 10485760)`, then runs `hdiutil detach -force` or `kill -9`, then lets go.

"Mid-flush" is as close as we can get without kernel hooks. The test pauses just before the flush, starts the yank or kill from another thread, then releases, so the flush races the yank. That runs 20 times with jitter.

### 11.3 Yank tests (`#[ignore]`, run with `--ignored`; images always detached and deleted)

- **Destinations:** a 2 GB `hdiutil` image as APFS, as exFAT, and (smaller) as FAT32. The source is a temp folder on the internal disk, and for one case a second exFAT image as a "source card".
- **For each stop point above, on each filesystem:**
  1. Start a job of 40 files of 5–90 MB plus one 2.1 GB file.
  2. Wait for `REACHED`, then yank: `kill -9` the image's `diskimages-helper` (the "drive") and clear the dead mount. `detach -force` alone doesn't count as a yank, because it writes cached data back first (§4.7).
  3. Re-attach.
  4. Check the invariants below.
  5. Resume and check again.
- **The invariants (the test oracle):**
  1. Every file with a real name in the destination is byte-identical to its original (xxh3-128, `F_NOCACHE`), or was there before the job.
  2. No real-named file is incomplete.
  3. The journal loads, from slot A or B.
  4. Resume finishes the job.
  5. After the finish, no `.st-partial` and no orphaned `._.…st-partial` files are left.
  6. The originals are unchanged (hashed before and after).
  7. For moves, every file exists in at least one place: the destination, the source or the Trash (a `DirTrasher`-style test Trash).
- **The source card pulled mid-job:** the job is `WaitingForSource` and lists the right files; it resumes when the card is re-attached.
- **Two cards both named "Untitled"** with different UUIDs: the right job is offered for each, and nothing crosses over.
- **A re-attach after another volume took the old `st_dev` number:** the right card is still recognised.

### 11.4 `kill -9` tests

The CLI runs as a child process with `ST_FAULT=pause@<point>`, gets `SIGKILL` at each point, and then a fresh `safe-transfer resume` must satisfy the same invariants. An abort during the journal write must leave the previous slot loadable.

### 11.5 Edge-case fixtures (on APFS and exFAT destinations)

- **Names:** emoji, NFC and NFD forms of the same name, names that differ only by case (onto a case-insensitive disk), quotes, trailing spaces, a leading dot, 255-byte names, and characters exFAT refuses (an expected per-file failure).
- **Symlinks:** copied as links by default (the target isn't followed), dangling links, and a symlink loop in a folder copy.
- **Unreadable** files and folders (`chmod 000`): a per-file failure while the rest of the batch goes on. Also a read-only destination.
- **Special sizes:** zero-byte files, a 2.1 GB file, a sparse file (1 GiB logical, 32 KB of data).
- **Many small files:** 5,000 of them, plus a 20,000-file folder behind `ST_HUGE=1`.
- **Metadata:** files with ACLs (`deny delete`), `uchg`, Finder tags, a resource fork, and xattrs, checked against the §9 table.
- **iCloud:** dataless placeholders are skipped, never read (simulated with the flag, as ops' tests do).
- **The full disk:** a small image (`#[ignore]`) pauses the job as `DestinationFull`. It never hangs (the F-020 `COPYFILE_ERR` lesson).

### 11.6 On a real card (by hand, in M5)

A scripted run tells you "pull the card now" at random moments, ten times, on your own card and reader, with the same invariants checked afterwards. This is the only test that covers the reader's own cache and a real power cut (pulling the reader's cable rather than the card).

---

## 12. Benchmarks and the expected cost of checking

### 12.1 Measured today (disk images on the internal SSD, macOS 27.2, `spikes/safe-transfer/run.sh`)

| Measure | APFS image | exFAT image |
|---|---|---|
| `F_FULLFSYNC` accepted (file and folder) | yes | yes |
| `F_FULLFSYNC` per 50 MB file, written uncached | 9.4 ms | 4.7 ms |
| the same, written through the cache | 17.1 ms | 16.9 ms |
| 1 GiB file: one flush at the end | 10 ms | 19–22 ms |
| 1 GiB file: a flush every 64 MB chunk (16 flushes) | **+145 ms** | +87–750 ms (noisy) |
| 1 GiB file: a flush every 16 MB chunk (64 flushes) | +590 ms | +300–1,000 ms |
| Read-back after an uncached write: cached pages | 0 % | 0 % |
| Read-back after a cached write: cached pages / speed | 100 % / 19.6 GB/s | 100 % / 18.8 GB/s |
| After `msync(MS_INVALIDATE)`: cached pages | 0 % | 0 % |

**The caveat:** a disk image is a file on the internal SSD, and the host caches that file. That's why the exFAT image "wrote" at 7 GB/s. The **flush counts and the cache behaviour are real**; the **speeds aren't card or USB speeds**. The real numbers come from your card and drives in M5.

### 12.2 Estimates against the M1 real-card numbers

The card (`W2`, exFAT, USB reader) wrote at about 119 MB/s and read at about 161 MB/s. The internal SSD reads back at about 5 GB/s (SPIKES §3).

| Job | Today (M1) | With Safe Transfer (estimate) | Extra |
|---|---|---|---|
| Card → Mac: 198 ARW + HIF files, 3.7 GB | 22.9 s | ≈ 24–26 s (read-back 0.8 s, about 200 flushes ≈ 2–3 s, partly overlapped) | **+5–15 %** |
| Card → Mac: a 600 × 50 MB job, 30 GB | ≈ 3.1 min | ≈ 3.3–3.5 min | +6–12 % |
| Mac → card: 1 GiB file | 9.0 s | ≈ 16–17 s (read-back from the card ≈ 6.7 s, 16 flushes ≈ 0.3–0.8 s) | **+75–85 %** |
| Mac → card: 600 × 50 MB, 30 GB | ≈ 4.2 min | ≈ 7.5 min | ≈ +80 % |
| Mac → card: 5,000 small files, 168 MB | 73.9 s | target ≤ 96 s with group commit (20 drive flushes instead of 5,000; §4.7) | ≤ +30 % |
| Same APFS volume, 1 GiB (clone) | 0.4 ms | 0.4 ms + one folder flush | – |
| Mac → external USB SSD (≈ 1 GB/s each way) | – | the read-back roughly doubles the time for big files | +60–100 % (to be measured) |

**What this means for you:**
- **Importing from a card** costs a few seconds per job.
- **Writing to a slow card or disk** costs about as long again as the copy, because the only honest check is reading it back from that card.
- **Overlapping doesn't help on one card:** checking one file while copying the next only helps when reading and writing hit different devices. On a single card both compete.

### 12.3 What M5 measures (`fstudio-bench` rows + `bench/history.md`)

- Plain `cp`, ops today, and Safe Transfer with and without verify, for:
  - 1 GiB and 10 GiB files
  - 600 × 50 MB files
  - 5,000 small files
- Directions: Mac → card, card → Mac, Mac → USB SSD, Mac → HDD.
- The cost of a flush per file and per chunk on each real device.
- Chunk sizes of 16, 64 and 256 MB.
- 1, 2 and 4 workers on removable media.

---

## 13. Milestones (your M1–M5)

Safe Transfer is a separate track you asked for. File Studio's own M3 hasn't started and is waiting for your sign-off on M2. Safe Transfer's M1–M3 touch only the new crates, so they don't block or collide with File Studio's work. Its M4 changes ops, core, ffi and the Swift shell, so it's scheduled with the coordinator after M2 is signed off.

| ST milestone | Delivers | Done when |
|---|---|---|
| **M1: core copy** | The crates, the platform traits (macOS, plus Linux and Windows stubs that compile), single-file chunk pipeline (partial → chunks + hashes → flush → evict → verify → exclusive rename → folder flush), clones, symlinks, holes, `remove.rs` + grep test, CLI `copy`/`plan`/`compare`/`volumes` | unit tests; single-file fault tests; fmt, clippy and test clean |
| **M2: journal + resume** | Multi-file jobs, the journal (§3), resume (§6), volume UUIDs, reconnect detection, `WaitingForSource`/`WaitingForDestination`, small-file group commit, CLI `jobs`/`resume`/`cancel` | **the yank suite (§11.3) and `kill -9` suite (§11.4) pass** on APFS, exFAT and FAT images |
| **M3: moves, conflicts, metadata, space** | §7–§10, the metadata report, the exFAT `._` placement decision, the edge-case fixtures | all §11.5 fixtures pass; the metadata table is confirmed or corrected |
| **M4: into File Studio** | the ops backend switch (§0.4 B–C), core owns the Engine, ffi mirrors, resume and source-missing prompts plus the §6.1 manual resume (list row, badge, sidebar item, `transfer.resume`/`transfer.resume_all`) in Swift (display only), settings (auto-resume, verify policy) | ops' existing tests pass on both backends; app proofs of a yank and a resume; your sign-off flips the default |
| **M5: benchmarks + polish** | §12.3 on your real card and drives, the real-card yank script (§11.6), removal of ops' old data path (§0.4 D), report | numbers in `bench/history.md`, known issues listed |

---

## 14. Decisions (taken 2026-11-13)

The user approved every recommendation, and added the opt-in re-read mode.

| # | Decision | Value |
|---|---|---|
| 1 | Default chunk size | **64 MB** (16–256 MB configurable) |
| 2 | Verify policy | **Always, except instant APFS clones.** No off switch in v1 |
| 3 | Journal format | **JSON**: two-line files with a checksum trailer, A/B slots, segments of 1,024 files |
| 4 | Same-volume APFS copies | **Keep `copyfile(CLONE_FORCE)`**; reported as "cloned", not "verified" |
| 5 | The fstudio-ops flush gap | Being fixed now by the coordinator, separately: `F_FULLFSYNC` before the rename, eviction before the verify read, and the leftover sweep keyed by volume UUID |
| 6a | Crate name | **`safe-transfer` / `safe_transfer`** (an exception to F-003's prefix) |
| 6b | Moves off a card | Originals go to the **card's own Trash**, with a one-time report line |
| 6c | Small-file group commit | **Yes, first-class in M2** (§4.7): files under 8 MiB, per-file `fsync` plus one `F_FULLFSYNC` per group of ≤ 256 files, 64 MiB or 1 s; target 5,000 files ≤ 2.4 s |
| 6d | "Skip if identical" | **Offered**, not the default (M3) |
| 6e | Re-read originals | **Opt-in, off by default**, explained in first-run setup (§4.6); key `operations.reread_originals` |
| 6f | Finished journals | **Kept 30 days** |
| 6g | Two destinations from one read | **Later (v2)**, with Import Studio |
| 6h | Workers on removable media | **One copying while the previous file is checked**; four for small files; M5 measures |

## 15. M2 status (2026-11-13)

M2 is built in `crates/safe-transfer`: multi-file jobs, the journal, resume, the small-file group commit, and the yank and `kill -9` suites. The CLI gained `copy` for folders, `jobs`, `resume`, `cancel` and `report`.

**What exists:**
- **`Engine`**: `open`, `plan`, `create`, `start`, `pause`, `resume`, `cancel(KeepResumable | Close)`, `job`, `jobs`, `report`, `wait`, `volumes_changed` and `damaged`. Events are `JobState`, `Progress`, `FileDone`, `FileNotCopied`, `WaitingForSource`, `WaitingForDestination`, `ResumeAvailable` and `JobFinished`.
- **Planning** (stat only):
  - folders are expanded with symlinks copied as links (followed links get a loop guard)
  - the job's own partials and the AppleDouble `._` companions on exFAT and FAT sources are skipped
  - existing files are skipped ("already exists; nothing was replaced"), which is the only conflict choice until M3
  - names that clash within the job on a case-insensitive disk are skipped
  - iCloud placeholders are skipped
  - copying a folder into itself is refused
  - a job that won't fit is refused up front, with the numbers
- **The journal** follows §3 exactly. Paths are stored canonically (`/private/var/…`), so a card that comes back under another mount point is found by UUID and its paths are moved there.
- **Resume** follows §6:
  - a partial's chunks are re-hashed and the matching run kept
  - an original that changed is copied again and reported under `restarted`
  - a file already at its real name counts only if its content is identical (this covers a rename that landed just before the journal caught up)
  - a missing source card makes files `WaitingForSource`
  - a pulled destination makes the job `WaitingForDestination`, keeping its partials for resume
- **Group commit** follows §4.7: per-file `fsync`, one `F_FULLFSYNC` per group, then check and rename each file, then one folder flush. The journal now commits the same way: each save `fsync`s every file it writes, renames them, then does one `F_FULLFSYNC` on the folder.

**Found while building:**
1. **Rust's `File::sync_all()` is `F_FULLFSYNC` on macOS, not `fsync(2)`.** The first group-commit build used it for the "cheap" per-file flush, so every file still paid a full drive barrier: 5,000 files on the APFS image took 57–64 s. `sys::os_sync` now calls `fsync(2)` directly. `fstudio-ops` uses `sync_all` only as a fallback after `F_FULLFSYNC` fails (`sys/mod.rs:220`), which is harmless, but anyone writing a cheap flush should know this.
2. **One 4 MiB write can complete several small chunks.** Only the last chunk was logged. That is harmless with 64 MB chunks, but it stopped chunk reuse in the 1 MiB-chunk tests. All completed chunks are now logged in order, and the `kill -9` test checks reuse: 9 of 9 chunks kept after a kill mid-verify or just before the big file's rename, and some kept after a kill mid-chunk.
3. **A journal behind the disk made resume call a finished file "already exists".** Nothing was lost, but the report was wrong. Resume now compares content instead (point 3 of "Resume" above).
4. **Compressed originals were copied as zeros, and "verified" (found by CI).**
   - **Cause:** for an APFS-compressed (decmpfs) file, `SEEK_DATA` says the whole data fork is a hole, because the content lives in a resource fork. The loop skipped reading those "holes", so it hashed zeros, wrote zeros, and the read-back matched. `/usr/share/dict/web2` came out as 2.5 MB of zeros.
   - **Fix (`710b3d1`):** every byte of the original is always read. Holes only decide where the destination may stay sparse, and a 64 KiB piece is left unwritten only if it is reported as a hole and reads as zeros.
   - **The CI failure:** `ditto --hfsCompression` no longer compresses on macOS 27, so the test was vacuous locally and only bit on the macOS 26 runner. The new test copies `/usr/share/dict/web2`, which is compressed on every macOS, and compares the bytes.
   - **Not fixed here:** `fstudio-ops`' `copy_loop` has the same logic.
5. **Small-file overheads:** a 4 MiB buffer was allocated per file, progress events walked every file, and the journal flushed each file it wrote. All three are fixed.

**Tests:** all pass, and the disk-image ones ran in one batch of about 32 yanks.

| Suite | What it covers |
|---|---|
| `src` unit tests (35) | journal encode/decode against every truncation and bit flip, A/B slots, backups, leftover temp files, segment records, removal of job folders, volume matching by UUID, path remapping, chunk hashing, partial names, space |
| `tests/copy.rs` (18) | the single-file pipeline (M1) |
| `tests/jobs.rs` (10) | folder jobs and reports, one drive flush per group (fewer than 40 flushes for 600 files, against at least 600), skip-never-replace, `Exact` items, refusal when it won't fit, copy-into-itself, cancel then resume, pause and resume, reopening the engine, a damaged journal left alone |
| `safe-transfer-cli` `kill -9` | 15 stop points (every per-file point, every group point, both journal points), each followed by `resume`: the real names are complete right after the kill, the resumed job finishes identical to the originals, and no partials are left |
| `tests/faults.rs`, ignored | helper-kill yanks of APFS and exFAT images at all 9 file and group points, each followed by `ResumeAvailable`, resume and an identical tree; a card back under another mount point; a pulled source card (`WaitingForSource`, then resume); an original changed while the card was out (`restarted`); the M1 single-file suites; the negative control |

**Small-file benchmark** (`spikes/safe-transfer/bench_m2.sh`; release build; 5,000 files of 1–64 KiB, 166 MB, every file verified).

These were measured while the Mac was in use (load average 5–7, with DxO PhotoLab and other apps running), so treat them as upper bounds. Run the script on an idle Mac for the gate.

| Destination | Group commit (3 runs) | Flush per file (1,000 files) | Gain |
|---|---|---|---|
| Internal SSD | 2.06 / 2.06 / 2.46 s | 7.8 ms/file → ≈ 39 s for 5,000 | ≈ 17× |
| APFS disk image | **2.42 / 2.83 / 2.94 s** | 16.8 ms/file → ≈ 84 s | ≈ 30× |
| exFAT disk image | 22.7 / 23.1 / 23.7 s | 16.2 ms/file → ≈ 81 s | ≈ 3.5× |

- **The APFS gate (2.4 s or less)** was met only at the best run (2.42 s), and the median is 2.83 s. A profile shows the time in work that has to happen: reading every file back from the image, the group and journal flushes, and renames. The §4.7 probe's floor for the same work was 1.8 s on an idle machine. The gate needs re-measuring on an idle Mac before it is called passed or failed.
- **exFAT is 1.7–1.8× the unflushed probe (13.5 s), against a target of 1.25×.** The flushes aren't the cost: the probe showed a group barrier adds about 5 % there. The cost is that macOS's exFAT driver runs in user space (FSKit), so every metadata call is a round trip. Each file needs several: the metadata copy, the check, the check-then-rename and the metadata check. Fewer calls per file (skipping the xattr copy when there are none, skipping ACL work on exFAT, reusing descriptors) helped less than the run-to-run noise. It is still 3.5× faster than a flush per file. Real cards come in M5.

**Final M2 run** (13:27; load average about 4; `bench_m2.sh` with 3 GB images):

| Measure | Internal SSD | APFS image | exFAT image |
|---|---|---|---|
| 5,000 small files, group commit (3 runs) | 2.18 / 2.26 / 2.27 s | **2.44 / 2.81 / 3.04 s** | 23.7 / 24.6 / 24.6 s |
| Flush per file (1,000 files) | 8.5 ms/file | 32.8 ms/file | 18.3 ms/file |
| 1 GiB, `cp` + `sync` (2 runs) | 0.27 / 0.22 s | 0.56 / 0.58 s | 0.21 / 0.28 s |
| 1 GiB, Safe Transfer (2 runs) | 0.56 / 0.51 s | 0.83 / 1.88 s | 0.54 / 1.29 s |
| …of which the read-back | 0.22 s | 0.24 / 0.28 s | 0.22 / 0.20 s |
| …of which 16 per-chunk flushes | 0.05–0.07 s | 0.16–0.29 s | 0.03–0.77 s |

Read the 1 GiB rows with care:
- **`cp` + `sync` isn't durable.** `sync` doesn't empty the drive's cache, and the host caches the disk images' backing files. So `cp` measures RAM.
- **The source was warm in memory.**
- **Safe Transfer's time is the guarantee:** uncached writes, a drive flush per 64 MB chunk, and an uncached read-back of the whole file at about 5 GB/s.
- **The real ratio needs real devices** (M5, against the M1 card numbers).

**Left for M3 and later:**
- conflict choices (Replace, Keep Both, Ask, Skip if identical) and moves
- folder dates and metadata (folders get the time they were made)
- the AppleDouble placement decision on exFAT
- `volumeAvailableCapacityForImportantUsage` instead of `statfs` for free space
- real `flock` on Linux (the stub doesn't lock)
- parallel workers per volume (M2 runs one file pipeline per job)

## 16. M3 status (2026-11-13)

M3 is built: conflicts, moves, folder metadata, the AppleDouble decision, purgeable free space, Linux `flock`, and parallel workers. The runner moved to `src/runner.rs`. A run is now a series of passes: each pass copies, moves and decides what it can; then folders whose contents are done are finished, deepest first; then, if a question is open, the job waits for the answer and passes again.

**Conflicts (§8):**
- **Choices:** per class (files, folders): Ask, Skip, Skip if identical (otherwise ask), Replace, Keep Both, and Merge for folders. Defaults: ask for files, merge for folders.
- **Asking:** a taken name is a question (`Event::Conflict`, `Engine::conflicts`, `Engine::resolve_conflict(job, id, decision, apply_to_all)`). The item waits and the rest of the job carries on. When only questions are left, the job is `WaitingForAnswer`.
- **Apply to all:** answers every waiting item of the same class, and the ones still to come. It is stored in the journal's head as `rules`.
- **Replace (files):**
  1. The new copy is written, flushed and verified under its partial name.
  2. The journal records its hash.
  3. The old file goes to the Trash.
  4. The new copy is renamed in, exclusively.

  A crash leaves either the old file at the name, or the old file in the Trash with the new copy's name briefly empty (resume finishes the rename). Never anything incomplete.
- **Replace (folders):** the new folder is built under a hidden partial name. Only when everything inside it is done does the old folder go to the Trash and the new one take its name. If anything inside fails, the old folder stays, and the incomplete new one goes to the Trash. A folder is never half-replaced.
- **Keep Both:** Finder's "name 2", "name 3", for files and whole folders.
- **Planning:** taken names are listed in `JobPlan.conflicts` instead of being skipped.

**Moves (§7):**
- **Same volume (by UUID):** one exclusive rename. A whole folder moves in one rename when its name is free. Into an existing folder (merge), each item is renamed, and the emptied original folder is removed with `rmdir` (the new `remove.rs::remove_empty_source_dir`).
- **Across volumes:** copy, flush and verify, then the original goes to the Trash after a check that it hasn't changed. Emptied original folders are removed the same way. `Disposal::Keep` keeps the originals.
- **When an original is kept:** if it can't be trashed, it is kept and the report says why. A move never ends with a file missing from both places.
- **Conflicts:** moves follow the same answers (Replace trashes the old item first, then renames).
- **Report fields:** `moved`, `sources_trashed`, `sources_kept`, `replaced`, `kept_both`, `waiting_for_answer`.

**Folder metadata:** a folder this job made gets its original's dates, extended attributes, ACL and flags after all its contents are in, because adding files changes a folder's dates. What couldn't be kept goes into the report. A merged folder keeps its own.

**AppleDouble on exFAT and FAT (decided):** extended attributes are written on the partial, before the rename, and travel with it. The card ends up with `._<name>` for the final name and never an orphaned `._.<name>…st-partial`. Resume and cancel remove a partial's companion with the partial. The exFAT image test asserts both.

**Free space** on APFS now includes purgeable space (`volumeAvailableCapacityForImportantUsage`, as Finder counts it).

**Linux:** real `flock` and `getrandom`, compiled with clippy for the Linux target.

**Parallel workers:** `Options.workers`, automatic by default.
- **Small files:** 4 at a time write and `fsync`, then one drive flush for the group, then 4 at a time verify and rename.
- **Big files:** 2 at a time (one copying while the other is checked). The results are identical for 1, automatic and 8 workers (`one_worker_and_many_give_the_same_result`).

**For hosts and scripts:**
- `DirTrash`, a folder used as the Trash for tests and scripts.
- The CLI's new options: `--move`, `--keep-originals`, `--on-conflict`, `--folders`, `--workers`, `answer`, `questions`, and a hidden `--trash-dir`. In a terminal, questions are asked interactively; elsewhere the default is skip.

**Tests (all pass):**
- **M3 suite (`tests/m3.rs`, 13 tests):**
  - asked names wait while the rest is copied, then follow the answers
  - apply to all
  - policies (skip if identical, keep both twice)
  - folders: merge, keep both, skip and replace
  - a folder never half-replaced
  - same-volume moves, as one rename and merging into an existing folder
  - a cross-volume move (simulated with a second volume UUID, with a test Trash)
  - a move that keeps its originals
  - moves following conflict answers
  - folder dates and tags
  - 1, automatic and 8 workers
  - purgeable free space
- **CLI `kill -9` while replacing** (4 points: before the old item goes to the Trash, twice; after a rename; just before the big file's rename): every name holds the complete old or the complete new version, or is briefly empty with the old one in the Trash. Every old version is either still in place or in the Trash. Resume finishes, and all 32 old versions end up in the Trash.
- **Disk-image yanks (new, `#[ignore]`):**
  - a cross-volume move onto exFAT, yanked at 5 points (mid-chunk, before an original goes to the Trash, before the group flush, mid-group rename, before the folder flush): every original is whole somewhere (in place, in the Trash, or complete on the card), and resume finishes identical with every original in the Trash
  - Replace onto exFAT, yanked before the old item goes to the Trash, after a rename, and before a folder swap: the old version is never lost, a name is never half-new, and resume finishes
- **The full `--ignored` batch ran once:** 11 tests, about 40 yanks, all pass.

**Benchmark** (`bench_m2.sh`, release; load average about 3.4, with Firefox and other apps running):

| | Run 1 | Run 2 | Run 3 |
|---|---|---|---|
| **Gate: 5,000 small files onto an APFS image** (fresh image, measured first) | 2.26 s | 2.46 s | 2.45 s |
| The same, later in the same script | 1.84 s | 1.83 s | 1.84 s |
| A separate fresh-image run (4 workers) | 1.82 s | 1.83 s | 2.05 s |
| …and with 1 worker | 2.27 s | 2.34 s | 2.35 s |
| Internal SSD | 1.00 s | 1.00 s | 1.01 s |
| exFAT image | 13.4 s | 13.5 s | 13.4 s |
| 1 GiB, Safe Transfer (SSD / APFS / exFAT image) | 0.56 / 0.61 / 0.52 s | 1.21 / 1.61 / 0.72 s | |

- **The gate:** 7 of 9 APFS-image runs are under 2.4 s, and the median of all 9 is 1.84 s. The two over (2.45 and 2.46 s) were the first writes into a just-created image, while its backing file grows. The gate is unchanged. It needs an idle machine to call it cleanly passed; the numbers above are what this machine gave under load.
- **exFAT** is now at the unflushed probe's time (13.5 s). Parallel workers hide the user-space driver's per-call cost, so my own 1.25× target is met.
- **Internal SSD:** 5,000 small files take 1.0 s, down from 2.1–2.5 s in M2.

**Left for M4 and M5:**
- M4: wiring into File Studio (the backend switch, the prompts and the manual resume UI of §6.1).
- M5: real-card benchmarks and the real-card yank script. Retiring the old ops data path (§0.4 D) comes with M5.

---

## 17. M4 design (proposed 2026-11-13, awaiting approval)

This section says exactly how §0.3 and §0.4 B are built. Nothing in it is coded yet.

### 17.1 The seam: one ops operation is one Safe Transfer job

There are two ways ops could hand copies over:

| | **A. Per op (recommended)** | B. Per file |
|---|---|---|
| What ops hands over | The whole Copy, Duplicate, Move or MoveTo op, as one Safe Transfer job | Each file, from ops' `copy_file`, to `safe_transfer::copy_file` |
| Small-file group commit (5,000 files ≤ 2.4 s) | yes | no: one `F_FULLFSYNC` per file, as ops does today |
| Resume inside a big file, from its flushed chunks | yes | no: the file restarts |
| "Waiting for CLIENT_A", the resume prompt, "Insert X to resume" (§6.1) | yes, from Safe Transfer's journal | would have to be rebuilt in ops |
| Replace: new copy verified *before* the old one goes to the Trash | yes | needs a second change in ops |
| Size of the change | larger: a new ops module that maps events | small |

**A is what the approved proposal describes (§0.3)**, and it is the only one that delivers §6.1 and the group-commit speed. B would be thrown away once A exists. So: A, with no B stage.

### 17.2 fstudio-ops

- **The switch:** `OpsConfig.copy_backend: CopyBackend { Ops, SafeTransfer }`, default `Ops` (F-026). The setting is `operations.copy_backend` = `"ops"` | `"safe_transfer"`.
- **The engine:** `OpQueue::open` gains an optional `Arc<safe_transfer::Engine>`. Core owns the engine and passes it in, including when it reopens the queue after a settings change. With no engine, the switch has no effect.
- **What goes to Safe Transfer** when the switch is on:
  - Copy, Duplicate, Move and MoveTo ops. Safe Transfer itself does same-volume moves as one rename.
  - Ops checks first, exactly as today:
    - plan refusals
    - F-023 protected moves
    - undo's "the folder it goes back into is gone"
  - Rename, Trash, Delete Immediately, New Folder, New File and Compound stay on ops' own path.
- **A new module, `st_bridge.rs`,** runs the op as a job:
  - It builds the `JobSpec` from the op's roots:
    - `Item::Exact` carries Duplicate's "name copy" and pre-answered Keep Both names.
    - Ops' rules become a `ConflictPolicy`; an unanswered rule becomes `Ask`.
    - The moved originals go to ops' own `Trasher`, through an adapter that implements Safe Transfer's `Trash` trait. Tests keep their folder trash, and Put Back is unchanged.
    - `workers` comes from `file_workers`.
    - `reread_source` comes from `operations.reread_originals`.
  - It records the job id on the op row: a new ops migration adds a column `st_job`.
  - It maps events:
    - `Progress` → `OpProgress`
    - `Conflict` → `OpConflict`, with ops' `ConflictInfo` built from Safe Transfer's (`ConflictId` is the job's conflict id)
    - `resolve_conflict` → `Engine::resolve_conflict` (apply-to-all included)
    - `FileDone` / `FileNotCopied` → ops item rows: dest, `created` or `merged`, `dest_ident`, `replaced_trash`, Trash location. Undo and its "changed since" checks then work unchanged.
    - `WaitingForDestination` / `WaitingForSource` → a new `OpState::Waiting`, with `OpStatus.waiting_for: Vec<String>` (volume names)
    - `JobFinished` → `OpReport`
  - Pause maps to Safe Transfer's `pause` and `resume`.
  - **Cancel keeps today's ops meaning:** stop, keep what is finished, remove the partials. That is `cancel(Close)`.
  - A crash, a pulled card or a quit leaves the op `Interrupted` (or `Waiting`), and Safe Transfer resumes it.
- **Resume:**
  - `OpQueue::interrupted()` includes ops that have an `st_job`, with `OpStatus.resume`: Ready, NeedsVolumes([names]) or InUseElsewhere.
  - `OpQueue::resume(id)` on such an op calls `Engine::resume` and re-attaches the bridge. Ops' own `recover()` isn't used for them.
- **ops' tests run on both backends:**
  - The tests' `env_with` reads `FSTUDIO_COPY_BACKEND` (default `ops`), and CI runs the ops suite a second time with `safe_transfer`.
  - Fault tests: ops' `FaultHook` is mapped onto Safe Transfer's fault points. Safe Transfer's `fault-injection` feature is enabled only through ops' dev-dependency. The mapping:
    - BeforeCopy → before_write
    - AfterCopy → before_flush
    - Verify → a verify mismatch
    - BeforePlace → before_rename
    - BeforeSourceRemove → before_source_trash
  - Where the new backend is deliberately stricter, the test asserts the stricter result per backend; nothing is loosened. The cases:
    - moved originals go to the Trash instead of being removed
    - Replace keeps the old item until the new one is verified
    - verify is always on
    - partials are named `.st-partial`

### 17.3 fstudio-core

- **The engine:**
  - Core owns one `safe_transfer::Engine`, with its journal in `~/Library/Application Support/File Studio/transfers`. Tests use a temporary folder through `EngineConfig`.
  - `volumes_changed()` also calls the engine's `volumes_changed()`.
- **Settings**, dotted keys:
  - `operations.copy_backend` (default `ops`)
  - `operations.reread_originals` (default off)
  - `operations.auto_resume`, which is passed to the engine
- **Events:**
  - `ResumeAvailable` → `ShellRequest::ResumePrompt { op, label, volume, files_left, bytes_left }`
  - `WaitingForSource` → `ShellRequest::SourceMissing { op, files_by_source }`
- **Commands** (F-006):
  - `transfer.resume` (the selected interrupted row, or a sidebar disk) and `transfer.resume_all`
  - Both are added to the sidebar volume menu (shown only when that disk has unfinished jobs) and to the palette and menus.

### 17.4 fstudio-ffi and Swift (display only, F-001)

- **The FFI mirrors:**
  - `ResumePrompt` and `SourceMissing`
  - `OpStatus.waiting_for` and `.resume`
  - `OpState::Waiting`
- **The prompt:** Resume, Later, and Discard… (which asks first, as in §6.1).
- **`InterruptedRow`** shows "Insert CLIENT_A to resume" (disabled), "Waiting for Untitled", or "Free space on CLIENT_A". The existing ring badge counts these rows.
- **Settings:** two toggles in the Operations pane, Re-read originals and Resume transfers automatically. The backend switch goes under Advanced.
- **Setup:** one first-run page with the §4.6 re-read text.

### 17.5 Order of work

1. ops: the bridge, the switch, and ops' tests on both backends. This is the gate for everything else.
2. core: engine ownership, settings, events, commands.
3. ffi and Swift.
4. The app proofs: a yank and a resume through the app path, on a disk image. These are `#[ignore]`d and run as one batch.

The default stays `ops` until the user signs off.

## Appendix: reproduce the measurements

```bash
spikes/safe-transfer/run.sh
```

It builds `st_probe`, makes 3 GB APFS and exFAT images and a small FAT image in a temporary folder, prints everything in §3.4 and §12.1, and then detaches and deletes all of it.

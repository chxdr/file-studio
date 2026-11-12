# File Studio — Milestone 0 spike results

Throwaway measurement spikes. Code lives in `spikes/` (a standalone Cargo workspace, **not** product code). All numbers below were produced by those binaries; re-run commands are in each section. Nothing here has been tuned beyond what is stated.

## Machine and method

| Item | Value |
|---|---|
| Machine | Apple M6 Mac mini, 24 GiB RAM |
| OS / toolchain | macOS 27.2.1, rustc/cargo 1.99.1, `--release` |
| Internal volume | APFS on the internal SSD (`/System/Volumes/Data`), 233 GiB free |
| "Second volume" | **APFS sparse disk image on the same internal SSD** (`hdiutil`), mounted at `/Volumes/FSSpike` |
| External drive | **None available.** `/Volumes` held only `Macintosh HD`, `Recovery` and a read-only disk image (`Win Finder 0.3.0`) which I did not touch. |
| Stats | median / p95 (nearest rank) over N runs; N is stated per table |
| Test data | generated into the session scratch dir or the disk image, never in the repo; removed afterwards |

**Caveats that matter for every table**

* "Cross-volume" means *internal SSD → sparse image on the same SSD*. It is a different filesystem instance, so `clonefile` fails with `EXDEV` as on a real second volume, but throughput is limited by the loopback image layer and varied by ±40 % between runs (sparse-file growth, writeback). Real external-drive numbers are still missing — **please tell me whether to test on a real drive (and which one is a scratch volume).**
* **Cold cache** was produced by detaching and re-attaching the image (drops that volume's cached metadata and data). `purge` needs sudo, which this session doesn't have. Cold numbers include the image layer and so are probably somewhat pessimistic versus a bare SSD, but they are the only genuinely cold numbers I could get.
* Warm = repeated runs in one process after the first.

### Licences (all GPLv3-compatible)

Direct dependencies marked `*`; the full resolved tree for `aarch64-apple-darwin` (`cargo metadata`):

| Crate | Version | Licence | Notes |
|---|---|---|---|
| libc * | 0.2.191 | MIT OR Apache-2.0 | |
| rayon * (+ rayon-core, crossbeam-*, either) | 1.12.0 | MIT OR Apache-2.0 | |
| unicode-normalization * (+ tinyvec) | 0.1.26 | (MIT OR Apache-2.0) AND Unicode-3.0; tinyvec: Zlib OR Apache-2.0 OR MIT | |
| xxhash-rust * | 0.8.19 | **BSL-1.0** (Boost) | GPL-compatible (FSF lists Boost as compatible) |
| core-foundation *, core-foundation-sys * | 0.10.1 / 0.8.7 | MIT OR Apache-2.0 | |
| **notify** * | 8.2.2 | **CC0-1.0** | public-domain dedication; GPL-compatible. Its macOS backend uses `objc2-core-services` / `objc2-core-foundation` 0.3.2 (Zlib OR Apache-2.0 OR MIT). Other transitive: notify-types, walkdir, same-file (Unlicense/MIT), rustix (Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT), log, once_cell, bitflags, cfg-if, errno, getrandom, fastrand (all MIT/Apache-2.0). |

I checked licences with `cargo metadata`; I did not run `cargo-deny`. No GPL-incompatible licence appears.

### Unsafe inventory

Every `unsafe` block in `spikes/` has a `// SAFETY:` comment. Distinct unsafe surfaces (all raw FFI to the OS; none is memory-unsafe by design):

| Where | Call | Why unsafe / argument |
|---|---|---|
| `list.rs` | `fstatat(dirfd, name, AT_SYMLINK_NOFOLLOW)` | zeroed `stat` out-buffer; NUL-terminated name; caller's open dirfd |
| `list.rs` | `getattrlistbulk` + record parsing | kernel fills ≤ buffer; records parsed with unaligned reads bounded by each record's own length and the returned-attribute bitmap |
| `list.rs` | `fts_open/read/set/close`, `FTSENT` deref | hand-written struct layout (see Surprise S1) |
| `fsevents.rs` | `FSEventStreamCreate/SetDispatchQueue/Start/Stop/Invalidate/Release`, `dispatch_queue_create` | context pointer (boxed `Sender`) outlives the stream (freed in `Drop` after stop→invalidate→release) |
| `sys.rs` / `s3_copy.rs` | `copyfile`, `copyfile_state_*`, `clonefile`, `fcntl(F_NOCACHE / F_FULLFSYNC)`, `lseek(SEEK_DATA/SEEK_HOLE)` | valid NUL-terminated paths; state alloc/free paired; callback ctx outlives the call (scoped thread) |
| `s4_icloud.rs` | `lstat`, `listxattr`, `CFURLCopyResourcePropertyForKey`, CF create/release | two-call `listxattr` protocol; CF objects released in matched pairs |
| `s1_listing.rs` | `CFStringCreateWithBytes`, `CFStringCompareWithOptionsAndLocale` | comparator reads strings that outlive the sort |
| `s5_case.rs` | `pathconf(_PC_CASE_SENSITIVE)` | read-only query |

---

## Spike 1 — Listing speed

`spikes/src/list.rs`, `spikes/src/bin/s1_listing.rs`. Run: `s1_listing gen <dir> <n>`, `s1_listing run <dir> [runs] [method…]`.

**Data**: synthetic folders of 1k / 10k / 100k entries (2 % sub-folders), mixed-case ASCII, digits, NFC and NFD accented names, sparse files of varied size. All five strategies return identical counts and identical sum of regular-file sizes (asserted). Each entry carries name, type, size, mtime (ns) and `st_flags`.

**Strategies**

1. `std::fs::read_dir` + `std::fs::symlink_metadata(path)` (the literal request) — and a variant using `DirEntry::metadata()` (fstatat under the hood).
2. `getattrlistbulk(2)`, attributes: returned-attrs, error, name, objtype, modtime, flags, file data length. Two buffer policies: fixed 256 KiB, and **adaptive** (first call 16 KiB, then 256 KiB).
3. `fts(3)` (`FTS_PHYSICAL|FTS_NOCHDIR`, no recursion).
4. `readdir` (names only, `d_type` free) + parallel `fstatat` via rayon (first 100 entries stat'd serially so first rows aren't blocked behind the full name scan).

"first100" = time until 100 entries *with metadata* are available. "all" = all entries with metadata. Sort columns use the 100k-entry result of the same run.

### Warm cache (internal SSD, 11 runs each; median / p95 ms)

| N | method | first 100 | all + metadata |
|---|---|---|---|
| 1k | std read_dir + symlink_metadata | 0.15 / 1.16 | 1.07 / 4.45 |
| 1k | std read_dir + DirEntry::metadata | 0.11 / 0.14 | 0.81 / 0.89 |
| 1k | getattrlistbulk 256K | 0.40 / 0.75 | 0.41 / 0.76 |
| 1k | **getattrlistbulk adaptive** | **0.09 / 0.14** | **0.39 / 0.48** |
| 1k | fts | 0.46 / 0.59 | 0.49 / 0.62 |
| 1k | readdir + rayon fstatat | 0.15 / 0.20 | 1.21 / 1.36 |
| 10k | std + symlink_metadata | 0.24 / 0.96 | 14.92 / 34.24 |
| 10k | std + DirEntry::metadata | 0.17 / 0.22 | 9.70 / 11.95 |
| 10k | getattrlistbulk 256K | 1.68 / 2.09 | 5.16 / 6.59 |
| 10k | **getattrlistbulk adaptive** | **0.15 / 0.16** | **5.04 / 5.31** |
| 10k | fts | 8.98 / 9.24 | 9.22 / 9.49 |
| 10k | readdir + rayon fstatat | 0.23 / 0.35 | 10.94 / 11.39 |
| 100k | std + symlink_metadata | 0.28 / 1.35 | 161.04 / 332.70 |
| 100k | std + DirEntry::metadata | 0.31 / 0.38 | 160.53 / 177.20 |
| 100k | getattrlistbulk 256K | 2.80 / 5.30 | 79.77 / 98.29 |
| 100k | **getattrlistbulk adaptive** | **0.27 / 0.34** | **79.68 / 84.07** |
| 100k | fts | 89.73 / 155.55 | 92.47 / 163.90 |
| 100k | readdir + rayon fstatat | 0.32 / 0.40 | 109.55 / 121.10 |

### Cold cache (image volume re-attached before every run; 4 runs each; median ms, worst run in brackets)

| N | method | first 100 | all + metadata |
|---|---|---|---|
| 10k | std + symlink_metadata | 8.2 (8.8) | 104 (111) |
| 10k | std + DirEntry::metadata | 8.5 (9.3) | 104 (114) |
| 10k | getattrlistbulk 256K | 58.8 (60.2) | 79 (80) |
| 10k | **getattrlistbulk adaptive** | **12.5 (15.9)** | **78 (86)** |
| 10k | fts | 85.7 (92.2) | 86 (92) |
| 10k | readdir + rayon fstatat | 9.0 (9.5) | 87 (116) |
| 100k | std + symlink_metadata | 16.4 (17.2) | 1071 (1141) |
| 100k | std + DirEntry::metadata | 16.6 (17.9) | 1059 (1064) |
| 100k | getattrlistbulk 256K | 182.8 (186.0) | 827 (857) |
| 100k | **getattrlistbulk adaptive** | **22.8 (24.5)** | **851 (894)** |
| 100k | fts | 864 (921) | 868 (925) |
| 100k | readdir + rayon fstatat | 17.1 (22.1) | 847 (1092) |

Cold cost is ≈ 8 µs/entry regardless of method (metadata I/O dominates); bulk/rayon only trim the CPU part.

### Sorting (100k entries, warm, median ms; identical for every listing method)

| Sort | 10k | 100k |
|---|---|---|
| Name: natural + case-insensitive + NFC-normalised, precomputed keys, single thread | 2.1 | 25 |
| Same, rayon `par_sort` | 1.3 | 12 |
| Size | 0.12 | 1.5 |
| Date (ns) | 0.13 | 1.7 |
| Finder-style: `CFStringCompareWithOptionsAndLocale` (caseless, non-literal, localized, numeric), includes CFString creation | 11.4 | **147** |

The name comparator I wrote compares folded codepoints (NFC + lowercase, ASCII fast-path), digit runs numerically. It is **not** locale collation (no diacritic-aware ordering, e.g. "ü" sorts after "z"). The CoreFoundation row shows what Finder-parity ordering costs: ~6× slower at 100k; for 10k it is still comfortably inside budget.

### Budget check

| Budget | Result (best method = getattrlistbulk adaptive) | Verdict |
|---|---|---|
| 10k: first rows < 50 ms | 0.15 ms warm, 12.5 ms cold | ✅ |
| 10k: fully sorted < 200 ms | 5 + 2.1 ≈ **7 ms** warm; 78 + 2 ≈ 80 ms cold | ✅ |
| 100k < 1 s | warm 80 + 25 = **≈105 ms** (≈92 ms with parallel sort); cold 851 + 25 ≈ **880 ms** | ✅ but cold is within ~12 % of the limit, measured on an image volume |
| Cold 100k with `std`/`fts` | 1.07–1.1 s / 0.87 s + fts blocks 864 ms before the first row | ❌ (std), ❌ first-row (fts) |

### Recommendation

* **Use `getattrlistbulk` with an adaptive buffer (16 KiB first call → 256 KiB after) as the primary lister on macOS.** Streaming the first call gives rows in 0.1–0.3 ms warm (12–23 ms cold) and the fixed-256 KiB variant must be avoided (58–183 ms to first row when cold because the first call fetches ~3k entries). It is also ~2× faster than `std` for the full listing and returns `st_flags` (including `SF_DATALESS`, spike 4) for free.
* Keep a portable `read_dir` + `DirEntry::metadata()` implementation for Linux/Windows shells (not `symlink_metadata(path)`: `DirEntry::metadata` was up to 35 % faster at 1k–10k entries and equal at 100k).
* **Do not use `fts`**: it reads the whole directory before returning the first entry (90 ms at 100k warm, 864 ms cold) and offers nothing bulk doesn't. `readdir` + rayon is the fallback if bulk is unavailable (e.g. some network filesystems return `ENOTSUP`; that needs a runtime fallback — untested here).
* Sort off the UI thread; key-precompute + parallel sort; treat localized collation as a deliberate choice (147 ms at 100k).
* Not measured: network volumes, exFAT/FAT external drives (bulk attribute support varies), HDDs.

### Surprises

* **S1 — FTSENT layout.** The macOS 27 SDK ships *two* `FTSENT` layouts: `fts_compat.h` (plain `fts_*` symbols, `fts_name` inline array, `short fts_level`) and `fts_new.h` (`$NFTS`-suffixed symbols, `fts_name` pointer, `long long fts_number`). Which is used depends on the deployment target. My binding follows the compat layout (checked by asserting `fts_namelen`). Another reason not to use fts.
* `fts` returns each skipped directory twice (pre- and post-order) unless `FTS_DP` entries are filtered.

---

## Spike 2 — FSEvents

`spikes/src/fsevents.rs` (raw FFI, dispatch-queue delivery), `spikes/src/bin/s2_fsevents.rs`. Run: `s2_fsevents <scratch> [iters]` (`KINDS=delete` limits ops).

**Method**: a stream on a temp directory (internal SSD, real path); the thread timestamps the instant before the write syscall and the callback timestamps on entry (before any allocation); latency = difference until an event for the probe path arrives. 20 iterations per cell, 150–1150 ms idle gap between iterations (so events don't coalesce with the previous one). "dir-level" = no `FileEvents` flag (paths are directories). `NoDefer` = `kFSEventStreamCreateFlagNoDefer`. Creates write 4 KiB; modify appends 1 byte; rename and delete operate on pre-created files.

### Write → callback latency (median / p95 ms; 0 missed events in every cell)

| Config | create | modify | rename | delete |
|---|---|---|---|---|
| dir-level, latency 0.0 | 11.5 / 11.7 | 11.5 / 11.7 | 11.5 / 11.8 | 11.4 / 11.7 |
| dir-level, latency 0.05 | 52.6 / 62.7 | 42.5 / 62.7 | 62.2 / 63.0 | 62.1 / 62.8 |
| dir-level, latency 0.5 | 512 / 513 | 437 / 512 | 45.8 / 212 | 45.9 / 211 |
| FileEvents, latency 0.0 | 11.2 / 11.7 | 11.5 / 12.5 | 11.4 / 11.9 | 11.5 / 11.8 |
| FileEvents, latency 0.01 | 21.8 / 22.7 | 22.0 / 22.6 | 12.8 / 22.9 | 16.6 / 22.3 |
| FileEvents, latency 0.05 | 62.0 / 62.7 | 61.8 / 62.8 | 62.0 / 62.8 | 61.9 / 62.8 |
| FileEvents, latency 0.1 | 112 / 113 | 112 / 113 | 112 / 113 | 112 / 114 |
| FileEvents, latency 0.5 | 479 / 513 | 511 / 513 | 54.5 / 211 | 204 / 213 |
| **FileEvents \| NoDefer, latency 0.05** | **11.5 / 11.7** | **11.3 / 11.7** | **11.7 / 12.0** | **11.9 / 12.7** |
| **FileEvents \| NoDefer, latency 0.5** | **11.6 / 11.7** | **11.1 / 11.7** | **11.4 / 12.0** | **11.7 / 12.7** |
| notify 8.2.2 `recommended_watcher` (FSEvents backend) | 11.2 / 11.7 | 11.5 / 11.8 | – | – |

(Some rename/delete cells with large latency are low because the fixture's own setup write — done *shortly before* the probe — had already armed the latency timer; they are an artefact, not a benefit. Treat the large-latency rows as "up to the latency value".)

### Findings

* **Floor ≈ 11 ms** (min 7–11 ms) for any single event — fseventsd's own batching — regardless of file vs directory granularity or operation type.
* **Without `NoDefer`, latency adds the full latency parameter** (11 → 22 → 62 → 112 → ~500 ms for 0 / 0.01 / 0.05 / 0.1 / 0.5 s). With `NoDefer`, an isolated event is delivered at the floor and the latency only governs coalescing of *subsequent* events (per Apple's documentation; I did not separately measure a burst).
* **File-level vs directory-level cost the same latency.** `kFSEventStreamCreateFlagFileEvents` just changes the path granularity: you get the changed file's path (and create/modify/rename/remove flags) instead of "something in this directory". It increases event volume for busy folders.
* **notify 8.2.2** measured the same latency as raw FSEvents (11.2 / 11.5 ms median) — no measurable overhead on macOS. Licence **CC0-1.0**, GPL-compatible. I did not examine what knobs (latency, NoDefer) its FSEvents backend exposes; it showed default behaviour equal to `FileEvents|NoDefer`-like timing here.

### Re-list vs incremental on a 10k folder (single file created; 20 runs; median / p95 ms)

| Step | median | p95 |
|---|---|---|
| write → event (`FileEvents\|NoDefer`, latency 0) | 11.06 | 11.63 |
| incremental apply: `fstatat` + binary-search insert into the sorted vector | **0.064** | 0.095 |
| full re-list (`getattrlistbulk`) + key build + re-sort, 10 020 entries | **28.0** | 30.4 |
| **end-to-end incremental** (event + apply) | **11.1** | 11.7 |
| end-to-end full re-list (event + re-list) | 39.0 | 42.0 |

(The re-list took 28 ms here vs ≈ 7 ms measured in spike 1 on a long-settled folder; the folder had just been created, so some of the metadata was not hot in cache. Even so it is inside the budget at 10k. At 100k a full re-list is ≈ 105 ms + 11 ms event = 116 ms warm — over budget — so incremental is required for large folders.)

### Recommendation (meets < 100 ms)

1. Watch **only the folders currently visible** (one stream per window/tab folder, or one stream over the set of visible paths), with `kFSEventStreamCreateFlagFileEvents | kFSEventStreamCreateFlagNoDefer` (consider `WatchRoot` for rename/delete of the watched folder itself, and `IgnoreSelf` if our own operations should not echo — neither measured).
2. Latency parameter ≈ 30–50 ms: isolated edits arrive at the ~11 ms floor, bursts (copy of 5 000 files) are coalesced.
3. **Apply events incrementally**: per path `fstatat`, then insert/replace/remove in the sorted model (≈ 0.06 ms per event at 10k). Do an own 16 ms debounce only for UI invalidation, not for processing.
4. **Fall back to a full re-list** when the batch carries `MustScanSubDirs` / `UserDropped` / `KernelDropped` / `RootChanged`, or when events in a batch exceed ~5 % of entries (relist is cheaper than thousands of inserts); then re-sync.
5. Dedupe/ordering: events can arrive for paths already reflected by a re-list; applying `lstat` results (not event flags) keeps it idempotent — the incremental path above does exactly that.
6. **Crate choice**: for the macOS core either is fine on latency. I recommend a small `Watcher` trait with `notify` as the cross-platform backend (inotify/ReadDirectoryChangesW later) and a direct FSEvents implementation only if `notify` can't expose `NoDefer`/event flags/`MustScanSubDirs` (needs a check); the raw FFI in `fsevents.rs` is ~100 lines and shows the direct route is cheap.

---

## Spike 3 — Copy

`spikes/src/sys.rs` (copyfile/clonefile FFI), `spikes/src/bin/s3_copy.rs`, drivers in `spikes/scripts/` (`copy_bench.sh`, `preserve_check.sh`). Data: 1 GiB and 10 GiB incompressible files; 5 000 files of 1–64 KiB in 50 sub-folders (168 MB). "Same volume" = internal SSD → internal SSD; "cross volume" = internal → sparse image on the same SSD (see caveats). All times include file creation, exclude any final fsync unless stated. Method names: `copyfile_all` = `copyfile(…, COPYFILE_ALL|COPYFILE_EXCL)` (no clone); `copyfile_all_clone` = `COPYFILE_ALL|COPYFILE_CLONE`; `rw:<KiB>:<cache>` = manual `read`/`write` loop (cache: `n` default cache, `s` = F_NOCACHE on source, `d` = on destination, `b` = both). `cp`, `cp-c`, `ditto` = the system tools (process start included).

### Large files, same volume (median ms / MB/s; 5 runs for 1 GiB, 3 runs for 10 GiB)

| Method | 1 GiB | 10 GiB |
|---|---|---|
| `clonefile(2)` | **0 ms** (< 1 ms) | – |
| `copyfile_all_clone` | 0 ms | 0 ms |
| `cp -c` / `ditto` (both clone on APFS) | 1 / 2 ms | – / 2 ms |
| `copyfile_all` (no clone) | 1211 ms / 887 MB/s | 13 659 ms / 786 MB/s |
| `copyfile_data` (data only) | 1204 ms / 891 | – |
| rw 64 KiB | 233 ms / 4 606 | – |
| rw 256 KiB / 1 MiB / 4 MiB / 8 MiB | 217 / 206 / 212 / 212 ms (≈ 5.0–5.2 GB/s) | rw 1 MiB: 10 189 ms / 1 054 MB/s |
| rw 1 MiB, F_NOCACHE dst / both | 197 / 216 ms | rw 4 MiB dst-nocache: 11 340 ms / 947; **4 MiB both-nocache: 7 684 ms / 1 397** |
| rw 1 MiB + `F_FULLFSYNC` | 1 182 ms / 908 MB/s | – |
| `cp` | 252 ms / 4 262 | 10 711 ms / 1 002 |

**Important reading of this table.** The ≈ 5 GB/s rows are the *page cache absorbing the write* (the source was warm in RAM and nothing is flushed when the call returns). The sustained device rate is ≈ 0.9–1.4 GB/s: it shows up in the 10 GiB column and in the `F_FULLFSYNC` row (908 MB/s). `copyfile(3)`'s data path behaves like a write-through copy (≈ 0.9 GB/s, ≈ the fsynced loop), so it looks 4–5× "slower" than `cp` on a warm 1 GiB file but is within ~25 % at 10 GiB and is the only variant that reports durable-ish progress. **On the 5 % budget: raw `copyfile` is *not* within 5 % of `cp` for a warm, cached, same-volume non-clone copy; it is for cloneable copies (both ~instant) and — cross-volume — it is faster (below).** Using F_NOCACHE on both ends gave +30 % on a 10 GiB copy (avoids evicting the whole cache) and was neutral/slightly negative at 1 GiB. Buffer size from 256 KiB up made no difference.

### Large files, cross volume (median ms / MB/s)

| Method | 1 GiB (5 runs) | 10 GiB (3 runs) |
|---|---|---|
| `clonefile` | EXDEV (fails, as expected) | – |
| `copyfile_all` | 1 544 / 695 (later rounds 1 742–1 848) | 15 833 / **678** |
| `copyfile_all_clone` (falls back to copy) | 1 582 / 679 | 17 959 / 598 |
| `copyfile_data` | 966–1 713 / 627–1 112 | – |
| rw 1 MiB (default cache) | 1 681 / 639 | 21 171 / 507 |
| rw 4 MiB F_NOCACHE dst / both | 1 550 / 693 · 1 602 / 670 | 21 423 / 501 · 19 949 / 538 |
| `cp` / `ditto` | 1 619 / 1 626 | 20 164 / 21 658 (≈ 532 / 496 MB/s) |
| **Finder** (AppleScript `duplicate`, 3 runs, includes osascript start) | **1 136, 1 012, 1 026 ms** | – |

Cross-volume numbers drifted by up to 40 % between rounds (image growth/writeback). Within that noise `copyfile`, `cp`, `ditto` and a manual loop are indistinguishable at 1 GiB, `copyfile` was ~20–30 % ahead at 10 GiB, and Finder was in the same 1.0–1.1 s band as the fastest `copyfile` runs. **This cannot settle the "within 5 % of Finder/cp" budget for cross-volume; it needs a real external drive.**

### 5 000 small files (median ms / p95; 5 runs)

| Method | same volume | cross volume |
|---|---|---|
| per-file `copyfile(ALL)`, serial | 998 / 1037 | 4 268 / 4 413 |
| `copyfile(ALL\|RECURSIVE)` on the folder | 1 014 / 1 121 | 1 610 / 4 338 |
| per-file `copyfile`, rayon 2 / 4 / 8 threads | 649 / **403** / 391 | 1 391 / **1 174** / 1 467 |
| per-file `copyfile(ALL\|CLONE)` | 499 / 551 | 1 170 / 1 288 (no clone possible) |
| `clonefile` of the **directory** | **28 / 38** | EXDEV |
| `cp -R` / `cp -Rc` / `ditto` | 964 / 506 / 622 | 1 250 / 1 279 / 1 242 |

Parallelism of 4 gives 2.5× on small files; cross-volume serial runs had large p95 stalls.

### Verify pass (xxh3-64, F_NOCACHE read)

| Scenario | Read only | Read + xxh3 | Hash overhead |
|---|---|---|---|
| 1 GiB, warm (page cache), 1–4 MiB buffers | ≈ 17.5 GB/s | ≈ 11.5 GB/s | CPU-bound: xxh3 ≈ 11.5 GB/s single-thread |
| 10 GiB, partly cached | 7.3–7.6 GB/s | 6.3–7.2 GB/s | – |
| **10 GiB, truly cold (volume re-attached), F_NOCACHE, 1 MiB** | **5.8 / 5.5 GB/s** | **4.9 / 4.8 GB/s** | **≈ 12–15 %** |
| 10 GiB cold, no F_NOCACHE (cache-populating) | 4.3 GB/s | 4.0 GB/s | – |

A verify pass over a freshly written 10 GiB file therefore costs ≈ 2.1 s cold on this SSD (vs ≈ 15 s for the copy) — about 14 % extra time on a cross-volume copy. F_NOCACHE did not slow cold reads and avoids evicting the user's cache; note F_NOCACHE only avoids *populating* the cache, it does not evict pages that are already cached, so a verify of data we just wrote through the cache is not a device read unless the destination was also written with F_NOCACHE (use `COPYFILE_NOCACHE` / F_NOCACHE on the destination fd, as in Import Studio's `sys.rs`).

### What is preserved (1 file with: custom xattr, ACL `everyone deny delete`, Finder tags (`_kMDItemUserTags`), Finder comment, 5 000-byte resource fork; and a 1 GiB sparse file with 2 × 4 KiB of data = 32 KiB on disk)

`yes` = identical to source after copy. Both volumes behave identically except where noted. `mdls` showed `(null)` for both source and copies (the scratch dir isn't Spotlight-indexed) so Spotlight attributes were **not** verifiable; xattr contents were compared directly (`xattr -p/-l`).

| Method | xattrs/tags/comment/rsrc | ACL | mtime+mode | birth time | Sparse (1 GiB file, KiB on disk) |
|---|---|---|---|---|---|
| `copyfile ALL` (no clone) | yes | yes | yes | yes | **no: 1 048 576 KiB (hole filled in)** |
| `copyfile ALL\|CLONE` same volume | yes | yes | yes | yes | yes (32 KiB, clone) |
| `copyfile ALL\|CLONE` cross volume | yes | yes | yes | yes | **no: 1 048 576 KiB** |
| `copyfile ALL\|DATA_SPARSE` | yes | yes | yes | yes | **yes: 32 KiB** |
| `clonefile` alone (same vol) | yes | **NO** | yes | yes | yes |
| manual rw loop, nothing else | **NO (all lost)** | NO | NO | NO | no (fills holes) |
| manual rw + `SEEK_DATA/HOLE` + `copyfile(COPYFILE_METADATA)` afterwards | yes | yes | yes | yes | **yes: 32 KiB** |
| `cp` | yes (xattrs/tags/rsrc) | **NO** | **NO** | NO | yes |
| `cp -p` | yes | yes | yes | yes | yes |
| `cp -c` | yes | **NO** | yes (same vol) / NO (other) | same | yes |
| `ditto` (same vol) | yes | yes | yes | yes | yes (32 KiB) |
| `ditto` (other vol) | **FAILED**: `Permission denied` | – | – | – | – |

(The birth-time check is weak: all files were created within seconds, and `stat %B` has 1-second resolution. Values that matched for methods that also matched mtime are probably right; the `NO`s are real.)

### Progress callback, pause and cancel (`COPYFILE_STATE_STATUS_CB`)

Implemented in `cbdemo` (`s3_copy cbdemo <src> <dst>`), 1 GiB, both volumes:

* The callback is invoked on the copying thread with `what=COPYFILE_COPY_DATA, stage=COPYFILE_PROGRESS`; **bytes copied** come from `copyfile_state_get(state, COPYFILE_STATE_COPIED, &off_t)`. It fired **1 024 times for 1 GiB — once per MiB** (max gap between callbacks 1 048 576 bytes).
* **Pause** = block inside the callback (we slept in a 2 ms loop on an `AtomicBool`). Measured: copied bytes were 217 055 232 before and after a 500 ms pause (delta 0), then the copy resumed and finished with the full 1 073 741 824 bytes and rc 0.
* **Cancel** = return `COPYFILE_QUIT (2)` from the callback. `copyfile` returned −1 with `errno = ECANCELED (89)` **1.3–1.7 ms** after the request (≈ 540 MB already copied) and **removed the partial destination** (it did not exist afterwards).
* Callback is not delivered for `COPYFILE_CLONE` (instant), so UI should treat clone as indivisible.

### Recommendation

1. **Same-volume APFS: `copyfile(ALL|CLONE)`** (instant, preserves ACL because it applies metadata after the clone; raw `clonefile` drops ACLs). Skip the verify pass for clones (nothing was read/written). Fall back automatically when the volume can't clone.
2. **Cross-volume / non-APFS: `copyfile(COPYFILE_ALL | COPYFILE_DATA_SPARSE)` with the status callback**, *or* an own loop. The `copyfile` route preserves everything we tested and was the fastest cross-volume at 10 GiB; the callback gives 1 MiB-granular progress, pause and cancel with automatic partial-file cleanup. **Always add `COPYFILE_DATA_SPARSE`** — plain `COPYFILE_ALL` (and `ALL|CLONE` across volumes) inflates a 32 KiB sparse file to 1 GiB. Keep the own loop (1–4 MiB buffer, F_NOCACHE on both ends for files ≥ ~1 GiB, `SEEK_DATA/SEEK_HOLE`, metadata applied afterwards with `copyfile(COPYFILE_METADATA)`) as the path for when we need in-flight hashing or per-chunk journaling; its throughput is equal.
3. **Small files: parallelise (4 workers)**; clone whole directories on same-volume APFS (`clonefile` on the folder: 28 ms for 5 000 files vs ~1 s) but note that this skips per-file conflict handling, so use it only when the destination doesn't exist.
4. **Verify**: separate pass with F_NOCACHE, xxh3 — ≈ 12–15 % extra over a cold device read; verify against a re-read of the *destination* (written with F_NOCACHE), then remove the source for moves (matches the data-safety rule).
5. **Atomic placement and ACLs (surprise S2).** Write to a temporary name in the destination folder, `renamex_np(RENAME_EXCL)` into place (as Import Studio does), and apply ACLs/immutable flags **after** the rename. A file carrying `deny delete` cannot be renamed (`mv a b` → `Permission denied`, tested) or removed, and `ditto` cross-volume of exactly such a file fails with `Permission denied`. Also: cleanup code must clear ACLs (`chmod -N`) before it can remove a temp file.

### Surprises

* **S2** (above): `deny delete` ACLs block rename/remove of the temp file; `ditto` fails across volumes on such a file.
* **S3:** `copyfile(COPYFILE_ALL)` silently de-sparses; you must opt in to `COPYFILE_DATA_SPARSE`.
* **S4:** the headline 5 GB/s "copy" speeds are cache artefacts; compare copy methods on durable time (fsync) or ≥ 10 GiB inputs.
* `cp` (no `-p`) drops ACLs, mtime and birth time; `cp -c` does not copy ACLs; plain `clonefile` doesn't either.

---

## Spike 4 — iCloud Drive / File Provider placeholders

`spikes/src/bin/s4_icloud.rs`. **Strictly read-only.** `~/Library/Mobile Documents` on this Mac contains **64 evicted (`SF_DATALESS`) entries** (e.g. under `com~apple~CloudDocs/Downloads`, `com~apple~mail`), so I tested against two files there: one dataless, one materialised, in the same folder. `~/Library/CloudStorage` is empty (no third-party File Provider domains), so those are untested.

I **never called `open(2)`/`read`** on any placeholder. After every probe the program re-`lstat`s the file and reports whether `SF_DATALESS` cleared.

| Probe | Cost (dataless file) | Triggered a download? |
|---|---|---|
| `lstat` / `std::fs::symlink_metadata` / `metadata` | 9–13 µs | **No** (flag still set) |
| `getattrlistbulk` on the parent, with `ATTR_CMN_FLAGS` | 0.17 ms for 27 entries | **No** |
| `listxattr` | 24 µs → `com.apple.FinderInfo`, `com.apple.lastuseddate#PS` | **No** |
| `CFURLCopyResourcePropertyForKey` `NSURLIsUbiquitousItemKey` | 0.36 ms | No |
| … `NSURLUbiquitousItemDownloadingStatusKey` | **13.8 ms first call, then 1.6 ms** | No (`…StatusNotDownloaded` / `…StatusCurrent`) |
| … `NSURLUbiquitousItemIsDownloadingKey` / `IsUploadedKey` | 1.2–2.1 ms | No |
| … `NSURLFileSizeKey` / `NSURLFileAllocatedSizeKey` | 15–25 µs | No (size = logical, allocated = 0) |

What a dataless file looks like to POSIX: `st_flags = 0x40000060` (= `SF_DATALESS 0x40000000 | UF_COMPRESSED 0x20 | 0x40`), `st_size` = full logical size (3 636 540), **`st_blocks = 0`**; a materialised file reports `st_flags = 0x40`, normal blocks. `ls -lO` shows `compressed,dataless`.

### Cheap detection from Rust

* **`st_flags & SF_DATALESS (0x4000_0000)`** from `lstat`/`fstatat`, or **`ATTR_CMN_FLAGS` in the same `getattrlistbulk` call that lists the folder** — zero extra syscalls. That is what `list.rs` already returns in `Entry.flags`. `std::fs::Metadata` does not expose `st_flags`, so the portable-looking `std` path can't see it; use `libc::lstat`/`fstatat` or bulk.
* Use `NSURLUbiquitousItemDownloadingStatusKey` etc. only for the *visible, few* items where you need "downloading / waiting / uploading" states: at 1.6 ms (first call 14 ms) it is 100–1 000× the cost of a stat and goes through IPC to the file-provider daemon; never call it for a whole 10k folder.
* Show a "cloud / not downloaded" badge from the flag; display `st_size` as the logical size but **do not** use `st_blocks`/allocated size for "size on disk" of placeholders (it is 0).

### Not tested — would modify local state of the user's iCloud data

These are documented/known triggers but I did **not** run them: `open`/`read`/`mmap` of the content (including hashing, copying, `copyfile`, `clonefile`), Quick Look previews and `QLThumbnailGenerator`, `NSFileCoordinator` reads, and `readdir` of a *dataless directory* (directories can be dataless too: the flag is set on some `Documents` folders here). Treat all of them as "may download" and gate them behind the flag. **Proposed method if you want these measured** (needs your OK because it creates and evicts a file in your iCloud Drive): create `~/Library/Mobile Documents/com~apple~CloudDocs/FileStudioSpikeScratch/probe.bin` (a few MB), `brctl evict` it, then run each probe and re-check `SF_DATALESS`/`brctl status`, and finally delete the scratch folder.

Design consequences: the lister must carry the flag; preview/thumbnail/hash/copy/size-folder pipelines must refuse (or ask) for dataless items; "Download now" = `NSFileManager startDownloadingUbiquitousItemAtURL` (not tested) rather than reading the file.

---

## Spike 5 — Case sensitivity and Unicode normalisation (APFS)

`spikes/src/bin/s5_case.rs`, on the internal default (case-insensitive) APFS volume and a temporary **case-sensitive APFS** image (`hdiutil create -fs "Case-sensitive APFS"`).

| Test | Default APFS (case-insensitive) | Case-sensitive APFS |
|---|---|---|
| `pathconf(_PC_CASE_SENSITIVE)` | **0** | **1** |
| create `File`, then `file` | 2nd fails `EEXIST` | both exist |
| stat `FILE` | found | not found |
| create NFC `café`, then open the NFD spelling | found | **found** |
| create NFD `café` when NFC exists (`create_new`) | `EEXIST` | `EEXIST` |
| Names returned by `getattrlistbulk` / `read_dir` | exactly as stored (NFC stays NFC, NFD stays NFD) | same |
| Look up NFC `über` when NFD `u¨ber` is stored | found | found |
| Look up upper-case NFD `CAFÉ` | found | **not found** |

Conclusions:

* **APFS is normalisation-insensitive on lookup but normalisation-preserving on storage**, on both volume kinds (unlike HFS+, which forced NFD). So a folder can show names in a mix of NFC and NFD, and two *visually identical* names can't coexist in one folder. The file manager must (a) **normalise (NFC) and fold for sorting, searching and conflict prediction** (spike 1's sort does), (b) display the stored name unchanged, and (c) never rely on byte-equality to decide two names differ — let the OS decide with an atomic exclusive create/`renamex_np(RENAME_EXCL)` and map `EEXIST` to the conflict dialog.
* **Case sensitivity is per volume**: query `pathconf(path, _PC_CASE_SENSITIVE)` (or `VOL_CAP_FMT_CASE_SENSITIVE` via `getattrlist`) for the *destination* volume. Copying a folder with `File` and `file` from a case-sensitive volume to a case-insensitive one produces collisions → pre-check with the destination's mode. Case-only renames (`File` → `file`) on a case-insensitive volume were not tested here.
* Case-sensitive volumes still treat NFC/NFD as the same name.

---

## Summary table

| Spike | Headline | Recommendation |
|---|---|---|
| 1 Listing | 10k: 0.15 ms to first rows, ≈ 7 ms warm sorted (≈ 80 ms cold). 100k: ≈ 105 ms warm, ≈ 880 ms cold (image volume) | `getattrlistbulk`, adaptive 16 K→256 K buffer; `DirEntry::metadata` for portable shells; no `fts`; precomputed-key parallel sort |
| 2 FSEvents | 11 ms latency floor; latency param adds to it unless `NoDefer`; notify = raw; incremental apply 0.06 ms vs 28 ms relist at 10k | `FileEvents\|NoDefer`, latency 30–50 ms, incremental model update, full relist on drop/overflow; `notify` (CC0) behind a trait |
| 3 Copy | clone = 0 ms; sustained copy 0.8–1.4 GB/s same volume, 0.5–0.7 GB/s cross (image); verify +12–15 % cold | `copyfile(ALL\|CLONE)` same volume; `copyfile(ALL\|DATA_SPARSE)` + callback cross volume; parallel ×4 for small files; ACLs after rename; xxh3 verify with F_NOCACHE |
| 4 iCloud | stat flags/bulk FLAGS detect dataless at no cost and never download; CF keys 1.6–14 ms | flag from listing; no previews/hash/copy/size of dataless; open item: read/QL tests need approval |
| 5 Case/NFD | per-volume `pathconf`; APFS stores as given, matches normalisation-insensitively | NFC+fold for sort/conflict prediction, OS for the authoritative collision |

## Open questions for the coordinator

1. **External drive**: none mounted. Which real drive/volume (and is it scratch) may I test cross-volume copy and listing on? All cross-volume numbers above are internal-SSD → disk image.
2. **iCloud**: may I create and evict a scratch file in iCloud Drive to test open/read, Quick Look and directory-listing triggers?
3. Is Finder-parity *localized* name collation required (147 ms at 100k) or is folded-codepoint natural order acceptable by default?

## Reproduce

```
cd spikes && cargo build --release
s1_listing gen /tmp/x 100000 ; s1_listing run /tmp/x 11
scripts/mkimage.sh <work>            # APFS sparse image -> /Volumes/FSSpike
scripts/cold_listing.sh <work> 4     # cold-cache listing via image re-attach
s2_fsevents <scratch> 20             # KINDS=delete for one op
scripts/copy_bench.sh <work> <same-vol-dir> <other-vol-dir>
scripts/preserve_check.sh <work> <other-vol-dir>
s3_copy verify <file> 1024 1 ; s3_copy cbdemo <src> <dst>
s4_icloud <dataless-file> <materialised-file>
s5_case <dir>
```

# Safe Transfer tests

```bash
cargo test -p safe-transfer -p safe-transfer-cli --all-features            # everything except disk images
cargo test -p safe-transfer --all-features --test faults -- --ignored --test-threads=1   # disk-image yank tests
spikes/safe-transfer/bench_m2.sh                                            # small-file benchmark (release)
```

**The suites:**
- `copy.rs`: the single-file pipeline (M1) on temp folders of the internal disk.
- `jobs.rs`: multi-file jobs through the engine (M2), on temp folders.
- `faults.rs`:
  - named fault points (`--features fault-injection`)
  - the `#[ignore]`d disk-image tests, for single files and for jobs
- `safety_grep.rs`: only `src/remove.rs` may remove anything.
- `../../safe-transfer-cli/tests/cli.rs`: the binary, including `kill -9` at 15 paused stop points, each followed by `safe-transfer resume`. These use temp folders and no disk images.

Every engine test uses its own temporary journal folder (`--journal-dir` for the CLI), never `~/Library/Application Support/Safe Transfer`. The whole crate is macOS-only for tests (F-024); on Linux only the pure unit tests run.

## The disk-image ("yank") tests

The yank tests are all `#[ignore]`d, so a plain `cargo test` never touches a disk image. Run them with `--ignored`, in one batch, not over and over while iterating.

**What they do:**
1. Each test creates a small `hdiutil` image (APFS or exFAT) in a temporary folder and mounts it at a private mount point inside that folder, never under `/Volumes`.
2. It **yanks** the image: `kill -9` on the image's own `diskimages-helper` (the process that plays the drive), then a forced unmount. A plain `hdiutil detach -force` is not a yank, because it writes cached data back first, so unflushed files survive it (proposal §4.7). `the_yank_harness_loses_unflushed_writes` checks that the harness really loses unflushed data.
3. It re-attaches the image (once at a different mount point, as a card coming back as "Untitled 1" would). It then checks that every real file name holds the complete file, resumes the job, and compares the whole tree with the originals.
4. Images are detached and deleted when the test ends, including on failure.

The tests only touch the images they create, found by their own image path, never volumes you mounted (cards, drives, other images). They list mounted volumes to find their own by UUID; that only reads `getmntinfo`.

**Expect notifications.** macOS shows a **"Disk Not Ejected Properly"** notification for every yank (the volumes are called `STTEST`). That's the point of the test, not a problem. A full `--ignored` run yanks about **32** times:
- the single-file suites: 5 points on each of 2 filesystems
- the job suites: 9 points on each of 2 filesystems
- the remount, pulled-source-card and changed-original tests: one each
- the negative control: one

The notifications can be cleared in Notification Centre.

**A note on exFAT:** `hdiutil info` lists an exFAT image's partition as `Windows_NTFS`. That is only the MBR partition type (0x07), which exFAT shares with NTFS. The filesystem really is exFAT: `mount` shows `exfat, … fskit`, `diskutil info` shows "File System Personality: ExFAT", and `an_exfat_card_gets_a_flushed_verified_copy_and_honest_metadata` asserts `fs_type == "exfat"`.

The scripts in `spikes/safe-transfer/` behave as follows:
- `run.sh` and `batch.sh` mount their images under `/Volumes` (`STProbeAPFS`, `STX`, `STF`, `STBA`, `STBX`). `batch.sh` yanks them the same way, with the same notifications.
- `bench_m2.sh` mounts privately and doesn't yank.
- All of them detach and delete their images on exit.

# Safe Transfer tests

```bash
cargo test -p safe-transfer -p safe-transfer-cli --all-features            # everything except disk images
cargo test -p safe-transfer --all-features --test faults -- --ignored --test-threads=1   # disk-image yank tests
spikes/safe-transfer/bench_m2.sh                                            # small-file benchmark (release)
```

**The suites:**
- `copy.rs`: the single-file pipeline (M1) on temp folders of the internal disk.
- `jobs.rs`: multi-file jobs through the engine (M2), on temp folders.
- `m3.rs`: conflicts, moves (cross-volume simulated with a second volume UUID), folder metadata and workers (M3). It uses a folder as the Trash, never the real one.
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

**Guards against a stuck disk-image service:**
- **Every external tool call has a 60 s limit.** This covers `hdiutil`, `diskutil`, `ps` and `kill`. A call that runs over is stopped, and the test fails with a message saying so, instead of hanging.
- **The helper to kill is found from `hdiutil info -plist`.** The harness matches the image path and reads its `hdid-pid`. It then refuses to kill unless:
  - that process is a `diskimages-helper`,
  - it is run by the user who owns the image,
  - and it serves no other image.

  A shared daemon such as `diskimagesiod` is never killed.
- **A yank counts only once it has fully taken effect:** the helper has exited and the image has left `hdiutil info`, each within 30 s.
  - A yank that fails inside a fault hook is recorded. The next `put_back` fails the test with the reason.
  - `put_back` also refuses to attach while the image is still listed. Attaching an image that its old helper still serves waits forever; that is how a run once hung for more than 70 minutes.
- **Clean-up runs on drop, including on panic.** It force-detaches the image. If the image is still listed after that, it kills the image's own helper with the same checks. It never panics, and it prints a warning if an image is left behind.

If a run still fails with "didn't finish in 60 s", the disk-image service itself is stuck (even `hdiutil create` becomes slow). Restart the Mac before running the yank tests again.

The tests only touch the images they create, found by their own image path, never volumes you mounted (cards, drives, other images). They list mounted volumes to find their own by UUID; that only reads `getmntinfo`.

**Expect notifications.** macOS shows a **"Disk Not Ejected Properly"** notification for every yank (the volumes are called `STTEST`). That's the point of the test, not a problem. A full `--ignored` run yanks about **40** times:
- the single-file suites: 5 points on each of 2 filesystems
- the job suites: 9 points on each of 2 filesystems
- the remount, pulled-source-card and changed-original tests: one each
- the M3 move suite (5 points) and replace suite (3 points)
- the negative control: one

The notifications can be cleared in Notification Centre.

**A note on exFAT:** `hdiutil info` lists an exFAT image's partition as `Windows_NTFS`. That is only the MBR partition type (0x07), which exFAT shares with NTFS. The filesystem really is exFAT: `mount` shows `exfat, … fskit`, `diskutil info` shows "File System Personality: ExFAT", and `an_exfat_card_gets_a_flushed_verified_copy_and_honest_metadata` asserts `fs_type == "exfat"`.

The scripts in `spikes/safe-transfer/` behave as follows:
- `run.sh` and `batch.sh` mount their images under `/Volumes` (`STProbeAPFS`, `STX`, `STF`, `STBA`, `STBX`). `batch.sh` yanks them the same way, with the same notifications.
- `bench_m2.sh` mounts privately and doesn't yank.
- All of them detach and delete their images on exit.

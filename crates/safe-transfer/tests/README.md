# Safe Transfer tests

```bash
cargo test -p safe-transfer -p safe-transfer-cli --all-features            # everything except disk images
cargo test -p safe-transfer --all-features --test faults -- --ignored --test-threads=1   # disk-image yank tests
```

- `copy.rs`: the single-file pipeline on temp folders (internal disk only).
- `faults.rs`: named fault points (`--features fault-injection`), plus the `#[ignore]`d disk-image tests.
- `safety_grep.rs`: only `src/remove.rs` may remove anything.
- `../../safe-transfer-cli/tests/cli.rs`: the binary, including `kill -9` at paused fault points (no disk images).

## The disk-image ("yank") tests

The yank tests are all `#[ignore]`d, so a plain `cargo test` never touches a disk image. Run them with `--ignored`, in one batch, not over and over while iterating.

**What they do:**
1. Each test creates a small `hdiutil` image (APFS or exFAT) in a temporary folder and mounts it at a private mount point inside that folder, never under `/Volumes`.
2. It **yanks** the image: `kill -9` on the image's own `diskimages-helper` (the process that plays the drive), then a forced unmount. A plain `hdiutil detach -force` is not a yank, because it writes cached data back first, so unflushed files survive it (proposal §4.7). `the_yank_harness_loses_unflushed_writes` checks that the harness really loses unflushed data.
3. It re-attaches the image, checks that the real file name holds either the complete file or nothing, and detaches and deletes the image when done (including on failure).

The tests only touch the images they create, found by their own image path, never volumes you mounted (cards, drives, other images).

**Expect notifications.** macOS shows a **"Disk Not Ejected Properly"** notification for every yank (the volume is called `STTEST`). That's the point of the test, not a problem. A full `--ignored` run yanks about a dozen times. The notifications can be cleared in Notification Centre.

**A note on exFAT:** `hdiutil info` lists an exFAT image's partition as `Windows_NTFS`. That is only the MBR partition type (0x07), which exFAT shares with NTFS. The filesystem really is exFAT: `mount` shows `exfat, … fskit`, `diskutil info` shows "File System Personality: ExFAT", and `an_exfat_card_gets_a_flushed_verified_copy_and_honest_metadata` asserts `fs_type == "exfat"`.

The measurement scripts in `spikes/safe-transfer/` (`run.sh`, `batch.sh`) mount their images under `/Volumes` (`STProbeAPFS`, `STX`, `STF`, `STBA`, `STBX`). `batch.sh` yanks them the same way, with the same notifications. Both scripts detach and delete their images on exit.

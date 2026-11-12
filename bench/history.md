# Benchmark history

Numbers come from `fstudio-bench` (starting in M1). M0 numbers come from the throwaway spikes (`docs/SPIKES.md`). The machine is an Apple M6 Mac mini with 24 GB RAM, running macOS 27.2.

## M0: 2026-11-12 (spikes)

| Measure | warm | cold |
|---|---|---|
| 10k listing, first 100 rows (getattrlistbulk, adaptive buffer) | 0.15 ms | 12.5 ms |
| 10k listing + metadata + name sort | ≈7 ms | ≈80 ms |
| 100k listing + metadata + sort | ≈105 ms | ≈880 ms (disk image) |
| 100k name sort, precomputed keys, single thread / par_sort | 25 / 12 ms | – |
| 100k sort with CoreFoundation localised compare | 147 ms | – |
| FSEvents write → callback (FileEvents\|NoDefer) | 11.5 ms median | – |
| Incremental apply of one event, 10k folder | 0.064 ms | – |
| Copy 10 GiB, same volume, copyfile ALL (no clone) | 786 MB/s | – |
| Copy 10 GiB, cross volume (disk image), copyfile ALL | 678 MB/s | – |
| Copy 5,000 small files, same volume, 4 workers | 403 ms | – |
| xxh3 verify, 10 GiB, F_NOCACHE | – | 4.9 GB/s |

## M1: 2026-11-12 (fstudio-bench)

Measured through the `fstudio-core` API on synthetic fixtures (warm cache). Latency budgets are checked against p95; for MB/s rows the p95 column is the slow tail (5th percentile).

| Measure | median | p95 | budget | result | note |
|---|---|---|---|---|---|
| 1k folder: first rows (ListingChanged) | 1.7 ms | 2.0 ms | – | – | warm cache |
| 1k folder: fully sorted | 4.2 ms | 5.2 ms | – | – | warm cache |
| 10k folder: first rows (ListingChanged) | 0.506 ms | 1.5 ms | ≤ 50.0 ms | PASS | warm cache |
| 10k folder: fully sorted | 10.0 ms | 18.4 ms | ≤ 200 ms | PASS | warm cache |
| 100k folder: first rows (ListingChanged) | 0.608 ms | 0.769 ms | ≤ 50.0 ms | PASS | warm cache |
| 100k folder: fully sorted | 94.5 ms | 103 ms | ≤ 1000 ms | PASS | warm cache |
| 10k folder: re-sort | 5.9 ms | 7.9 ms | – | – | set_view → Resorted event |
| 10k folder: filter per keystroke | 6.2 ms | 8.6 ms | ≤ 16.0 ms | PASS | set_filter → Resorted event; budget: one 60 Hz frame |
| 100k folder: re-sort | 7.7 ms | 9.7 ms | – | – | set_view → Resorted event |
| 100k folder: filter per keystroke | 8.9 ms | 15.8 ms | – | – | set_filter → Resorted event |
| rows(start, 60) on 100k | 14.9 µs | 25.5 µs | ≤ 1000 µs | PASS | formatted sizes and dates; budget: well under a frame |
| watcher: file created → Patched (10k folder) | 7.2 ms | 11.8 ms | ≤ 100 ms | PASS | FSEvents with NoDefer; write → event at the shell |
| completion per keystroke (10k frecent, 100k-entry folder) | 0.081 ms | 0.557 ms | ≤ 16.0 ms | PASS | path-like and bare-word input |
| copy 1 GiB, same volume (clone) — queue | 0.445 ms | 0.727 ms | – | – | cp -c median 1.3 ms |
| copy 5,000 small files, same volume — queue | 444 ms | 488 ms | – | – | cp -R median 1574 ms (cp -R does not clone) |
| copy 1 GiB, cross volume (disk image) — queue | 1321 MB/s | 1025 MB/s | – | – | cp: 1296 MB/s |
| copy 1 GiB cross volume: queue time ÷ cp time | 0.981 × | 0.981 × | ≤ 1.050 × | PASS | medians; page cache not purged (no sudo), see F-015 |
| copy 5,000 small files, cross volume — queue | 1198 ms | 1724 ms | – | – | cp -R median 1855 ms |
| journal overhead per item (5,000 files, queue − cp -R) | -131 µs | -131 µs | – | – | 166372324 bytes total; negative means the queue's parallel workers beat cp |
| copy 1 GiB cross volume with verify — queue | 1064 MB/s | 719 MB/s | – | – | copy + xxh3 re-read |

## M1 real card: 2026-11-12 (user's 128 GB SD card "W2", exFAT, USB reader)

Run by hand through the `fstudio` CLI. Each time includes a final `sync`. Card reads were cold: the card was unmounted and remounted before each run.

| Case | `cp` | File Studio | Finder |
|---|---|---|---|
| 1 GiB file, Mac → card (3 rounds) | 9.02–9.04 s | 9.03–9.11 s (**≤ 1 % of cp**) | 9.25–9.42 s |
| 5,000 small files (168 MB), Mac → card | 86.7 s (`cp -R`) | **73.9 s** | – |
| Real camera folder, card → Mac: 198 files (Sony ARW + HIF pairs), 3.7 GB | 25.2 s (`cp -R`) | **22.9 s** | – |
| Same, with xxh3 verify | – | **20.8 s** (verify on) | – |
| Cold listing of a 199-entry DCIM folder (`fstudio ls`, including process start) | – | 62 ms | – |

The copied raws were md5-identical to the card. The copy budget (within 5 % of `cp`/Finder) **passes on real removable media**. exFAT has no `clonefile` and no exclusive rename, so this run exercised the check-then-rename fallback; xattrs land as AppleDouble `._` files, as with Finder.

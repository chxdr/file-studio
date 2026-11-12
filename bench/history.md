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

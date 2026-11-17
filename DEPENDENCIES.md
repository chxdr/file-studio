# Dependencies and licences

Project licence: **GPL-3.0-or-later**. Every dependency must be GPLv3-compatible; `cargo deny check licenses` enforces this (see `deny.toml`).

## Direct dependencies

| Crate | Version | Used by | Licence |
|---|---|---|---|
| thiserror | 2 | most crates | MIT OR Apache-2.0 |
| serde / serde_json | 1 | model, core | MIT OR Apache-2.0 |
| rusqlite (bundled SQLite) | 0.41 | db | MIT (SQLite: public domain) |
| parking_lot | 0.12 | db | MIT OR Apache-2.0 |
| uniffi | 0.32 | ffi | MPL-2.0 |
| tempfile (dev) | 3 | tests | MIT OR Apache-2.0 |
| libc | 0.2 | fs, path, ops (sys modules) | MIT OR Apache-2.0 |
| crossbeam-channel | 0.5 | fs, path, ops | MIT OR Apache-2.0 |
| rayon | 1 | fs, view | MIT OR Apache-2.0 |
| unicode-normalization | 0.1 | view, path | MIT OR Apache-2.0 |
| plist | 1 | fs (Finder tags) | MIT |
| core-foundation / -sys | 0.10 / 0.8 | fs (FSEvents) | MIT OR Apache-2.0 |
| nucleo-matcher | 0.3 | path (fuzzy completion) | MPL-2.0 |
| xxhash-rust (xxh3) | 0.8 | ops (verify) | BSL-1.0 |
| objc2 / objc2-foundation / objc2-encode | 0.6 / 0.3 / 4.1 | ops (Trash) | MIT |
| regex | 1 | search, rename | MIT OR Apache-2.0 |
| kamadak-exif | 0.6 | preview, rename (EXIF) | BSD-2-Clause |
| syntect (pure-Rust regex engine, `fancy-regex`) | 5 | preview (syntax highlighting) | MIT |
| quick-xml | 0.42 | xmp (culling marks in sidecars, read-only) | MIT |

Indirect additions in M3, all checked by `cargo deny`: `regex-automata`, `regex-syntax` (MIT/Apache-2.0), `aho-corasick` (Unlicense/MIT), `mutate_once` (BSD-2-Clause), `fancy-regex` (MIT), `bincode` 1 (MIT), `flate2` and `miniz_oxide` (MIT/Apache-2.0), `walkdir` and `same-file` (Unlicense/MIT).

Planned (listed in the root `Cargo.toml` as workspace dependencies, not used yet): clap, anyhow, dirs. `cargo deny check licenses` passes as of the M1 merge.

Spikes (`spikes/`, not shipped): see `docs/SPIKES.md` for the licence list, which adds notify (CC0-1.0) and core-foundation (MIT/Apache-2.0).

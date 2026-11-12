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

Planned (listed in the root `Cargo.toml` as workspace dependencies, not used yet): libc, crossbeam-channel, rayon, unicode-normalization, xxhash-rust (BSL-1.0), clap, anyhow, dirs. Each is added to the table above when a crate starts using it.

Spikes (`spikes/`, not shipped): see `docs/SPIKES.md` for the licence list, which adds notify (CC0-1.0) and core-foundation (MIT/Apache-2.0).

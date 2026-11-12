# File Studio: rules for every session

A fast, Mac-native file manager: a Rust core with thin native shells. The licence is GPL-3.0-or-later.

**Read first:**
- `DECISIONS.md` (F-001…), which is binding.
- `docs/PROPOSAL.md` (architecture and core API).
- `docs/SPIKES.md` (measured platform behaviour).
- `docs/HANDS-ON.md` §8–9 (address bar spec, keymap).

The original brief is `/Users/milky/Downloads/file-studio-prompt.md`.

## Ownership
- Each session owns the files named in its brief and touches nothing else.
- `Cargo.toml` (root), `fstudio-model`, `fstudio-db`, `DECISIONS.md`, `DEPENDENCIES.md` and `CLAUDE.md` are the coordinator's. If you need a shared type or a decision changed, say so in your final report; don't edit them.
- Add dependencies in your own crate's `Cargo.toml`, using `x.workspace = true` when the root already lists the dependency, or a plain version otherwise. List every new dependency and its licence in your report. Every dependency must be GPLv3-compatible (see `deny.toml`).
- Commit on your own branch with clear messages. Never merge, push or rebase `main`.

## Core rules
- **No app logic in shells.** Behaviour lives in Rust (F-001).
- **Data safety (F-007):**
  - Nothing is hard-deleted except through the single gated `delete_immediately` path in `fstudio-ops`.
  - Nothing is ever silently overwritten: writes go to a partial file, then `renamex_np(RENAME_EXCL)`.
  - Cross-volume moves remove the source only after the copy is done.
  - Errors are reported per file.
- **Unsafe code** is allowed only in a `sys` module (`src/sys.rs` or `src/sys/`), with `#![allow(unsafe_code)]` on that module and a `// SAFETY:` comment on every block. Everywhere else, `unsafe_code = "deny"` applies.
- **Names:** keep names exactly as the filesystem returns them. Use NFC + case-folded keys only for comparing, sorting and matching (F-017).
- **iCloud placeholders:** never read the content of an entry with `SF_DATALESS` without an explicit request to (F-016).
- **Threads:** no async runtime (F-010). FFI-facing calls must not block on disk unless documented.

## Tests
- Run `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all --check` before every commit.
- Tests use temporary directories (`tempfile`), never real user folders. Cover the nasty cases that apply to your crate:
  - unicode (NFC vs NFD) names
  - names that differ only by case
  - huge folders (gate anything over 10k entries behind `#[ignore]` or an env var)
  - symlink loops
  - read-only and permission-denied paths
  - a full disk (a small `hdiutil` image, `#[ignore]` by default)
- Detach and delete any disk images and temporary data you create.

## Style
- Use doc comments on every public item, written in plain language.
- Prefer small, pure functions that can be tested without I/O.

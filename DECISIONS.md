# Architecture decisions

Each entry records a decision, why it was made, and its status. Big changes get asked for before they land. The format follows Import Studio's `DECISIONS.md` (D-…); File Studio entries are numbered F-….

---

## F-001: Rust core, thin native shells (accepted 2026-11-12)
All app logic lives in the Rust workspace:
- path parsing and completion, and frecency
- listing, sorting, filtering, type-to-select and selection rules
- window, pane, tab and history state
- the operation queue, its journal and undo
- conflict rules, rename previews, the keymap, the command registry, hints and settings

Shells only render, turn input into neutral `KeyChord`s and intents, and make OS calls that must be Cocoa (Quick Look, `NSWorkspace`, drag pasteboards, trash, File Provider status), handing straight back to the core. See `docs/PROPOSAL.md` §1.
**Why:** the Linux (GTK4/libadwaita) and Windows ports must be new shells, not rewrites. This is the same as Import Studio's D-001.

## F-002: The core models the window, not just the files (accepted)
`Workspace` (windows → panes → tabs → location, history, view options, selection) lives in Rust and is persisted in SQLite. The shell keeps only transient visual state and reports scroll offsets back so they can be restored.
**Why:** session restore, saved workspaces and the other shells all come from one implementation.

## F-003: Crate layout with the `fstudio-` prefix (accepted)
The crates are `model`, `fs`, `path`, `view`, `ops`, `rename`, `search`, `preview`, `db`, `keymap`, `core`, `ffi`, `cli` and `bench`. The dependency graph is acyclic: `model` ← `fs`/`path`/`view`/`keymap`/`db` ← `ops` ← `core` ← `ffi`/`cli`. `rename`, `search` and `preview` are stubs until M3. See `docs/PROPOSAL.md` §2.
**Why:** the prefix avoids shadowing Rust's `core` (Import Studio's D-004). `view` and `model` keep the pure logic testable without I/O.

## F-004: FFI is UniFFI (proc-macro mode), linked as a static library (accepted)
`scripts/build-core-apple.sh` builds `fstudio-ffi`, generates the Swift bindings and writes `target/uniffi-headers`. The app links `libfstudio_ffi.a` directly. There is no XCFramework snapshot, which carries over Import Studio's D-024 fix. A Linux shell in gtk4-rs links `fstudio-core` with no FFI.
**Why:** this is proven in Import Studio (D-002, D-024).

## F-005: Listings never cross the FFI whole (accepted)
A listing is a core-side, immutable, versioned `Arc<Listing>` snapshot, already sorted and filtered. The shell asks for `rows(start, count)` for the visible rows only, and the core formats the display strings. Icons are resolved by icon key on the Swift side and never cross the boundary.
**Why:** this is the 100k-entry budget. The shell never sorts and never holds a second model.

## F-006: One command registry (accepted)
Every action is a registered command with an id, title, category, default bindings per profile (Standard, Vim) and an enablement predicate. The registry drives menus, the palette, the held-modifier overlay, the hints, the Settings shortcut editor and conflict detection. A test checks that every menu item and overlay row maps to a command.
**Why:** this is Import Studio's D-029 catalogue idea, extended so that rebinding anything is a data change.

## F-007: Data safety is structural (accepted)
- Every mutating file operation goes through `fstudio-ops` and is journaled in SQLite before it touches the disk.
- Writes land as `.<name>.fstudio-partial` and are renamed into place with `renamex_np(RENAME_EXCL)` (Linux: `renameat2(RENAME_NOREPLACE)`). Filesystems without exclusive rename get a check-then-rename fallback.
- Cross-volume moves remove the source only after its copy is done (and verified, if verify is on).
- "Replace" sends the old file to the Trash.
- `delete_immediately()` is the only caller of `remove_file`/`remove_dir_all` on user data. It is gated by a setting that is off by default, and a test enforces that.
- Errors are per file and never end a batch silently.
**Why:** this is the brief's non-negotiable data-safety section, and the same approach as Import Studio's D-011.

## F-008: Trash goes through the OS (accepted)
On macOS the core calls `Platform::trash`, which the shell implements with `NSFileManager.trashItem`, so Finder's Put Back works. The journal records the Trash location so that File Studio's undo can put items back itself. Linux will use the freedesktop Trash spec in Rust.
**Why:** Finder-compatible Put Back matters more than keeping this in pure Rust.

## F-009: Path bar engine (accepted)
- **Parsing** is in `fstudio-path`: quotes, `file://`, backslash escapes, `~`/`~user`, `$VAR`/`${VAR}`, relative paths and lexical normalisation. The result is `Dir`, `File` (select it in its folder), `Missing` (offer Create folder) or `Remote` (later).
- **Completion** merges the children of the typed parent (cached by path and mtime) with an in-memory frecency index, ranked by `nucleo-matcher` with a frecency boost.
- **Frecency** uses zoxide's algorithm, stored in SQLite.
- **Budget:** under 16 ms per keystroke, checked in `fstudio-bench`.
- **Shell:** the bar is a custom AppKit view, not `NSPathControl`.

## F-010: No async runtime until remote filesystems (accepted)
The core uses plain threads, crossbeam channels and rayon, with one I/O worker set per volume. FFI calls never block on disk except documented `stat`-like queries. Events go out through an `EventSink` callback, coalesced to at most every 16 ms per kind.
**Why:** this matches Import Studio. Async only pays off for network I/O (M5).

## F-011: Product defaults (accepted 2026-11-12, user approved the proposal as drafted)
These are the answers to proposal §13, taken from its recommendations.

| Setting | Default |
|---|---|
| Bundle ID | `app.filestudio.mac` |
| Platform | macOS 26+, Apple Silicon (arm64) only |
| Layout | Single pane with tabs; dual pane one keystroke away, remembered per workspace |
| Keymap | the Finder-compatible Standard keymap; Vim is an opt-in profile offered in first-run setup |
| zoxide history | first-run setup offers a one-time import from zoxide, if its database is found |

Every one of these is also a setting.

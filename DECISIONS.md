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

## F-012: Feature ranking signed off (accepted 2026-11-12)
`docs/RESEARCH.md` §3 is the v1 / later / never list. The user changed it as follows (see "Sign-off changes" in that file):
- **Disk-usage map:** moved from never to later (M4).
- **Vim profile:** moved to v1, offered in setup.
- **Saved workspaces:** moved to M2, next to session restore.
- **Per-folder view memory:** on by default in "smart" mode.
- **Gallery and icon views:** stay in M3.

## F-013: macOS lister is `getattrlistbulk` with an adaptive buffer (accepted, M0 spike)
- **First call:** a 16 KiB buffer, so the first rows arrive in 0.1–0.3 ms warm and 12–23 ms cold for 10k–100k entries.
- **Later calls:** 256 KiB.
- **What it returns:** name, type, size, mtime and `st_flags` (dataless detection, F-016) in one pass.
- **Portable shells:** use `read_dir` + `DirEntry::metadata()`.
- **No `fts`:** it reads the whole folder before returning the first entry, and the SDK ships two `FTSENT` layouts.
- **Runtime fallback** to `readdir` + parallel `fstatat` when bulk returns `ENOTSUP` (network filesystems; still untested).
- **Sorting:** precomputed keys (natural, case-folded, NFC), with `par_sort` above about 20k entries. 100k entries sort in 12–25 ms.
- **Measured:** 10k fully sorted in ≈7 ms warm and ≈80 ms cold; 100k in ≈105 ms warm and ≈880 ms cold. The cold figure is measured on a disk image and is within 12 % of the 1 s budget. See `docs/SPIKES.md` §1.

## F-014: Watcher uses FSEvents with file events and `NoDefer`, applied incrementally (accepted, M0 spike)
- **What is watched:** only visible folders, with `FileEvents | NoDefer` and a latency of 30–50 ms. An isolated change arrives at the ≈11 ms floor, and bursts coalesce.
- **How events are applied:** each event becomes an `fstatat` plus a sorted insert, replace or remove in the snapshot. That takes ≈0.06 ms at 10k, ≈11 ms end to end.
- **When to re-list:** on `MustScanSubDirs`, a dropped stream or root change, or more than ≈5 % of entries in one batch. A full re-list of 100k entries takes ≈116 ms, which is over budget, so incremental updates are required.
- **Implementation:** sits behind a `Watcher` trait. `notify` (CC0) measured the same latency as raw FSEvents. The decision between `notify` and the spike's ≈100-line direct FFI depends on whether `notify` exposes `NoDefer` and the drop flags; that is checked in M1.

## F-015: Copy engine (accepted, M0 spike; amends F-007)
- **Same APFS volume:** `copyfile(COPYFILE_ALL | COPYFILE_CLONE)`. It is instant and keeps ACLs; plain `clonefile` drops them. Clones skip verify.
- **Across volumes or on non-APFS:** `copyfile(COPYFILE_ALL | COPYFILE_DATA_SPARSE | COPYFILE_EXCL)` with the status callback.
  - Progress arrives about once per MiB.
  - **Pause** blocks inside the callback.
  - **Cancel** returns `COPYFILE_QUIT`, which removes the partial file within ≈2 ms.
  - Without `DATA_SPARSE`, `COPYFILE_ALL` silently turns a 32 KiB sparse file into 1 GiB.
- **Own read/write loop** (1–4 MiB buffers, `F_NOCACHE` on both ends for files of 1 GiB or more, `SEEK_DATA`/`SEEK_HOLE`, then `copyfile(COPYFILE_METADATA)`): kept for in-flight hashing and per-chunk journaling. Its throughput matched `copyfile`.
- **Small files:** 4 parallel workers per volume, about 2.5× faster. A whole-folder `clonefile` (28 ms for 5,000 files) is used only when the destination folder doesn't exist, because it skips per-file conflict checks.
- **Placement:** write to `.<name>.fstudio-partial`, then `renamex_np(RENAME_EXCL)`, and apply ACLs and immutable flags **after** the rename. A `deny delete` ACL blocks renaming or removing the temp file, so cleanup clears ACLs first.
- **Verify:** xxh3 over an `F_NOCACHE` re-read of a destination written with `F_NOCACHE`. It adds ≈12–15 % (≈4.9 GB/s cold).
- **Budget note:** copy speed is compared on durable time (fsync, or inputs of 10 GiB and up). The "5 GB/s" figures for `cp` on warm 1 GiB files are the page cache absorbing the write. Cross-volume parity with Finder and `cp` still needs a real external drive.

## F-016: iCloud and File Provider placeholders are detected from the listing (accepted, M0 spike)
- **Detection:** the `SF_DATALESS` bit (0x4000_0000) in `st_flags` comes free with the listing. `lstat`, `getattrlistbulk`, `listxattr` and the size/allocated-size resource keys never trigger a download.
- **Policy:** previews, thumbnails, hashing, copying and folder-size work refuse dataless items or ask first. Sizes show the logical size; "size on disk" is 0 for placeholders.
- **Rich status** ("downloading", "uploaded") goes through `Platform::cloud_status` (1.6–14 ms per item) for visible items only.
- **Download Now** uses `startDownloadingUbiquitousItemAtURL`.
- **Not tested, by choice** (user, 2026-11-12): whether open/read, Quick Look and listing a dataless *folder* trigger downloads. All three are treated as "may download" and gated behind the flag.

## F-017: Names, Unicode and case (accepted, M0 spike)
- **APFS behaviour:** names are stored exactly as given but looked up normalisation-insensitively, on both case-sensitive and case-insensitive volumes.
- **Display:** the core shows the stored name unchanged.
- **Comparison:** sorting, filtering, matching and conflict *prediction* use NFC + case-folded keys. The authoritative collision check is the OS's exclusive create or rename (`EEXIST` → conflict dialog).
- **Case sensitivity** is queried per destination volume with `pathconf(_PC_CASE_SENSITIVE)`, so copying `File` and `file` onto a case-insensitive volume is caught in the plan.

## F-018: Address bar behaviour and Standard keymap follow `docs/HANDS-ON.md` §8–9 (accepted, M0 hands-on)
**Address bar:**
- **States:** REST → EDIT → COMPLETING → committed or cancelled. It never navigates or creates anything without an explicit Return.
- **Editing and completion:**
  - Entering edit mode selects all of the text.
  - Esc first closes the list, then cancels.
  - Losing focus reverts.
  - Tab extends the common prefix, then cycles.
  - ⌘Return opens in the other pane.
  - ⌥⌫ deletes back one path component.
  - Completion rows show each candidate's full breadcrumb (from ForkLift).
- **Overflow:** collapsed middle segments sit behind a "…" menu that lists them by name.
- **Paste cleanup:** follows the §8 pipeline and shows a visible "cleaned up" chip; ⌘Z restores the original text.
- **Errors:** shown inline in the bar (from Nimble Commander). The parser is fuzz-tested, because Marta crashed on `file://` input.

**Keymap:**
- Finder's binding wherever Finder has one: **Return renames**, ⌘↓/⌘O open, and Space is Quick Look.
- From Marta, ForkLift and Nimble: F5/F6/F7/F8 for copy, move, new folder and Trash; Tab switches panes; ⌘\ toggles dual pane.
- **Deliberate deviations from Finder:** ⌘P is quick open, ⇧⌘P is the palette, and ⌘F filters the current folder. Each is documented in Settings.
- **Type-to-select** is the default for printable keys; filter-on-type is a setting.

## F-019: Visual defaults: ForkLift's layout, system materials and accent (accepted, M0 hands-on; user preference)
- **Chrome:** a unified ~52 pt toolbar with at most 10 default controls.
- **Pane header (36 pt):** a location glyph, then the address bar (13 pt), with an 11 pt subtitle. At rest it is flat like ForkLift; on hover or focus it gets a capsule outline like QSpace.
- **Active pane:** a 2 pt outline plus a 3 pt header bar, in `controlAccentColor` (never a fixed brand colour).
- **List rows:** 22 pt (Compact 20, Comfortable 26), 16 pt icons, no zebra stripes; secondary columns at 12 pt with monospaced digits.
- **Sidebar:** 200 pt wide, system `.sidebar` material, with a filter field at the bottom.
- **Materials:** file lists are opaque content backgrounds, and glass appears only in native chrome and transient surfaces.

## F-020: Ops amendments from the M1 implementation (accepted, M1; amends F-007, F-008, F-015)
- **Removal lives in one file (amends F-007).** Every removal of a user-visible path is in `fstudio-ops/src/remove.rs`, and `tests/safety_grep.rs` enforces it. It has four entry points:
  - `delete_immediately`, which needs a `DeleteGate` that exists only when the setting is on
  - `remove_partial`, which refuses any name that isn't one of our temporary names
  - `remove_moved_source`, which needs a `Placed` proof and refuses if the source changed since it was copied
  - `remove_moved_source_dir`, which only removes empty folders (`rmdir`)
- **Trash is implemented in Rust (amends F-008)** via `objc2-foundation` (`NSFileManager trashItemAtURL:`), so Finder's Put Back works and the CLI can trash too. The `Trasher` trait stays, so tests and the Linux shell can plug in their own.
- **Temporary names carry the journal item id:** `.<name>.<item>.fstudio-partial`. Copying `File` and `file` in parallel onto a case-insensitive volume made the plain names collide.
- **The `copyfile` status callback must stop on `COPYFILE_ERR` (amends F-015).** Returning "continue" retries forever; it hung on a full disk. The spike code has the same bug and must not be copied.
- **Scheduling is per volume pair** (source device, destination device), so a card-to-SSD copy doesn't block an SSD-to-SSD copy.
- **Durability:** moves full-sync each file before placing it, and the folder after the rename. Plain copies sync only when `fsync_copies` is on. Verified copies use our own copy loop, which reads the source once, hashes it while copying and writes the destination uncached. Per-file full syncs made verified small-file copies about 50× slower.
- **Untested:** the real system Trash (`SystemTrasher`), iCloud placeholders and a real external drive.

## F-021: Core behaviour settled while integrating (accepted, M1)
- **Per-folder view memory** ("smart", F-012) remembers sort order and view mode. "Show hidden files" stays per tab.
- **Pasting a copy into its own folder** duplicates the item ("name 2"), as Finder does.
- **Undo that has to refuse** (the item changed, or the original place is now taken) returns a message in the `Outcome` for the shell to show, not an error.
- **Commands that need the OS** (Quick Look, Get Info, Open With, Reveal in Finder, Open in Terminal, the palette and dialogs) come back to the shell as a `ShellRequest`. A test checks that every registered command is either handled by the core or returns one, except an explicit list of later features: compress, make alias, global search, connect to server.

## F-022: Context menus are defined by the core (accepted, M2 first task)
The right-click menus have a core model, `context_menu(target, ctx) -> Vec<MenuItem>`, built from registered commands. The targets are: a file, a folder, a multi-selection, empty space in a list, an address-bar segment, a sidebar item and a tab. Each target has its own default contents, order, separators, enablement and submenus (Copy Path styles, Open With). Users can customise it in Settings (add, remove, reorder), and custom shell actions (M4) can join it. The shell only draws the menu and sends the chosen command back.
**Why:** this follows F-001 and F-006: Swift must not decide which actions apply to what. It also brings the user's "everything customisable" requirement to menus.

## F-023: No silent moves of important folders (accepted 2026-11-12, after a real incident)
**The incident:** while trying the first M2 build, the user accidentally moved `~/Applications`, `~/Downloads` and their `~/Import Studio` project into other folders in about a minute, most likely by dragging sidebar favourites. Nothing was lost: the ops journal recorded every move, and all three were put back. But the app gave no sign anything had moved.

**Rules:**
- **Sidebar drags are references.** They reorder or pin and never move a real folder; they use a private pasteboard type.
- **Moving a protected folder needs confirmation from the core, with Cancel as the default.** Protected folders are: the home folder, the standard home folders, the system folders, volume roots, and ancestors of open tabs. The list is a setting; the confirmation itself is always on.
- **Every move by drag, F6 or cut/paste shows a toast** with Undo.
- **The drop target is exactly what's highlighted at mouse-up.**

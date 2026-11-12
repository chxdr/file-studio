# File Studio: architecture proposal (pre-code)

Status: **approved 2026-11-12** (as drafted). The agreed decisions are F-001…F-011 in `DECISIONS.md`. Sections 6, 9 and 11 get spike numbers when `docs/SPIKES.md` lands.

**Environment checked on 2026-11-12:**
- Machine: Apple M6, 24 GB RAM, macOS 27.2.
- Tools: Xcode 27.2, Rust 1.99 (cargo 1.99.1), xcodegen installed.
- `~/File Studio` held only an initial commit (licence, readme, `.gitignore`).

**Inputs:**
- The brief.
- Import Studio's `DECISIONS.md` (D-001…D-033), `docs/PROPOSAL.md`, `istudio-cull::keymap`, `istudio-core::hints`, `ShortcutToasts.swift`, `DebugAutomation.swift`, `CoreAPI.swift` and `scripts/build-core-apple.sh`.

**Running in parallel**, and feeding sections 6 and 9 when they land:
- the research report (`docs/RESEARCH.md`)
- the platform spikes (`docs/SPIKES.md`)

Numbers marked *(spike)* below are placeholders until those reports arrive.

---

## 1. Guiding decisions

1. **The core owns every behaviour; the shell only renders.** This carries over Import Studio's D-001.
   - **In Rust:** path parsing and completion, frecency, listing, sorting, filtering, type-to-select matching, selection and cursor rules, tab, pane and workspace state, history (back/forward), the operation queue and its journal, conflict rules, undo, rename previews, the keymap, the command registry, hints, and settings.
   - **In Swift:** drawing, turning `NSEvent`s into neutral `KeyChord`s and mouse intents, and the OS calls that only exist in Cocoa: Quick Look, `NSWorkspace` (icons, open-with, eject), drag pasteboards, Services, Share, `NSFileManager.trashItem`, and File Provider status. Each of these calls goes back into the core straight away.
2. **The core is the model of the window, not only of the files.** A `Workspace` (windows → panes → tabs → location and history, plus view settings and selection) lives in Rust and is persisted by Rust. Session restore, saved workspaces and the Linux and Windows shells then come for free. The shell keeps only transient visual state, such as scroll offsets and animation progress, and reports the scroll offset back so it can be restored.
3. **Lists never cross the FFI boundary whole.** A listing is a core-side `Arc<Listing>` snapshot: sorted, filtered and versioned. `NSTableView` asks for `rows(range)` for the visible rows only. That is about 60 small records per frame, not 100k. The shell never sorts.
4. **Data safety is structural, not careful.**
   - Every mutating file operation goes through `fstudio-ops`, which journals it in SQLite before it touches the disk.
   - There is no code path from the UI to `unlink(2)` except the opt-in "Delete Immediately" command. That command lives in one function, behind a setting that is off by default, and a test asserts it is the only caller.
   - Every write lands as `.<name>.fstudio-partial`. It is then renamed into place with `renamex_np(RENAME_EXCL)` (`renameat2(RENAME_NOREPLACE)` on Linux), so an existing file can never be replaced by accident. This is Import Studio's D-011.
   - Cross-volume moves run copy → verify (optional xxh3, `F_NOCACHE`) → remove the source. A source is never removed before its copy is confirmed.
5. **Latency budgets are architectural.** The listing is streamed: the first chunk is sorted and shown, then the final sort replaces it with a stable cursor. Completion and frecency run against in-memory indexes. The watcher patches snapshots incrementally instead of re-listing. All I/O happens off the calling thread. FFI calls never block on disk except the explicitly synchronous `stat`-style ones, and those are documented as such.
6. **One command registry drives everything.** Each command has an id, a title, a category, default bindings per keymap profile (Standard, Vim), and an enablement predicate. The registry feeds menus, the command palette, the held-modifier overlay, the "Did you know?" hints, the Settings › Shortcuts editor and the rebinding conflict checker. A test checks that every menu item and every overlay row maps to a registered command, as Import Studio's catalogue test does.

---

## 2. Repo layout

```
~/File Studio/
├── Cargo.toml                 # workspace, shared lints (unsafe_code = "deny" except sys modules), release profile
├── rust-toolchain.toml        # 1.99, same as Import Studio
├── LICENSE                    # GPL-3.0-or-later
├── DECISIONS.md               # ADR log (F-001…)
├── DEPENDENCIES.md            # every dep + licence; enforced by cargo-deny
├── deny.toml
├── crates/
│   ├── fstudio-model/         # plain shared types: EntryId, Kind, FileMeta, errors, KeyChord — no I/O
│   ├── fstudio-fs/            # listing, metadata, xattrs/tags, volumes, watcher (Platform trait: macOS/Linux)
│   ├── fstudio-path/          # parse/normalise/expand, breadcrumbs, completion, fuzzy match, frecency
│   ├── fstudio-view/          # Listing snapshots: sort (natural, NFC-aware), filter, type-to-select, selection
│   ├── fstudio-ops/           # operation queue, copy/move/trash/extract, verify, conflicts, journal, undo
│   ├── fstudio-rename/        # token AST, regex, case, sequences, EXIF tokens, preview, presets   (M3)
│   ├── fstudio-search/        # Spotlight (via Platform) + parallel walk, filters, result listings  (M3)
│   ├── fstudio-preview/       # metadata summaries: EXIF/video/code; line counts; syntax spans       (M3)
│   ├── fstudio-db/            # SQLite (rusqlite, bundled): schema, migrations, settings, frecency, journal
│   ├── fstudio-keymap/        # command registry, bindings, profiles (Standard/Vim), overlay catalogue, hints
│   ├── fstudio-core/          # facade: Engine, Workspace (windows/panes/tabs), events, settings, threads
│   ├── fstudio-ffi/           # UniFFI exports only (proc-macro mode), thin wrappers over fstudio-core
│   ├── fstudio-cli/           # `fstudio` CLI: ls, watch, complete, z, cp/mv/trash via the queue, undo, journal, settings
│   └── fstudio-bench/         # benchmark harness: synthetic trees, budgets, JSON → bench/history.md
├── apple/
│   ├── project.yml            # XcodeGen (the .xcodeproj is generated, not committed)
│   └── FileStudio/
│       ├── Core/              # CoreAPI.swift (the contract), RustCore.swift, Generated/
│       ├── App/               # app entry, AppModel, menus from the registry, Settings, Setup, DebugAutomation
│       ├── Window/            # window/toolbar/split controllers, tab bar
│       ├── PathBar/           # AppKit breadcrumb + field + completion popover
│       ├── List/              # NSTableView list view (cell reuse, layer-backed)
│       ├── Columns/           # Miller columns (custom, on NSTableViews)
│       ├── Sidebar/           # favourites, volumes, tags, recents
│       ├── Ops/               # queue popover/panel, conflict sheet
│       ├── Platform/          # Quick Look, NSWorkspace, trash, File Provider, drag pasteboards
│       ├── Views/             # ShortcutToasts, overlays, shared components
│       └── Mock/              # MockCore: 100k synthetic entries, fake queue (UI work without Rust)
├── scripts/
│   ├── build-core-apple.sh    # same pattern as Import Studio's D-024: static lib + module map, linked directly
│   └── bench.sh
├── bench/history.md
├── spikes/                    # M0 throwaway measurements (standalone workspace, not shipped)
├── docs/                      # PROPOSAL, RESEARCH, SPIKES, M1…
└── tests/                     # cross-crate integration tests (nasty trees built in tempdirs)
```

**Changes from the crate list in your brief, and why:**
- **`fstudio-view` is added.** Sorting, filtering, type-to-select and selection are pure logic over a listing, with no I/O. Keeping them out of `fs` means they can be tested exhaustively and reused by search results, archives and remote listings, all of which produce the same `Listing`.
- **`fstudio-model` is added.** It holds shared plain types, so `path`, `view` and `ops` don't depend on each other or on `fs`. It is Import Studio's `istudio-model` idea.
- **`keymap` and `core` are separate crates.** The command registry and hint rules are large and pure, while `core` is the threaded facade.
- **`fstudio-bench` is its own crate.** It generates synthetic trees (10k/100k flat, deep, unicode, a mix of sparse and large files) and checks every budget in the brief.
- **`rename`, `search` and `preview` are stubbed in M1** (their crates exist so the API shape is fixed) and are filled in M3.

**The dependency graph has no cycles:** `model` ← `fs`, `path`, `view`, `keymap`, `db` ← `ops` ← `core` ← `ffi` / `cli`.

---

## 3. FFI: UniFFI, as in Import Studio

**Choice:** UniFFI 0.32 in proc-macro mode (MPL-2.0, GPL-compatible), for the same reasons as Import Studio's D-002:
- It handles callbacks and objects, and generates Swift.
- A community C# generator exists for Windows.
- The Linux shell (gtk4-rs + libadwaita, in Rust) links `fstudio-core` directly, with no FFI at all.

**Build:** a copy of Import Studio's `build-core-apple.sh` with the D-024 fix built in from day one.
- It runs `cargo build -p fstudio-ffi --release`, then `uniffi-bindgen` to generate the Swift.
- It writes `target/uniffi-headers` and links `libfstudio_ffi.a` directly.
- There is no XCFramework snapshot.
- A `RustCore` legacy target runs the script before the app builds.

**Hot paths:**

| Path | Approach | Expected cost |
|---|---|---|
| Visible rows | `listing.rows(start, count) -> Vec<RowInfo>`: about 60 records of name, icon key, size, date, kind, flags and tags. Called on scroll, cached per row index by Swift, and dropped when the listing version changes. | ~5–20 µs per call *(spike)* |
| Icons | Swift asks `NSWorkspace` for an icon by icon key (UTI, or a path for bundles), cached per key. Icons never cross the boundary. | OS-side |
| Completion | `complete(input, cursor) -> Completions`, at most 50 items | < 16 ms total, < 2 ms FFI |
| Events | `EventSink` callback trait, coalesced every ≤ 16 ms per kind, hopped to the main actor | — |

`RowInfo` carries a precomputed display string for size and date, formatted by the core with the locale the shell passes in. Columns therefore never re-format strings in Swift, and Linux and Windows show the same text.

---

## 4. Core public API (sketch for approval)

This is illustrative Rust. Names will shift, but the shape is what I'm asking you to approve.

```rust
// ───────── Engine: one per process ─────────
#[uniffi::export]
impl Engine {
    #[uniffi::constructor]
    fn new(config: EngineConfig, events: Arc<dyn EventSink>, platform: Arc<dyn Platform>) -> Result<Arc<Self>>;

    // Settings: one typed struct, persisted in SQLite by the core
    fn settings(&self) -> Settings;
    fn update_settings(&self, s: Settings) -> Result<()>;

    // Workspace model (windows → panes → tabs). Restored on launch.
    fn restore_session(&self) -> WorkspaceState;           // or a default single window
    fn workspace(&self) -> Arc<Workspace>;
    fn saved_workspaces(&self) -> Vec<WorkspaceSummary>;
    fn save_workspace(&self, name: String) -> Result<()>;
    fn open_workspace(&self, id: WorkspaceId) -> Result<WorkspaceState>;

    // Commands: the single entry point for keys, menus, palette, toolbar, drags
    fn handle_key(&self, chord: KeyChord, ctx: InputContext) -> Option<Outcome>;
    fn perform(&self, cmd: CommandInvocation, via: Via) -> Result<Outcome>; // via = Keyboard|Mouse|Menu|Palette
    fn commands(&self) -> Vec<CommandInfo>;                // registry: id, title, category, bindings, enabled
    fn palette(&self, query: String, ctx: InputContext) -> Vec<PaletteItem>; // commands + folders + recents (M3)
    fn held_shortcuts(&self, mods: Mods, ctx: InputContext) -> Option<HeldShortcuts>;   // overlay
    fn hint_for(&self, cmd: CommandId) -> Option<Hint>;                                // "did you know?"

    // Path bar engine (stateless queries + frecency writes)
    fn breadcrumbs(&self, path: String) -> Vec<Crumb>;     // segments, icons keys, volume root, package flags
    fn parse_location(&self, raw: String, cwd: String) -> ParsedLocation;
        // expands ~ and $VARS; strips quotes, file://, backslash escapes; →
        // Dir(path) | File{dir, select} | Missing{path, nearest_existing} | Remote(url, later)
    fn complete(&self, input: String, cursor: u32, cwd: String) -> Completions; // children + fuzzy + frecent
    fn subfolders(&self, path: String) -> Vec<Crumb>;      // chevron menu
    fn copy_path_as(&self, path: String, style: PathStyle) -> String; // POSIX/escaped/~-relative/file URL

    // Operations
    fn ops(&self) -> Arc<OpQueue>;
    fn undo(&self) -> Result<Option<Outcome>>;
    fn redo(&self) -> Result<Option<Outcome>>;
    fn undo_state(&self) -> UndoState;                     // Edit-menu labels: "Undo Move of 12 Items"

    // OS hooks forwarded by the shell
    fn volumes_changed(&self);
    fn app_will_quit(&self);                               // flush session + journal
}

// ───────── Tab: one location + history + view state ─────────
#[uniffi::export]
impl Tab {
    fn id(&self) -> TabId;
    fn location(&self) -> Location;                        // path (later: remote URL, archive path, search)
    fn go(&self, to: String) -> Result<Outcome>;           // via parse_location; records frecency
    fn back(&self) / forward(&self) / up(&self) -> Result<Outcome>;
    fn listing(&self) -> Arc<Listing>;                     // current snapshot (versioned)
    fn set_view(&self, v: ViewOptions);                    // mode, columns, sort, hidden, folders-first, filter text
    fn set_visible_range(&self, start: u32, count: u32);   // prioritises metadata/folder-size/git work
    fn selection(&self) -> Selection;  fn set_selection(&self, s: SelectionChange);
    fn type_select(&self, text: String) -> Option<u32>;    // row to jump to
}

// ───────── Listing: immutable, sorted, filtered snapshot ─────────
#[uniffi::export]
impl Listing {
    fn version(&self) -> u64;
    fn len(&self) -> u32;
    fn state(&self) -> ListingState;                       // Loading{partial} | Complete | Error(e)
    fn rows(&self, start: u32, count: u32) -> Vec<RowInfo>;
    fn index_of(&self, id: EntryId) -> Option<u32>;
    fn path_at(&self, i: u32) -> String;
}

// ───────── Operation queue ─────────
#[uniffi::export]
impl OpQueue {
    fn enqueue(&self, op: OpRequest) -> Result<OpId>;      // Copy|Move|Trash|DeleteImmediately|NewFolder|Rename|Extract…
    fn plan(&self, op: OpRequest) -> Result<OpPlan>;       // dry run: counts, bytes, conflicts found up front
    fn list(&self) -> Vec<OpStatus>;                       // state, bytes, files, speed, ETA, current file, errors
    fn pause(&self, id: OpId); fn resume(&self, id: OpId); fn cancel(&self, id: OpId);
    fn reorder(&self, id: OpId, new_index: u32);
    fn resolve_conflict(&self, id: OpId, c: ConflictId, d: ConflictDecision, apply_to_all: bool);
        // KeepBoth | Replace(→ old one to Trash) | Skip | Merge(folders) ; Compare is a shell view over ConflictInfo
    fn interrupted(&self) -> Vec<OpStatus>;                // from the journal after a crash/quit
}

// ───────── Callbacks implemented by the shell ─────────
#[uniffi::export(with_foreign)]
pub trait EventSink: Send + Sync { fn on_event(&self, e: CoreEvent); }

#[uniffi::export(with_foreign)]
pub trait Platform: Send + Sync {          // OS calls the core needs but must not implement itself on macOS
    fn trash(&self, path: String) -> Result<String, PlatformError>;  // NSFileManager.trashItem → location in Trash
    fn spotlight(&self, q: SpotlightQuery) -> Vec<String>;           // M3
    fn cloud_status(&self, paths: Vec<String>) -> Vec<CloudStatus>;  // only if the spike shows Rust can't do it safely
}

pub enum CoreEvent {
    ListingChanged { tab: TabId, version: u64, kind: ChangeKind },   // Partial | Sorted | Patched | Error
    OpProgress { ops: Vec<OpStatus> },                               // coalesced
    OpConflict { op: OpId, conflict: ConflictInfo },
    OpFailed  { op: OpId, path: String, error: String },             // per file, never silent
    OpFinished { op: OpId, report: OpReport },
    UndoChanged { state: UndoState },
    VolumesChanged { volumes: Vec<VolumeInfo> },
    Hint { hint: Hint },
    SettingsChanged,
}
```

**Not in Swift, by construction:**
- whether a dropped file is copied or moved (same volume → move; ⌥ → copy; the core decides, from the modifiers and both paths)
- what happens on Return, Space or ⌘↓
- what selection follows a delete
- the order of rows
- what a pasted string means

---

## 5. Data model and data safety

### SQLite (`fstudio-db`, WAL mode, a single file in `~/Library/Application Support/File Studio/`)

| Table | Contents |
|---|---|
| `settings` | one JSON document plus a schema version; typed in Rust, with migrations |
| `frecency` | path, rank, last_access (zoxide's algorithm: rank += 1 per visit, ageing when the total exceeds a cap, score = rank × recency weight) |
| `session`, `workspaces` | serialised `WorkspaceState` |
| `ops`, `op_items` | the journal: one row per op and per item, with state `planned → in_progress → done/failed/skipped`, source, destination, partial name, bytes, hash, conflict decision, undo data |
| `undo` | stack of op ids plus inverse descriptions; survives relaunch, capped by a setting |
| `learned_shortcuts` | hint counts (Import Studio's D-030) |
| `folder_sizes` | path, size, count, mtime fingerprint (M3) |

### The rules, and how each is enforced

| Rule | Mechanism | Test |
|---|---|---|
| Never hard-delete by default | Trash goes through `Platform::trash`, which keeps Finder's Put Back. `delete_immediately()` is the only `remove_file`/`remove_dir_all` caller and is gated by `settings.allow_delete_immediately` (off). | grep-test for `remove_file` callers; integration test with the setting off |
| Never overwrite silently | Writes use a partial name plus `RENAME_EXCL`. A conflict pauses that item and emits `OpConflict`, unless a rule the user chose applies. "Replace" sends the old file to the Trash first. | tests for every decision × file/folder/symlink |
| Resumable | The journal is written before each step. On launch, `interrupted()` lists unfinished ops; partials are ours, so they are removed and their items redone. | kill-the-process test (fork, SIGKILL mid-copy, resume, compare trees) |
| Cross-volume move | Per item: copy → (verify) → fsync → remove the source. The source is removed only after the destination is `done`. Folder moves remove the source folder only once it is empty of copied items. | fault-injection test: a failure at each step leaves the source intact |
| Undo | Every op records its inverse. Copy → trash the copies. Move → move back. Trash → put back. Rename → rename back. New folder → trash it if it is still empty. Undo refuses, with a reason, if the world changed since (destination modified). | round-trip tests |
| Per-file errors | Errors are attached to items and never abort the batch unless you ask (setting: stop on first error). | permission-denied, full-disk (small hdiutil image) and read-only tests |

### Copy engine (pending the spike numbers)

- Same APFS volume: `clonefile`, or `copyfile(COPYFILE_CLONE)` as the fallback.
- Otherwise `copyfile(COPYFILE_ALL | COPYFILE_NOFOLLOW | …)`, which preserves xattrs, ACLs, tags, comments and resource forks.
- The status callback drives progress and pause/cancel. Sparse files use `COPYFILE_DATA_SPARSE`.
- The optional xxh3 verify re-reads the destination with `F_NOCACHE` after `F_FULLFSYNC`, as in Import Studio's D-011.
- Linux will use `copy_file_range`, `ioctl FICLONE` and xattr copies behind the same trait.
- Each volume gets its own I/O worker, so a card copy doesn't block an internal SSD copy. The number of parallel ops per volume is a setting (default 1 for spinning disks and cards, 2 for SSDs).

### macOS specifics

- **Packages:** treated as files by default; "Show Package Contents" is a command.
- **iCloud/File Provider:**
  - Listing uses only calls that the spike shows do not materialise dataless files.
  - The download state comes from `st_flags & SF_DATALESS` plus a resource key read through `Platform`, if needed.
  - Previews of dataless files show a placeholder with a Download button and never auto-fetch (a setting can change that).
- **Unicode and case:**
  - The core keeps names exactly as the filesystem returns them.
  - Comparisons, sorting and fuzzy matching use NFC-normalised, case-folded keys.
  - Collision checks ask the destination volume whether it is case-sensitive (`pathconf _PC_CASE_SENSITIVE`) and compare with NFD/NFC awareness.
- **Symlinks:** never followed by recursive ops unless asked. Loop detection uses `(dev, ino)` sets. Aliases are resolved through `Platform` on demand only.

---

## 6. Listing, watching and the < 50 ms first-rows budget

```
go(path) ─► fs worker: getattrlistbulk (or best from spike) in chunks of ~2k
             │  chunk 1 ─► view: sort chunk 1, publish Listing{Partial}  ─► ListingChanged(Partial)  (< 50 ms)
             │  … all chunks ─► view: full sort (parallel for > 20k), publish Listing{Sorted}, keep the cursor row
             └► watcher (FSEvents, per tab dir, file-level events, latency ~ 30 ms) ─► lstat changed names ─► patch
                 snapshot (insert/remove/update in O(log n)) ─► ListingChanged(Patched)
```

- **Two-phase metadata:** the cheap attributes come in the bulk call (name, type, size, dates, flags, inode). Expensive ones (Finder tags, kMDItem, image dimensions, git status) are fetched lazily for the visible range plus a margin, by priority.
- **Sorting** precomputes collation keys once per entry (natural numeric, case-folded, NFC) and sorts indices. *(Spike: 100k sort time.)*
- **Watcher:**
  - One FSEvents stream for the union of open tab directories, rebuilt as tabs change (the latency comes from the spike).
  - A `Watcher` platform trait (`fsevents` on macOS, `inotify` later) lives in `fstudio-fs`.
  - When events overflow or `MustScanSubDirs` fires, the core re-lists and diffs.

---

## 7. Path bar engine (`fstudio-path`): how it hits < 16 ms

1. **Parse.** The steps run in order:
   - trim the input and strip matching quotes
   - convert `file://` URLs (percent-decoded)
   - unescape backslashes (Terminal style)
   - expand `~` and `~user`
   - expand `$VAR` and `${VAR}`
   - resolve relative paths against the tab's cwd
   - normalise `.` and `..` lexically (with a later `realpath` check)

   Results are `Dir`, `File` (select it in its parent), `Missing` (offer "Create folder", plus the nearest existing ancestor), or `Remote` (later).
2. **Complete.** There are two sources, merged:
   - **Children of the typed parent.** The first time a parent is completed, its subdirectories are listed into an LRU cache keyed by `(path, mtime)`. Each keystroke after that matches in memory.
   - **Frecent folders:** an in-memory index of up to 10k frecent paths, loaded at start.

   Matching uses `nucleo-matcher` (MPL-2.0; Helix's fuzzy matcher, subsequence with smart case), scoring the last segment higher. The final score is the match × the frecency boost. Typing `gig` from anywhere ranks `~/Pictures/2026/2026-11-11_gig` first if it is frecent.

   Tab completes to the longest common prefix, then cycles, as a shell does. The result says which characters to highlight.
3. **Frecency** is recorded on every `go`, with a setting to exclude paths (for example `/Volumes/*` cards, temporary directories). It can be imported from zoxide's database once, if found (opt-in).
4. **Budget check:** `fstudio-bench` replays typed prefixes against a 10k-entry frecency index and a 100k-child folder and asserts that p99 stays under 16 ms.

**Shell side:** a custom AppKit view, not `NSPathControl`, which can't edit or drop at segments well enough.
- At rest it is a row of crumb buttons, each with an icon and a chevron menu.
- Clicking empty space, pressing ⌘L or pressing `/` swaps in an `NSTextField` with the text selected, using a 0.14 s cross-fade.
- Completions appear in a child panel.
- The view also handles spring-loaded drop targets per crumb, plus the ⌥-click and right-click menu.

---

## 8. Keymap, palette, hints, settings

- **`fstudio-keymap`:**
  - a `Command` enum
  - a registry with metadata
  - `Binding { chord(s), when: Context }` with context predicates: list focused, path bar editing, palette open, Vim normal mode
  - profiles: Standard (Finder-compatible: ⌘↑, ⌘↓, ⌘⌫, ⌘⇧N, ⌘⇧G, Space…) and Vim (hjkl, gg/G, `/`, `yy`/`p` as copy/paste via the queue, multi-key sequences with a timeout)
  - user overrides stored as a diff against the profile
  - conflict detection
- **Overlay and hints** are a straight port of Import Studio's `hints.rs` (D-029): `ShortcutOverlay` and `ShortcutHints` settings (corner, delay, frequency, stop when learned) and the learned counts saved in SQLite (D-030). `ShortcutToasts.swift` is copied and adapted.
- **Settings:**
  - One typed `Settings` struct, with sub-structs per Settings tab: General, Appearance, Views & Columns, Path Bar, Navigation, Operations & Safety, Shortcuts, Hints, Previews, Advanced.
  - Every feature gets on/off, timing, placement and frequency knobs, as the brief requires.
  - The defaults are the product decisions and are documented in the Settings help text.
  - The Swift Settings window is a SwiftUI `Settings` scene with tabs, copied from Import Studio's structure.
- **Command palette** (M3): `palette(query)` merges commands, frecent folders, recent items, open tabs and saved workspaces, all ranked by `nucleo` plus usage.

---

## 9. Benchmarks (`fstudio-bench`, `scripts/bench.sh` → `bench/history.md`)

**Synthetic fixtures** are generated into a temp directory, with a cache, never committed:
- flat folders of 1k, 10k and 100k entries
- a deep tree
- unicode and case twins
- sparse files
- a 5k-small-files folder
- 1 GB and 10 GB files

**Measured, with median and p95:**
- listing to first rows and to fully sorted
- re-sort
- filter-as-you-type per keystroke
- completion per keystroke
- frecency query
- watcher event to patched listing
- copy throughput against `cp` (clone, same-volume, cross-volume)
- verify throughput
- journal overhead per item
- FFI `rows()` call cost

The shell side (scroll frame times) is measured in M2 via `-FSDebugScript` plus signposts.

---

## 10. Shell essentials (M2), so the core API is shaped for them

- **Window:** native `NSToolbar` with a sidebar (`NSSplitViewController`), a tab bar per pane (custom, with a sliding selection pill, D-026 style), and the path bar under the toolbar (always visible, as the brief requires).
- **List view:** `NSTableView`, view-based with reused cells (or a cell-based fast path if the 120 Hz scroll test fails), and columns driven by `ViewOptions` from the core.
- **Column view:** a horizontal stack of `NSTableView`s, each bound to a core `Listing`.
- **Quick Look:** `QLPreviewPanel` data source in `Platform/`; Space is a core command that tells the shell to toggle it.
- **Ops UI:** a toolbar progress ring that opens a popover queue (pause, reorder, cancel), and a conflict sheet with side-by-side metadata and thumbnails.
- **First-run setup:** about three pages (Welcome → Full Disk Access with a deep link → keymap profile and the dual-pane default), skippable, with D-026 motion.
- **Debug automation:** `-FSDebugScript "key:cmd+l;type:~/Pic;key:tab;snap:pathbar;quit" -FSDebugSnapshotDir …` (port of `DebugAutomation.swift`, plus a `type:` step), and `-FSMockCore`.

---

## 11. Dependencies (initial; all GPL-3-compatible, `cargo-deny` enforced)

| Crate | Use | Licence |
|---|---|---|
| uniffi 0.32 | FFI | MPL-2.0 |
| rusqlite (bundled) | DB | MIT (SQLite: public domain) |
| nucleo-matcher | fuzzy completion/palette | MPL-2.0 |
| xxhash-rust (xxh3) | verify | BSL-1.0 |
| libc | sys modules only | MIT/Apache-2.0 |
| fsevent-sys *or* own FFI (from the spike) | watcher | MIT |
| unicode-normalization, icu_collator? | NFC keys / collation (decide after the spike) | MIT/Apache-2.0 / Unicode-3.0 |
| crossbeam-channel, rayon, parking_lot | threading | MIT/Apache-2.0 |
| serde, serde_json, thiserror, tracing | plumbing | MIT/Apache-2.0 |
| regex | filters, rename | MIT/Apache-2.0 |
| clap, anyhow | CLI | MIT/Apache-2.0 |
| tempfile (dev) | tests | MIT/Apache-2.0 |

There is no tokio: plain threads and channels, as in Import Studio, until remote filesystems (M5) need async I/O.

---

## 12. Milestone 1 plan and session split (after your OK)

| Session | Owns | Model | Why |
|---|---|---|---|
| Workspace skeleton (me, here) | root `Cargo.toml`, `fstudio-model`, `-db` scaffolding, `DECISIONS.md`, `DEPENDENCIES.md`, `deny.toml`, crate stubs with API signatures | Opus | sets the contracts every other session builds on |
| fs + view | `fstudio-fs`, `fstudio-view` (listing, watcher, sort, filter, type-select) | Sonnet | well specified by the spikes |
| path engine | `fstudio-path` (parse, complete, frecency) | Sonnet | well specified, test-heavy |
| ops queue | `fstudio-ops` (queue, copy/move/trash, journal, undo, conflicts) | **Opus** | data safety and concurrency |
| keymap + settings | `fstudio-keymap`, settings in `fstudio-core` | Sonnet | port of Import Studio's patterns |
| core facade + CLI + bench | `fstudio-core`, `fstudio-cli`, `fstudio-bench` | Opus (me), after the others merge | integration |
| review | each branch before merging | Opus (me) | the brief says to review, not trust |

The four crate sessions don't overlap: each owns its crate directory only. I own the root files and `fstudio-model`, and I merge everything.

---

## 13. Questions for you

1. **Bundle ID / name.** `app.filestudio.mac`, mirroring `app.importstudio.mac`. OK?
2. **Platforms.** macOS 26+ and Apple Silicon only (arm64), as in Import Studio's D-008?
3. **Default layout.** Single pane with tabs, and dual pane one keystroke away (F3 or ⌘⌥D), remembered per workspace? Or dual pane on by default?
4. **Default keymap.** Finder-compatible Standard, with Vim as an opt-in profile chosen in setup?
5. **zoxide import.** Offer once at first run to import your zoxide database to seed frecency, if one is found?
6. **Trash vs Put Back.** Use `NSFileManager.trashItem` through `Platform` (Finder's Put Back works) rather than moving into `.Trashes` ourselves (pure Rust, but Put Back would only work inside File Studio)? I recommend the former; Linux will use the freedesktop Trash spec in Rust.

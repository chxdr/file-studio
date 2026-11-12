# Safety audit: unintended file operations (main @ 46b20d4, 2026-11-12)

This is an independent audit, written after the sidebar-drag incident (F-023). It asks one question: **where can a user gesture cause a file operation the user didn't mean, or didn't see?**

It is a report only. No product code was changed.

## How the findings were proven

- **Core probes.** 15 throwaway integration tests use the `fstudio-core` test harness (temporary home, data folder and folder Trash). Their source is `safety-audit-probes.rs` in the shared side-session notes folder. Drop it into `crates/fstudio-core/tests/` to rerun:
  ```
  cargo test -p fstudio-core --test safety-audit-probes -- --nocapture
  ```
  They passed on `main`, which means every behaviour described below happens.
- **App run.** One run of the real Debug app used `scripts/run-debug.sh` with `HOME` set to a scratch folder of throwaway files, and a temporary `-FSDataDir`.

Each finding names the probe (pN) or app run that proves it. Findings with neither are from reading the code, and say so.

## Findings, most severe first

Severity scale: **data loss** > **silent move or overwrite** > **unconfirmed destructive action** > **missing feedback**.

---

### S1. Data loss: confirming a dialog acts on the selection at confirm time, not on what the dialog named

**Repro (p13):**
1. Turn on Delete Immediately.
2. Paste `thesis.pdf` into `work/`.
3. Straight away, click `junk.tmp` and press ⌥⌘⌫.
4. The sheet says *Delete "junk.tmp" immediately?*.
5. The paste lands while the sheet is up.
6. Click **Delete Immediately**.

**Result:** `work/` is empty. `thesis.pdf` was hard-deleted along with `junk.tmp`. The same happens to "Move to Trash?" (with *Ask before moving to the Trash* on, or Vim `x`, which always asks).

**Why:**
- `ConfirmSheets.swift:45` performs the command again with `.confirmed`, which carries no paths.
- `commands.rs:1131-1160` (`file.trash` / `file.delete_immediately`) then calls `need_selection(&t)` again, reading whatever is selected *now*.
- The paths shown in the dialog (`ShellRequest::ConfirmTrash { paths }` / `ConfirmDeleteImmediately { paths }`) are thrown away.
- Anything that changes the selection while the sheet is up changes what gets deleted. S4 is one such cause; watcher patches that remap rows are another.

**Fix:**
- Make the confirmation carry the paths: `CommandArg::ConfirmedPaths(Vec<PathBuf>)`, or a one-shot confirmation token that the core stores together with the exact `sel` it showed.
- On `Confirmed`, act **only** on those paths. Refuse with a message if any of them has changed identity (dev/ino) since the dialog was shown.
- Never call `need_selection` on the confirmed path.

---

### S2. Silent move: any file URL dropped on the list or a crumb is moved, with no protection and no feedback (incident class)

**Repro:**
- **App run:** `drop:$HOME/Downloads#Projects#-` in a scratch home. `~/Downloads` was moved into `Projects`. The status message was empty and no toast appeared (screenshot: list shows `Import Studio, Projects, …`; Downloads gone).
- **p6:** `drop_files([~/Downloads], ~/Import Studio)` returns an `Outcome` with no shell request and no message, and the move happens.

**Why:**
- `SidebarViewController.swift:312-318` writes every row's **file URL** (favourites, recents and volumes), and `:102` allows local `.move`.
- `FileTable.swift:125` and `CrumbStripView.swift:119` accept any `.fileURL`, then `FileTable.swift:661→630` (`dropItems`, `drop.rs:137`) and `PathBarView.swift:1082` (`drop_files`, `commands.rs:649`) queue a Move on the same volume.
- Neither core path knows about protected folders, and neither returns anything the shell shows. `FileTable.performDrop` only `print`s the op id (`FileTable.swift:631`).
- Volume rows have the same problem: dragging a USB volume's sidebar row onto the list is a cross-volume **Copy of the whole volume**, unconfirmed.

**Status:** `claude/m2-tour-favs-cd3d09` (commit ce48530) fixes the sidebar pasteboard, and adds `protect.rs`, `ConfirmMove` and the "Moved X to Y" toast. **Check when merging:**
- (a) `drop_items` (list) and `drop_files` (crumbs) **both** go through `protect.rs`. The handover says `drop_items` "refuses", so confirm that a refusal is shown to the user and isn't just a beep.
- (b) Volume and Recents rows are covered by the private-type rule too, not only favourites.
- (c) A drop from Finder of a protected folder (for example `~/Documents` dragged from a Finder window) also asks.

---

### S3. Silent move or trash: Redo replays the original request by path, with no identity check, even after a relaunch

**Repro (p1):**
1. Move `~/Downloads` into `Projects`, then press ⌘Z (it is put back).
2. Quit and relaunch.
3. The Edit menu still says **Redo Move of "Downloads"**.
4. Press ⇧⌘Z (or Vim ⌃R). `~/Downloads` is moved into `Projects` again, with no question.

In other words: undoing tonight's incident leaves a one-keystroke way to repeat it, which survives a restart.

**Repro (p2):**
1. Trash `Report.txt`, then ⌘Z.
2. Rename it to `Report-old.txt` and save new work as `Report.txt`.
3. Relaunch, then ⇧⌘Z. The **new** `Report.txt` is trashed.

**Why:**
- `fstudio-ops/src/queue.rs:698-713` (`redo`) re-enqueues `row.request.clone()`, which is paths only.
- The undo/redo stacks live in the SQLite journal, and nothing clears them on `OpQueue::open` (`queue.rs:201-224`).
- `undo.rs` checks identities, but redo checks nothing.

**Fix:**
- Redo must be the inverse of the undo op, checked the same way undo is. Record the dev/ino of each item the undo put back. On redo, refuse ("“Report.txt” changed since it was put back") unless every source is still that object.
- Clear the redo stack at launch, or at least refuse redo across launches.
- Redo of a move must go through the F-023 protected-folder confirmation like any other move.

---

### S4. Unintended target: a pending "select what we just pasted" is merged into the user's later selection

**Repro (p5):**
1. ⌘V a 4 MB file into `work/`.
2. Immediately click `mine.txt`.
3. When the copy lands, the selection becomes `["mine.txt", "pasted.bin"]`.
4. ⌘⌫ trashes both.

The same happens with F6, ⌘X/⌘V, ⌘D and right-click → Move to Trash. Context-menu items for File and Multi targets carry no paths, so they run on the selection at the moment of the click. S1 is the worst consequence.

**Why:**
- `paste_into` / F5 / F6 / New Folder call `select_when_present` (`tab.rs:1098-1117`), which parks `pending` names for up to 5 s, or until loading is final.
- `publish` (`tab.rs:659-679`) then **toggles them into the current selection** (`sel.toggle(i)`) instead of replacing it.
- `set_selection` (`tab.rs:938`) never clears `pending`, so a user click doesn't cancel the program's wish.

**Fix:**
- Any user selection change (`set_selection`, type-select, cursor moves, `menu_target_at`) clears `st.pending` and `pending_until`.
- When the pending selection is applied, it replaces the selection only if the selection hasn't changed since the op was queued (keep a selection generation number). It never merges.

---

### S5. Unconfirmed destructive action: ghost rows after a case-only or NFC↔NFD rename; acting on the ghost hits the real file

**Repro:**
- **App run:** rename `Notes.txt` to `NOTES.txt`. The list shows **both** `NOTES.txt` and `Notes.txt` (`LIST row 3 NOTES.txt`, `row 4 Notes.txt`), and the screenshot shows 4 items when the disk has 3.
- **p12:** for `Notes.txt → NOTES.txt` and `Cafe\u301.txt → Café.txt`, the ghost is still there after 15 s. Only a refresh removes it.
- **p10:** the user sees an apparent duplicate and trashes the "old" one (the ghost). The real, renamed file goes to the Trash, and the list then still shows both rows.

**Why:**
- The FSEvents patch path describes an event path with `stat_entry` (`fstudio-fs/src/list.rs:269-277`), which takes the entry **name from the path**, not from the disk.
- On a case- and normalisation-insensitive APFS volume, `lstat("…/Notes.txt")` succeeds after the rename, so the old name is upserted as a live entry (see also `watch.rs:134-143`, which decides Created vs Removed with the same `lstat`).
- The ghost's path then resolves to the real file in every op.

**Fix:**
- When patching, get the real on-disk name (`getattrlist` `ATTR_CMN_NAME`, or `fcntl(F_GETPATH)` on an `O_SYMLINK` open). If it differs from the event's name, treat the event name as **removed** and the real name as created.
- Also dedupe by (dev, ino) inside a listing: two rows can never be the same object.
- As a guard in ops: before acting, `rename_target`/`need_selection` could check that the selected name matches the on-disk name exactly.

---

### S6. Unconfirmed destructive action: holding ⌘⌫ (or F6) walks down the list

**Repro (p15):** select `a1` and deliver ⌘⌫ three times, as key auto-repeat does. `a1`, `a2` and `a3` are trashed. After each trash, the core selects the next row, and the repeat acts on it. F6 (move to the other pane) behaves the same way (`commands.rs:1181`), as do ⌘D, F7 and ⇧⌘N, which make one item per repeat.

**Why:**
- No key path looks at `NSEvent.isARepeat`: not `KeyChord+NSEvent.swift:9-49`, not `KeyRouter.swift:51-55`, and not the core keymap.
- `select_after_removing` (`commands.rs:1143/1159/1181`) moves the selection to a new item immediately, before the op even runs.

**Fix:** carry `is_repeat` in `KeyChord` (or `InputContext`). The keymap ignores repeats for every command except cursor movement and scrolling. The cheapest version: in `KeyRouter.route`, drop `e.isARepeat` events unless the core says the command is repeatable.

---

### S7. Silent move: Paste and ⌥⌘V use a stale in-app clipboard; the system pasteboard is ignored

**Repro (p14):**
1. ⌘C `a/old-copied.txt` in File Studio.
2. Later, copy other files in Finder.
3. Go to `b/` in File Studio and press ⌥⌘V ("Move Item Here").
4. `old-copied.txt` is **moved** from `a/` to `b/`. ⌘V would paste the stale item too.

Also, a ⌘X "cut" never expires. A ⌘V hours later in any window moves the items.

**Why:**
- `paste_into` (`commands.rs:667`) reads only `shared.clipboard`.
- The shell never calls `setClipboard`/`Pasteboard.readFiles` (they are unused; `Pasteboard.swift:23`).
- Meanwhile the menus enable Paste from the **system** pasteboard (`FileTable.swift:703`, `menus.rs:824`, `can_paste = core clipboard || pasteboard_has_files`). So the menu says "Paste" because of Finder's files, then pastes something else.

**Fix:**
- On paste, the shell passes the system pasteboard's file URLs and `changeCount`.
- The core uses its own clipboard only if the pasteboard's `changeCount` is the one recorded when File Studio wrote it (`WriteFilePasteboard`). Otherwise it uses the pasteboard's files, always as a copy.
- `file.move_here` / a cut-paste of a protected folder goes through F-023 confirmation and shows the toast.
- Consider expiring a cut when the system pasteboard changes.

---

### S8. Silent move (code reading): the list drop target is recomputed at mouse-up from a row index

**Repro (not automatable):** scripts can't run a real `NSDraggingSession`. The steps would be:
1. Sort by Date Modified, in a folder where a copy is running.
2. Drag a file over folder row 5 (highlighted).
3. A new item lands above it, and row 5 is now a different folder.
4. Release before the next `validateDrop`.

**Why:**
- `tableView(_:acceptDrop:row:)` (`FileTable.swift:661-664`) re-derives the destination from `row` against the *current* rows (`self.row(row)?.kind`, then `s.pathAt(row)` in `dropDestination`, `FileTable.swift:609-613`).
- What was highlighted at the last `validateDrop` (`:644-659`) isn't stored. F-023 requires "the drop target is exactly what's highlighted at mouse-up".
- Separately, the spring-load timer (`armSpring`, `:672-688`) is only reset from `validateDrop`/`acceptDrop`/`endedAt`, with no `draggingExited`. A drag that hovers a folder row and then leaves the table (to the sidebar, crumbs or the other pane) still fires `nav.open` 0.8 s later. The pane navigates underneath a drag that is now somewhere else.

**Fix:**
- In `validateDrop`, store `(dest path, listing version)`. In `acceptDrop`, drop onto that stored path, and refuse if the listing version has changed since and the path is no longer at that row.
- Cancel `springTimer` when the drag leaves the table (an `NSTableView` subclass `draggingExited`, or check `info.draggingLocation` in the timer).
- The tour-favs branch says it "drops only on the folder highlighted at validate time" for FileTable and the crumb strip. Verify that this covers the listing-version case.

---

### S9. Undo moves something unexpected: undoing a Copy or Duplicate trashes a folder that has had work added since

**Repro (p3):**
1. Copy folder `Site` into `dst/`.
2. Save `dst/Site/new-work.txt`.
3. ⌘Z (intending to undo something else, say).
4. `dst/Site` goes to the Trash **with `new-work.txt`**, with no message.

**Why:** `undo.rs` `check_dest` only checks `same_object` (dev/ino) for folders. A folder's inode doesn't change when files are added. New Folder's undo checks that the folder is empty (`undo.rs:139-146`); Copy and Duplicate don't.

**Fix:** for folder copies, refuse (or ask) when the folder has entries the op didn't create. Compare the folder's child count, or the max mtime, with the journal's items. Or, more simply, refuse if the folder's mtime changed after the op finished.

---

### S10. Undo moves something unexpected: undoing a Trash puts back whatever now has that Trash name

**Repro (p4):**
1. Trash `x/a.txt` in File Studio.
2. Empty the Trash in Finder.
3. Trash a different `a.txt` from Finder.
4. ⌘Z in File Studio. The *other* `a.txt` is put into `x/`.

(Shown with the folder Trash; the system Trash reuses freed names the same way. Not run against the real Trash.)

**Why:** `undo.rs:118-130` checks only `meta::exists(loc)`. No dev/ino was recorded for the trashed item.

**Fix:** record the item's identity when it is trashed (`trash_loc` + dev/ino), and refuse "no longer in the Trash" when the object at `loc` is a different one.

---

### S11. Missing feedback: no move, trash, undo or redo shows anything

**Repro:**
- **App run:** a list drop that moved `Downloads` left `status:""` and no toast.
- **Code:** `ToastCenter.show` is only called for settings and op errors (`OpsModel.swift:138`). `AppModel.handle` (`AppModel.swift:313-319`) shows only `out.message`. Drops, F6, cut/paste, ⌘⌫, ⌘Z and ⇧⌘Z return no message.
- Undo and redo are app-global (one journal stack). ⌘Z in window B silently undoes an op from window A in a folder that isn't on screen.

**Fix:** F-023 says every move shows a toast with Undo. Extend that to Trash, Undo and Redo ("Put back 3 items", "Redid: Move of “Downloads”"), and name the folder when it isn't visible in this window. The tour-favs branch adds the move toast; Trash, Undo and Redo still need it.

---

### S12. Missing feedback: New Folder with Selection hides its move

**Repro (p7):**
- The `Outcome` has only the New Folder op (`ops=[OpId(1)]`).
- The Move is queued from a background thread (`commands.rs:1102-1126`), so the shell never learns of it, and on `main` it bypasses any move protection.
- Undo takes two steps. After the first ⌘Z ("Undo Move of “a”"), the empty folder is left behind.

**Fix:** queue it as one compound op (NewFolder then Move in a single journal op, with one undo entry), and return its id. Or at least return both ids and run the Move through the same protection as F6.

---

### S13. Missing feedback: "Create Folder" in the address bar

**Repro (p8 and code):**
- Typing a mistyped `~/Documnets/Projects/2026` and pressing Return shows the offer, with **Create 3 Folders** pre-highlighted (`PathBarView.swift:595-597`).
- A second Return, or a held Return (no `isARepeat` filter, see S6), creates all three.
- ⌘Return creates them **at once**, without first showing the offer (`PathBarView.swift:679`).
- Undo removes only the last folder per step ("Undo New Folder").

This is not destructive, but a typo plus a double Return litters the disk.

**Fix:**
- Highlight "Go to Nearest Folder" by default when more than one folder would be created.
- Make ⌘Return with an offer show the offer rather than create.
- Journal the chain as one op.

---

### S14. Lower-severity rename notes (p9, p11)

**Checked and fine:**
- `/` is rejected.
- `:` is allowed (APFS stores it; Finder shows `/`).
- A case-only rename lands correctly on disk; the only bug is the ghost row (S5).
- Renaming onto another item's name (`a.txt → Taken.txt` when `taken.txt` exists) stops at a conflict (`WaitingForDecision`), so it never overwrites.

**Not fine (missing feedback):**
- Renaming to a leading-dot name hides the item from the list with no warning. Finder warns.
- Changing the extension has no warning. Finder asks.
- Undoing a move whose original parent folder was deleted silently recreates the parent folder tree with `create_dir_all` (`fstudio-ops/src/engine.rs:347-349`). That happens outside the journal, so it can't be undone.

## Entry points: gesture → Swift → core → op

C = confirmed first, V = visible feedback (toast, message or progress), U = undoable. "Ring" means the toolbar op ring only, which is invisible for a fast same-volume move.

| Gesture | Swift (file:line) | Core call | Op | C | V | U |
|---|---|---|---|---|---|---|
| Drop file URLs on the list or a folder row (any source, sidebar rows included on main) | `List/FileTable.swift:644` validate, `:661` accept → `:630` | `drop_items` `drop.rs:137` | Move (same volume) / Copy | no | no (print only) | yes |
| Drop on a column-view list | same FileTable (Miller source) | `drop_items` | Move/Copy | no | no | yes |
| Drop on an address-bar crumb or chevron | `PathBar/CrumbStripView.swift:384` → `PathBarView.swift:1082` | `drop_files` `commands.rs:649` | Move/Copy | no | message only on error | yes |
| Drop on the sidebar | `Sidebar/SidebarViewController.swift:340` | `add_favourite`/`move_favourite` | none (favourites only) | – | – | – |
| Drop on the tab bar | `Window/PaneTabBar.swift:41,51` (plain text, tab ids) | `move_tab` | none | – | – | – |
| Drag source: list rows | `FileTable.swift:589-605` (file URLs, local move) | – | – | – | – | – |
| Drag source: sidebar rows | `SidebarViewController.swift:312-318` (**file URLs** + private type) | – | causes S2 | – | – | – |
| ⌘⌫ / F8 / toolbar trash / menu | `KeyRouter.swift:51` / `WindowToolbar.swift:197` / `AppDelegate.swift:82` | `file.trash` `commands.rs:1131` | Trash | only if setting on | no | yes |
| Vim `x` | KeyRouter | `file.trash` + `Confirm` | Trash | yes (but see S1) | no | yes |
| ⌥⌘⌫ | KeyRouter / menu | `file.delete_immediately` `:1146` | DeleteImmediately | setting gate + sheet, Cancel default (see S1) | no | **no** |
| Confirm-sheet "yes" | `Ops/ConfirmSheets.swift:45` | same command + `.confirmed` | re-reads selection (S1) | – | – | – |
| F6 / menu Move to Other Pane | KeyRouter (ListFocused + dual) | `:1167` | Move | no | no | yes |
| F5 | KeyRouter | `:1167` | Copy | no | ring | yes |
| ⌘X then ⌘V; Vim `dd` / `p` | KeyRouter; `FileTable.swift:64` (menu paste) | `edit.cut` `:1219`, `paste_into` `:667` | Move | no | no | yes |
| ⌘C then ⌘V | as above | `paste_into` | Copy / Duplicate (same folder) | no | ring | yes |
| ⌥⌘V (Move Item Here) | KeyRouter / menu | `file.move_here` `:1191` | Move (stale clipboard, S7) | no | no | yes |
| ⌘D | KeyRouter | `:1163` | Duplicate | no | ring | yes |
| ⇧⌘N / F7, ⌥⇧⌘N | KeyRouter / toolbar | `:1072` | NewFolder / NewFile | no | in-place rename | yes |
| ⌃⌘N (New Folder with Selection) | KeyRouter / Multi menu | `:1102` | NewFolder + hidden Move (S12) | no | no | 2 steps |
| In-place rename: Return, type, Return / click away | `List/RenameController.swift:108` | `rename_item` `:558` | Rename | conflict sheet if taken | no | yes |
| Rename dialog (⇧F6) | `ShellRequest::RenameDialog` | – | not built on main | – | – | – |
| ⌘Z / Vim `u` | KeyRouter / menu; debug `DebugSteps.swift:81` | `engine.undo` `engine.rs:335` | inverse ops (S9, S10) | no | message only on refusal | – |
| ⇧⌘Z / Vim ⌃R | KeyRouter / menu; debug `DebugSteps.swift:82` | `engine.redo` `:354` | replay by path (S3) | no | no | yes |
| Address bar: Return on a missing path, then Return; or ⌘Return | `PathBar/PathBarView.swift:679,830,905-908` | `create_folder_and_go` `commands.rs:574` | NewFolder × N | the offer (pre-selected) | navigates | per folder |
| Context menus (file / multi / empty space) | `Platform/ContextMenuBuilder.swift:37` | `perform(item.invocation)` | as the command | as the command | as the command | as the command |
| Crumb context menu | `PathBarView.swift:709` | `perform` with a `Path` arg | no file ops by default | – | – | – |
| Ops queue: resolve conflict (Replace) | `Ops/ConflictSheet.swift` → `resolveConflict` | `fstudio-ops` | old item to the Trash | yes (sheet) | yes | yes |
| Debug only: `op:copy` | `Views/DebugSteps.swift:38` | `enqueue` | Copy | – | – | yes |
| Direct `std::fs` writes in core/ffi | only `engine.rs:111` (data dir) and `drop.rs`/`probe.rs` metadata reads; ops' `engine.rs:348` `create_dir_all` (S14) | – | – | – | – | – |

## Things checked and found safe

- **Keyboard focus:** every file command is bound `ListFocused` (`registry.rs:56`). F5 and F6 also need dual pane.
- **Which tab a command hits:** `KeyRouter.context` takes the tab from the focused list's `coreTab`. A list in the inactive pane that has first responder acts on *its own* tab, and F6 then targets the other pane. Menu commands with focus outside a list fall back to `active_tab()` of the key window, as Finder does.
- **Vim:** `dd` only marks for cut, and `x` always asks.
- **Delete Immediately:** off by default, and its sheet defaults to Cancel.
- **Right-click:** follows Finder's rule (`menu_target_at` selects the clicked row when it wasn't selected). The crumb and sidebar menus pass a `Path` argument and contain no file ops by default. Note that a user can *add* `file.trash` to the crumb or sidebar menu in Settings, and `file.trash` ignores the `Path` argument, so it would trash the list selection. Restrict `menu_choices` per kind, or make every file command honour `Path`.
- **Drop decisions:** `decide()` refuses drops onto the item itself, into its own subfolder or its own folder, and onto unwritable or non-folder destinations. ⌥ beats ⌘.
- **Tab drags:** these carry a private plain-text id, not file URLs.
- **Undo of a Move:** refuses when the original place is taken. Undo of a New Folder refuses when the folder isn't empty.

## What I couldn't check

- **Real AppKit drag sessions.** Scripts can't start an `NSDraggingSession`, so S8 (row shift during a drag, spring-loading while scrolling) comes from reading the code. The `drop:` step calls the same `performDrop` as `acceptDrop`.
- **The real system Trash.** S10 was shown with the folder Trash.
- **External volumes and cross-volume moves**, **iCloud placeholders**, and **case-sensitive volumes**. On a case-sensitive volume, S5 shouldn't occur.
- **The F-023 fix branch** (`claude/m2-tour-favs-cd3d09`). The audit is of `main`. S2 lists what to check in that branch.
- **Multi-window races** beyond reading the code (for example a sheet in window A while window B changes the same folder).

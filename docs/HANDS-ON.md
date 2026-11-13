# Hands-on trials: address bar and keyboard model

Date: 2026-11-12. Scope: only the address bar / path display and the keyboard model, because milestones 1 and 2 lock them. This fills the QSpace Pro and Nimble Commander gaps in `docs/RESEARCH.md` (§1, §3.1, §3.4, §5).

Apps tried (macOS 26, Apple Silicon, all installed to `~/Applications`): QSpace Pro 7.0.4 (trial), ForkLift 4.7.7 (trial), Marta 0.8.2, Nimble Commander 1.9.0, with Finder as baseline.

## How to trust this document

- Every cell below is something I saw on screen unless it says **(not verified)**.
- Input was synthetic typing into the path fields, not a real ⌘V paste. Apps that react to paste events (as opposed to text content) could behave differently.
- Sample sizes are tiny: one pass per behaviour, three tries only for QSpace's 10k timing.
- Popup menus opened from segment chevrons and ForkLift's/Marta's drop targets did not render in my screenshots, so **chevron menus and drag-onto-segment are not verified** for any app. The right-click menus I could capture are listed.
- Not examined: rebinding UI of QSpace, ForkLift and Marta (Marta's and Nimble's are described from docs-in-app and prefs), Vim modes (none seen anywhere), Quick Look behaviour, copy/move execution.
- Things that happened and that you should know about:
  - **Marta crashed twice** (SIGTRAP, 0.8.2) while I typed a `file://…` URL into its Go to Path box. Diagnostic reports: `~/Library/Logs/DiagnosticReports/Marta-2026-11-12-181532.ips` and `…181609.ips`. I clicked Ignore (not Report) on both macOS crash dialogs.
  - **Nimble Commander** triggered a macOS "would like to access files on a network volume" prompt after I fed it a path beginning with a quote character. I clicked **Don't Allow**. Cause unconfirmed.
  - ForkLift's welcome dialog and QSpace's privacy/activation screens were clicked through by you, not me. QSpace shows a "Buy" button in the sidebar while on trial.
  - macOS showed Finder's *Recent* view (your own files) when I activated Finder; I did not look further and navigated straight to the test tree.

## Test tree

Under the session scratchpad: `testtree/flat10k` (10,000 empty files), `Pictures/2026/2026-11-11_gig/raw/IMG_0001.jpg`, `docs with spaces/it's "quoted"/`, `unicode/` (Café, Zoë résumé, 日本語, each in NFC and NFD), a symlink `link_to_gig`, and `otherpane/`. Marta showed the symlink as a 29-byte file-like row; I did not test symlink semantics further.

## 1. Address bar / path display

| | Finder | QSpace Pro | ForkLift 4 | Marta | Nimble Commander |
|---|---|---|---|---|---|
| Always visible? | No: optional path bar at window bottom (it was on in the window I saw) | **Yes**, breadcrumb bar on top of each of 4 default panes | Pane header: folder name + item count; a breadcrumb row showed after deep navigation | **Yes**, breadcrumb on top of each pane | Plain-text path as panel title |
| Form | Breadcrumb, read-only | Breadcrumb **and** text field | Breadcrumb in header; text entry is a separate popup | Breadcrumb; text entry is a separate modal | Text only, not interactive (one click and one right-click did nothing) |
| Enter edit | ⇧⌘G sheet | **Click empty bar space, or ⌘L** | ⇧⌘G popup | ⌘G modal "Go to Path"; clicking empty bar space did nothing | ⇧⌘G modal "Go to the folder" |
| What is selected on entry | n/a | **Whole current path** (both ways) | Field empty (list shows subfolders) | **Whole current path** | Field empty |
| Leave edit | Esc/Cancel | **Esc: back to breadcrumb, nothing navigated** | Esc closes popup | Cancel or Esc | Cancel or Esc |
| Return on a folder | Goes there | Goes there | Goes to the highlighted result | Goes there | Goes there |
| Return on a file path | (not tested) | **Opens parent, selects the file** | (not tested) | **Opens parent, selects the file** | Inline error "Not a directory"; dialog stays |
| Non-existent path | Dialog stays, nothing happens | **Navigates anyway**: pane shows "Can't find the location" + Choose Location; bad path stays in breadcrumb | Empty result list | **Go button greys out**, Return does nothing, no message | Inline "No such file or directory"; dialog stays |
| Create-folder offer | No | No | No | No | No |
| Long paths | Truncates middle in bottom bar | Middle segments shrink to bare ▸ markers (names hidden), `t…` stubs | Middle segments collapse to `…`, later to initials (`p… t… cl…`) | Clips the **left** end silently | Left-truncates with `…` |

Segment click behaviour I saw: clicking a Marta segment (or its separator) goes there and **selects the child you came from**; a QSpace segment click also navigates.

The screenshots taken during these trials were kept out of the repository: they show other companies' apps.

## 2. Completion

| | Finder | QSpace Pro | ForkLift 4 | Marta | Nimble Commander |
|---|---|---|---|---|---|
| Kind | Spotlight-style folder list in the ⇧⌘G sheet **(not tested in depth)** | **Prefix**, per segment, dropdown under the bar | **Prefix on subfolders + history**, results shown as full breadcrumb (`Macintosh HD ▸ Users ▸ milky ▸ Pictures`) | None in the Go to Path box. A separate ⌘P "Lookup" searches via Spotlight (global, not path-based) | None seen in the Go dialog |
| Tab | n/a | **Completes the next segment, appends `/`, shows the child list** (list included a `.` row) | not tested | n/a | n/a |
| ↑/↓ | n/a | not tested | Moves the highlight (first row preselected) | n/a | n/a |
| Esc | Cancel | Leaves edit, discards | Closes popup | Cancels | Cancels |
| Recents/frecency | No | **No in the bar**; Go ▸ Recent Locations submenu | Recents in sidebar auto-filled; Go ▸ Recent Folders; Quick Open (history). Not frecency-ranked that I could see | Favourites (⌥2), no frecency | Go ▸ Frequently Visited submenu (the only frecency-like feature I found, not in the bar) |
| Fuzzy / subsequence | No | No | No (`pict` matched, `pictx` returned nothing) | No | No |

**Finding: none of the four does fuzzy/subsequence matching or frecency inside the address bar.** The brief's differentiator is open ground. `gig`→`2026-11-11_gig` from anywhere is not something any of them does.

## 3. Paste (typed into the path field; see caveat above)

| Input form | Finder | QSpace Pro | ForkLift 4 | Marta | Nimble Commander |
|---|---|---|---|---|---|
| `"…/docs with spaces"` (quoted) | **Fails** | **Works** | No results | Fails (Go greys out) | **Works** (quotes stripped) |
| `…/docs\ with\ spaces` (Terminal-escaped) | **Works** (unescapes) | **Works** | No results | Fails | Fails ("No such file") |
| `file:///…/docs%20with%20spaces` | **Works** | **Works** | No results | **Crashed the app (2/2)** | Fails |
| `~/Library` | (not tested) | (not tested) | no completion; Return not tried | **Accepted** (Go enabled) | accepted by the button; navigation not verified |
| Plain path with spaces | Works | Works | Works, one result row | Works | Works |

QSpace is the only one that handles all three shell-style forms. Nobody does a visible "cleaned up your paste" hint.

## 4. Segment interactions

| | QSpace Pro | ForkLift 4 | Marta | Nimble |
|---|---|---|---|---|
| Right-click segment | Open in New Window / New Tab, Quick View, Get Info, Open in Terminal, **Copy Filename, Copy Path**, Show in Finder, Services | menu did not render in my capture **(not verified)**. File ▸ Copy Path is a submenu (variants exist) | Open, Open in New Tab, **Copy Path**, Add to Favorites | none (no segments) |
| Chevron/▸ menus | ▸ markers present; popup not captured **(not verified)** | not verified | **None**: separators are plain | none |
| ⌥-click variants | not tested | not tested | not tested | n/a |
| Drop on segment | not tested | not tested | not tested | n/a |

None of the apps offers POSIX / escaped / `~`-relative copy variants from the segment that I could see. Marta's palette has `Copy Paths ⌘F12` and `Copy Folder Path F12`; Nimble has Copy Item Path ⌥⌘C, Copy Item Directory ⌥⇧⌘C, Copy Item Name ⇧⌘C.

## 5. Keyboard model

### Default bindings observed (from menus, palettes and Marta's tutorial)

| Action | Finder | QSpace | ForkLift | Marta | Nimble |
|---|---|---|---|---|---|
| Open | ⌘↓ / ⌘O | ⌘O | ⌘↓ | **Return** | **Return** (⌘↓ enter) |
| Up | ⌘↑ | ⌘↑ | ⌘↑ | **⌫** | ⌘↑ |
| Back / forward | ⌘[ ⌘] | ⌘[ ⌘] | ⌘[ ⌘] | (not read) | ⌘[ ⌘] |
| Rename | Return | menu item, no key shown | **Return** | ⇧F6 | ⌃F6 in place; F6 rename/move dialog |
| New folder | ⇧⌘N | ⇧⌘N | ⇧⌘N | F7 | ⇧⌘N, F7 |
| Trash | ⌘⌫ | ⌘⌫ | ⌘⌫ | ⇧F8 (delete) | ⌘⌫; F8 delete |
| Copy / move to other pane | n/a | menu "Copy to Pane / Move to Pane" submenus, **no keys shown** | **F5 / F6** | **F5 / F6** | **F5 / F6** |
| Switch pane | n/a | (not tested) | (not tested) | **Tab** | (not tested; F1/F2 open left/right location menus) |
| New tab | ⌘T | (not read) | ⌘T | ⌘T | ⌘T |
| Quick Look | Space / ⌘Y | ⌘Y | ⌘Y | **Space** | ⌘Y (F3 viewer) |
| Show hidden | ⇧⌘. | (not read) | (not read) | ⇧⌘. per palette (hard to read) | hidden files were shown by default in home |
| Go to folder | ⇧⌘G | **⌘L** (edit bar) | ⇧⌘G | ⌘G | ⇧⌘G |
| Palette | none | Quick Launch menu (not captured) | none | **⇧⌘P Actions panel** (fuzzy, shows each action's key) | none |
| Info / duplicate | ⌘I / ⌘D | ⌘I / ⌘D | ⌘I / ⌘D | (not read) | ⌘D |

### Behaviour

| | Finder | QSpace | ForkLift | Marta | Nimble |
|---|---|---|---|---|---|
| Type-to-select | Yes | **Not observed** (typed `file_0123` in a date-sorted list; selection did not move) | **Yes** (jumped to `file_0123x`) | **Yes**: "Quick Search"; `\:` prefix = substring, `/:` = regex, Esc cancels | **No: it filters.** Overlay shows query and match count (`10`), underlines matches, Esc clears and keeps the selection |
| Vim mode | No | none seen | none seen | none seen | none seen |
| Rebinding | none | not examined | not examined | Text file `conf.marco`; "Open default configuration" shows every action; docs lag (RESEARCH §1) | Preferences ▸ Hotkeys: filter box, Action / Primary / **Secondary** columns, **Customized** and **Conflicts** tabs, "Always use Fn buttons as F1…F19" |
| Discoverability | Menus | Menus | Menus + toolbar tooltips | **Bottom F-key bar**, palette lists keys, first-run tutorial that teaches ⌘⇧P | Menus show keys; F-key bar; Hotkeys prefs is a searchable full list |

Other keyboard facts: Marta's ⌘P "Lookup" is a Spotlight query over your whole Mac (it surfaced a file name from outside the test tree on the first keystrokes), so it is a global finder rather than a go-to-folder; avoid that surprise in File Studio.

## 6. Rough timing: showing the 10k flat folder

By stopwatch-level I mean: Return, then repeated screenshots about 0.5 s apart. Resolution is about ±1 s. These are not benchmarks (a separate session owns those).

| App | Result | Tries |
|---|---|---|
| Nimble Commander | Listing present on the first screenshot, **under ~1 s**; brief (multi-column) view | 1 |
| Marta | Under ~1.5 s; dialog dismissal included | 1 |
| ForkLift | Still the old view on the first screenshot, listed within ~2 s | 1 |
| Finder (icon view) | Old view still showing at ~2.5 s, listed by ~3 s | 1 |
| QSpace Pro | Spinner, then rows at **~3 to 4.5 s**, consistent | **3** |

Treat the ordering (Nimble < Marta < ForkLift < Finder ≈ QSpace) as indicative only. Note the default sort differs (QSpace opened date-descending).

## 7. Per-app: steal this / avoid this

### QSpace Pro
- **Steal:** click empty bar space or ⌘L → edit with the **entire path selected**; this is exactly the brief's behaviour.
- **Steal:** Tab appends `/` and shows the child list immediately.
- **Steal:** quoted, escaped and `file://` input all just work. Paste cleanup is table stakes.
- **Steal:** Return on a file path opens the parent and selects the file.
- **Avoid:** a non-existent path navigates into an error pane and leaves the dead path in the breadcrumb. That is where the "Create folder" offer belongs.
- **Avoid:** long paths turn into anonymous ▸ markers. Hidden segments must stay reachable by name.
- **Avoid:** 3 to 4 s to show 10k files, and a type-to-select that did nothing for me.

### ForkLift 4
- **Steal:** results show the **full breadcrumb** of each candidate (`Macintosh HD ▸ Users ▸ milky ▸ Pictures`), which disambiguates same-named folders.
- **Steal:** Return renames (Finder-compatible); F5/F6 are copy/move; sidebar Recent Folders fill automatically.
- **Avoid:** the address bar is not editable in place, and the header shows only a name until you go deep.
- **Avoid:** prefix-only matching, no recovery of quoted/escaped/`file://` paste, no result at all for a typo (no hint).
- **Avoid:** onboarding dialog with iCloud sync pre-ticked and a newsletter field.

### Marta
- **Steal:** Actions palette (⇧⌘P) where every row shows its key; it doubles as a cheat sheet. Same for the bottom F-key bar and the first-run tutorial.
- **Steal:** clicking a segment selects the child you came from; right-click segment has Copy Path and Add to Favorites.
- **Steal:** Quick Search modes (prefix, `\:` substring, `/:` regex).
- **Avoid:** edit box is a modal with no completion, Go greys out silently on bad input (no message), no quote/escape cleanup, and **it crashes on `file://`**: validate in fuzz tests.
- **Avoid:** hand-edited `conf.marco` as the only rebinding route; clipped left end of long paths with no indicator.

### Nimble Commander
- **Steal:** Hotkeys preferences: searchable table, primary + secondary key, a **Conflicts** tab and a **Customized** tab.
- **Steal:** inline error text in the box (`No such file or directory` / `Not a directory`) instead of silently doing nothing.
- **Steal:** quote-stripping on paste; type-to-filter overlay with live match count and Esc-to-clear that keeps the selection (offer as a mode next to type-to-select).
- **Avoid:** path is a dead text title; the modal opens empty (you lose the current path as a starting point); no completion.
- **Avoid:** a leading quote seems to have been treated as a path to resolve (network volume prompt); parse and strip before touching the filesystem.

### Finder (baseline)
- **Steal:** it accepts Terminal-escaped paths and `file://` URLs in ⇧⌘G, and type-to-select is the default. Keep those semantics so Finder users have nothing to relearn.
- **Avoid:** path bar read-only and optional; quoted paths fail; Return = rename is the one Finder binding that conflicts with every dual-pane manager (we keep Finder's).

## 8. Recommended behaviour spec: File Studio address bar

Principle: the bar is always visible, never modal, and **never navigates or creates anything without an explicit Return**.

### States

```
REST ──(enter edit)──▶ EDIT ──(keystroke)──▶ COMPLETING
  ▲                      │  ▲                    │
  │◀──── Esc ────────────┘  └──── Esc once ──────┤ (list closes, text kept)
  │◀──── Return (valid) ───────────────────────────┤ COMMITTED → REST
  │◀──── Esc / focus loss ─────────────────────────┘ CANCELLED → REST
```

### REST

- Breadcrumb: one folder icon plus name per segment, always visible in every pane, even when the file list has focus.
- Overflow: keep the first segment and the last two; collapse the middle into a single **`…` button that opens a menu listing the hidden segments by name** (fixes QSpace/ForkLift anonymous markers).
- Click segment: go there; select the child folder you came from (Marta).
- Click chevron: menu of that segment's subfolders (keys: ↑/↓ move, Return go, → submenu, Esc closes).
- Right-click or ⌥-click segment: Copy Path (POSIX), Copy as Escaped, Copy as `~`-relative, Copy as `file://` URL, Open in Terminal, Reveal in Finder, Open in New Tab, Add to Favourites.
- Drop on segment: move (⌘ default on same volume, copy across volumes, ⌥ to force copy); spring-load after ~0.8 s hover.
- Click empty space to the right of the last segment: enter EDIT.

### EDIT (entered, nothing typed yet)

- Enter by: click empty space, ⌘L, ⇧⌘G, or `/` or `~` pressed while the list has focus (in that case the typed character **replaces** the selection, so `/` starts a fresh absolute path).
- Content: the full POSIX path of the pane's folder, with a trailing `/`, **all text selected** (QSpace, Marta). A setting may show `~/` instead of the home path.
- The breadcrumb is replaced by the field in place; no modal, no sheet.
- Leave without navigating: **Esc**, or the pane/window losing focus. On focus loss revert silently to REST with the old path (Explorer; avoids Dolphin's argued-over leftovers). The edited draft is not kept.
- ⌘L while already editing re-selects all.

### COMPLETING (any keystroke that changes the text)

The suggestion list appears under the bar, updated per keystroke within the 16 ms budget; it comes only from the cached index and the frecency DB (never a live walk of slow or cloud volumes; never touches placeholders).

- **Path-like input** (starts with `/`, `~`, `.`, `$`, or contains `/`): list the children of the deepest existing directory that match the final segment, ordered exact-prefix, then subsequence, then frecency. Show inline ghost text for the best prefix match.
- **Bare word** (e.g. `gig`): fuzzy/subsequence over frecent folders, ranked frecency × match quality (zoxide style). Each row shows the **full breadcrumb** of the candidate (ForkLift) and a recent-use hint.
- Case-insensitive and Unicode-normalisation-insensitive matching (NFC input must match NFD names).

Exact key semantics in EDIT/COMPLETING:

| Key | Behaviour |
|---|---|
| Printable | Insert; refresh list. |
| Tab | If the candidates share a longer common prefix than typed, extend to it (append `/` if the result is a unique directory) and **keep the list open**. If nothing to extend: select the first row. Further Tab cycles down, ⇧Tab cycles up. A single unique directory candidate completes to `dir/` and immediately lists its children (QSpace). |
| ↓ / ↑ | Move the highlight through the list (first ↓ highlights row 1; ↑ from row 1 returns to the typed text). Highlight previews the row in the field (greyed), without navigating. |
| → at end of text, or End | Accept the inline ghost text. |
| Return | A highlighted row: go to it. No highlight: resolve the typed text (see below). |
| ⌘Return | As Return, but open in the **other pane / new tab** (dual-pane) and keep focus in the bar. |
| Esc | First Esc closes the list (text kept). Second Esc cancels the edit. With no list open, one Esc cancels. |
| ⌥⌫ | Delete back one path component (treat `/` as a word boundary). |
| ⌘⌫ | Clear to the start of the line. |
| ⌘A, ⌘C, ⌘V, ⌘X, ⌘Z | Normal text-field semantics. |
| ⌘D | Add the highlighted or current folder to Favourites (hotlist). |

### Paste (and Return-time) cleanup pipeline, in order

1. Trim whitespace and newlines; if multi-line, use the first non-empty line and show a one-line notice.
2. Strip one pair of matching surrounding `"` or `'` quotes.
3. `file://[localhost]/…` → percent-decode to a POSIX path.
4. If the text contains `\ ` style escapes and the literal string is not an existing path, **unescape** (`\ `, `\(`, `\'`, `\&`, …).
5. Expand `~`, `~user`, `$VAR`, `${VAR}`.
6. Resolve with Unicode-normalisation and case-insensitive lookup; display the **on-disk spelling** after commit.
7. Show a transient `Cleaned up paste: removed quotes` chip so the change is never silent. Original text is recoverable with ⌘Z.

(Quoted, escaped and `file://` all have to pass, since only QSpace manages that today.)

### COMMITTED (Return, or clicking a suggestion)

- Directory → navigate, return to REST, **keyboard focus moves to the file list**, selection on the first item (or on the child you came from when going up). Push history, bump frecency (only on explicit commits; passive browsing does not count).
- **File path** → navigate to the parent and **select that file** (do not open it). ⇧Return or ⌘O on a file opens it.
- Symlink to a directory → navigate to the resolved folder, show the original name in the breadcrumb with a link glyph.
- Archive paths (`foo.zip/inside`) are future work; reserve the syntax.

### Non-existent path (Return on text that resolves to nothing)

- Do **not** navigate (avoid QSpace's dead-path pane) and do **not** silently ignore it (avoid Marta's greyed button). Keep EDIT open, outline the field, and show an inline row at the bottom of the list: `"x/y" does not exist. Nearest folder: ~/Pictures/2026`.
- That list gets one special row, `Create folder "x/y"` (with a count of intermediate folders if more than one). **Return on that row** (⌘Return from the field) creates it through the operation queue (undoable) and then navigates.
- ↑/↓ may select `Go to nearest folder`. Plain Return with the error row not highlighted does nothing but keep the message.
- Permission-denied or offline volume: same pattern with the specific reason; never a modal.

### CANCELLED

- Esc (with no list), or focus loss: restore the previous breadcrumb, discard the draft, return focus to the list (Esc) or to wherever focus went (focus loss). No history change, no frecency change.

### Cross-cutting

- The bar is per pane; ⌘L / `/` act on the active pane. Dual-pane: ⇧⌘L focuses the other pane's bar.
- Accessibility: the bar is a combo box with the list as its popup; announce count and highlighted row; honour Reduce Motion for spring-loading.
- Failure modes to fuzz in CI: `file://` with odd percent-escapes (the Marta crash), leading quote, trailing backslash, `~~`, NUL, 4 KB lines, paths on unmounted volumes.

## 9. Recommended default Standard keymap

Principle: Finder's binding wherever Finder has one; dual-pane F-key additions from Marta/ForkLift/Nimble (the three agree on F5/F6/F7); everything rebindable, with a conflict-detecting table like Nimble's Hotkeys pane and a palette showing each action's key like Marta's. `⌥⌘` marks a Finder-compatible chord.

| Action | Standard binding | Source / note |
|---|---|---|
| Open | ⌘↓ and ⌘O | Finder. Return is rename (see below). |
| Up / enclosing folder | ⌘↑ | Finder, QSpace, ForkLift, Nimble |
| Back / forward | ⌘[ / ⌘] | Finder + all four |
| Rename (in place) | **Return** | Finder, ForkLift. Nimble/Marta users get F6/⇧F6 below. |
| New folder | ⇧⌘N, plus **F7** | Finder + Marta/Nimble |
| New file | ⌥⇧⌘N | ForkLift/Nimble |
| Move to Trash | ⌘⌫, plus F8 (→ Trash, never permanent) | Finder; Marta/Nimble F8 |
| Delete permanently | ⌥⌘⌫ | Finder |
| Duplicate | ⌘D | Finder |
| Get Info | ⌘I | Finder |
| Quick Look | **Space** and ⌘Y | Finder, Marta |
| Copy / paste / move-paste | ⌘C / ⌘V / ⌥⌘V | Finder |
| **Copy to other pane** | **F5** | Marta, ForkLift, Nimble |
| **Move to other pane** | **F6** | same |
| Rename via dialog / move-as | ⇧F6 | Marta; mirrors Total Commander |
| Switch pane | **Tab** (focus list ↔ other pane); ⌘⌥← / ⌘⌥→ also | Marta; Tab is only live when the address bar is not editing |
| Toggle dual pane | ⌘\ | new |
| Copy list path / name | ⌥⌘C / ⇧⌘C; folder path ⌥⇧⌘C | Finder (⌥⌘C) + Nimble |
| New tab / close tab / new window | ⌘T / ⌘W / ⌘N | Finder + all |
| Next / previous tab | ⌃Tab / ⌃⇧Tab | macOS convention |
| Show hidden files | ⇧⌘. | Finder |
| Go to folder / focus address bar | **⌘L**, ⇧⌘G, `/` | QSpace ⌘L + Finder ⇧⌘G + brief |
| Focus other pane's address bar | ⇧⌘L | new |
| **Command palette** | **⇧⌘P** | Marta |
| Quick open (folders + recents, frecency) | **⌘P** | deliberate deviation: Finder's ⌘P (print) is meaningless here; scope is the frecency DB, not Spotlight (see §5 note) |
| Filter list (filter-as-you-type) | **⌘F** (Esc clears, keeps selection) | Nimble behaviour on Finder's find key |
| Search (global) | ⌥⌘F | Finder, later milestone |
| Select all / invert | ⌘A / ⌃⌘A | Finder's ⌘A; invert is new |
| Select / deselect by pattern | ⌘= / ⌘− (not bare `+`/`-`, which would break type-to-select) | Nimble |
| Type-to-select | **Default for any printable key**, 1 s reset, prefix match | Finder, ForkLift, Marta |
| Switch to filter-on-type | Setting "Typing filters instead of selects" | Nimble's behaviour as an option |
| View: icons / list / columns / gallery | ⌘1 / ⌘2 / ⌘3 / ⌘4 | Finder |
| Open in terminal | ⇧⌘R | ForkLift |
| Open in editor | ⌘E | QSpace/ForkLift |
| Connect to server | ⌘K | Finder, ForkLift (later) |
| Fn-key mode | Preference "Always use F-keys" | Nimble pref |

Notes:
- **⌘P / ⇧⌘P / ⌘F** are the only places this map picks something Finder does not use for the same purpose. Document each as an intentional deviation in Settings ▸ Keyboard.
- **Return renames** (Finder), so Open is ⌘↓/⌘O. The Vim profile (later) maps `l`/Return to open, `h` up, `j/k` move, `/` filter, `:` palette; it must not depend on any Standard binding being free.
- Tab for pane switching must yield to a text field: inside the address bar, Tab is completion (§8).
- Discoverability: ship Marta's bottom F-key bar as an optional strip, a palette that prints each action's key, and the shortcut overlay already planned in M2.

## 10. Visual design: ForkLift (main reference) and QSpace

The user likes ForkLift's look most and QSpace Pro's second. Measurements are read off 1x window screenshots (window size equals point size), so row and bar heights are accurate to about ±1 pt; **font sizes and materials are estimates**, not read from the apps. ForkLift was captured with a single tab, so **tab styling and the queue/progress UI were not seen** (its right-hand inspector has three icon tabs: info, transfers, warnings, which is where progress lives).

### ForkLift 4 (dark theme "ForkLift")

| Element | What it looks like |
|---|---|
| Title bar / toolbar | **One unified band, ~52 pt**, no separate title bar. Traffic lights sit in the sidebar's top; the folder name is the window title, immediately right of a back/forward pill. Remaining controls are grouped in capsules: view-mode segmented control (icons / list / columns), a group menu, star and connect, refresh/copy/new-folder/trash, then a ~270 pt search field. About 14 controls in all, quiet monochrome glyphs, none labelled. |
| Path / breadcrumb | In each pane's **header (~36 pt)**: a 28 pt drive or home glyph, then the breadcrumb on one line (13 pt, `›` separators, middle segments abbreviated) with a dimmer 11 to 12 pt subtitle underneath (`12 items, 241.5 GB available`). Not a field; no capsule or border. |
| Active pane | A **~3 pt accent bar (orange) across the top of the active pane's header**. Nothing else changes; no outline, no dimming of the other pane. |
| Divider | 1 pt hairline between panes, no visible splitter handle. |
| Sidebar | **~200 pt** wide, items on a **32 pt** pitch, rounded-rect grey selection, 11 pt dim section headers (Devices, Recent Folders, Tags, Favorites), tinted orange icons for places, coloured dots for tags, drive free-space in secondary text. |
| Rows | **22 pt pitch**, 16 pt icons, ~13 pt system text, no zebra striping, hover/selection as a soft grey fill. Column header ~28 pt, sort chevron on the active column. Size and date columns in secondary colour. |
| Inspector | Optional ~300 pt right panel with collapsible "General / Permissions" sections. |
| Materials | Looks **flat and opaque** (dark grey, content slightly lighter than the sidebar); I saw no visible blur or glass. Accent is **ForkLift's own orange** from its built-in theme (chosen in the welcome dialog), not the system accent. Folder icons are custom magenta. |

### QSpace Pro 7 (where it differs, and what is worth borrowing)

- **Denser and busier:** rows are ~20 pt with zebra striping, ~12 pt text, toolbar ~50 pt with around 15 glyphs in several groups plus a layout popup ("Default") and a Buy button; opens as **four** panes.
- **Active pane:** a **violet 2 pt rounded outline around the whole pane**, path bar included. At four panes this reads much better than ForkLift's single bar.
- **Path bar:** a **rounded capsule (~30 pt) inside each pane** with back/forward arrows on the left and search plus ⋯ on the right. It looks like a field, so it invites a click, and that matches the edit-on-click behaviour (§1).
- **Sidebar:** ~187 pt, darker than the content, grouped (Bookmarks, iCloud, Servers, Locations, Finder Tags, Workspace) with a filter field at the bottom.
- Borrow: the capsule path bar with its inline affordances, the whole-pane active outline, the sidebar filter. Avoid: zebra striping and the toolbar glyph count.

### What to adopt for File Studio (concrete defaults)

| Item | Spec |
|---|---|
| Window chrome | Native unified toolbar (`NSToolbar`, `.unified`), ~52 pt, no separate title bar; sidebar runs full height with traffic lights over it. At most 10 default controls, glyph-only with tooltips and a label-on-hover option. |
| Pane header | 36 pt: 28 pt location glyph + the **address bar** (13 pt) + 11 pt secondary subtitle (`N items, X GB available`). In REST the breadcrumb is drawn like ForkLift (flat, no border); on hover/focus it gains QSpace's capsule outline (~30 pt) to signal it is editable. |
| Active pane | A 2 pt accent-colour outline on the whole pane plus a 3 pt accent bar on the header (both subtle; the outline is the strong cue). Inactive pane text stays full strength. |
| Divider | 1 pt hairline, 8 pt invisible drag target, double-click to equalise. |
| Rows | **List: 22 pt** (Compact 20 pt, Comfortable 26 pt). Icons 16 pt. No zebra by default (setting). Column header 28 pt. |
| Type styles | Names: system `body` 13 pt. Size/date/kind: 12 pt, secondary colour, **monospaced digits**, right-aligned for sizes. Sidebar items 13 pt, section headers 11 pt semibold, secondary. Pane subtitle 11 pt. Address bar 13 pt, last segment medium weight. |
| Sidebar | 200 pt default (min 160), 28 to 32 pt rows, 11 pt section headers, system `.sidebar` material, tinted symbol icons using the **system accent**, filter field at the bottom (QSpace). |
| Materials | Toolbar and sidebar: system materials (`.sidebar` / unified toolbar, which provide glass on macOS 26 for free by using native controls). File lists and panes: **opaque `.contentBackground`**; do not put vibrancy behind rows. Inspector: `.underWindowBackground`/sidebar. Transient surfaces (completion list, command palette, menus): `.popover` or `.hudWindow`. |
| Accent colour | `controlAccentColor` everywhere (selection, active pane, progress). ForkLift's fixed orange is the cautionary tale: it ignores the user's macOS choice. Folder icons are the system ones. |
| Queue / progress | Not observed in ForkLift; plan from this session: a transfers tab in the inspector plus a toolbar progress ring (M2 queue UI). |

## 11. What to build or check next

1. Fuzz the address bar parser with the inputs in §8 (Marta crash class).
2. Decide the first-run default: type-to-select (recommended) vs filter.
3. Verify chevron menus, drop-on-segment, and QSpace/ForkLift rebinding in a follow-up pass if they matter for M2 (they were not verifiable in this session).
4. Real clipboard paste (⌘V) for QSpace, ForkLift and Nimble: if their paste differs from typing, update the §3 table.

## 12. Installed during this session, and removed

Installed via Homebrew casks with `--appdir=~/Applications`: QSpace Pro 7.0.4, ForkLift 4.7.7, Marta 0.8.2, Nimble Commander 1.9.0 (4329). The user then asked for everything to be removed, so all four apps were quit and uninstalled (`brew uninstall --cask --zap`; ForkLift's uninstall step needed `sudo` for a helper path that did not exist, so its app, Caskroom entry and zap paths were deleted by hand), their `~/Library` data removed, and the scratch test tree deleted.

Left behind: empty sandbox container folders that macOS protects from deletion without Full Disk Access (not removable from here):
`~/Library/Containers/com.jinghaoshe.qspace.pro.FinderExtension`, `~/Library/Containers/com.jinghaoshe.qspace.pro.StashShelfShareExtension`, `~/Library/Group Containers/group.com.jinghaoshe.qspace.pro.shared`, `~/Library/Group Containers/J3CP9BBBN6.com.binarynights.ForkLift`. Delete them in Finder if you want them gone. brew also moved a `~/QSpace` folder (QSpace's own) to the Trash.

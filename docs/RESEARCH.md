# File Studio: Milestone 0 research report

Status: draft for sign-off. Scope: competitive and user-demand research only. Platform spikes (listing speed, FSEvents, `copyfile`/`clonefile`, iCloud placeholders) are a separate session and are not covered here.

## How to read this report (and how far to trust it)

- **Evidence tags** in the feature table: **●** strong (several independent sources, or the vendor's own repeated marketing plus user echo), **◐** medium (one or two user sources), **○** inferred (product-design judgement or well-known behaviour I did not re-verify in this pass).
- **Source limits, stated plainly.** The search tooling returned vendor pages, MacRumors/Apple Community/Tidbits threads, KDE and GNOME bug trackers, review sites and aggregator blogs. It surfaced very little direct Reddit or Hacker News comment text (one HN item: a "Show HN" for a new dual-pane Mac app). So Reddit/HN "demand" below is mostly *inferred from the same complaints recurring in the forums I could read*, and I have not quoted Reddit users. Where a claim is from my own background knowledge rather than a fetched page, it is tagged **(bg)**. Pricing changes often; verify before quoting it externally.
- Sources are numbered **[S1]…** and listed at the end. Quotes are kept to short fragments.
- "v1" = milestones 1–3 of the brief. "later" = milestones 4–5 or beyond.

---

## 1. Product summaries

### Finder (macOS 26 "Tahoe")
- **Strengths:** native, free, Quick Look, Spotlight, tags, iCloud/AirDrop integration, the default everything else is measured against. Column view and gallery view are genuinely good.
- **Complaints:**
  - No dual pane; path is hidden unless you enable the path bar; no cut-and-paste move (⌘C then ⌥⌘V is the hidden equivalent) [S1].
  - Folders-on-top only applies when sorting by name; view options are "frequently forgotten", and `.DS_Store` litter annoys mixed-OS users [S1].
  - Tahoe-era regressions: Apple Community threads about drag and drop, right-click items and new window/tab disappearing after 26.1, and file dialogs that are unusably slow [S2][S3]. One MacRumors thread is titled around Apple's "slipping UI polish" [S2].
  - Large folders: a user timed ~8 s to list 10,000 files on a network share versus ~0.3 s for `ls`; thumbnail generation on external drives multiplies the delay [S4].
  - Previewing or entering folders of iCloud/OneDrive/Dropbox placeholders can materialise (download) files; a QNAP note says Finder needs the file to build a preview and that turning off the preview panel stops it [S5].
  - Photographers/DITs: Pomfort argues Finder copies lack resume and checksum-backed records [S6].
- **Pricing:** free (bundled).

### Path Finder (Cocoatech)
- **Strengths:** the "depth" option: dual-pane browser (TechRadar called it "the real star" of v5 [S7]), drop stack, built-in terminal, hex/text viewers, bulk rename, tagging and many panels. Mac-native Cocoa, long history.
- **Complaints:**
  - Dual-pane split ratio not preserved, uneven panes reset on reopen [S8].
  - Mixed stability reports after macOS 26 updates; a Setapp reviewer says it "crashes alot" and another says Alfred now opens folders in Finder instead after 26.4 [S3].
  - Steeper learning curve, a sprawling UI [S8].
  - The licence model: moved to subscription years ago [S9].
- **Pricing:** no perpetual licence. About $29.95/year subscription, or fixed-term licences (1 yr $32.95, 2 yr $55.95, 5 yr $122.95); also via Setapp [S9]. Community reaction to the subscription switch is a long-running grievance (Tidbits thread "am I out?") [S9].

### ForkLift 4 (BinaryNights)
- **Strengths:** dual pane plus the best-known remote story on Mac (SFTP, S3, WebDAV, cloud drives), sync and compare, favourites, multi-rename, tabs, Setapp availability. Users cite dual pane productivity as the reason to pay [S8].
- **Complaints:**
  - v4 launch: no upgrade path from v3 caused uproar; the developer later offered a retroactive bonus for old serial numbers [S10].
  - Early v4 sync was slow with weak progress UI for a 30k-file S3 sync, and removed v3's explicit file selection (later partly restored in 4.0.3) [S10].
  - Remote bugs and crashes in point releases; recent Setapp reviews split between "buggy" and "best FTP client" [S10].
- **Pricing:** one-time licence with paid upgrades per major version; listings vary between ~$20 and ~$30 [S10][S8]. Verify.

### QSpace Pro
- **Strengths:** multi-pane (more than two), workspace groups, Finder extension, server connections in the sidebar [S11]. Priced low.
- **Complaints:** I found almost no independent review or forum text; the only data is a MacUpdate listing (rating 4.8 on 5 ratings, ~$13.99, v3.3.2) [S11]. Treat QSpace as **under-researched**; recommend a hands-on trial before the next milestone.
- **Pricing:** appears one-time (a reviewer mentions a perpetual licence) [S11]. Verify.

### Marta
- **Strengths:** native Swift, keyboard-driven dual pane, "fully customisable and extensible" (plugins, themes, keybind files), free [S12]. Praised as suiting people who already live on command palettes and terminals [S12].
- **Complaints:** documentation lags the app (action identifiers must be looked up when customising) [S12]; sustained-development status is unclear from what I could read; a listing said "beta" and that a paid licence might be introduced [S12], unconfirmed.
- **Pricing:** free; closed source **(bg)**.

### Commander One (Eltima)
- **Strengths:** polished Total-Commander-style dual pane; Pro pack adds cloud/FTP/archives [S13].
- **Complaints:** the Pro Pack is needed for the useful features; learning curve for people new to dual pane [S13]; Mac App Store build has limits versus the direct build [S13].
- **Pricing:** free core, Pro roughly $30 one-time per Mac; major updates may be paid [S13].

### Nimble Commander
- **Strengths:** free, open-source Mac-only dual pane with 100+ keyboard actions, native and fast **(bg: C++/Objective-C, built for large folders)** [S13].
- **Complaints:** thin documentation, plugin-dependent advanced features, a dated look, one-maintainer risk [S13] **(bg)**. Evidence here is weak.
- **Pricing/licence:** free/open-source per [S13]; I recall GPL-3 with a paid App Store build **(bg, verify licence, because it matters for code reuse)**.

### Directory Opus (GP Software, Windows)
- **Strengths:** the "gold standard for depth": completely replaces Explorer, native multithreaded C++, "greatest strength" is how configurable it is (colours, toolbars, menus, hotkeys), batch rename, filters on the toolbar, directory compare, remote file editing, multi-protocol [S14]. Excellent support forum: users praise the fast replies [S15].
- **Complaints:** price; busy UI; steep configuration surface [S14][S15].
- **Pricing:** perpetual licence but after a year you must subscribe for further updates; ~US$62-70 for the cheapest edition (price in AUD on the vendor site) [S15][S16]. Version 13 pricing caused forum debate [S16].

### Total Commander
- **Strengths:** the original keyboard-driven dual pane: multi-rename tool, compare, sync, archive plugins, a huge plugin ecosystem [S15][S17]. Users: "so much more powerful overall" [S17].
- **Complaints:** dated interface and heavy-feeling configuration [S17].
- **Pricing:** shareware with an unlimited trial, ~$50 [S15].

### XYplorer
- **Strengths:** portable, tabs, scripting and search/automation, "lighter" than Opus [S15][S17]. Favoured by tinkerers.
- **Complaints:** weaker metadata preview in one forum comparison [S17].
- **Pricing:** ~$35 with lifetime-licence history [S15][S17].

### Files (Windows)
- **Strengths:** modern WinUI look, tabs, Columns view, dual pane (added in v4), tags, cloud integration; much-praised by Windows press as the best Explorer alternative [S18].
- **Complaints:** historically unstable with large folders and performance that degrades with long use; user reports vary by machine [S18]. Columns view had a string of bugs (unexpected scrolling, crash in context menu), later fixed [S18].
- **Pricing:** free/open source with a paid Store build **(bg)**.

### Dolphin (KDE)
- **Strengths:** the feature-rich Linux reference; split view; an editable/breadcrumb location bar; panels (terminal, info, places); highly configurable.
- **Complaints:**
  - Split view is capped at two and lives inside a tab rather than tabs inside each pane; horizontal split is a wishlist item [S19].
  - Location bar behaviour is argued over for years: Esc returns to breadcrumbs, Ctrl+L doesn't toggle back, the bar doesn't revert to breadcrumbs on focus loss as Windows does [S20].
- **Pricing/licence:** free, GPL.

### Nautilus (GNOME Files)
- **Strengths:** minimal, fast, consistent, modern; Ctrl+L location entry with completion **(bg)**.
- **Complaints:** no dual pane (a "reintroduce split view" request) and users tile two windows instead [S19].
- **Pricing/licence:** free, GPL.

### yazi, ranger, nnn (terminal)
- **yazi:** Rust, async I/O, native inline image preview over kitty/sixel protocols, plugins in Lua; positioned as the new default over ranger [S21].
- **ranger:** Python, Miller columns, vi keys, previews via `scope.sh`; reported to block on large directories and previews [S21].
- **nnn:** tiny and very low resource use; "fast but minimalist to the point of being unusable" for some [S21]; plugin system for everything else.
- **Themes to steal:** vi motion, marks and jump, bulk rename in `$EDITOR`, "pick file" integration, chooser mode, async previews, cd-on-quit.
- **Pricing/licence:** free; yazi MIT, ranger GPL-3, nnn BSD **(bg)**.

### Raycast / Alfred-style command palettes (pattern)
- **Pattern:** one global invocation key, fuzzy match over everything (actions, files, recents), learns your usage (frecency), shows the shortcut next to each result, and allows chaining arguments. Alfred's file navigation uses a type-a-path, Tab-to-complete, arrow-to-actions flow that power users love **(bg)**.
- **Evidence of fit:** Marta's audience overlaps heavily with command-palette users [S12]; Alfred's "open folder" integration breaking with Path Finder on macOS 26.4 shows how much users rely on launcher↔file-manager hand-off [S3].
- **Steal:** actions + folders + recents in a single list; show bound shortcut; "run with argument" (e.g. new folder named X); frecency ranking.

---

## 2. Recurring themes

### Top Finder pain points (ranked by how often they appear in what I read)
1. **No dual pane / no side-by-side** [S1][S8][S19].
2. **Hidden path and slow path entry**; no always-on editable bar [S1] (and the same gap the brief identifies).
3. **Large-folder and external/network slowness** [S4].
4. **Regressions and polish loss in Tahoe**: dialogs, drag and drop, context menu [S2][S3].
5. **Weak file operations**: no real queue, no pause/resume, no verify, no resume after failure; photographers/DITs use other tools [S6].
6. **Cut/paste move hidden; no "new file"** ("can't create a new file without opening an app first… 20 years" quoted in a roundup) [S1].
7. **View settings forgotten; sort quirks; `.DS_Store` litter** [S1].
8. **Cloud placeholders materialise on preview/navigate** [S5].
9. **Weak batch rename and no folder compare/sync** (brief; consistent with alternatives' marketing) **(bg)**.

### What power users switch apps for
- **Dual pane with copy/move to the other pane** is the single most-cited reason [S8][S19].
- **Keyboard depth and rebinding** (Marta, Nimble, TC, Opus) [S12][S14][S17].
- **Remote protocols in the same UI** (ForkLift, Commander One, Opus) [S10][S13].
- **Configurability** is Opus's identity [S14].
- **Sync/compare and a trustworthy copy** [S6][S10].
- **Speed on big folders** (Nimble, Opus, yazi) [S14][S21].
- **Stability and licence trust:** the strongest *anti*-switching forces are subscription conversions (Path Finder, Opus update subscription) and crashy releases [S9][S10][S16].

### Cross-cutting lessons
- Tools that win are *fast first*, *configurable second*, and *pretty third*; Opus and Total Commander survive on the first two despite the third [S14][S17].
- Dual pane implementations are judged on boring details: **split ratio persistence, tabs per pane, vertical/horizontal, more than two panes** [S8][S19]. File Studio should make those boring details correct.
- Every popular manager has a "licence incident" in its history. A GPL-3 free app with no nags avoids the largest category of user anger.

---

## 3. Ranked feature table

Rank is global (1 = highest priority across all areas). Verdict reasons are one line. "Best" names who to study.

### 3.1 Address bar and navigation

| Rank | Feature | Who does it best (why) | Demand evidence | Verdict |
|---|---|---|---|---|
| 1 | Always-visible breadcrumb/text address bar, in every pane | Windows Explorer and Dolphin do hybrid bars (click empty space to edit) but toggle poorly; nobody is "always visible + editable" [S20] | ● Finder's hidden path is a top complaint [S1]; Dolphin debates show users care about the toggle details [S20] | **v1**: the #1 feature of the brief |
| 2 | Fuzzy + frecency path completion (zoxide-style) | Shell tools (zoxide, fzf) and Alfred's file nav **(bg)**; no GUI manager does frecency in its bar | ◐ Command-palette audience overlap [S12]; zoxide popularity **(bg)** | **v1**: the differentiator; engine in the `path` crate |
| 3 | Segment chevrons list subfolders | Windows Explorer's breadcrumb chevrons **(bg)** | ◐ Explorer/Dolphin users expect it | **v1**: cheap, core to the bar |
| 4 | Tab completion like a shell + `~`/env expansion | Nautilus Ctrl+L, shells **(bg)** | ○ | **v1**: part of the bar contract |
| 5 | Paste cleanup (quotes, `file://`, escaped paths; file path selects file) | Nobody well | ○ Terminal-to-Finder paste pain is common **(bg)** | **v1**: tiny effort, big delight |
| 6 | Drag onto a segment with spring-loaded folders | Finder spring-loaded folders **(bg)** | ○ | **v1**: expected on Mac |
| 7 | Back/forward history per tab, mouse buttons 4/5 | ForkLift 4.x added buttons 4/5 explicitly [S10] | ◐ | **v1**: table stakes |
| 8 | "Create folder" offer on a non-existent path | Nobody | ○ | **v1**: cheap and distinctive |
| 9 | Sidebar with favourites, volumes, eject, cards/drives | Finder; ForkLift favourites; QSpace server entries [S11] | ● | **v1**: needed from day one |
| 10 | Recents/jump list dropdown, "go to recent" | Opus/TC hotlist ("Ctrl+D") **(bg)**; Raycast recents | ◐ | **v1**: falls out of the frecency DB |
| 11 | Bookmarks/hotlist with assignable keys | Total Commander, Opus [S14][S17] | ◐ | **v1**: simple version; richer later |
| 12 | Remote URLs (`sftp://`, `smb://`, `s3://`) in the bar | ForkLift, Opus [S10][S14] | ● | **later**: milestone 5 |
| 13 | Path syntax into archives (`foo.zip/inside`) | Total Commander, Opus, Nimble **(bg)** | ◐ | **later**: with archives |

### 3.2 Views and columns

| Rank | Feature | Who does it best (why) | Demand evidence | Verdict |
|---|---|---|---|---|
| 14 | List view with sortable, customisable columns | Opus (the most configurable columns) [S14] | ● | **v1**: core |
| 15 | Column (Miller) view | Finder (polished), yazi/ranger (keyboard) [S21] | ● Files fixed a series of Columns bugs because users use it [S18] | **v1**: milestone 2 per brief |
| 16 | Sort controls: folders-first always, natural sort, case/locale-aware | Opus/TC **(bg)** | ● Finder's name-only folders-on-top is a complaint [S1] | **v1**: trivial in Rust core |
| 17 | Show/hide hidden files toggle (⌘⇧.) | Finder | ◐ | **v1** |
| 18 | Per-folder view memory (optional, off or smart by default) | Finder (flaky) [S1]; Opus has strong per-folder formats **(bg)** | ● Forgotten settings complaint [S1] | **v1**: optional; store in the DB, not `.DS_Store` |
| 19 | Icon grid with thumbnail size slider | Finder; Files [S18] | ● | **v1** (M3) |
| 20 | Gallery view for photos/raws | Finder gallery | ◐ photographer fit (user brief) | **v1** (M3) |
| 21 | Packages/bundles as one item, "Show Package Contents" | Finder | ○ | **v1**: correctness |
| 22 | Folder-size column, cached | Opus, Path Finder, XYplorer **(bg)** | ◐ | **v1** (M3) per brief |
| 23 | Row colouring rules by filter (Opus-style) | Directory Opus [S14] | ○ | **later** |
| 24 | Flat/recursive "show all files below" view | Directory Opus **(bg)** | ○ | **later**: needs the search engine |
| 25 | Group-by (kind, date) | Finder, Files [S18] | ○ | **later** |

### 3.3 Tabs, panes and workspaces

| Rank | Feature | Who does it best (why) | Demand evidence | Verdict |
|---|---|---|---|---|
| 26 | Tabs (drag out/in, reopen closed tab) | Files, ForkLift, Finder | ● | **v1** |
| 27 | Session restore (tabs, selection, scroll) | XYplorer, Opus **(bg)** | ◐ | **v1**: brief requires it |
| 28 | Optional dual pane (horizontal or vertical) | ForkLift, Marta, TC, Opus [S8][S12][S17] | ● the #1 switching reason [S8][S19] | **v1** (M3) |
| 29 | Tabs *inside each pane*, remembered split ratio | Opus/TC; Dolphin gets it wrong (split inside the tab) [S19]; Path Finder loses ratios [S8] | ● | **v1**: avoids known failures |
| 30 | F5/F6-style copy/move to other pane | TC, Marta, Nimble | ● | **v1** (M3) |
| 31 | Swap panes / mirror folder / "same as other pane" | TC, Opus **(bg)** | ○ | **v1** (M3) cheap |
| 32 | More than two panes | QSpace [S11] | ○ | **never** (for now): complexity, tabs cover it |
| 33 | Saved workspaces | ForkLift 4.3 "smarter workspaces" [S10]; XYplorer | ◐ | **later** (M4) |
| 34 | Synchronised browsing between panes | Opus, TC **(bg)** | ○ | **later** |
| 35 | Locked/pinned tabs | Browser norm | ○ | **later** |

### 3.4 Keyboard and command palette

| Rank | Feature | Who does it best (why) | Demand evidence | Verdict |
|---|---|---|---|---|
| 36 | Every action rebindable, conflict detection | Marta, Opus, TC [S12][S14] | ● | **v1**: brief non-negotiable |
| 37 | Type-to-select | Finder | ● | **v1** |
| 38 | Filter-as-you-type in current folder | Opus toolbar filters [S14]; WhimFiles (new Show HN app) markets it | ◐ | **v1** |
| 39 | Command palette (actions, folders, recents) | Raycast/Alfred pattern; Marta audience [S12] | ◐ | **v1** (M3) |
| 40 | Shortcut hints overlay + "did you know" toasts | Nobody in file managers | ○ Import Studio precedent | **v1**: design-language feature |
| 41 | Keyboard selection ops: select by pattern, invert, add/remove | Total Commander (+/−/*) [S17] | ◐ | **v1** |
| 42 | Vim-style keymap | yazi/ranger/nnn [S21] | ◐ | **later**: ship engine v1, preset in M4 |
| 43 | Multi-key sequences/marks/jump-to-mark | yazi, ranger [S21] | ○ | **later** |
| 44 | Quick Look on Space | Finder | ● | **v1** |

### 3.5 File operations and queue

| Rank | Feature | Who does it best (why) | Demand evidence | Verdict |
|---|---|---|---|---|
| 45 | Visible queue: pause/resume/reorder/cancel, speed/ETA | Opus, TeraCopy-style tools [S6] | ● Finder gap [S6] | **v1**: milestone 1/2 |
| 46 | Clear conflict dialog (keep both, replace, skip, compare, apply to all) | Opus, TC **(bg)** | ● | **v1** |
| 47 | Trash-first delete, Put Back | Finder | ● | **v1**: brief data-safety |
| 48 | Journaled, resumable, undoable ops | Nobody on Mac | ◐ Pomfort-style resume demand [S6] | **v1**: differentiator |
| 49 | Verified copy (xxh3) | Pomfort, Hedge, TeraCopy [S6] | ● photographers/DIT [S6] | **v1**; show an honest "verified from disk" caveat (page-cache note) [S22] |
| 50 | APFS clones; xattr/tags/ACL/resource-fork preservation | Finder; poor in some alternatives **(bg)** | ○ | **v1**: spike-session dependent |
| 51 | Cut/paste to move (⌘X) | Windows, Files **(bg)** | ● [S1] | **v1**: with a visible pending-cut state |
| 52 | New file (and from template) | Missing in Finder [S1] | ● | **v1** |
| 53 | Per-file error reporting without batch death | Opus **(bg)** | ○ | **v1** |
| 54 | Background priority / throttle | Opus **(bg)** | ○ | **later** |
| 55 | Skip junk on copy (.DS_Store etc.) | Opus filters **(bg)** | ○ | **later** |
| 56 | Secure erase | None sensible on SSD | ○ | **never**: meaningless on APFS/SSD, risk of false assurance |

### 3.6 Rename

| Rank | Feature | Who does it best (why) | Demand evidence | Verdict |
|---|---|---|---|---|
| 57 | Batch rename with live preview, tokens, regex | Opus, TC multi-rename [S14][S17] | ● | **v1** (M3) |
| 58 | EXIF/camera tokens and sequence numbers | Photographer niche; Opus supports metadata fields **(bg)** | ○ user brief | **v1** (M3) |
| 59 | Presets and undo | Opus **(bg)** | ○ | **v1** (M3) |
| 60 | Fast inline rename (Return/F2, keep extension selection) | Finder/Files | ● | **v1** |
| 61 | Rename in `$EDITOR` (list of names) | nnn/ranger/yazi [S21] | ○ | **later** |

### 3.7 Search

| Rank | Feature | Who does it best (why) | Demand evidence | Verdict |
|---|---|---|---|---|
| 62 | Spotlight-backed fast search + walk fallback | Finder (speed); XYplorer/Opus (accuracy) [S15] | ◐ | **v1** (M3) |
| 63 | Name glob/regex, size/date/kind filters | Opus, XYplorer [S14] | ● | **v1** (M3) |
| 64 | Results open as a normal tab | Opus collections/results **(bg)** | ○ | **v1** (M3) |
| 65 | Content search | Opus; Spotlight; ripgrep **(bg)** | ◐ | **later**: Spotlight content first, ripgrep later |
| 66 | Saved searches | Finder smart folders **(bg)** | ○ | **later** |
| 67 | Own persistent index | XYplorer / Everything **(bg)** | ○ | **never**: duplicates Spotlight; costs disk and CPU |

### 3.8 Preview

| Rank | Feature | Who does it best (why) | Demand evidence | Verdict |
|---|---|---|---|---|
| 68 | Preview pane with EXIF/codec/syntax-highlighted code | Path Finder, Finder preview; yazi for code/images [S7][S21] | ◐ | **v1** (M3) |
| 69 | Never download placeholders on list/preview; show state | Nobody [S5] | ● | **v1**: a headline differentiator |
| 70 | Embedded-JPEG raw preview, fast | FastRawViewer-class apps [S23] | ◐ photographers | **v1** |
| 71 | Hex/binary viewer | Path Finder, TC [S7][S17] | ○ | **later** |
| 72 | Archive contents preview | TC/Opus **(bg)** | ○ | **later** |
| 73 | Image compare / cull side-by-side | Not file managers (Import Studio's job) | ○ | **never**: stay out of Import Studio's territory |

### 3.9 Tags and metadata

| Rank | Feature | Who does it best (why) | Demand evidence | Verdict |
|---|---|---|---|---|
| 74 | Finder tags, fully compatible, keyboard assign | Finder; Path Finder **(bg)** | ◐ | **v1** |
| 75 | Get Info panel (inline, multi-select aware) | Finder; Path Finder | ● | **v1** |
| 76 | Permissions/owner editing | Path Finder, TC **(bg)** | ○ | **later** (M4) |
| 77 | xattr/comment viewer-editor | Path Finder **(bg)** | ○ | **later** |

### 3.10 Power extras

| Rank | Feature | Who does it best (why) | Demand evidence | Verdict |
|---|---|---|---|---|
| 78 | Terminal here + embedded follow-the-folder terminal | Path Finder, Kommanda-style newcomers [S7] | ◐ | **later** (M4) |
| 79 | Git status badges | Not mainstream; Dolphin plugin **(bg)** | ○ | **later** |
| 80 | Archives as folders | TC, Opus, Nimble, ForkLift [S10][S13][S17] | ● | **later** (M4) |
| 81 | Folder compare and sync | ForkLift, TC, Opus [S10][S17] | ● but sync complaints show it must be trustworthy [S10] | **later** (M4) |
| 82 | Duplicate finder | Opus, TC **(bg)** | ○ | **later** |
| 83 | Custom shell actions with `{selected}`/`{dir}` | Opus, TC buttons, Marta | ◐ | **later** (M4) |
| 84 | Volume awareness: eject, mount state, card detection | Finder; Import Studio flow | ● | **v1** |
| 85 | Disk-usage map | Not required | ○ | **never**: GrandPerspective-class apps own it |

### 3.11 Remote

| Rank | Feature | Who does it best (why) | Demand evidence | Verdict |
|---|---|---|---|---|
| 86 | SFTP | ForkLift, Opus [S10][S14] | ● | **later** (M5) |
| 87 | SMB (beyond Finder's) / WebDAV | ForkLift, Commander One [S10][S13] | ◐ | **later** (M5) |
| 88 | S3 and S3-compatible | ForkLift, Commander One Pro [S10][S13] | ◐ | **later** (M5) |
| 89 | Google Drive/OneDrive native APIs | ForkLift, Commander One [S10][S13] | ◐ | **never**: use File Provider mounts, less maintenance |
| 90 | iOS/Android device browsing | Commander One markets it [S13] | ○ | **never**: separate problem space |

### 3.12 Extensibility

| Rank | Feature | Who does it best (why) | Demand evidence | Verdict |
|---|---|---|---|---|
| 91 | Settings in a text/JSON export, shareable keymaps | Marta (files), yazi [S12][S21] | ◐ | **v1**: cheap, honours "everything customisable" |
| 92 | Plugin/scripting API | Marta, yazi (Lua), Opus (scripts), XYplorer [S12][S14][S21] | ◐ | **later** (post-M5) |
| 93 | Custom toolbar/button editor | Opus, TC [S14][S17] | ○ | **later** |
| 94 | Themes beyond system accent/appearance | Marta themes [S12] | ○ | **never** (v1): native look is the point |

### 3.13 Accessibility

| Rank | Feature | Who does it best (why) | Demand evidence | Verdict |
|---|---|---|---|---|
| 95 | Full VoiceOver incl. address bar segments | Finder | ○ | **v1**: brief requires it |
| 96 | Reduce Motion, increased contrast, Dynamic-type-like scaling | System | ○ | **v1** |
| 97 | Everything reachable by keyboard | Marta [S12] | ● | **v1** |
| 98 | Localisation | System | ○ | **later** |

### 3.14 Performance

| Rank | Feature | Who does it best (why) | Demand evidence | Verdict |
|---|---|---|---|---|
| 99 | 100k-entry folders smooth; recycled rows, no SwiftUI per row | Opus (native C++), Nimble, yazi [S14][S21] | ● Finder and Files both criticised here [S4][S18] | **v1** |
| 100 | Async, never-blocking listing and thumbnails | yazi vs ranger lesson [S21] | ● | **v1** |
| 101 | Watcher-driven refresh < 100 ms, no polling | Finder | ◐ | **v1** |
| 102 | Low idle CPU/RAM, no beach balls after sleep | Finder regressions [S2] | ◐ | **v1** |

(Numbering reflects the global order I would implement and pitch; the table groupings follow the brief. The 102 rows include a handful of "never" entries so the exclusion decisions are explicit.)

### Top 20, in order
1. Always-on editable breadcrumb address bar
2. Fuzzy + frecency completion
3. Fast list view with customisable columns on 100k-item folders
4. Column (Miller) view
5. Tabs with session restore
6. Dual pane with F5/F6 and per-pane tabs (ratio remembered)
7. Rebindable keys, type-to-select, filter-as-you-type
8. Command palette
9. Visible, pausable file-operation queue
10. Conflict dialog with compare
11. Journaled, undoable operations; Trash-first
12. Verified copy (xxh3) with APFS clone
13. Batch rename with live preview and EXIF tokens
14. Quick Look + preview pane with metadata
15. No placeholder downloads; show iCloud state
16. Spotlight + walk search, results as a tab
17. Finder tags, fully compatible
18. Folder sizes, cached
19. Cut/paste move and New File
20. Shortcut overlay and "did you know" toasts

---

## 4. Anti-features (what users hate)

| Anti-feature | Evidence | Policy for File Studio |
|---|---|---|
| Subscriptions or expiring licences for a utility | Path Finder's conversion, "am I out?" thread [S9]; Opus update subscription debate [S16] | GPL-3, free; any paid support should never gate features |
| Paid upgrade with no path for existing owners | ForkLift 4 uproar [S10] | N/A, but avoid "Pro pack" tiers (Commander One's complaint [S13]) |
| Sluggish big folders | Finder 8 s for 10k on network [S4]; Files [S18] | Budgets in the brief; never block the main thread |
| Crashes after OS point releases | Path Finder/Finder reports [S2][S3] | Debug automation on each macOS beta |
| Non-native UI | Dated looks of TC/Opus/Nimble noted [S17] | Native AppKit/SwiftUI shell |
| Placeholder downloads on preview/navigation | [S5] | Never trigger; show cloud state |
| Silent overwrites or lost state in copy | Pomfort critique [S6] | Journal + conflict rules + verify |
| Sync UIs with poor progress and no per-item selection | ForkLift 4 sync [S10] | Plan preview and per-item control before sync ships |
| Panes whose split ratio resets | Path Finder [S8] | Persist ratios |
| Nag screens, upsell dialogs | Common trial-ware complaint **(bg)** | None |
| Hidden or inconsistent toggles (Ctrl+L semantics, Esc behaviour) | Dolphin bug history [S20] | Document the bar's states and make them configurable |
| `.DS_Store`/metadata litter | [S1] | Store view state in the DB; never write `.DS_Store` |

---

## 5. Notes for the always-on editable address bar

**Who comes closest today**
- **Windows Explorer / Files:** breadcrumb at rest, click empty space to edit, chevrons list subfolders, reverts to breadcrumb when focus is lost [S20]. Not frecency-aware.
- **Dolphin:** the same hybrid with a check-mark button to return to breadcrumbs; Ctrl+L selects all; Esc exits [S20]. Its users' long-running bug threads are a free spec for edge cases.
- **Nautilus:** Ctrl+L text entry with completion **(bg)**; no breadcrumb-edit hybrid.
- **Alfred / Raycast:** type-a-path with Tab completion and frecent ranking **(bg)**; not a persistent bar.
- **Terminal tools (zoxide, fzf, yazi):** the matching and ranking algorithm, not UI **(bg)**.
- **Marta / Path Finder / ForkLift:** path bars exist but are not frecency-driven **(bg)**; none is "always editable".

**What to steal**
| From | Steal |
|---|---|
| Explorer/Files | Click-empty-space-to-edit, chevron menus, revert on focus loss |
| Dolphin | Explicit states and a visible way back to breadcrumbs; `Esc` = leave edit, Ctrl+L = enter edit and select all (the two behaviours users argued about) [S20] |
| Nautilus | Shell-style Tab and inline completion |
| zoxide | Frecency ranking, subsequence matching, "jump from anywhere" |
| Alfred | Result list with an action per result (open, reveal, copy path) |
| Finder | Spring-loaded folders when dragging onto segments |
| TC/Opus | Directory hotlist with assignable keys feeding the same suggestions |
| yazi | Async candidate generation so typing never stalls on slow volumes |

**Design risks to decide explicitly**
- Edit-mode revert rules (focus loss vs Esc vs Return) so there are no surprises [S20].
- Suggestions from offline/slow volumes must come from the cached frecency DB, never a live walk, to hold the 16 ms budget.
- Do not touch cloud placeholders when completing paths.

---

## 6. Open questions for the user

1. **Dual pane default.** Off with a toggle (Finder-like first run) or on (Marta/Commander style)? Research suggests it is the top switching reason, but it may overwhelm casual users.
2. **Licence/commercial posture.** Pure free GPL-3 with no paid tier? The anti-feature table argues strongly for no tiers; a donation/sponsor link is the usual compromise.
3. **Scope of "never".** Are you comfortable with the explicit nevers (own search index, more than two panes, device browsing, secure erase, themes in v1)?
4. **Vim keymap timing.** v1 preset or M4? Engine is v1 either way.
5. **Per-folder view memory.** Default on (like Finder), off, or "smart" (only when you changed something)?
6. **Gallery/icon views in M2 or M3?** The brief leaves them loosely placed; I put them in M3.
7. **Photographer features at the edge of Import Studio:** how much culling-like functionality (ratings, side-by-side compare) belongs here? I assumed none.
8. **Hands-on gaps.** I could not find independent reviews of QSpace Pro or Nimble Commander's current state and only thin Reddit/HN text. Would you like a hands-on session with QSpace, Marta, Nimble and ForkLift 4 (trial licences) before M1 locks the keymap and address-bar behaviour?
9. **Code-reuse licensing.** Nimble Commander and Files are open source; the licence check (GPL-3 vs MIT) determines whether we can read or reuse their code. Should a dependency/licence audit session verify this?
10. **Cloud placeholders:** is "never materialise, show cloud icon, ask to download" acceptable as the default for iCloud, or do you want a per-folder override?

---

## Sources

- [S1] MacMost, "How to fix these 20 Mac Finder annoyances": https://macmost.com/how-to-fix-these-20-mac-finder-annoyances.html; Cocoatech, "Why Apple's Finder is not enough if you are in tech": https://cocoatech.io/post/why-apples-finder-is-not-enough-if-you-are-in-tech/ ; also the Lemmy and r/MacOS threads surfaced by the same search (reached through search snippets, not read in full).
- [S2] Apple Community, "Multiple Finder issues after macOS Tahoe 26.1": https://discussions.apple.com/thread/256193784 ; MacRumors, "macOS Tahoe Finder bug underscores Apple's slipping UI polish": https://forums.macrumors.com/threads/macos-tahoe-finder-bug-underscores-apples-slipping-ui-polish.2477639/page-9
- [S3] Apple Community, "macOS Tahoe File chooser / Download window unusable": https://origin-discussions2-us-dr-prz.apple.com/thread/256186450 ; Setapp Path Finder reviews: https://setapp.com/apps/path-finder/customer-reviews ; MacRumors "Best Finder alternative" thread: https://forums.macrumors.com/threads/best-finder-alternative.2258057/page-38
- [S4] Apple Community, "Finder extremely slow when opening large folders": https://origin-discussions2-us-dr-prz.apple.com/en/thread/256205645?page=2 ; https://discussions-jp-prz.apple.com/thread/254482112
- [S5] QNAP FAQ on Finder previews triggering downloads: https://www.qnap.com/en-ca/how-to/faq/article/why-does-the-qsync-file-status-change-from-online-only-to-locally-available-when-i-select-the-file-in-macos-finder ; Eclectic Light, "Are we ready for dataless files": https://eclecticlight.co/2023/08/06/last-week-on-my-mac-are-we-ready-for-dataless-files ; unverified Tahoe 26.4 report: https://macos-tahoe.com/blog/icloud-drive-auto-download-bug-macos-tahoe-26-4-fix-2026/
- [S6] Pomfort, "Why not to copy footage with Finder": https://pomfort.com/article/why-to-never-offload-footage-with-the-macos-finder/ ; SLR Lounge on verification: https://slrlounge.com/hedge-for-mac-backup-and-verify-files/
- [S7] TechRadar, Path Finder 5.0.3 review: https://techradar.com/reviews/pc-mac/software/utilities/other-software/cocoatech-path-finder-5-0-3-528156/review
- [S8] MacRumors, "Best Finder alternatives" thread: https://forums.macrumors.com/threads/best-finder-alternatives-file-managers.2258057/page-45 and https://spy.macrumors.com/threads/best-finder-alternatives-file-managers.2258057/page-46 ; roundup: https://macos-tahoe.com/blog/best-finder-alternatives-mac-file-managers-2026/ (commercial blog, low weight)
- [S9] Path Finder pricing and subscription history: https://tidbits.com/?p=74208 ; https://talk.tidbits.com/t/pathfinder-going-to-a-subscription-model-am-i-out/18179 ; https://mjtsai.com/blog/?p=35103 ; https://www.techbloat.com/software/path-finder/pricing/
- [S10] ForkLift 4: Pawel Grzybek, "ForkLift 4: ready or not": https://pawelgrzybek.com/forklift-4-ready-or-not/ ; BinaryNights, "Showing appreciation to our loyal users": https://blog.binarynights.com/2023/09/06/showing-appreciation-to-our-loyal-users-an-important-update/ ; MacPowerUsers thread: https://talk.macpowerusers.com/t/forklift-4-by-binary-nights-finder-alternative/34693/37 ; Setapp reviews: https://setapp.com/apps/forklift/customer-reviews ; version history: https://binarynights.com/versionhistory
- [S11] QSpace Pro MacUpdate listing: https://qspace-pro.macupdate.com
- [S12] Marta: https://marta.sh/ ; sspai review: https://sspai.com/post/112934 ; We Are Developers showcase: https://www.wearedevelopers.com/magazine/760-tool-showcase-marta-sh-a-fast-native-fully-customisable-and-extensible-two-panel-file-commander ; MacPowerUsers thread on a new keyboard-driven manager: https://talk.macpowerusers.com/t/built-a-keyboard-driven-file-manager-for-mac-would-love-your-feedback/44185
- [S13] Commander One reviews: Macworld https://www.macworld.com/article/3409155/commander-one-2-1-review-mac-file-manager-now-works-with-ios-devices.html ; OSNews https://www.osnews.com/story/28916/review-commander-one-finder-alternative/ ; pricing https://mac.eltima.com/commander-one-purchase.html ; Nimble Commander https://sourceforge.net/p/nimble-commander/code and https://apps.apple.com/in/app/nimble-commander/id905202937
- [S14] Directory Opus: https://dopus.com ; docs https://docs.dopus.com/ ; PCWorld review https://www.pcworld.com/article/486551/directory_opus_32bit.html ; SourceForge summary https://sourceforge.net/app/directory-opus/
- [S15] Zabkat, "Review: the best generic file manager software" (prices as of Feb 2026): https://www.zabkat.com/declone/best-windows-file-manager.htm
- [S16] Directory Opus forum, "Price of version 13": https://resource.dopus.com/t/price-of-version-13/46084?page=6 ; ElevenForum on the subscription: https://www.elevenforum.com/t/directory-opus-new-licensing-model-subscription.22192
- [S17] Total Commander vs XYplorer vs Opus discussions: https://www.xyplorer.com/xyfc/viewtopic.php?p=109944 ; https://newsgroup.xnview.com/viewtopic.php?p=100947 ; https://www.saashub.com/compare-total-commander-vs-xyplorer
- [S18] Files (Windows): Windows Central https://www.windowscentral.com/software-apps/windows-11/the-best-file-explorer-alternative-on-windows-11-just-got-better-at-handling-large-folders ; https://www.windowscentral.com/software-apps/files-v4-ultimate-file-explorer-replacement-windows-11 ; Reddit r/Windows11 thread (mirror): https://redlib.hbubli.cc/r/Windows11/comments/1kfvq86/what_do_you_think_of_the_files_app_windows
- [S19] GNOME Discourse, dual-pane request for Nautilus: https://discourse.gnome.org/t/feature-request-reintroduce-dual-pane-split-view-mode-in-nautilus/28364 ; KDE Discuss/Bugzilla on Dolphin split view: https://bugs.kde.org/show_bug.cgi?id=507619 ; https://discuss.kde.org/t/dolphin-what-could-change-stay-the-same-ideas/37624
- [S20] Dolphin location-bar behaviour: https://bugs.kde.org/show_bug.cgi?id=383984 ; https://bugs.kde.org/show_bug.cgi?id=412214 ; https://mail.kde.org/pipermail/kfm-devel/2018-November/024304.html
- [S21] yazi/ranger/nnn comparisons: https://blog.starmorph.com/blog/yazi-terminal-file-manager-guide ; https://maketecheasier.com/yazi-is-the-best-terminal-based-file-manager-you-are-looking-for ; r/commandline "Is yazi overhyped" (mirror): https://reddit.fsky.io/r/commandline/comments/1jcej42/is_yazi_overhyped ; https://bradleyfay.github.io/trek/comparison/
- [S22] NewsShooter on page-cache read-back caveat for verification: https://www.newsshooter.com/?p=319175
- [S23] diglloyd on FastRawViewer for organising shoots: https://diglloyd.com/blog/2023/20231204_1540-FastRawViewer-organizing-shoots.html

---

## Sign-off changes (user, 2026-11-12)

The ranking is approved with these changes, which override the table above:

| # | Feature | Was | Now |
|---|---|---|---|
| 85 | Disk-usage map (QDirStat/WinDirStat-style treemap) | never | **later (M4)**: builds on cached folder sizes (#22) |
| 42 | Vim keymap profile | later | **v1**: the engine ships in M1, and the profile is offered in first-run setup (M2) |
| 33 | Saved workspaces | later (M4) | **v1 (M2)**: early, alongside session restore, which uses the same core `Workspace` state (F-002) |
| 18 | Per-folder view memory | v1, optional | **v1, on by default in "smart" mode**: only folders whose view you changed are remembered; stored in the DB, never in `.DS_Store` |
| 19, 20 | Icon grid and gallery views | v1 (M3) | unchanged: M3 |

All the other "never" entries stand.

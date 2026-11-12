# PathBar/ — the address bar (address-bar session)

The always-visible address bar of each pane (F-009, F-018, F-019, HANDS-ON §8 and §10).
`PathBarPlaceholderView` (the window layer's name for it) is a typealias of `PathBarView`;
create it with `PathBarView(model:paneID:)` and call `show(_ tab:)` with the pane's active tab.
It adapts to any height from 20 pt (the capsule is at most 28 pt).

- `PathBarView.swift` — the state machine (REST → EDIT → COMPLETING), keys, Return,
  Missing → Create Folder, inline errors, the paste-cleanup chip, keystroke timing.
- `CrumbStripView.swift` — REST: crumbs, chevrons, "…" overflow, hover, drop targets and
  spring-loading, VoiceOver buttons. `CrumbLayoutPlan` is the pure fitting rule.
- `FolderMenu.swift` — the cascading subfolder menu of a chevron or "…" (→ submenu, typing selects).
- `PopupList.swift` — the `.popover` panel and row drawing shared by the list and the menus.
- `PathBarStyle.swift` — sizes, colours, icons.

Core calls used: `breadcrumbs`, `subfolders`, `complete`, `makeTabCycle` (Tab), `deletePathComponent`
(⌥⌫), `parseLocation` (paste chip), `perform(addressbar.commit…)`, `createFolderAndGo`,
`contextMenu(.crumb)`, `dropAction` / `dropFiles`. Keys arrive as `ShellRequest.textField(command:)`
from the core's AddressBarEditing bindings; ⌘L / ⇧⌘G / `/` / `~` as `.focusAddressBar`.

DEBUG builds print `PATHBAR keystroke→list <ms>` per keystroke (and an os_signpost interval
"keystroke→list", subsystem app.filestudio, category PathBar).

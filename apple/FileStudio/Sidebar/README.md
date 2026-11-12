# Sidebar/ — window/sidebar/tabs session

`SidebarViewController.swift`: a source-list `NSOutlineView` (native keyboard
navigation, real NSMenus, drag and drop) in the split view's sidebar item
(`.sidebar` material), 200 pt, filter field at the bottom (F-019).

- Favourites from `AppModel.favourites` (the core's store):
  - drag to reorder (`moveFavourite`);
  - drop folders from the list, Finder or another app to pin them at the drop
    position (`addFavourite`; the core de-duplicates by folder);
  - drag one out of the sidebar and let go (poof, `Poof.swift`), or select it
    and press ⌫ / ⌘⌫, to unpin it (`sidebar.remove`);
  - right-click › Remove from Favourites / Rename Favourite… (the core asks
    for the name with `ShellRequest.askFavouriteName`; `SidebarServices` shows
    the sheet);
  - a favourite whose folder has gone (`Favourite.missing`) is dimmed with a
    question-mark folder; clicking it offers Remove.
  **F-023:** every sidebar drag is a reference. Rows carry only private
  pasteboard types (`app.filestudio.sidebar-favourite`, `…sidebar-item`),
  never a file URL, and the list and crumb drop targets also refuse any drag
  whose source is the sidebar (`SidebarViewController.isSidebarDrag`). No
  gesture inside the sidebar can queue a file operation; reorder and pin
  touch only the favourites store. `SidebarDragProbe.swift` (`simdrag:`)
  replays sidebar drags onto list rows, crumbs and sidebar rows through the
  real handlers.
- Recent Folders from `engine.frecent("", …)` (favourites and home left out).
- Locations from `AppModel.volumes` with free space; removable and network
  volumes get an eject button → `drive.eject` → `ShellRequest.eject`, carried
  out off the main thread with `NSWorkspace.unmountAndEjectDevice`; failures
  show an alert and a status message (`SidebarServices`).
- Tags: Finder's tag names and colours (browsing by tag needs search, M3).
- A click or arrow key goes to the folder in the focused tab; middle-click
  opens it in a new tab (`nav.open_new_tab`); right-click is the core's
  `contextMenu(.sidebarItem…)` (recent folders use the folder menu).

`SidebarDebugSteps.swift` has the favourites script steps (`favlog`,
`favdrop`, `favmove`, `favdragout`, `favselect`, `favmenu`, `foldermenu`,
`tabmenu`, `favrename`).

Off-screen window snapshots draw the sidebar blank; use the `sidebarsnap:`
debug step (it renders the sidebar's own layers) or `winlog` (SIDEBAR lines).

# Sidebar/ — window/sidebar/tabs session

`SidebarViewController.swift`: a source-list `NSOutlineView` (native keyboard
navigation, real NSMenus, drag and drop) in the split view's sidebar item
(`.sidebar` material), 200 pt, filter field at the bottom (F-019).

- Favourites from `AppModel.favourites`: drag to reorder (`moveFavourite`),
  drop folders to add (`addFavourite`).
- Recent Folders from `engine.frecent("", …)` (favourites and home left out).
- Locations from `AppModel.volumes` with free space; removable and network
  volumes get an eject button → `drive.eject` → `ShellRequest.eject`, carried
  out off the main thread with `NSWorkspace.unmountAndEjectDevice`; failures
  show an alert and a status message (`SidebarServices`).
- Tags: Finder's tag names and colours (browsing by tag needs search, M3).
- A click or arrow key goes to the folder in the focused tab; middle-click
  opens it in a new tab (`nav.open_new_tab`); right-click is the core's
  `contextMenu(.sidebarItem…)` (recent folders use the folder menu).

Off-screen window snapshots draw the sidebar blank; use the `sidebarsnap:`
debug step (it renders the sidebar's own layers) or `winlog` (SIDEBAR lines).

# Sidebar/ — window/sidebar/tabs session

Favourites (from `AppModel.favourites`; edit with `addFavourite`,
`moveFavourite`, `renameFavourite`, `removeFavourite`), Locations (from
`AppModel.volumes`), later Tags; 200 pt, `.sidebar` material, filter field at
the bottom (F-019). Right-click menus come from
`engine.contextMenu(.sidebarItem(path:kind:), …)`; handle
`.askFavouriteName`, `.eject` and `.showDriveInfo` as needed (Platform has
defaults for eject and drive info).

`SidebarPlaceholderView.swift` is a minimal SwiftUI list showing that wiring.
Known: off-screen debug snapshots draw the sidebar material blank.

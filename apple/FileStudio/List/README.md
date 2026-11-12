# List/ — list/column session

The list view (F-005, F-019): view-based `NSTableView`, 22 pt rows (density
from settings), columns from `ViewState`, 120 Hz scrolling over 100k rows,
rubber band, rename in place (`.beginRename` → `engine.renameItem`), drag and
drop (`engine.enqueue`), the type-to-select and filter UI.

Contract: rows come only from `tab.api.rows(start:count:)` for the visible
range, cached per `listing.version`; selection changes are
`SelectionChange`s sent to the core and drawn back from `Row.isSelected`;
keys go through KeyRouter (adopt `CoreFocusReporting` with `.list`);
right-click is `engine.menuTarget(tab:row:)` then `engine.contextMenu(...)`,
drawn with `ContextMenuBuilder`. Report `setVisibleRange` and
`setScrollAnchor` as the user scrolls.

`ListPlaceholderView.swift` shows that wiring in its simplest form; replace it.

# List/ — list view (owned by the list/column session)

The pane's file area (F-005, F-019). `ListPlaceholderView` keeps its
placeholder name because the window embeds it: `ListPlaceholderView(model:)`,
`.scrollView` (an NSView container that shows the list or the column view),
`.table` (the focused table, for first responder) and `.show(tab)`.

- `FileTable.swift`: one table of files (list view, or one column of the
  column view). View-based NSTableView with reused, layer-backed cells that
  draw themselves. Rows come from `rows_packed` blocks (RowSource.swift)
  cached per listing version; the selection is the core's (NSTableView's own
  stays empty); clicks, ⌘/⇧-clicks and the rubber band send `SelectionChange`s;
  keys go through KeyRouter (`.list` focus). Partial→Sorted and re-sorts keep
  the top item; small visible patches fade (not under Reduce Motion). Drag out
  as file URLs; drops go through `dropEffect`/`dropItems` (the core decides
  move or copy) with spring-loaded folders after 0.8 s. Right-click is
  `menuTarget` + `contextMenu`. Empty, loading (spinner after 150 ms) and error
  states (Full Disk Access hint for EPERM) are `ListStateView`.
- `FileCells.swift`: drawn cells and the row view (selection in the accent
  colour, emphasised only in the active pane; VoiceOver labels).
- `ListPlaceholderView.swift`: columns, order, widths and sort from the core's
  `ViewState` (per-folder memory is the core's); header click = `sortByColumn`;
  header right-click = column chooser; handles `beginRename`,
  `textField(rename.*)` and Quick Look (asks before previewing dataless items,
  follows the selection while open).
- `RenameController.swift`: rename in place (Finder's selection without the
  extension; Return/focus loss commit through `renameItem`; Esc cancels;
  errors inline).
- `ListDebug.swift` (DEBUG): automation steps `bench:scroll`, `bench:open`,
  `drop:`, `dropeffect:`, `density:`, `appearance:`, `columns:`, `sortcol:`,
  `headermenu`, `ctx:`, `select:`, `state:`, `listlog`, `snapview:`,
  `viewtree:`, `resize:` (see the file header).

On macOS 26 a view that draws with `draw(_:)` inside a nested scroll view can
end up in a shared content layer that covers its siblings: give such views
their own layer (`wantsLayer`, `updateLayer`).

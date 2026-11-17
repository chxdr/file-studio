# Grid/ — icon view and gallery (M3)

Both are shown by `List/ListPlaceholderView` when the tab's view mode is Icons
(⌘1) or Gallery (⌘4); the mode is remembered per folder by the core.

The core decides (`crates/fstudio-core/src/grid.rs`): the icon layout
(`gridLayout`: items per row, cell size), the arrow keys in 2D (`handle_key`;
the shell reports the column count with `setGridColumns`), ↑/↓/j/k moving a
row, ←/→ stepping through the gallery, the thumbnail size (`grid.icon_size`,
⇧⌘= / ⇧⌘-, the slider), the pixel sizes asked for (`thumbPixels`,
`galleryPixels`) and the order thumbnails are made in (`thumbnailRequests`,
`galleryRequests`). It never lists an iCloud placeholder (F-016), a folder or a
package.

- `GridAPI.swift` the contract (`GridEngineAPI`), `grid.*` settings, and
  `GridMarks` (Import Studio culling marks; see the TODO there).
- `GridRust.swift` / `GridMock.swift` the engines' side.
- `ThumbnailLoader.swift` QuickLook thumbnails off the main thread, in the core's
  order, a few at a time, with work that left the core's list cancelled; one
  shared cache (384 MB). When QuickLook won't make a size (camera raws stop at
  their embedded preview), ImageIO's embedded preview is used.
- `GridTile.swift` one layer-drawn tile (picture, name, selection, marks).
- `TileGrid.swift` the scrolling collection used by both views: tile reuse,
  clicks/⌘/⇧/rubber band, double-click opens, drag out, drops with
  spring-loaded folders (the core decides move or copy), context menu, rename
  (`RenameHost`), focus (`.list`). The filmstrip's scroll view turns a mouse
  wheel's vertical turn into sideways scrolling.
- `IconGridView.swift` the grid plus the size slider bar.
- `GalleryView.swift` big picture, name, marks, photo details (the preview
  pane's summaries), filmstrip.
- `GridDebugSteps.swift` (DEBUG) `gridlog`, `gridwait`, `gridmarks:fake`,
  `gridbench:<s>:<wheel|smooth>[:<speed>]`, `gridsize:`.

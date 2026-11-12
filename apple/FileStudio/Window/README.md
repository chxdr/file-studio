# Window/ — window/sidebar/tabs session

The main window (F-002, F-019). The core owns windows, panes, tabs and the
split; everything here mirrors `WindowModel` and sends changes back through the
engine (`activateTab`, `moveTab`, `focusPane`, `toggleDual`, `setLayout`).

- `MainWindowController.swift`: one per core window. Unified toolbar, sidebar
  split (200 pt), panes. Clicking or focusing a pane makes it active in the
  core; the core's active pane pulls keyboard focus. Mouse buttons 4/5 run
  `nav.back`/`nav.forward`. Tab/⇧Tab order: sidebar → address bar → list (per
  pane) → sidebar filter. Handles `focusList`, `focusFilter`, `toggleSidebar`.
  Frames autosave per core window. Keep `CoreWindowIdentifying`, `allPanes`,
  `activePane` (other folders use them).
- `WindowToolbar.swift`: the NSToolbar (≤ 10 default controls, customisable):
  back/forward, view mode (icons/gallery greyed until M3), dual pane (hold for
  layout: side by side/stacked, swap, equalise), new folder, Trash, share, the
  ops slot (`OpsPlaceholderView`, 44×28 pt), the filter field (`.filter`
  focus; `setFilter` on the focused tab).
- `PaneSplitView.swift`: one or two panes, 1 pt hairline, 8 pt drag target,
  double-click equalises (`pane.equalize`); the ratio goes to `setLayout`.
- `PaneViewController.swift`: tab bar · 36 pt header (location glyph, the
  address bar, the "N items, X GB available" subtitle) · list or columns ·
  status line. Active pane: 2 pt accent outline + 3 pt header bar (dual only).
- `PaneTabBar.swift`: SwiftUI tabs with a sliding pill, drag to reorder or to
  the other pane, close on hover, middle-click closes, +, core tab menu.
- `PaneStatusLine.swift`: counts, selection size, free space; in the active
  pane also pending keys, status messages (cleared after 5 s) and the queue.
- `WindowSupport.swift`: `PaneContent` (the list/column hand-shake),
  `WindowPreferences` (reads tab bar always / setup done from core settings; one-time UserDefaults migration),
  `Motion`, `PaneText`, `MiddleClickCatcher`.
- `WindowDebugSteps.swift`: DEBUG script steps (`winlog`, `layout:`,
  `movetab:`, `setup:`, `sidebarsnap:`, `button:`, `focus:`, `eject:`…).

Embedding other folders' views: `PathBarPlaceholderView(model:paneID:)` +
`show(_:)`; `ListPlaceholderView(model:)` through `PaneContent`;
`ColumnsPlaceholderView()` is used once it adopts `PaneContent` (until then
columns mode shows the list); `OpsPlaceholderView(model:)` in the toolbar.

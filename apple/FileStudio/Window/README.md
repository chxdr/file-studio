# Window/ — window/sidebar/tabs session

The main window: unified toolbar (~52 pt, ≤ 10 controls), the sidebar split,
one or two panes with a tab bar each (sliding selection pill, D-026), the
active-pane outline and header bar (F-019), the 1 pt divider (double-click
equalises → `pane.equalize`).

`MainWindowController.swift` is a working placeholder: it mirrors
`WindowModel.state` (panes and their active tabs), stacks a path-bar strip,
the list and a status line per pane, sets the first responder and reports the
key window to `AppModel`. Replace its layout freely; keep:
- `CoreWindowIdentifying` on the controller (KeyRouter and debug automation use it);
- closing a window calls `engine.closeWindow`, and windows open/close only
  from `AppModel.windows` (the core decides);
- tab, pane and layout changes go through the engine (`activateTab`,
  `moveTab`, `toggleDual`, `setLayout`), never local state.

# Views/ — ops-UI session (shared bits)

Settings (SwiftUI tabs per PROPOSAL §8; read `engine.settings()`, write
`engine.updateSettings`, keys via `setCommandKey`, menus via `menuAdd`/
`menuRemove`/`menuMove`/`resetMenu`), "Did you know?" toasts
(`AppModel.hint`), the held-modifier overlay (`engine.heldShortcuts`), the
status line, first-run setup.

`SettingsPlaceholderView` and `StatusLinePlaceholderView` show the wiring;
replace them.

# App/

- `main.swift`, `AppDelegate.swift`: launch, one `MainWindowController` per
  core window, Settings window, quit.
- `EngineFactory.swift`: the only place that picks `RustEngine` or `MockEngine`.
- `AppModel.swift`: the main-actor model. Routes `CoreEvent`s to `WindowModel`
  and `TabModel`, runs commands (`perform`), keys (`handleKey`) and
  outcomes (`handle`), and dispatches `ShellRequest`s to registered handlers.
  Its header lists the extension points; build on those rather than editing it.
- `KeyRouter.swift`: every key-down goes to the core first. Views report
  their focus with `CoreFocusReporting`.
- `KeyChord+NSEvent.swift`: NSEvent → `KeyChord`, nothing more.
- `MainMenu.swift`: the menu bar from the command registry (first pass).
- `DebugAutomation.swift`: `-FSDebugScript` (DEBUG only).
- `Tour*.swift`: the power tour (`app.power_tour`, Help › File Studio Power
  Tour, and "Take the 1-minute tour" on setup's last page). `TourCards` holds
  the eight cards; their keys (and the `{command}` placeholders in their text)
  come from `engine.commandKeys`, so rebinding updates the tour. `TourDemos`
  plays the looping demos (frozen under Reduce Motion), built from the app's
  own views fed a fixed dataset (`TourDemoParts`: real list rows and cells,
  pane header, crumbs, suggestion list, pane outline; plus the real held-⌘
  overlay, hint, toast, queue popover, ring, conflict sheet and Shortcuts key
  slots). Setup's single/dual-pane cards use the same panes. A live pane is
  not possible: `AppModel` is one per process. When a view changes, the demo
  changes with it; when what the core *says* changes (titles, kinds, toast
  text), update the fixed strings in `TourDemos`. Debug steps:
  `tour:<n>|menu|close`, `tourlog` (`TourDebugSteps`).

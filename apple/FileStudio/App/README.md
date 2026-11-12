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

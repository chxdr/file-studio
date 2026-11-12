# File Studio for Mac (the shell)

A thin AppKit/SwiftUI shell over the Rust core (F-001). Swift draws, turns
input into core calls and makes Cocoa-only OS calls. It never decides what a
key, click or menu item means.

## Build and run

```bash
cd apple && xcodegen          # generates FileStudio.xcodeproj (not committed)
xcodebuild -project FileStudio.xcodeproj -scheme "File Studio" -configuration Debug build
```

The `RustCore` target runs `scripts/build-core-apple.sh` first (release static
library, Swift bindings in `FileStudio/Core/Generated`, headers in
`target/uniffi-headers`). The app links `libfstudio_ffi.a` directly (F-004,
Import Studio's D-024).

Launch flags:
- `-FSMockCore`: run on `Mock/MockCore.swift`, no Rust.
- `-FSDataDir <dir>` (DEBUG): use a throwaway database folder.
- `-FSDebugScript "…" -FSDebugSnapshotDir <dir>` (DEBUG): scripted run, see
  `App/DebugAutomation.swift` for the steps.

`scripts/run-debug.sh [--mock] "step;step;…"` builds and runs a script in one go.

## Layout and owners

| Folder | What lives there | Owner |
|---|---|---|
| `Core/` | `CoreAPI.swift` (the only interface views use), `RustCore.swift`, `Generated/` | foundation; changes go through the coordinator |
| `Mock/` | `MockCore.swift` | foundation; extend it when you need new fake data |
| `App/` | launch, `AppModel`, key routing, menus, debug automation | foundation |
| `Platform/` | Cocoa-only OS work: Quick Look, NSWorkspace, Terminal, icons, pasteboard, volumes, context-menu drawing | foundation, shared |
| `Window/`, `Sidebar/` | window, toolbar, panes, tab bar, sidebar | window/sidebar/tabs session |
| `PathBar/` | the address bar | address-bar session |
| `List/`, `Columns/` | list and column views | list/column session |
| `Ops/`, `Views/` | queue UI, conflict sheet, Settings, toasts, overlay | ops-UI session |

Each folder has a README saying what belongs there. The placeholders in the
UI folders are meant to be replaced; they show how the wiring works.

## Rules

- Views talk to `EngineAPI`/`TabAPI` and the value types in `CoreAPI.swift`,
  never to `Ffi…` types.
- Add your own models in your own folder and attach them to a window or tab
  (`windowModel.attachment(MyModel.self) { … }`); don't edit `AppModel`.
- Handle the `ShellRequest`s your views own with
  `model.registerShellHandler(for:)`.
- Keyboard focus: adopt `CoreFocusReporting` so `KeyRouter` tells the core
  where a key was pressed.

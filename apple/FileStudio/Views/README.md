# Views/ — ops-UI session (shared bits)

Toasts, the held-modifier overlay, the "Did you know?" hint, the status line,
and the look-and-motion settings. Settings itself is `App/Settings*.swift`.

## Overlays (drawn over every core window)
`OverlayController` (installed from `AppDelegate`) puts one click-through
`OverlayHost` on each core window and draws, from `OverlayState`:
- **Held modifiers** (`ShortcutToasts.swift`): ⌘ ⇧ ⌥ ⌃ (or Fn alone) held for
  `overlay.delayMs` asks the core (`engine.heldShortcuts`) and shows a pill,
  "+", and key cards grouped by category, in `overlay.corner`. It knows no
  shortcuts itself. The cards flow into as many columns as fit beside the
  pill (up to four), shrink to compact cards when the full size would
  overflow, and only then scroll: while the list is up its panel takes the
  mouse wheel (`OverlayHost.scrollWheel`), so a mouse scrolls it with the
  modifier still held; elsewhere the overlay stays click-through.
- **Hint**: whatever sets `AppModel.hint` (an `Outcome.hint` after a command
  done with `Via.mouse` / `.menu`, or a `.hint` event) is shown for
  `hints.seconds` in `hints.corner`, with a bounce. Its ⋯ menu turns hints off
  or less frequent and opens Settings › Shortcuts.
- **Toasts**: every new `AppModel.statusMessage` becomes a toast (undo
  refusals, errors, "Copied path").

## Showing a message yourself (ToastCenter)
```swift
ToastCenter.of(model).show("Cleaned up paste: removed quotes")                    // info
ToastCenter.of(model).show("Couldn't undo", detail: coreMessage, style: .error)   // .info .success .warning .error
ToastCenter.of(model).show("Moved", actionTitle: "Undo") { … }                    // with a button
```
They appear bottom-centre of the front core window, stack up to three, and
fade after `seconds` (default 3.5). `ToastCard` is the reusable look, for
anything that wants the same card (the address bar's paste chip, say).

## Motion
`AppearanceModel.of(model)` reads `appearance.*` from core settings:
- `motion` (Full / Reduced / None; the system's Reduce Motion caps Full at
  Reduced): use `appearance.animation(.spring(...))` (nil when nothing should
  move) and `appearance.transition(...)` (the full one, a fade, or none)
  instead of raw animations.
- `tabBarAlways`, `pathBarSubtitle`: for the window and path-bar sessions.

## Status line
`StatusLinePlaceholderView(model:tab:)` (name kept for the window layer)
shows counts, the running operation ("Copying… 42 %", click opens the queue
popover), pending keys and the last message.

`DebugSteps.swift` adds debug-automation steps (see its header).

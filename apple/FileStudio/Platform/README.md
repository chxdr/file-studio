# Platform/

Cocoa-only work the core asks for (F-021), shared by every session:
- `PlatformActions.swift`: default handlers for open, Open With, editor,
  Quick Look, Get Info, reveal, Open in Terminal (Terminal, iTerm or Ghostty
  per `settings.general.terminalApp`), Copy Path, file pasteboard, eject.
- `QuickLook.swift`: the `QLPreviewPanel` data source.
- `IconCache.swift`: icons by the core's icon key, cached.
- `Pasteboard.swift`: paths and file URLs.
- `VolumeWatcher.swift`: mount/unmount/rename → `engine.volumesChanged()`.
- `ContextMenuBuilder.swift`: draws the core's `[MenuItem]` as an `NSMenu`
  and fills the system submenus.

Nothing here decides what to do, only how macOS does it.

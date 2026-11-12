# Ops/ — ops-UI session

- `OpsPlaceholderView(model:)` (name and init kept for the window layer): the
  **toolbar item**. A progress ring (overall progress while ops run) with a
  badge for pending conflicts (orange), failed items (red) or interrupted ops
  (blue). Click: the queue popover.
- `OpsQueueView`: active ops (progress, bytes and files, speed, ETA, current
  file; pause/resume, cancel, drag to reorder), failed items per file, finished
  ops until cleared (Show in Folder), interrupted ops from a previous run
  (Resume / Cancel).
- `ConflictSheet`: sheet on the front window, both items side by side, Keep
  Both (Return) / Replace (⌘R) / Skip (S, Esc) / Merge (M, folders only),
  Apply to all (A), Compare (⌘Y, Quick Look), "3 of 12". The core decides what
  each choice does.
- `ConfirmSheets`: `.confirmTrash` / `.confirmDeleteImmediately` questions; on
  yes the command is performed again with `.confirmed`.
- `OpsModel` (`OpsModel.of(model)`): queue order, per-file errors,
  interrupted ops, pending conflicts, `statusText` ("Copying… 42 %").

Edit-menu Undo/Redo titles come from `AppModel.undoState` (the core words
them); `Views/OverlayController` keeps the menu items current.

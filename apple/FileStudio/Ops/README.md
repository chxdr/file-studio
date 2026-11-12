# Ops/ — ops-UI session

The operation queue UI (PROPOSAL §10): toolbar progress ring and popover queue
(pause/resume/cancel/reorder through the engine), the conflict sheet
(`.opConflict` → `resolveConflict`, with Keep Both / Replace / Skip / Merge and
"apply to all"), interrupted ops at launch (`interruptedOps`, resume or cancel),
per-file failures (`.opFailed`), undo labels (`AppModel.undoState`).
Confirmation sheets: `.confirmTrash`, `.confirmDeleteImmediately`.

`AppModel.ops` holds the latest `OpStatus` of each op. `OpsPlaceholderView`
shows a tiny list in the sidebar; replace it.

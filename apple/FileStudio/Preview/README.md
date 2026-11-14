# Preview pane (M3)

The right-hand inspector that follows the cursor: a picture, then details.

- **Core:** `crates/fstudio-preview` makes a `Preview` per path (EXIF for JPEG, HEIC, PNG, WebP, TIFF-based
  raws, Canon CR3 and Fuji RAF; codec, duration and frame rate from the `moov` box of MP4/MOV; encoding,
  line count and syntax spans for text; item count for folders; tags, dates, size). Settings: `preview.*`.
- **Never fetches:** an iCloud placeholder (`SF_DATALESS`) is described from `stat` only. The pane shows a
  Download button; nothing downloads by itself (F-016). QuickLook thumbnails are never requested for one.
- `PreviewAPI.swift` the contract (`EngineAPI.makePreviewer()`); `RustPreviewer.swift` / `MockPreviewer.swift`.
- `PreviewModel.swift` follows the cursor: cancel the previous work, wait 45 ms, summarize on a background
  task (cached in the core by path, mtime, size), then ask QuickLook (`Thumbnails.swift`) for a picture.
- `PreviewPanel.swift` the SwiftUI pane; `SyntaxTheme.swift` token class → colour; `PreviewController.swift`
  hosts it as an `NSSplitViewItem(inspectorWithViewController:)` (wired in `MainWindowController`).
- `SettingsPreviewsPane.swift` Settings → Previews. Debug steps: `PreviewDebugSteps.swift`.
- Toggle: View → Show or Hide Preview (⌥⌘P, also ⌥⌘I), command `view.toggle_inspector`.

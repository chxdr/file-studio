# File Studio as the Mac's file explorer: handover

Branch `claude/elegant-hofstadter-de0709`. Tested on macOS 27.2.1 (26B131).

## What it does

1. **Default file viewer** (Settings › General › Default file viewer).
   - **Make File Studio the Default File Viewer** writes the global
     `NSFileViewer` preference with this app's bundle id. It also asks
     LaunchServices for `public.folder`.
   - **Switch Back to Finder** removes `NSFileViewer`. It gives
     `public.folder` back only if Finder no longer has it.
   - The status line comes from the core (`viewer_status`). It names the
     viewer: Finder, this app, the other File Studio copy, or another app.
   - Folders from other apps open in a new tab or a new window, set by
     `integration.folder_opens_in`.
2. **Open/Save panel jump** (Settings › General › Open and Save panels).
   - It is off by default (`integration.panel_jump`). Turning it on explains
     why Accessibility is needed, then shows macOS's prompt.
   - The `panel.jump_here` command is registered in the keymap with ⌃⌥⌘G,
     and the user can rebind it. While the feature is on, that key is a
     system-wide Carbon hotkey.
   - The core says which folder (the active pane's), whether a window is a
     panel (`classify_panel`; AppKit panels carry `AXIdentifier`
     `open-panel` / `save-panel`), and where the floating button goes.
   - The shell raises the panel and types ⌘⇧G. It waits for the Go to Folder
     field and sets its value through AX, falling back to typing. Then it
     presses Return.
   - The optional floating "Go to “Folder”" button (`integration.panel_jump_button`)
     is a non-activating panel. It polls twice a second while the feature is on.
3. **⌘C → chat apps.** No change was needed.
   - `Pasteboard.writeFiles` writes one `public.file-url` item per file, and
     macOS derives the legacy `NSFilenamesPboardType` list from them.
   - It also writes the paths as plain text on the first item. This is the
     same shape as Finder's copy; Finder's text is the names instead of the
     full paths.

## Platform limits found

- **Folders opened directly can't be taken from Finder.** This covers
  `open <folder>` and a folder in the Dock.
  - LaunchServices refuses to change the `public.folder` handler for any app,
    even back to Finder: `NSWorkspace.setDefaultApplication` and
    `LSSetDefaultRoleHandlerForContentType` both return paramErr (-50).
  - Only `NSFileViewer` (reveals) can be switched. Settings says so.
- **A revealed folder opens instead of being selected in its parent.**
  - `NSWorkspace`'s reveal reaches the `NSFileViewer` app as a plain `odoc`.
    Its keys are `----` and `aapd`, identical to `open -a "File Studio" <folder>`.
  - Files are fine: they show in their folder, selected.
  - Finder's own reveal event (`misc`/`mvis`) is handled too, for scripts.
- **Debug and Release don't fight, but LaunchServices picks the copy.**
  - Each app only ever writes its own bundle id, and only when its button is
    clicked. Settings shows "File Studio Debug is your default file viewer"
    from the Release app, and the other way round.
  - With the Debug app as viewer and not running, LaunchServices chooses
    among every registered `app.filestudio.mac.debug` copy. There are many
    old worktree builds.
- **The keystrokes go to the app in front.**
  - The jump activates the panel's app first.
  - Before ⌘⇧G, before ⌘A and typing, and right before Return, it checks
    `panel_key_refusal` (core, unit-tested). The panel's app must still be
    in front, the Go to Folder field still focused, the panel still open,
    and, before Return, the field's value read back right then must equal
    the path. Otherwise it stops with a message and never presses Return.
  - A panel that ignores ⌘⇧G or has no Go to Folder field gets a message
    and no typing.
- **Open question:** twice, while a full `cargo test --workspace` ran at the
  same time, the `sysreveal:` debug proof showed the new tab listed but
  nothing selected. It was not reproduced in 8 later app runs, or in 18
  repeated and parallel runs of the core tests.

## Verified automatically

- **Rust:**
  - `system_integration` unit tests: viewer states, open/reveal plans
    (NFD and case names, root, trailing slash), panel classification,
    button placement, hotkey rules.
  - `tests/system_open.rs`: engine opens and reveals on temporary folders,
    covering new tab vs window, reusing the front tab, literal `$`/`~`/quote
    names, NFD, missing paths, symlink loops and dangling links, the
    `panel.jump_here` command, and settings defaults.
- **run-debug (real app, Rust core):**
  - `sysopen:` opens the folder in a new tab.
  - `sysreveal:` sends a real `misc/mvis` event to the app, and the parent
    opens with both files selected.
  - `panelprobe:open|save` shows that AX and the core classify AppKit Open
    and Save panels.

## Verified by hand in this session (real system flow, then restored)

- **Making the Debug app the viewer:**
  - `open -R <file>` landed in File Studio with the file selected.
  - A JXA call to `NSWorkspace.activateFileViewerSelectingURLs` from another
    process (the call Safari's "Show in Finder" makes) did the same, with
    two files selected.
- **File Studio's own Reveal in Finder** while it is the viewer reached
  Finder and didn't loop back.
- **Cold launch:** an open event while the app wasn't running was queued and
  then shown.
- **Switch Back** removed `NSFileViewer`, and the next JXA reveal went to
  Finder (no event reached File Studio).
- `NSFileViewer` is unset on this Mac again.

## Not verified (needs a person)

- Safari's or Chrome's "Show in Finder" clicked by hand. Chrome isn't
  installed; Firefox is.
- The panel jump against TextEdit, Safari's upload panel and Discord
  (Vesktop). Desktop control was declined, and the self-test only runs while
  File Studio Debug is in front, which macOS didn't allow mid-session.
- ⌘V of copied files in Discord and Messages.
- The floating button's look and position.
- Whether macOS asks "File Studio wants to control Finder" the first time
  Reveal in Finder runs while File Studio is the viewer. If refused, it opens
  the folder in Finder without selecting.

## Shared files touched (small blocks)

- **fstudio-core:**
  - `engine.rs`: `new_window_with`, and `insert_tab` is now `pub(crate)`.
  - `settings.rs`: the `integration` group.
  - `outcome.rs`: `ShellRequest::JumpFilePanel`.
  - `commands.rs`: `panel.jump_here`.
  - `lib.rs`.
- **fstudio-keymap:** `registry.rs` (one command) and `tests/command_ids.txt`.
- **fstudio-ffi:** `types.rs` (one variant) and `lib.rs` (`mod`).
- **Swift:**
  - `CoreAPI.swift`: one case, its kind, and the `openFromSystem` requirement.
  - `RustCore.swift`.
  - `AppDelegate.swift`: three hooks.
  - `PlatformActions.swift`: reveal.
  - `SettingsPanes.swift`: one line, plus Show in Finder.
- **project.yml:** `INFOPLIST_FILE` (`apple/Info-FileStudio.plist`:
  `public.folder`, rank Alternate), `CODE_SIGN_ENTITLEMENTS`
  (`apple/FileStudio.entitlements`: `automation.apple-events`), and the
  Apple Events usage text.
- No new dependencies.

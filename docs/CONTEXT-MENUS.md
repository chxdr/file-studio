# Context menus: default contents (spec for F-022)

Status: proposed 2026-11-12, waiting for the user's review.

**How these menus work:**
- The core builds each menu from registered commands (`fstudio-keymap` ids in brackets), and the shell draws it.
- Every menu can be customised in Settings: add, remove and reorder items, and add custom actions (M4).
- Items that don't apply are **hidden**, not greyed out. The exception is Paste, which is greyed when the clipboard is empty, as in Finder.
- The order follows Finder where Finder has the item. Power-user items form their own groups.
- Each item shows the user's current binding as its shortcut.
- **Favourites:** folder-like targets (folder, package, several items with at least one folder, address-bar segment, tab) offer **Add to Favourites** `favourites.add` (⌃⌘T, as Finder's Add to Sidebar). When the folder is already a favourite (for several items: every folder among them is), it is hidden and **Remove from Favourites** `favourites.remove` shows instead. A favourite is matched by the folder it points at, so a symlink or an NFC/NFD spelling of a pinned folder counts as pinned.

## 1. File
1. Open `nav.open` (⌘O)
2. Open With › `file.open_with`: the default app, the other apps that open this type, Other…
3. Open in New Tab `nav.open_new_tab` · Open in Other Pane `nav.open_other_pane` (dual pane only)
4. ───
5. Quick Look `preview.quick_look` (Space) · Get Info `preview.get_info` (⌘I)
6. ───
7. Rename `file.rename` (Return) · Duplicate `file.duplicate` (⌘D) · Make Alias `file.make_alias` (later) · Compress `file.compress` (later)
8. ───
9. Copy `edit.copy` · Cut `edit.cut` (marks the item to move; deletes nothing) · Paste `edit.paste` (greyed when the clipboard is empty)
10. Copy to Other Pane `file.copy_to_other_pane` (F5) · Move to Other Pane `file.move_to_other_pane` (F6), dual pane only
11. ───
12. Copy Path ›
    - Path `path.copy_path`
    - Name `path.copy_name`
    - Escaped `path.copy_escaped`
    - Relative to ~ `path.copy_home_relative`
    - Quoted `path.copy_quoted`
    - File URL `path.copy_file_url`
13. ───
14. Tags › (Finder tags as coloured dots, plus Tags…) *(new command: `tags.toggle` with a tag argument, `tags.edit`)*
15. ───
16. Reveal in Finder `file.reveal_in_finder` · Open in Terminal `terminal.open` (at the file's folder) · Open in Editor `file.open_in_editor`
17. Share › · Quick Actions › · Services ›: the system's own submenus, drawn by the shell
18. ───
19. Move to Trash `file.trash` (⌘⌫)
20. Delete Immediately… `file.delete_immediately`, only when the setting is on

## 2. Folder
The same as a file, with these changes:
- Open in New Tab and Open in Other Pane move directly under Open.
- Open in Terminal opens inside the folder.
- Adds Get Folder Size `file.folder_size` and Show Disk Usage `nav.disk_usage` *(M3)*, after Get Info.
- Packages also get Show Package Contents *(new command: `nav.show_package_contents`)*.
- Add to Favourites `favourites.add` (⌃⌘T) or Remove from Favourites `favourites.remove` follows Open in Editor, in the Reveal/Terminal group.

## 3. Multiple selection
1. Open (opens each one)
2. Quick Look N Items · Get Info
3. ───
4. Batch Rename N Items… *(M3)* · Duplicate · Compress N Items (later)
5. New Folder with Selection `file.new_folder_with_selection`
6. ───
7. Copy / Cut / Paste · Copy or Move to Other Pane
8. Copy Paths › (as in the file menu, one path per line)
9. ───
10. Add to Favourites `favourites.add` (adds only the folders) or Remove from Favourites `favourites.remove`
11. ───
12. Tags › · Share ›
13. ───
14. Move N Items to Trash

## 4. Empty space in a folder
1. New Folder `file.new_folder` · New File `file.new_file`
2. Paste · Paste as Move `file.move_here`
3. ───
4. View as › `view.icons` / `view.list` / `view.columns` / `view.gallery`
5. Sort by › `sort.name` / `sort.kind` / `sort.size` / `sort.date_modified` / `sort.date_created`; Reverse `sort.reverse`; Folders First `sort.folders_first`
6. Show Hidden Files `view.show_hidden`
7. ───
8. Select All `select.all` · Invert Selection `select.invert` · Select by Pattern… `select.by_pattern`
9. ───
10. Copy Folder Path `path.copy_folder_path` · Open in Terminal · Reveal in Finder
11. Refresh `view.refresh`

## 5. Address-bar segment (right-click or ⌥-click)
1. Open · Open in New Tab · Open in Other Pane
2. ───
3. Copy Path › (Path, Escaped, Relative to ~, Quoted, File URL)
4. ───
5. Open in Terminal · Reveal in Finder
6. Add to Favourites `favourites.add` or Remove from Favourites `favourites.remove`
7. Get Info

The segment's chevron is a separate menu: the folder's subfolders.

## 6. Sidebar item
1. Open · Open in New Tab · Open in Other Pane
2. Reveal in Finder · Get Info · Copy Path
3. ───
4. Remove from Favourites `sidebar.remove` · Rename Favourite… `sidebar.rename` (favourites only; a favourite whose folder has gone is shown dimmed and still offers Remove)
5. Drives and cards also get Eject and Show Drive Info *(new commands)*, and every volume gets Show Disk Usage `nav.disk_usage` *(M3)*

## 7. Tab
1. New Tab `tab.new` · Duplicate Tab `tab.duplicate` · Reopen Closed Tab `tab.reopen_closed`
2. Move Tab to Other Pane `tab.move_to_other_pane`
3. ───
4. Close Tab `tab.close` · Close Other Tabs `tab.close_others`
5. ───
6. Copy Path · Add to Favourites `favourites.add` or Remove from Favourites `favourites.remove` (the tab's folder) · Save Workspace… `workspace.save`

## Later
- **Custom actions (M4):** shell commands with `{selected}` and `{dir}`, placed anywhere.
- **Git (M4):** Show Changes and Copy Remote URL, inside repositories only.
- **Archives (M4):** Extract Here and Browse Archive.
- **Compare Folders… (M4):** two folders selected, or one in each pane.

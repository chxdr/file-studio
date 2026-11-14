# Search/ (M3)

The search bar and the shell's view of search. All search behaviour is in Rust:
`crates/fstudio-search` (sources, filters, merging) and
`crates/fstudio-core/src/tab_search.rs` (search results as a tab, status text).

- `SearchAPI.swift`: `SearchTabAPI` on `RustTab` and `MockTab` (search, stop, end, status), and the
  conversions to and from the FFI records. The mock refuses to search.
- `SearchBarView.swift`: the bar between a pane's header and its list. Row 1 holds the field, the scope
  (This Folder / This Mac), the name match mode, Filters, the status with a spinner, and Stop / Close.
  Row 2 holds kind, size, date (Modified or Created), tags, content, hidden items and package contents.
- `SearchDebugSteps.swift`: `searchset:`, `searchwait`, `searchlog` for `scripts/run-debug.sh`.

## Keys

- **⌥⌘F** (anywhere) opens the bar for the focused pane's tab.
- **⌘F** still filters the list (F-018). Pressing **⌘F again** in the filter field opens search with
  the filter text.
- **In the field:**
  - typing searches as you go (0.3 s debounce);
  - **↓** or **Return** goes to the results;
  - **Esc** stops a running search, then closes the bar and shows the folder again.
- **In the list:** **Esc** stops a running search (`filter.clear` with no filter text). **Back** ends the
  search.

## Results

Results are the tab's rows. A row's `name` is its path below the search root (its identity, so selection,
Quick Look, drags and file operations work unchanged). The cell draws `displayName` and then
`locationText` (the folder) in secondary text. Sorting by name sorts by the item's own name.

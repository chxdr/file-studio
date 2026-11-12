# Columns/ — column view (owned by the list/column session)

`ColumnsPlaceholderView` (name kept; ListPlaceholderView embeds it when the
tab's view mode is Columns): a horizontal strip of FileTables. The tab's
folder is the column with focus; columns to its left are ancestors back to the
column view's root, each a tab-less core listing (`openListing`) with the
child selected; to its right a preview: the selected folder's listing, or a
small info column for a file (type icon only, never reads content). ←/→ are
the core's (`handle_key` in column mode); a click in another column is
`columnSelect`. Columns resize by dragging the divider; double-click fits the
longest visible name. The strip scrolls to keep the focused column visible.

# PathBar/ — address-bar session

The always-visible address bar (F-009, F-018, HANDS-ON §8): crumbs at rest
(`tab.api.breadcrumbs()`), chevron menus (`engine.subfolders`), edit mode with
the completion panel (`tab.api.complete`), paste chip (`parseLocation(...).chip`),
⌥-click / right-click menus (`contextMenu(.crumb(...))`), the "…" overflow menu.

Keys: the field must adopt `CoreFocusReporting` with `.addressBar`; KeyRouter
then sends Return/Esc/Tab/⌥⌫/⌘↩ to the core, which answers with
`ShellRequest.textField(command:)` (`addressbar.commit`, `addressbar.cancel`,
`addressbar.complete_next`, …). Commit by performing `addressbar.commit` with
`.path(text)`. ⌘L and `/` arrive as `.focusAddressBar(pane:text:)`.

`PathBarPlaceholderView.swift` does exactly this with a plain text field;
replace it.

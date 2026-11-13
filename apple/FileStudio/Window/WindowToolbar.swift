// WindowToolbar.swift — the unified NSToolbar (F-019): about 52 pt, at most
// ten default controls, glyph-only with tooltips, customisable through the
// standard sheet. Every control performs a core command on the window's
// focused tab; the bar only mirrors state (history, view mode, dual pane,
// selection, filter text).

import AppKit
import SwiftUI

@MainActor
final class WindowToolbar: NSObject, NSToolbarDelegate, NSSearchFieldDelegate, NSSharingServicePickerToolbarItemDelegate {
    private unowned let model: AppModel
    private let windowModel: WindowModel
    let toolbar = NSToolbar(identifier: "FileStudioMain")
    private weak var navGroup: NSToolbarItemGroup?
    private weak var modeGroup: NSToolbarItemGroup?
    private weak var dualItem: NSMenuToolbarItem?
    private weak var trashItem: NSToolbarItem?
    private weak var newFolderItem: NSToolbarItem?
    /// The workspace switcher (icon and name of the active workspace).
    private(set) weak var workspaceSwitcher: WorkspaceSwitcher?
    let filterField = FilterSearchField()

    private enum ID {
        static let nav = NSToolbarItem.Identifier("nav")
        static let mode = NSToolbarItem.Identifier("viewMode")
        static let dual = NSToolbarItem.Identifier("dualPane")
        static let newFolder = NSToolbarItem.Identifier("newFolder")
        static let trash = NSToolbarItem.Identifier("trash")
        static let ops = NSToolbarItem.Identifier("ops")
        static let share = NSToolbarItem.Identifier("share")
        static let search = NSToolbarItem.Identifier("filter")
        static let workspace = NSToolbarItem.Identifier("workspace")
        // Optional (customisation sheet only).
        static let up = NSToolbarItem.Identifier("up")
        static let refresh = NSToolbarItem.Identifier("refresh")
        static let hidden = NSToolbarItem.Identifier("showHidden")
        static let swap = NSToolbarItem.Identifier("swapPanes")
        static let newTab = NSToolbarItem.Identifier("newTab")
        static let terminal = NSToolbarItem.Identifier("terminal")
        static let info = NSToolbarItem.Identifier("getInfo")
        static let quickLook = NSToolbarItem.Identifier("quickLook")
    }

    /// Simple command buttons: id → (label, symbol, command, tooltip).
    private static let buttons: [NSToolbarItem.Identifier: (String, String, String, String)] = [
        ID.newFolder: ("New Folder", "folder.badge.plus", "file.new_folder", "New Folder (⇧⌘N)"),
        ID.trash: ("Move to Trash", "trash", "file.trash", "Move to Trash (⌘⌫)"),
        ID.up: ("Enclosing Folder", "arrow.up", "nav.up", "Enclosing Folder (⌘↑)"),
        ID.refresh: ("Refresh", "arrow.clockwise", "view.refresh", "Refresh"),
        ID.hidden: ("Hidden Files", "eye", "view.show_hidden", "Show or Hide Hidden Files (⇧⌘.)"),
        ID.swap: ("Swap Panes", "arrow.left.arrow.right", "pane.swap", "Swap Panes"),
        ID.newTab: ("New Tab", "plus.square.on.square", "tab.new", "New Tab (⌘T)"),
        ID.terminal: ("Terminal", "terminal", "terminal.open", "Open in Terminal"),
        ID.info: ("Get Info", "info.circle", "preview.get_info", "Get Info (⌘I)"),
        ID.quickLook: ("Quick Look", "eye.circle", "preview.quick_look", "Quick Look (Space)"),
    ]

    init(model: AppModel, window: WindowModel) {
        self.model = model
        self.windowModel = window
        super.init()
        toolbar.delegate = self
        toolbar.displayMode = .iconOnly
        toolbar.allowsUserCustomization = true
        toolbar.autosavesConfiguration = true
        filterField.delegate = self
        filterField.placeholderString = "Filter"
        filterField.sendsSearchStringImmediately = true
        filterField.setAccessibilityLabel("Filter the list")
        filterField.toolTip = "Filter the list (⌘F)"
    }

    private var focusedTab: TabModel? { windowModel.activeTabID.flatMap { model.tabModel($0) } }

    // MARK: Delegate

    func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        [.toggleSidebar, .sidebarTrackingSeparator, ID.nav, ID.workspace, .flexibleSpace, ID.mode, ID.dual, ID.newFolder,
         ID.trash, ID.share, ID.ops, ID.search]
    }

    func toolbarAllowedItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        toolbarDefaultItemIdentifiers(toolbar) + [ID.up, ID.refresh, ID.hidden, ID.swap, ID.newTab, ID.terminal, ID.info,
                                                  ID.quickLook, .space, .flexibleSpace]
    }

    func toolbar(_ toolbar: NSToolbar, itemForItemIdentifier id: NSToolbarItem.Identifier,
                 willBeInsertedIntoToolbar flag: Bool) -> NSToolbarItem? {
        switch id {
        case ID.nav:
            let g = NSToolbarItemGroup(itemIdentifier: id, images: [symbol("chevron.left"), symbol("chevron.right")],
                                       selectionMode: .momentary, labels: ["Back", "Forward"],
                                       target: self, action: #selector(navigate(_:)))
            g.label = "Back/Forward"
            g.paletteLabel = "Back/Forward"
            g.isNavigational = true
            g.subitems[0].toolTip = "Back (⌘[)"
            g.subitems[1].toolTip = "Forward (⌘])"
            g.subitems.forEach { $0.autovalidates = false }
            g.autovalidates = false
            navGroup = g
            refresh()
            return g
        case ID.mode:
            let g = NSToolbarItemGroup(itemIdentifier: id,
                                       images: [symbol("list.bullet"), symbol("rectangle.split.3x1"),
                                                symbol("square.grid.2x2"), symbol("photo.on.rectangle")],
                                       selectionMode: .selectOne, labels: ["List", "Columns", "Icons", "Gallery"],
                                       target: self, action: #selector(setMode(_:)))
            g.label = "View"
            g.paletteLabel = "View Mode"
            let tips = ["as List (⌘2)", "as Columns (⌘3)", "as Icons — coming in M3", "as Gallery — coming in M3"]
            for (i, s) in g.subitems.enumerated() {
                s.toolTip = "View " + tips[i]
                s.autovalidates = false
                s.isEnabled = i < 2
            }
            g.autovalidates = false
            modeGroup = g
            refresh()
            return g
        case ID.dual:
            let item = NSMenuToolbarItem(itemIdentifier: id)
            item.label = "Dual Pane"
            item.paletteLabel = "Dual Pane"
            item.toolTip = "Dual Pane (⌘\\). Hold for layout options."
            item.target = self
            item.action = #selector(toggleDual)
            item.showsIndicator = true
            item.menu = dualMenu()
            dualItem = item
            refresh()
            return item
        case ID.workspace:
            let item = NSToolbarItem(itemIdentifier: id)
            item.label = "Workspace"
            item.paletteLabel = "Workspace Switcher"
            item.toolTip = "Switch workspace (⌃⌘1…⌃⌘9)"
            let sw = WorkspaceSwitcher(model: model, window: windowModel.id)
            item.view = sw
            workspaceSwitcher = sw
            return item
        case ID.ops:
            let item = NSToolbarItem(itemIdentifier: id)
            item.label = "Operations"
            item.paletteLabel = "Operations Progress"
            item.toolTip = "File operations"
            let host = NSHostingView(rootView: OpsPlaceholderView(model: model).frame(width: 44, height: 28))
            host.sizingOptions = []
            host.frame = NSRect(x: 0, y: 0, width: 44, height: 28)
            item.view = host
            return item
        case ID.share:
            let item = NSSharingServicePickerToolbarItem(itemIdentifier: id)
            item.label = "Share"
            item.paletteLabel = "Share"
            item.toolTip = "Share"
            item.delegate = self
            return item
        case ID.search:
            let item = NSSearchToolbarItem(itemIdentifier: id)
            item.searchField = filterField
            item.label = "Filter"
            item.paletteLabel = "Filter"
            item.preferredWidthForSearchField = 200
            item.resignsFirstResponderWithCancel = false
            return item
        default:
            guard let (label, sym, command, tip) = Self.buttons[id] else { return nil }
            let item = NSToolbarItem(itemIdentifier: id)
            item.label = label
            item.paletteLabel = label
            item.toolTip = tip
            item.image = symbol(sym, label)
            item.isBordered = true
            item.target = self
            item.action = #selector(runButton(_:))
            item.autovalidates = false
            commands[id] = command
            if id == ID.trash { trashItem = item }
            if id == ID.newFolder { newFolderItem = item }
            return item
        }
    }

    private var commands: [NSToolbarItem.Identifier: String] = [:]

    /// Toolbars saved before workspaces existed don't have the switcher: add
    /// it once, after Back/Forward. If the user removes it later it stays out.
    func addWorkspaceSwitcherIfNew() {
        let key = "FSToolbarWorkspaceSwitcherAdded"
        guard !UserDefaults.standard.bool(forKey: key) else { return }
        UserDefaults.standard.set(true, forKey: key)
        let ids = toolbar.items.map(\.itemIdentifier)
        guard !ids.contains(ID.workspace) else { return }
        let at = ids.firstIndex(of: ID.nav).map { $0 + 1 } ?? min(2, ids.count)
        toolbar.insertItem(withItemIdentifier: ID.workspace, at: at)
    }

    /// The workspaces changed: new title and menu.
    func workspacesChanged() { workspaceSwitcher?.refresh() }

    private func symbol(_ name: String, _ label: String? = nil) -> NSImage {
        NSImage(systemSymbolName: name, accessibilityDescription: label) ?? NSImage()
    }

    private func dualMenu() -> NSMenu {
        let m = NSMenu()
        m.addItem(ActionItem(title: "Single Pane") { [weak self] in self?.setDual(false) })
        m.addItem(ActionItem(title: "Dual Pane") { [weak self] in self?.setDual(true) })
        m.addItem(.separator())
        m.addItem(ActionItem(title: "Side by Side") { [weak self] in self?.setOrientation(.horizontal) })
        m.addItem(ActionItem(title: "Stacked") { [weak self] in self?.setOrientation(.vertical) })
        m.addItem(.separator())
        m.addItem(ActionItem(title: "Swap Panes") { [weak self] in self?.run("pane.swap") })
        m.addItem(ActionItem(title: "Make Panes Equal") { [weak self] in self?.run("pane.equalize") })
        return m
    }

    // MARK: Actions

    private func run(_ command: String, arg: CommandArg? = nil) {
        model.perform(Invocation(id: command, arg: arg, tab: windowModel.activeTabID), via: .mouse, window: windowModel.id)
    }

    @objc private func navigate(_ sender: NSToolbarItemGroup) {
        run(sender.selectedIndex == 0 ? "nav.back" : "nav.forward")
    }

    @objc private func setMode(_ sender: NSToolbarItemGroup) {
        let ids = ["view.list", "view.columns", "view.icons", "view.gallery"]
        guard ids.indices.contains(sender.selectedIndex) else { return }
        run(ids[sender.selectedIndex])
        refresh()
    }

    @objc private func toggleDual() { run("pane.toggle_dual") }

    private func setDual(_ on: Bool) { if windowModel.isDual != on { toggleDual() } }

    private func setOrientation(_ o: Orientation) {
        if !windowModel.isDual { toggleDual() }
        do { try model.engine.setLayout(window: windowModel.id, orientation: o, splitRatio: windowModel.state.splitRatio) }
        catch { model.statusMessage = error.localizedDescription }
    }

    @objc private func runButton(_ sender: NSToolbarItem) {
        if let c = commands[sender.itemIdentifier] { run(c) }
    }

    // MARK: State

    /// Mirrors the focused tab and window into the controls.
    func refresh() {
        let t = focusedTab
        if let g = navGroup {
            g.subitems[0].isEnabled = t?.canGoBack ?? false
            g.subitems[1].isEnabled = t?.canGoForward ?? false
        }
        if let g = modeGroup, let t {
            let mode = t.api.view().mode
            let i = [ViewMode.list, .columns, .icons, .gallery].firstIndex(of: mode) ?? 0
            if g.selectedIndex != i { g.selectedIndex = i }
        }
        if let d = dualItem {
            let dual = windowModel.isDual
            let name = !dual ? "rectangle" : windowModel.state.orientation == .horizontal ? "rectangle.split.2x1"
                : "rectangle.split.1x2"
            d.image = symbol(name, dual ? "Dual pane on" : "Dual pane off")
            d.toolTip = (dual ? "Single Pane" : "Dual Pane") + " (⌘\\). Hold for layout options."
        }
        trashItem?.isEnabled = (t?.selectionCount ?? 0) > 0
        if let t, filterField.window?.firstResponder !== filterField.currentEditor() {
            let f = t.api.filter()
            if filterField.stringValue != f { filterField.stringValue = f }
        }
        filterField.coreTab = t?.id
    }

    // MARK: Filter field

    func controlTextDidChange(_ obj: Notification) {
        focusedTab?.api.setFilter(filterField.stringValue)
    }

    // MARK: Share

    func items(for pickerToolbarItem: NSSharingServicePickerToolbarItem) -> [Any] {
        guard let t = focusedTab else { return [] }
        let paths = t.api.selectedPaths()
        return (paths.isEmpty ? [t.location] : paths).map { URL(fileURLWithPath: $0) }
    }
}

/// The toolbar's filter field: tells KeyRouter it is the filter, so Return
/// and Esc go to the core (`filter.commit`, `filter.clear`).
@MainActor
final class FilterSearchField: NSSearchField, CoreFocusReporting {
    var coreFocus: Focus { .filter }
    var coreTab: TabID?
}

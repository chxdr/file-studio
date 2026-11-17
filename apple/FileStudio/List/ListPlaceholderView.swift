// ListPlaceholderView.swift — a pane's file area (F-005, F-019): the list
// view, or the column view when the tab's view mode is Columns. (The type
// keeps its placeholder name because the window embeds it as
// `ListPlaceholderView(model:)`, `.scrollView`, `.table` and `.show(_:)`.)
//
// List view: one FileTable with a 28 pt header. Columns, their order, widths
// and the sort are the core's `ViewState` (remembered per folder by the
// core); this only draws them and reports changes: a header click is
// `sortByColumn`, the header's right-click menu shows or hides columns,
// dragging and resizing columns writes them back with `setView`.
//
// It also carries out the shell requests that belong to the list:
// `beginRename` (the in-place field), `textField(rename.…)`, and Quick Look
// for dataless items (F-016: never preview one without asking).

import AppKit
import QuickLookUI

/// Holds the list or the column view; this is what the pane lays out.
@MainActor
final class ListHostView: NSView {
    override init(frame: NSRect) { super.init(frame: frame); wantsLayer = true }
    required init?(coder: NSCoder) { fatalError() }
    override var wantsUpdateLayer: Bool { true }
    override func updateLayer() { layer?.backgroundColor = NSColor.textBackgroundColor.cgColor }
    func setContent(_ v: NSView) {
        guard v.superview !== self else { return }
        subviews.forEach { $0.removeFromSuperview() }
        v.translatesAutoresizingMaskIntoConstraints = false
        addSubview(v)
        NSLayoutConstraint.activate([
            v.leadingAnchor.constraint(equalTo: leadingAnchor), v.trailingAnchor.constraint(equalTo: trailingAnchor),
            v.topAnchor.constraint(equalTo: topAnchor), v.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }
}

/// The 28 pt column header (F-019).
final class FileHeaderView: NSTableHeaderView {
    override init(frame: NSRect) { super.init(frame: NSRect(x: frame.minX, y: frame.minY, width: frame.width, height: 28)) }
    required init?(coder: NSCoder) { fatalError() }
    // Opaque, so rows never show through it (F-019: opaque content background).
    override var isOpaque: Bool { true }
    override func draw(_ dirtyRect: NSRect) {
        NSColor.textBackgroundColor.setFill()
        dirtyRect.fill()
        super.draw(dirtyRect)
    }
}

@MainActor
final class ListPlaceholderView: NSObject, NSMenuDelegate {
    private unowned let model: AppModel
    /// The view the pane lays out (list or column view).
    let scrollView = ListHostView()
    /// The list view's table.
    let list: FileTable
    /// The column view (made on first use).
    private(set) var columns: ColumnsPlaceholderView?
    /// The icon view and the gallery (Grid/, made on first use).
    private(set) var icons: IconGridView?
    private(set) var gallery: GalleryView?
    private(set) var tab: TabModel?
    private var observer: ObserverToken?
    private var handlerIDs: [Int] = []
    private var mode: ViewMode = .list
    private var columnsWriteTimer: Timer?
    private var applyingColumns = false
    private var qlPaths: [String] = []

    /// The table that takes keyboard focus: the list's, or the column view's focused column.
    var table: CoreTableView { mode == .columns ? (columns?.focusedTable?.table ?? list.table) : list.table }

    /// The table showing the tab's own folder.
    var activeFileTable: FileTable { mode == .columns ? (columns?.focusedTable ?? list) : list }

    /// The view that takes keyboard focus in the current mode.
    var focusView: NSView {
        switch mode {
        case .icons: icons?.grid.doc ?? table
        case .gallery: gallery?.focusView ?? table
        default: table
        }
    }

    /// The icon view's or gallery's tiles, when one of them is shown.
    private var activeGrid: TileGrid? {
        switch mode {
        case .icons: icons?.grid
        case .gallery: gallery?.strip
        default: nil
        }
    }

    /// The rename field of the view shown.
    private var activeRename: RenameController? { activeGrid?.rename ?? activeFileTable.rename }

    init(model: AppModel) {
        self.model = model
        list = FileTable(model: model, kind: .name)
        super.init()
        list.rename = RenameController(model: model, table: list)
        let header = FileHeaderView(frame: .zero)
        list.table.headerView = header
        list.scrollView.tile()
        let hm = NSMenu()
        hm.delegate = self
        header.menu = hm
        list.table.allowsColumnReordering = true
        list.table.allowsColumnResizing = true
        list.onColumnClick = { [weak self] id in self?.headerClicked(id) }
        list.onColumnsEdited = { [weak self] in self?.columnsEdited() }
        scrollView.setContent(list.view)
        observer = model.observe { [weak self] e in self?.event(e) }
        installHandlers()
        #if DEBUG
        ListDebug.install(model: model)
        GridDebugSteps.install(model: model)
        #endif
    }

    isolated deinit {
        for id in handlerIDs { model.unregisterShellHandler(id) }
    }

    /// Shows a tab (the pane's active tab changed).
    func show(_ t: TabModel) {
        tab = t
        #if DEBUG
        ListDebug.register(self)
        #endif
        list.show(.tab(t.api), owner: t.id)
        applyView()
    }

    // MARK: View state (mode, columns, sort)

    private func event(_ e: CoreEvent) {
        guard let t = tab else { return }
        switch e {
        case .listingChanged(let id, _, _) where id == t.id: applyView()
        case .selectionChanged(let id, _, _) where id == t.id: followQuickLook()
        case .tabsChanged: applyView()
        default: break
        }
    }

    /// Switches between list and column view and applies columns and the sort indicator.
    private func applyView() {
        guard let t = tab else { return }
        let v = t.api.view()
        let wanted: ViewMode = v.mode
        if wanted != mode {
            let hadFocus = list.isActive || (columns?.focusedTable?.isActive ?? false) || (activeGrid?.isActive ?? false)
            let old = mode
            mode = wanted
            // Stop the view going away (its thumbnails, rename and timers).
            switch old {
            case .columns: columns?.detach()
            case .icons: icons?.detach()
            case .gallery: gallery?.detach()
            case .list: break
            }
            switch mode {
            case .columns:
                let c = columns ?? ColumnsPlaceholderView(model: model)
                columns = c
                c.show(t)
                scrollView.setContent(c)
            case .icons:
                let g = icons ?? IconGridView(model: model)
                icons = g
                scrollView.setContent(g)
                g.show(t)
            case .gallery:
                let g = gallery ?? GalleryView(model: model)
                gallery = g
                scrollView.setContent(g)
                g.show(t)
            case .list:
                scrollView.setContent(list.view)
                list.show(.tab(t.api), owner: t.id)
            }
            if hadFocus { scrollView.window?.makeFirstResponder(focusView) }
        } else {
            switch mode {
            case .columns: columns?.show(t)
            case .icons: icons?.show(t)
            case .gallery: gallery?.show(t)
            case .list: break
            }
        }
        if mode == .list { applyColumns(v) }
    }

    private func applyColumns(_ v: ViewState) {
        let tv = list.table
        let want = v.columns
        let have = tv.tableColumns.map { ColumnSpec(id: ColumnID(rawValue: $0.identifier.rawValue) ?? .name, width: Double($0.width)) }
        if have.map(\.id) != want.map(\.id) || zip(have, want).contains(where: { abs($0.width - $1.width) > 0.5 }) {
            applyingColumns = true
            for c in tv.tableColumns { tv.removeTableColumn(c) }
            for spec in want {
                let c = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(spec.id.rawValue))
                c.title = spec.id.title
                c.width = CGFloat(spec.width)
                c.minWidth = 36
                c.maxWidth = 4000
                c.resizingMask = .userResizingMask
                c.headerCell.alignment = spec.id == .size ? .right : .left
                c.headerToolTip = spec.id == .name ? nil : "Right-click to choose columns"
                tv.addTableColumn(c)
            }
            list.columnIDs = want.map(\.id)
            applyingColumns = false
        }
        // The sort indicator.
        for c in tv.tableColumns {
            let id = ColumnID(rawValue: c.identifier.rawValue)
            let on = id?.sortBy == v.sort
            tv.setIndicatorImage(on ? NSImage(named: v.ascending ? "NSAscendingSortIndicator" : "NSDescendingSortIndicator") : nil,
                                 in: c)
            if on { tv.highlightedTableColumn = c }
        }
    }

    private func headerClicked(_ id: ColumnID) {
        guard let t = tab, let eng = model.engine as? ListEngineAPI else { return }
        do { model.handle(try eng.sortByColumn(tab: t.id, column: id), in: model.keyWindowID) } catch {
            model.statusMessage = (error as? CoreError)?.message
        }
    }

    /// The user resized or moved a column: write the columns back (debounced).
    private func columnsEdited() {
        guard !applyingColumns else { return }
        columnsWriteTimer?.invalidate()
        columnsWriteTimer = Timer.scheduledTimer(withTimeInterval: 0.3, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.writeColumns() }
        }
    }

    private func writeColumns() {
        guard let t = tab else { return }
        var v = t.api.view()
        v.columns = list.table.tableColumns.compactMap { c in
            ColumnID(rawValue: c.identifier.rawValue).map { ColumnSpec(id: $0, width: Double(c.width.rounded())) }
        }
        t.api.setView(v)
    }

    /// Shows or hides a column (the header menu).
    func toggleColumn(_ id: ColumnID) {
        guard id != .name, let t = tab else { return }
        var v = t.api.view()
        if let i = v.columns.firstIndex(where: { $0.id == id }) {
            v.columns.remove(at: i)
        } else {
            v.columns.append(ColumnSpec(id: id, width: id.defaultWidth))
        }
        t.api.setView(v)
    }

    // Header menu: the column chooser.
    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        let shown = Set(tab?.api.view().columns.map(\.id) ?? [])
        for id in ColumnID.allCases {
            let it = ActionItem(title: id.title) { [weak self] in self?.toggleColumn(id) }
            it.state = shown.contains(id) ? .on : .off
            it.isEnabled = id != .name
            menu.addItem(it)
        }
        menu.autoenablesItems = false
    }

    /// The header menu's items as text (debug logs).
    func headerMenuDescription() -> [String] {
        let m = NSMenu()
        menuNeedsUpdate(m)
        return m.items.map { "\($0.state == .on ? "✓ " : "  ")\($0.title)\($0.isEnabled ? "" : " (disabled)")" }
    }

    // MARK: Shell requests

    private func installHandlers() {
        handlerIDs.append(model.registerShellHandler(for: "beginRename") { [weak self] req, _ in
            guard let self, case .beginRename(let tabID, let name) = req, tabID == self.tab?.id else { return false }
            if let g = self.activeGrid {
                print("RENAME begin “\(name)” (grid)")
                g.rename?.begin(name: name)
                return true
            }
            let ft = self.activeFileTable
            if ft.rename == nil { ft.rename = RenameController(model: self.model, table: ft) }
            print("RENAME begin “\(name)”")
            ft.rename?.begin(name: name)
            return true
        })
        handlerIDs.append(model.registerShellHandler(for: "textField") { [weak self] req, _ in
            guard let self, case .textField(let cmd) = req, cmd.hasPrefix("rename."),
                  let r = self.activeRename, r.isActive else { return false }
            if cmd == "rename.cancel" { r.cancel() } else { r.commit() }
            return true
        })
        handlerIDs.append(model.registerShellHandler(for: "quickLook") { [weak self] req, ctx in
            guard let self, case .quickLook(let paths) = req, let t = self.tab,
                  ctx.tab == t.id || (ctx.tab == nil && self.model.focusedTab?.id == t.id) else { return false }
            return self.quickLook(paths)
        })
    }

    /// Names among `paths` that are iCloud placeholders (from the rows on screen).
    private func datalessNames(_ paths: [String]) -> [String] {
        let cache = activeGrid?.cache ?? activeFileTable.cache
        return paths.compactMap { p in
            let n = (p as NSString).lastPathComponent
            return cache.cachedRow(named: n)?.isDataless == true ? n : nil
        }
    }

    /// Quick Look, asking first for dataless items (opening them downloads them).
    private func quickLook(_ paths: [String]) -> Bool {
        let cloud = datalessNames(paths)
        qlPaths = paths
        print("QL toggle \(paths.map { ($0 as NSString).lastPathComponent })\(cloud.isEmpty ? "" : " dataless=\(cloud)")")
        guard !cloud.isEmpty else { return false } // the default handler shows the panel
        if let p = QLPreviewPanel.sharedPreviewPanelExists() ? QLPreviewPanel.shared() : nil, p.isVisible { p.orderOut(nil) }
        guard let w = scrollView.window else { return true }
        let a = NSAlert()
        a.messageText = cloud.count == 1 ? "“\(cloud[0])” is in iCloud" : "\(cloud.count) items are in iCloud"
        a.informativeText = "Previewing downloads it to this Mac."
        a.addButton(withTitle: "Download and Preview")
        a.addButton(withTitle: "Cancel")
        a.beginSheetModal(for: w) { resp in
            if resp == .alertFirstButtonReturn { QuickLookController.shared.toggle(paths) }
        }
        return true
    }

    /// While the panel is up, it follows the selection (arrow keys go to the core).
    private func followQuickLook() {
        guard QLPreviewPanel.sharedPreviewPanelExists(), let p = QLPreviewPanel.shared(), p.isVisible,
              let t = tab, model.focusedTab?.id == t.id else { return }
        let paths = t.api.selectedPaths()
        guard !paths.isEmpty, paths != qlPaths else { return }
        qlPaths = paths
        if !datalessNames(paths).isEmpty {
            p.orderOut(nil)
            model.statusMessage = "Not previewed: the item is in iCloud"
            return
        }
        print("QL follow \(paths.map { ($0 as NSString).lastPathComponent })")
        QuickLookController.shared.toggle(paths)
    }
}

// ListPlaceholderView.swift — PLACEHOLDER for the list view (F-005, F-019).
// The list/column session replaces it with the real NSTableView (columns from
// ViewState, 120 Hz scrolling, rubber band, drag and drop, rename in place).
// What it shows the wiring for:
// - rows come from `TabAPI.rows(start:count:)`, cached in blocks per listing
//   version; the shell never sorts or holds a second model;
// - clicks become `SelectionChange`s; the core's selection is drawn back;
// - keys never reach the table: KeyRouter sends them to the core first
//   (the table reports `.list` focus);
// - right-click builds the menu from `menuTarget` + `contextMenu`.

import AppKit

@MainActor
final class CoreTableView: NSTableView, CoreFocusReporting {
    var coreFocus: Focus { .list }
    var coreTab: TabID?
    var onMouseDown: ((NSEvent) -> Void)?
    var menuProvider: ((NSEvent) -> NSMenu?)?

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        onMouseDown?(event)
    }
    override func menu(for event: NSEvent) -> NSMenu? { menuProvider?(event) }
    override var acceptsFirstResponder: Bool { true }
    // The core handles ⌘C/⌘V/⌘A for lists through KeyRouter; these cover the menu items.
    @objc func copy(_ sender: Any?) { AppModel.current?.perform(Invocation(id: "edit.copy", tab: coreTab), via: .menu) }
    @objc func paste(_ sender: Any?) { AppModel.current?.perform(Invocation(id: "edit.paste", tab: coreTab), via: .menu) }
    override func selectAll(_ sender: Any?) {
        AppModel.current?.perform(Invocation(id: "select.all", tab: coreTab), via: .menu)
    }
}

@MainActor
final class ListPlaceholderView: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    private unowned let model: AppModel
    let table = CoreTableView()
    let scrollView = NSScrollView()
    private(set) var tab: TabModel?
    private var observer: ObserverToken?
    /// Rows by block start, for one listing version.
    private var cache: [UInt32: [Row]] = [:]
    private var cacheVersion: UInt64 = .max
    private static let block: UInt32 = 64
    private var rowHeight: CGFloat = 22

    init(model: AppModel) {
        self.model = model
        super.init()
        for (id, title, width) in [("name", "Name", 320.0), ("size", "Size", 80.0), ("date", "Date Modified", 170.0),
                                   ("kind", "Kind", 120.0)] {
            let c = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(id))
            c.title = title
            c.width = width
            table.addTableColumn(c)
        }
        table.style = .fullWidth
        table.usesAlternatingRowBackgroundColors = false
        table.rowHeight = rowHeight
        table.intercellSpacing = NSSize(width: 6, height: 0)
        table.allowsMultipleSelection = true
        table.dataSource = self
        table.delegate = self
        table.onMouseDown = { [weak self] e in self?.click(e) }
        table.menuProvider = { [weak self] e in self?.contextMenu(e) }
        scrollView.documentView = table
        scrollView.hasVerticalScroller = true
        scrollView.drawsBackground = true
        scrollView.backgroundColor = .textBackgroundColor
        scrollView.contentView.postsBoundsChangedNotifications = true
        NotificationCenter.default.addObserver(forName: NSView.boundsDidChangeNotification, object: scrollView.contentView,
                                               queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.reportVisibleRange() }
        }
        observer = model.observe { [weak self] e in self?.event(e) }
    }

    func show(_ t: TabModel) {
        tab = t
        table.coreTab = t.id
        rowHeight = model.settings.view.density.rowHeight
        table.rowHeight = rowHeight
        reload()
    }

    private func event(_ e: CoreEvent) {
        guard let t = tab else { return }
        switch e {
        case .listingChanged(let id, _, _) where id == t.id: reload()
        case .selectionChanged(let id, _, let cursor) where id == t.id:
            cache = [:]
            table.reloadData()
            syncSelection()
            if let c = cursor { table.scrollRowToVisible(Int(c)) }
        default: break
        }
    }

    private func reload() {
        cache = [:]
        cacheVersion = tab?.listing.version ?? .max
        table.reloadData()
        syncSelection()
        if let anchor = tab?.api.scrollAnchor(), let i = tab?.api.indexOf(name: anchor.name) {
            table.scrollRowToVisible(Int(i))
        }
    }

    private func syncSelection() {
        guard let t = tab else { return }
        let rows = IndexSet(t.api.selection().rows.map { Int($0) })
        if table.selectedRowIndexes != rows { table.selectRowIndexes(rows, byExtendingSelection: false) }
    }

    func row(_ i: Int) -> Row? {
        guard let t = tab, i >= 0 else { return nil }
        if t.listing.version != cacheVersion { cache = [:]; cacheVersion = t.listing.version }
        let start = UInt32(i) / Self.block * Self.block
        if cache[start] == nil { cache[start] = t.api.rows(start: start, count: Self.block) }
        let b = cache[start]!
        let k = i - Int(start)
        return b.indices.contains(k) ? b[k] : nil
    }

    // MARK: Data source

    func numberOfRows(in tableView: NSTableView) -> Int { Int(tab?.listing.count ?? 0) }

    func tableView(_ tv: NSTableView, viewFor col: NSTableColumn?, row i: Int) -> NSView? {
        guard let r = row(i), let col else { return nil }
        let id = col.identifier
        let cell = tv.makeView(withIdentifier: id, owner: nil) as? NSTableCellView ?? makeCell(id)
        switch id.rawValue {
        case "name":
            cell.textField?.stringValue = r.name
            cell.textField?.alphaValue = r.isCutPending || r.isHidden ? 0.5 : 1
            cell.imageView?.image = IconCache.shared.icon(key: r.iconKey, path: tab.map { $0.location + "/" + r.name })
        case "size": cell.textField?.stringValue = r.sizeText
        case "date": cell.textField?.stringValue = r.modifiedText
        default: cell.textField?.stringValue = r.isDataless ? r.kindText + " (in iCloud)" : r.kindText
        }
        return cell
    }

    private func makeCell(_ id: NSUserInterfaceItemIdentifier) -> NSTableCellView {
        let cell = NSTableCellView()
        cell.identifier = id
        let tf = NSTextField(labelWithString: "")
        tf.lineBreakMode = .byTruncatingMiddle
        tf.translatesAutoresizingMaskIntoConstraints = false
        if id.rawValue == "name" {
            tf.font = .systemFont(ofSize: 13)
            let iv = NSImageView()
            iv.translatesAutoresizingMaskIntoConstraints = false
            cell.addSubview(iv)
            cell.imageView = iv
            NSLayoutConstraint.activate([
                iv.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 2),
                iv.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
                iv.widthAnchor.constraint(equalToConstant: 16), iv.heightAnchor.constraint(equalToConstant: 16),
            ])
        } else {
            tf.font = .monospacedDigitSystemFont(ofSize: 12, weight: .regular)
            tf.textColor = .secondaryLabelColor
            if id.rawValue == "size" { tf.alignment = .right }
        }
        cell.addSubview(tf)
        cell.textField = tf
        NSLayoutConstraint.activate([
            tf.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: id.rawValue == "name" ? 24 : 2),
            tf.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -2),
            tf.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
        ])
        return cell
    }

    // MARK: Input

    private func click(_ e: NSEvent) {
        guard let t = tab else { return }
        let p = table.convert(e.locationInWindow, from: nil)
        let i = table.row(at: p)
        guard i >= 0 else { t.api.setSelection(.none); return }
        let r = UInt32(i)
        if e.clickCount == 2 {
            model.perform(Invocation(id: "nav.open", tab: t.id), via: .mouse)
            return
        }
        let f = e.modifierFlags
        t.api.setSelection(f.contains(.command) ? .toggle(r) : f.contains(.shift) ? .extendTo(r) : .only(r))
    }

    private func contextMenu(_ e: NSEvent) -> NSMenu? {
        guard let t = tab else { return nil }
        let i = table.row(at: table.convert(e.locationInWindow, from: nil))
        guard let target = try? model.engine.menuTarget(tab: t.id, row: i >= 0 ? UInt32(i) : nil),
              let items = try? model.engine.contextMenu(target, context: MenuContext(pasteboardHasFiles: Pasteboard.hasFiles()))
        else { return nil }
        return ContextMenuBuilder.menu(items, model: model)
    }

    private func reportVisibleRange() {
        guard let t = tab else { return }
        let r = table.rows(in: table.visibleRect)
        t.api.setVisibleRange(start: UInt32(max(r.location, 0)), count: UInt32(r.length))
        if r.length > 0, let top = row(r.location) {
            t.api.setScrollAnchor(ScrollAnchor(name: top.name, offset: 0))
        }
    }
}

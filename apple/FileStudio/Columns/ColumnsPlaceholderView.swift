// ColumnsPlaceholderView.swift — the column view (Miller columns, PROPOSAL
// §10): a horizontal stack of file tables, each bound to a core listing.
// (The type keeps its placeholder name; ListPlaceholderView embeds it.)
//
// The tab's folder is the column with keyboard focus. Columns to its left
// are its ancestors back to the column view's root, each a tab-less core
// listing (`openListing`) with the child we came from selected. To its right
// is a preview: the selected folder's listing, or a small file info column.
// ←/→ are the core's (`handle_key` in column mode); a click in another
// column is `columnSelect`, which moves the tab there. Rows, menus, drag and
// drop and rename are the same FileTable as the list view.

import AppKit

@MainActor
final class ColumnsPlaceholderView: NSView, FileTableDelegate {
    private unowned let model: AppModel
    private let scroll = NSScrollView()
    private let strip = ColumnStrip()
    private(set) var tab: TabModel?
    /// The column view's first folder.
    private(set) var root: String?
    private var cols: [Column] = []
    private var preview: Column?
    private let info = FileInfoColumn()
    private var observer: ObserverToken?
    /// Width for new columns (the last one the user set).
    private var newWidth: CGFloat = 230

    /// One column: a table on a tab or a listing, and its resize handle.
    final class Column {
        let dir: String
        let table: FileTable
        let handle: ListingAPI?
        let divider: ColumnDivider
        var width: CGFloat
        init(dir: String, table: FileTable, handle: ListingAPI?, divider: ColumnDivider, width: CGFloat) {
            self.dir = dir; self.table = table; self.handle = handle; self.divider = divider; self.width = width
        }
    }

    init(model: AppModel) {
        self.model = model
        super.init(frame: .zero)
        scroll.hasHorizontalScroller = true
        scroll.hasVerticalScroller = false
        scroll.autohidesScrollers = true
        scroll.drawsBackground = false
        scroll.horizontalScrollElasticity = .allowed
        scroll.verticalScrollElasticity = .none
        scroll.documentView = strip
        scroll.translatesAutoresizingMaskIntoConstraints = false
        addSubview(scroll)
        NSLayoutConstraint.activate([
            scroll.leadingAnchor.constraint(equalTo: leadingAnchor), scroll.trailingAnchor.constraint(equalTo: trailingAnchor),
            scroll.topAnchor.constraint(equalTo: topAnchor), scroll.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
        observer = model.observe { [weak self] e in self?.event(e) }
    }
    required init?(coder: NSCoder) { fatalError() }

    /// The column showing the tab's own folder.
    var focusedTable: FileTable? { cols.last?.table }
    /// Every column's folder, left to right (debug logs).
    var columnDirs: [String] { cols.map(\.dir) + (preview.map { [$0.dir + " (preview)"] } ?? []) }
    /// Each column's geometry (debug logs).
    var columnGeometry: [String] {
        (cols + (preview.map { [$0] } ?? [])).map { c in
            let t = c.table.table
            return "\((c.dir as NSString).lastPathComponent): view=\(c.table.view.frame) table=\(t.frame) "
                + "col=\(t.tableColumns.first?.width ?? -1) rows=\(t.numberOfRows) vis=\(t.rows(in: c.table.contentRect)) "
                + "hidden=\(c.table.view.isHiddenOrHasHiddenAncestor) win=\(c.table.view.window != nil) "
                + "rowViews=\((0..<t.numberOfRows).filter { t.rowView(atRow: $0, makeIfNecessary: false) != nil }.count) "
                + "layer=\(c.table.view.layer != nil) scrollLayer=\(c.table.scrollView.layer != nil) clipInsets=\(c.table.scrollView.contentView.contentInsets.top) clip=\(c.table.scrollView.contentView.bounds)"
        } + ["strip=\(strip.frame) clip=\(scroll.contentView.bounds)"]
    }

    override func layout() {
        super.layout()
        layoutColumns()
    }

    // MARK: Structure

    /// Shows a tab: rebuilds the columns for its folder.
    func show(_ t: TabModel) {
        tab = t
        sync()
    }

    /// Lets the column listings go (the pane went back to list view).
    func detach() {
        for c in cols { c.table.detach(); c.divider.removeFromSuperview(); c.table.view.removeFromSuperview() }
        cols = []
        dropPreview()
        tab = nil
        root = nil
    }

    private static func isAncestor(_ a: String, of b: String) -> Bool {
        a == b || b.hasPrefix(a.hasSuffix("/") ? a : a + "/")
    }

    /// The folders from the root to the tab's folder.
    private func chain(to loc: String) -> [String] {
        if root == nil || !Self.isAncestor(root!, of: loc) { root = loc }
        var dirs = [loc]
        var cur = loc
        while cur != root, cur != "/" {
            cur = (cur as NSString).deletingLastPathComponent
            dirs.insert(cur, at: 0)
        }
        return dirs
    }

    private func sync() {
        guard let t = tab else { return }
        let loc = t.api.location()
        let dirs = chain(to: loc)
        let hadFocus = cols.contains { $0.table.isActive }
        // Keep columns whose folder (and role) still match.
        var keep = 0
        while keep < cols.count, keep < dirs.count, cols[keep].dir == dirs[keep],
              (cols[keep].handle == nil) == (keep == dirs.count - 1) {
            keep += 1
        }
        for c in cols[keep...] { remove(c) }
        cols = Array(cols[..<keep])
        for (i, d) in dirs.enumerated() where i >= keep {
            let isTab = i == dirs.count - 1
            let ft = FileTable(model: model, kind: .miller)
            ft.delegate = self
            configure(ft)
            let handle: ListingAPI? = isTab ? nil
                : (model.engine as? ListEngineAPI)?.openListing(path: d, showHidden: t.api.view().showHidden)
            if let h = handle { ft.show(.listing(h), owner: t.id) } else { ft.show(.tab(t.api), owner: t.id) }
            let div = ColumnDivider()
            let col = Column(dir: d, table: ft, handle: handle, divider: div, width: newWidth)
            div.onDrag = { [weak self, weak col] dx in
                guard let self, let col else { return }
                col.width = max(120, min(col.width + dx, 900))
                self.newWidth = col.width
                self.layoutColumns()
            }
            div.onDoubleClick = { [weak self, weak col] in
                guard let self, let col else { return }
                col.width = self.fittingWidth(col.table)
                self.layoutColumns()
            }
            strip.addSubview(ft.view)
            strip.addSubview(div)
            cols.append(col)
        }
        if !cols.isEmpty, let last = cols.last, last.handle == nil {
            // The tab column's table follows the tab.
            last.table.show(.tab(t.api), owner: t.id)
        }
        selectChildren(dirs)
        updatePreview()
        layoutColumns()
        if hadFocus || window?.firstResponder === window, let tv = focusedTable?.table { window?.makeFirstResponder(tv) }
        scrollToFocus()
    }

    private func configure(_ ft: FileTable) {
        let c = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(ColumnID.name.rawValue))
        c.resizingMask = .autoresizingMask
        ft.table.addTableColumn(c)
        ft.table.headerView = nil
        ft.scrollView.tile()
        ft.table.columnAutoresizingStyle = .firstColumnOnlyAutoresizingStyle
        ft.rename = RenameController(model: model, table: ft)
    }

    private func remove(_ c: Column) {
        c.table.detach()
        c.table.view.removeFromSuperview()
        c.divider.removeFromSuperview()
    }

    /// Each ancestor column selects the folder of the next column.
    private func selectChildren(_ dirs: [String]) {
        for (i, c) in cols.enumerated() where c.handle != nil && i + 1 < dirs.count {
            let child = (dirs[i + 1] as NSString).lastPathComponent
            if c.handle!.selection().names != [child] { c.handle!.setSelection(.names([child])) }
        }
    }

    /// The column right of the tab's: the selected folder's listing, or file info.
    private func updatePreview() {
        guard let t = tab, let tabCol = cols.last else { dropPreview(); return }
        let sel = t.api.selection()
        let cursor = sel.names.count == 1 ? sel.cursor : nil
        let row = cursor.flatMap { tabCol.table.row(Int($0)) }
        guard let row else { dropPreview(); return }
        let path = (t.api.location() as NSString).appendingPathComponent(row.name)
        // A dataless folder isn't listed (listing it may download it, F-016).
        if row.kind == .folder && !row.isDataless {
            info.removeFromSuperview()
            if preview?.dir == path { return }
            dropPreview()
            let ft = FileTable(model: model, kind: .miller)
            ft.delegate = self
            configure(ft)
            guard let h = (model.engine as? ListEngineAPI)?.openListing(path: path, showHidden: t.api.view().showHidden)
            else { return }
            ft.show(.listing(h), owner: t.id)
            let div = ColumnDivider()
            let col = Column(dir: path, table: ft, handle: h, divider: div, width: newWidth)
            div.onDrag = { [weak self, weak col] dx in
                guard let self, let col else { return }
                col.width = max(120, min(col.width + dx, 900)); self.layoutColumns()
            }
            div.onDoubleClick = { [weak self, weak col] in
                guard let self, let col else { return }
                col.width = self.fittingWidth(col.table); self.layoutColumns()
            }
            strip.addSubview(ft.view)
            strip.addSubview(div)
            preview = col
        } else {
            dropPreview()
            info.show(row, path: path)
            if info.superview == nil { strip.addSubview(info) }
        }
        layoutColumns()
        scrollToFocus()
    }

    private func dropPreview() {
        if let p = preview { remove(p) }
        preview = nil
        info.removeFromSuperview()
    }

    // MARK: Layout

    private func layoutColumns() {
        let h = max(scroll.contentView.bounds.height, 1)
        var x: CGFloat = 0
        for c in cols + (preview.map { [$0] } ?? []) {
            c.table.view.frame = NSRect(x: x, y: 0, width: c.width, height: h)
            c.divider.frame = NSRect(x: x + c.width, y: 0, width: ColumnDivider.width, height: h)
            x += c.width + ColumnDivider.width
        }
        if info.superview != nil {
            info.frame = NSRect(x: x, y: 0, width: 260, height: h)
            x += 260
        }
        strip.frame = NSRect(x: 0, y: 0, width: max(x, scroll.contentView.bounds.width), height: h)
    }

    /// Keeps the tab's column (and its preview) in view.
    private func scrollToFocus() {
        layoutSubtreeIfNeeded()
        guard let last = cols.last else { return }
        var r = last.table.view.frame
        if let p = preview { r = r.union(p.table.view.frame) } else if info.superview != nil { r = r.union(info.frame) }
        strip.scrollToVisible(r)
    }

    /// A width that fits the longest visible name.
    private func fittingWidth(_ ft: FileTable) -> CGFloat {
        let n = min(ft.table.numberOfRows, 500)
        var w: CGFloat = 0
        for i in 0..<n {
            if let r = ft.row(i) {
                w = max(w, (r.name as NSString).size(withAttributes: [.font: CellStyle.nameFont]).width)
            }
        }
        return max(120, min(ceil(w) + 21 + 40, 900))
    }

    // MARK: Events

    private func event(_ e: CoreEvent) {
        guard let t = tab else { return }
        switch e {
        case .listingChanged(let id, _, _) where id == t.id:
            if t.api.location() != cols.last?.dir { sync() } else { updatePreview() }
        case .selectionChanged(let id, _, _) where id == t.id:
            updatePreview()
        case .listingChanged(let id, _, _):
            // An ancestor listing finished loading: select the child again.
            if cols.contains(where: { $0.handle?.id == id }) {
                selectChildren(cols.map(\.dir) + [t.api.location()])
            }
        case .tabsChanged:
            if t.api.location() != cols.last?.dir { sync() }
        default: break
        }
    }

    // MARK: FileTableDelegate

    func fileTable(_ ft: FileTable, clickedRow row: Int?) {
        guard let t = tab, let eng = model.engine as? ListEngineAPI else { return }
        let col = (cols + (preview.map { [$0] } ?? [])).first { $0.table === ft }
        guard let col, col.handle != nil else { return }
        let name = row.flatMap { ft.row($0)?.name }
        do {
            model.handle(try eng.columnSelect(tab: t.id, dir: col.dir, name: name), in: model.keyWindowID)
        } catch {
            model.statusMessage = (error as? CoreError)?.message
        }
        window?.makeFirstResponder(nil)
        DispatchQueue.main.async { [weak self] in
            if let tv = self?.focusedTable?.table { self?.window?.makeFirstResponder(tv) }
        }
    }

    func fileTableDidChange(_ t: FileTable) {}
}

/// The columns' document view.
final class ColumnStrip: NSView {
    override var isFlipped: Bool { true }
    override var isOpaque: Bool { true }
    override func draw(_ dirtyRect: NSRect) { NSColor.textBackgroundColor.setFill(); dirtyRect.fill() }
}

/// The handle between two columns: drag to resize, double-click to fit.
@MainActor
final class ColumnDivider: NSView {
    static let width: CGFloat = 5
    var onDrag: ((CGFloat) -> Void)?
    var onDoubleClick: (() -> Void)?
    private var last: NSPoint?

    override func draw(_ dirtyRect: NSRect) {
        NSColor.textBackgroundColor.setFill(); bounds.fill()
        NSColor.separatorColor.setFill()
        NSRect(x: bounds.midX - 0.5, y: 0, width: 1, height: bounds.height).fill()
    }
    override func resetCursorRects() { addCursorRect(bounds, cursor: .resizeLeftRight) }
    override func mouseDown(with e: NSEvent) {
        if e.clickCount == 2 { onDoubleClick?(); last = nil; return }
        last = e.locationInWindow
    }
    override func mouseDragged(with e: NSEvent) {
        guard let l = last else { return }
        onDrag?(e.locationInWindow.x - l.x)
        last = e.locationInWindow
    }
    override func mouseUp(with e: NSEvent) { last = nil }
    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .splitter }
}

/// The small preview for a selected file: icon, name, kind, size, dates.
/// Uses the type icon only (no thumbnails), so dataless items are never read.
@MainActor
final class FileInfoColumn: NSView {
    private let icon = NSImageView()
    private let name = NSTextField(wrappingLabelWithString: "")
    private let details = NSTextField(wrappingLabelWithString: "")

    override init(frame: NSRect) {
        super.init(frame: frame)
        icon.imageScaling = .scaleProportionallyUpOrDown
        name.font = .systemFont(ofSize: 13, weight: .semibold)
        name.alignment = .center
        name.maximumNumberOfLines = 3
        details.font = .monospacedDigitSystemFont(ofSize: 11, weight: .regular)
        details.textColor = .secondaryLabelColor
        let stack = NSStackView(views: [icon, name, details])
        stack.orientation = .vertical
        stack.alignment = .centerX
        stack.spacing = 8
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            icon.widthAnchor.constraint(equalToConstant: 96), icon.heightAnchor.constraint(equalToConstant: 96),
            stack.topAnchor.constraint(equalTo: topAnchor, constant: 28),
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 16),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -16),
            name.widthAnchor.constraint(lessThanOrEqualTo: stack.widthAnchor),
            details.widthAnchor.constraint(lessThanOrEqualTo: stack.widthAnchor),
        ])
    }
    required init?(coder: NSCoder) { fatalError() }

    override func draw(_ dirtyRect: NSRect) { NSColor.textBackgroundColor.setFill(); dirtyRect.fill() }

    func show(_ r: Row, path: String) {
        let img = IconCache.shared.icon(key: r.iconKey, path: r.kind == .package ? path : nil).copy() as? NSImage
        img?.size = NSSize(width: 96, height: 96)
        icon.image = img
        name.stringValue = r.name
        var lines = [r.kindText]
        if r.kind != .folder { lines.append("Size  \(r.sizeText)") }
        lines.append("Created  \(r.createdText)")
        lines.append("Modified  \(r.modifiedText)")
        if r.isDataless { lines.append("In iCloud (not downloaded)") }
        details.stringValue = lines.joined(separator: "\n")
        setAccessibilityLabel("\(r.name), " + lines.joined(separator: ", "))
    }
}

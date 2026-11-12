// FileTable.swift — one table of files: the list view, or one column of the
// column view. View-based NSTableView with reused, layer-backed, drawn cells
// (FileCells.swift). Everything it shows and does comes from the core:
// - rows: `rows_packed` blocks for the visible range plus a margin, cached per
//   listing version and dropped on every `ListingChanged` / selection change;
// - selection: clicks become `SelectionChange`s; NSTableView's own selection
//   stays empty, and the core's is drawn by FileRowView;
// - keys: never handled here; KeyRouter sends them to the core (`.list` focus);
// - menus: `menuTarget` + `contextMenu`; opening: `nav.open`; drops:
//   `dropEffect` / `dropItems`; rename: `beginRename` → `renameItem`.

import AppKit

/// What a column view needs to hear from one of its tables.
@MainActor
protocol FileTableDelegate: AnyObject {
    /// A plain click on a row (or empty space, `nil`) of a table that isn't
    /// the tab's own (a column of the column view).
    func fileTable(_ t: FileTable, clickedRow row: Int?)
    /// The visible range or content changed (the column view updates its preview).
    func fileTableDidChange(_ t: FileTable)
}

/// The NSTableView of a file table. Reports `.list` focus for its tab and
/// passes mouse input to its controller.
@MainActor
final class CoreTableView: NSTableView, CoreFocusReporting {
    var coreFocus: Focus { .list }
    var coreTab: TabID?
    var onMouseDown: ((NSEvent) -> Void)?
    var onMouseDragged: ((NSEvent) -> Void)?
    var onMouseUp: ((NSEvent) -> Void)?
    var menuProvider: ((NSEvent) -> NSMenu?)?
    var onFocusChange: (() -> Void)?
    /// Takes keyboard focus on click (only the tab's own table does).
    var takesFocus = true

    override func mouseDown(with event: NSEvent) {
        if takesFocus { window?.makeFirstResponder(self) }
        onMouseDown?(event)
    }
    override func mouseDragged(with event: NSEvent) { onMouseDragged?(event) }
    override func mouseUp(with event: NSEvent) { onMouseUp?(event) }
    override func rightMouseDown(with event: NSEvent) {
        if takesFocus { window?.makeFirstResponder(self) }
        super.rightMouseDown(with: event)
    }
    override func menu(for event: NSEvent) -> NSMenu? { menuProvider?(event) }
    override var acceptsFirstResponder: Bool { takesFocus }
    override func becomeFirstResponder() -> Bool {
        let ok = super.becomeFirstResponder()
        DispatchQueue.main.async { [weak self] in self?.onFocusChange?() }
        return ok
    }
    override func resignFirstResponder() -> Bool {
        let ok = super.resignFirstResponder()
        DispatchQueue.main.async { [weak self] in self?.onFocusChange?() }
        return ok
    }
    // Selection is the core's: NSTableView never selects on its own.
    override func selectRowIndexes(_ indexes: IndexSet, byExtendingSelection extend: Bool) {}
    // The core handles ⌘C/⌘V/⌘A for lists through KeyRouter; these cover the menu items.
    @objc func copy(_ sender: Any?) { AppModel.current?.perform(Invocation(id: "edit.copy", tab: coreTab), via: .menu) }
    @objc func paste(_ sender: Any?) { AppModel.current?.perform(Invocation(id: "edit.paste", tab: coreTab), via: .menu) }
    override func selectAll(_ sender: Any?) {
        AppModel.current?.perform(Invocation(id: "select.all", tab: coreTab), via: .menu)
    }
}

/// A scroll view that reports live scrolling (bench) and keeps an opaque background.
final class FileScrollView: NSScrollView {}

@MainActor
final class FileTable: NSObject, NSTableViewDataSource, NSTableViewDelegate, NSDraggingSource {
    unowned let model: AppModel
    let table = CoreTableView()
    let scrollView = FileScrollView()
    /// What hosts lay out: the scroll view with the state messages over it.
    let view = NSView()
    /// Empty, loading and error messages over the rows.
    let stateView = ListStateView()
    weak var delegate: FileTableDelegate?
    let cellKind: CellKind
    private(set) var source: RowSource?
    /// The tab whose commands this table's menus, opens and drops act through.
    private(set) var ownerTab: TabID?
    let cache = RowCache()
    private(set) var info = ListingInfo(version: .max, dir: "", state: .loading, error: nil, count: 0, totalCount: 0)
    private var shownCount = 0
    private var observer: ObserverToken?
    private var rowHeight: CGFloat = 22
    var rename: RenameController?
    /// Column identifiers (list view), set by the list controller.
    var columnIDs: [ColumnID] = [.name]
    /// Drag state.
    private var downEvent: NSEvent?
    private var downRow = -1
    private var deferredOnly = false
    private var band: RubberBand?
    private var springTimer: Timer?
    private var springRow = -1
    /// The folder the last validated drop highlighted (F-023: the drop goes
    /// exactly there, or nowhere).
    private var highlightedDrop: String?
    private var loadingTimer: Timer?

    init(model: AppModel, kind: CellKind) {
        self.model = model
        cellKind = kind
        super.init()
        table.style = .plain
        table.usesAlternatingRowBackgroundColors = false
        table.backgroundColor = .textBackgroundColor
        table.gridStyleMask = []
        table.intercellSpacing = NSSize(width: kind == .miller ? 0 : 8, height: 0)
        table.allowsMultipleSelection = true
        table.allowsEmptySelection = true
        table.focusRingType = .none
        table.columnAutoresizingStyle = .noColumnAutoresizing
        table.usesAutomaticRowHeights = false
        table.dataSource = self
        table.delegate = self
        table.onMouseDown = { [weak self] e in self?.mouseDown(e) }
        table.onMouseDragged = { [weak self] e in self?.mouseDragged(e) }
        table.onMouseUp = { [weak self] e in self?.mouseUp(e) }
        table.menuProvider = { [weak self] e in self?.contextMenu(e) }
        table.onFocusChange = { [weak self] in self?.refreshActive() }
        table.registerForDraggedTypes([.fileURL])
        table.setDraggingSourceOperationMask([.copy, .move, .generic], forLocal: true)
        table.setDraggingSourceOperationMask([.copy, .move, .link, .generic], forLocal: false)
        table.draggingDestinationFeedbackStyle = .regular
        table.setAccessibilityLabel("Files")
        scrollView.documentView = table
        scrollView.hasVerticalScroller = true
        scrollView.hasHorizontalScroller = kind != .miller
        scrollView.autohidesScrollers = true
        scrollView.drawsBackground = true
        scrollView.backgroundColor = .textBackgroundColor
        scrollView.contentView.postsBoundsChangedNotifications = true
        scrollView.translatesAutoresizingMaskIntoConstraints = false
        stateView.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(scrollView)
        view.addSubview(stateView)
        NSLayoutConstraint.activate([
            scrollView.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            scrollView.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            scrollView.topAnchor.constraint(equalTo: view.topAnchor),
            scrollView.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            stateView.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            stateView.centerYAnchor.constraint(equalTo: view.centerYAnchor, constant: -20),
            stateView.widthAnchor.constraint(lessThanOrEqualTo: view.widthAnchor, constant: -32),
        ])
        NotificationCenter.default.addObserver(self, selector: #selector(boundsChanged),
                                               name: NSView.boundsDidChangeNotification, object: scrollView.contentView)
        for n in [NSWindow.didBecomeKeyNotification, NSWindow.didResignKeyNotification] {
            NotificationCenter.default.addObserver(self, selector: #selector(windowKeyChanged(_:)), name: n, object: nil)
        }
        observer = model.observe { [weak self] e in self?.event(e) }
        setRowHeight(model.settings.view.density.rowHeight)
    }

    // MARK: Source

    /// Shows a tab or a listing. `owner` is the tab commands act through.
    func show(_ s: RowSource, owner: TabID) {
        let sameDir = source.map { $0.id == s.id } ?? false
        source = s
        ownerTab = owner
        table.coreTab = owner
        table.takesFocus = s.tab != nil
        if !sameDir { rename?.cancel() }
        reload(restoreScroll: !sameDir)
    }

    /// Detaches from its source (a column that is going away).
    func detach() {
        rename?.cancel()
        source = nil
        observer = nil
        springTimer?.invalidate()
        loadingTimer?.invalidate()
    }

    func setRowHeight(_ h: Double) {
        guard CGFloat(h) != rowHeight || table.rowHeight != CGFloat(h) else { return }
        rowHeight = CGFloat(h)
        table.rowHeight = rowHeight
        table.noteHeightOfRows(withIndexesChanged: IndexSet(integersIn: 0..<table.numberOfRows))
    }

    func row(_ i: Int) -> Row? {
        guard let s = source else { return nil }
        if cache.version != info.version { cache.invalidate(version: info.version) }
        return cache.row(i, from: s)
    }

    // MARK: Events

    private func event(_ e: CoreEvent) {
        guard let s = source else { return }
        switch e {
        case .listingChanged(let id, _, let kind) where id == s.id:
            listingChanged(kind)
        case .selectionChanged(let id, _, let cursor) where id == s.id:
            selectionChanged(cursor: cursor)
        case .settingsChanged:
            setRowHeight(model.settings.view.density.rowHeight)
            redrawVisible()
        default: break
        }
    }

    /// The top visible row's name and its offset, to keep it in place.
    private func topAnchor() -> (name: String, offset: CGFloat)? {
        let vis = table.rows(in: contentRect)
        guard vis.length > 0, let r = row(vis.location) else { return nil }
        let y = table.rect(ofRow: vis.location).minY - contentRect.minY
        return (r.name, y)
    }

    private func listingChanged(_ kind: ChangeKind) {
        guard let s = source else { return }
        let newInfo = s.listingInfo()
        let dirChanged = newInfo.dir != info.dir
        if dirChanged {
            rename?.cancel()
            reload(restoreScroll: true)
            return
        }
        switch kind {
        case .patched where canAnimatePatch(newInfo):
            applyPatch(newInfo)
        default:
            // Partial → Sorted, re-sorts and big patches: keep the top item in place
            // (unless the list is at the top: then it stays at the top).
            let atTop = contentRect.minY <= 0.5
            let anchor = atTop ? nil : topAnchor()
            info = newInfo
            cache.invalidate(version: info.version)
            shownCount = Int(info.count)
            table.reloadData()
            if let a = anchor, let i = s.indexOf(name: a.name) {
                scrollRow(Int(i), toOffset: a.offset)
            }
        }
        updateState()
        reportVisibleRange()
        rename?.reposition()
        delegate?.fileTableDidChange(self)
    }

    private func reload(restoreScroll: Bool) {
        guard let s = source else { return }
        info = s.listingInfo()
        cache.invalidate(version: info.version)
        shownCount = Int(info.count)
        table.reloadData()
        if restoreScroll {
            if let t = s.tab, let a = t.scrollAnchor(), let i = t.indexOf(name: a.name) {
                scrollRow(Int(i), toOffset: CGFloat(a.offset))
            } else if let c = s.selection().cursor {
                table.scrollRowToVisible(Int(c))
            } else {
                scrollToTop()
            }
        }
        updateState()
        reportVisibleRange()
        refreshActive()
        delegate?.fileTableDidChange(self)
    }

    /// The part of the table not under the header or the toolbar (table coordinates).
    var contentRect: NSRect {
        let clip = scrollView.contentView
        var r = clip.bounds
        let top = clip.contentInsets.top
        r.origin.y += top
        r.size.height = max(r.height - top - clip.contentInsets.bottom, 0)
        return r
    }

    func scrollToTop() {
        let clip = scrollView.contentView
        clip.scroll(to: NSPoint(x: clip.bounds.minX, y: -clip.contentInsets.top))
        scrollView.reflectScrolledClipView(clip)
    }

    private func scrollRow(_ i: Int, toOffset off: CGFloat) {
        let clip = scrollView.contentView
        let top = clip.contentInsets.top
        let y = table.rect(ofRow: i).minY - off - top
        let maxY = max(table.bounds.height - clip.bounds.height + clip.contentInsets.bottom, -top)
        clip.scroll(to: NSPoint(x: clip.bounds.minX, y: min(max(y, -top), maxY)))
        scrollView.reflectScrolledClipView(clip)
    }

    // MARK: Patches

    private var visibleNames: [String] = []

    /// Animate when the change is small, visible-only and motion is allowed.
    private func canAnimatePatch(_ n: ListingInfo) -> Bool {
        let delta = abs(Int(n.count) - shownCount)
        return delta > 0 && delta <= 12 && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
            && n.count < 50_000 && table.window != nil
    }

    /// Inserts and removes rows with a short fade, when the changes all fall
    /// in the visible range; otherwise reloads.
    private func applyPatch(_ n: ListingInfo) {
        guard let s = source else { return }
        let vis = table.rows(in: contentRect)
        let lo = max(vis.location - 2, 0)
        let hi = min(vis.location + vis.length + 2, shownCount)
        // Old names a little past the window, so rows shifted in aren't taken for new ones.
        let ext = min(hi + 16, shownCount)
        let oldNames = (lo..<ext).map { row($0)?.name ?? "" }
        let oldSet = Set(oldNames)
        let removed: [Int] = (lo..<hi).filter { s.indexOf(name: oldNames[$0 - lo]) == nil }
        info = n
        cache.invalidate(version: n.version)
        let newEnd = min(ext - removed.count, Int(n.count))
        let added: [Int] = lo < newEnd ? (lo..<newEnd).filter { i in row(i).map { !oldSet.contains($0.name) } ?? false } : []
        guard shownCount - removed.count + added.count == Int(n.count) else {
            shownCount = Int(n.count)
            table.reloadData()
            return
        }
        NSAnimationContext.runAnimationGroup { ctx in
            ctx.duration = 0.15
            table.beginUpdates()
            if !removed.isEmpty { table.removeRows(at: IndexSet(removed), withAnimation: .effectFade) }
            if !added.isEmpty { table.insertRows(at: IndexSet(added), withAnimation: .effectFade) }
            shownCount = Int(n.count)
            table.endUpdates()
        }
        redrawVisible()
    }

    // MARK: Selection

    private func selectionChanged(cursor: UInt32?) {
        guard let s = source else { return }
        info = s.listingInfo()
        cache.invalidate(version: info.version)
        if Int(info.count) != shownCount {
            shownCount = Int(info.count)
            table.reloadData()
        } else {
            redrawVisible()
        }
        if let c = cursor, band == nil { table.scrollRowToVisible(Int(c)) }
        delegate?.fileTableDidChange(self)
    }

    /// Reconfigures the visible rows from fresh data (selection, cut, density).
    func redrawVisible() {
        let active = isActive
        table.enumerateAvailableRowViews { rv, i in
            guard let rv = rv as? FileRowView else { return }
            let r = row(i)
            configureRowView(rv, r, active: active)
            for c in 0..<rv.numberOfColumns {
                if let cell = rv.view(atColumn: c) as? FileCellView, c < table.tableColumns.count {
                    configure(cell, column: table.tableColumns[c].identifier, row: r, active: active, selected: rv.coreSelected)
                }
            }
        }
    }

    /// The pane is active: key window and this table (or its rename field) has focus.
    var isActive: Bool {
        guard let w = table.window, w.isKeyWindow || Self.assumeKeyWindow else { return false }
        var r = w.firstResponder
        if let tv = r as? NSTextView, tv.isFieldEditor, let d = tv.delegate as? NSResponder { r = d }
        return r === table || (r as? NSView)?.isDescendant(of: table) == true
    }

    /// Debug scripts run with the app in the background, where no window is key.
    static var assumeKeyWindow = false

    private var lastActive: Bool?
    func refreshActive() {
        let a = isActive
        guard a != lastActive else { return }
        lastActive = a
        redrawVisible()
    }

    @objc private func windowKeyChanged(_ n: Notification) {
        guard (n.object as? NSWindow) === table.window else { return }
        refreshActive()
    }

    // MARK: Data source and delegate

    func numberOfRows(in tableView: NSTableView) -> Int { shownCount }

    func tableView(_ tv: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        let id = NSUserInterfaceItemIdentifier("row")
        let rv = tv.makeView(withIdentifier: id, owner: nil) as? FileRowView ?? {
            let v = FileRowView(); v.identifier = id; return v
        }()
        configureRowView(rv, self.row(row), active: isActive)
        return rv
    }

    private func configureRowView(_ rv: FileRowView, _ r: Row?, active: Bool) {
        rv.coreSelected = r?.isSelected ?? false
        rv.isCursorRow = r?.isCursor ?? false
        rv.paneActive = active
        if let r {
            rv.summary = [r.name, r.isDataless ? "in iCloud" : nil, r.kindText, r.kind == .folder ? nil : r.sizeText,
                          r.modifiedText, r.isSelected ? "selected" : nil].compactMap { $0 }.joined(separator: ", ")
        }
    }

    func tableView(_ tv: NSTableView, viewFor col: NSTableColumn?, row i: Int) -> NSView? {
        guard let col else { return nil }
        let measuring = RowCache.measuring
        let clock = ContinuousClock()
        let t0 = clock.now
        let cell = tv.makeView(withIdentifier: col.identifier, owner: nil) as? FileCellView ?? {
            let c = FileCellView(frame: .zero); c.identifier = col.identifier; return c
        }()
        let r = row(i)
        configure(cell, column: col.identifier, row: r, active: isActive, selected: r?.isSelected ?? false)
        if measuring {
            CellStyle.configureTime += clock.now - t0
            CellStyle.configureCalls += 1
        }
        if FirstRowsProbe.pending, r != nil { FirstRowsProbe.mark() }
        return cell
    }

    private func configure(_ cell: FileCellView, column: NSUserInterfaceItemIdentifier, row r: Row?, active: Bool,
                           selected: Bool) {
        let onAccent = selected && active
        let id = ColumnID(rawValue: column.rawValue) ?? .name
        guard let r else { cell.configure(text: "", row: nil, onAccent: false); return }
        switch cellKind {
        case .miller:
            cell.kind = .miller
            cell.configure(text: r.name, icon: icon(r), row: r, onAccent: onAccent, chevron: r.kind == .folder)
        default:
            switch id {
            case .name:
                cell.kind = .name
                cell.configure(text: r.name, icon: icon(r), row: r, onAccent: onAccent)
            case .size:
                cell.kind = .text(rightAligned: true)
                cell.configure(text: r.sizeText, row: r, onAccent: onAccent)
            case .modified:
                cell.kind = .text(rightAligned: false)
                cell.configure(text: r.modifiedText, row: r, onAccent: onAccent)
            case .created:
                cell.kind = .text(rightAligned: false)
                cell.configure(text: r.createdText, row: r, onAccent: onAccent)
            case .kind:
                cell.kind = .text(rightAligned: false)
                cell.configure(text: r.kindText, row: r, onAccent: onAccent)
            case .ext:
                cell.kind = .text(rightAligned: false)
                cell.configure(text: r.extText, row: r, onAccent: onAccent)
            }
        }
    }

    private func icon(_ r: Row) -> NSImage {
        IconCache.shared.icon(key: r.iconKey, path: r.kind == .package ? (info.dir as NSString).appendingPathComponent(r.name) : nil)
    }

    /// A header click (list view): the list controller sorts through the core.
    var onColumnClick: ((ColumnID) -> Void)?
    /// Columns resized or moved by the user.
    var onColumnsEdited: (() -> Void)?

    func tableView(_ tableView: NSTableView, didClick col: NSTableColumn) {
        if let id = ColumnID(rawValue: col.identifier.rawValue) { onColumnClick?(id) }
    }
    func tableViewColumnDidResize(_ notification: Notification) {
        onColumnsEdited?()
        rename?.reposition()
    }
    func tableViewColumnDidMove(_ notification: Notification) { onColumnsEdited?() }

    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool { false }
    func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat { rowHeight }

    // MARK: Scrolling

    @objc private func boundsChanged() {
        reportVisibleRange()
        rename?.reposition()
    }

    private var lastRange = NSRange(location: -1, length: 0)
    private func reportVisibleRange() {
        guard let s = source else { return }
        let r = table.rows(in: contentRect)
        guard r != lastRange else { return }
        lastRange = r
        // Prefetch the visible rows plus a margin, so new rows are ready before they show.
        let margin = max(r.length, 20)
        cache.prefetch(max(r.location - margin, 0)..<min(r.location + r.length + margin, shownCount), from: s)
        s.setVisibleRange(start: UInt32(max(r.location, 0)), count: UInt32(r.length))
        if let t = s.tab, r.length > 0, let top = row(r.location) {
            let off = table.rect(ofRow: r.location).minY - contentRect.minY
            t.setScrollAnchor(ScrollAnchor(name: top.name, offset: Double(off)))
        }
    }

    // MARK: Mouse

    private func rowAt(_ e: NSEvent) -> Int {
        let p = table.convert(e.locationInWindow, from: nil)
        let i = table.row(at: p)
        return i < shownCount ? i : -1
    }

    private func mouseDown(_ e: NSEvent) {
        guard let s = source else { return }
        #if DEBUG
        print("LIST mouseDown row=\(rowAt(e)) clicks=\(e.clickCount) source=\(s.id)")
        #endif
        let i = rowAt(e)
        let f = e.modifierFlags
        downEvent = e
        downRow = i
        deferredOnly = false
        rename?.commitIfActive()
        if s.tab == nil {
            // A column that isn't the tab's: the column view moves the tab here.
            delegate?.fileTable(self, clickedRow: i >= 0 ? i : nil)
            if e.clickCount == 2, i >= 0, let p = s.pathAt(UInt32(i)) {
                model.perform(Invocation(id: "nav.open", arg: .path(p), tab: ownerTab), via: .mouse)
            }
            return
        }
        if e.clickCount == 2, i >= 0 {
            model.perform(Invocation(id: "nav.open", tab: ownerTab), via: .mouse)
            return
        }
        if i < 0 {
            let keep = f.contains(.command) || f.contains(.shift)
            if !keep { s.setSelection(.none) }
            band = RubberBand(start: table.convert(e.locationInWindow, from: nil),
                              base: keep ? s.selection().rows : [], table: table)
            delegate?.fileTable(self, clickedRow: nil)
            return
        }
        let r = UInt32(i)
        let wasSelected = row(i)?.isSelected ?? false
        if f.contains(.command) {
            s.setSelection(.toggle(r))
        } else if f.contains(.shift) {
            s.setSelection(.extendTo(r))
        } else if wasSelected {
            deferredOnly = true // keep a multi-selection for dragging; collapse on mouse-up
        } else {
            s.setSelection(.only(r))
        }
    }

    private func mouseDragged(_ e: NSEvent) {
        guard let s = source, let down = downEvent else { return }
        if let b = band {
            table.autoscroll(with: e)
            let rows = b.update(to: table.convert(e.locationInWindow, from: nil))
            s.setSelection(.rows(rows.map { UInt32($0) }))
            return
        }
        let d = hypot(e.locationInWindow.x - down.locationInWindow.x, e.locationInWindow.y - down.locationInWindow.y)
        guard downRow >= 0, d > 4 else { return }
        downEvent = nil
        deferredOnly = false
        var paths = s.selectedPaths()
        if paths.isEmpty || !(row(downRow)?.isSelected ?? false), let p = s.pathAt(UInt32(downRow)) { paths = [p] }
        startDrag(paths, event: down)
    }

    private func mouseUp(_ e: NSEvent) {
        if let b = band { b.finish(); band = nil }
        if deferredOnly, downRow >= 0, let s = source { s.setSelection(.only(UInt32(downRow))) }
        deferredOnly = false
        downEvent = nil
    }

    // MARK: Drag out

    private func startDrag(_ paths: [String], event: NSEvent) {
        let start = table.convert(event.locationInWindow, from: nil)
        let items: [NSDraggingItem] = paths.enumerated().map { k, p in
            let item = NSDraggingItem(pasteboardWriter: URL(fileURLWithPath: p) as NSURL)
            let img = NSWorkspace.shared.icon(forFile: p)
            img.size = NSSize(width: 32, height: 32)
            item.setDraggingFrame(NSRect(x: start.x - 16 + CGFloat(k * 4), y: start.y - 16 + CGFloat(k * 4), width: 32, height: 32),
                                  contents: img)
            return item
        }
        table.beginDraggingSession(with: items, event: event, source: self)
    }

    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        context == .withinApplication ? [.copy, .move, .generic] : [.copy, .move, .link, .generic]
    }

    // MARK: Drop in

    /// The folder a drop at `row` goes into: a folder row, else this table's folder.
    func dropDestination(row: Int) -> String? {
        guard let s = source else { return nil }
        if row >= 0, let r = self.row(row), r.kind == .folder, let p = s.pathAt(UInt32(row)) { return p }
        return s.dir
    }

    private static func mods(_ f: NSEvent.ModifierFlags) -> Mods {
        Mods(shift: f.contains(.shift), cmd: f.contains(.command), alt: f.contains(.option), ctrl: f.contains(.control))
    }

    /// What dropping `paths` at `row` does (the core decides).
    func dropEffect(_ paths: [String], row: Int, flags: NSEvent.ModifierFlags) -> (DropEffect, String?) {
        guard let dest = dropDestination(row: row), let eng = model.engine as? ListEngineAPI else { return (.none, nil) }
        return (eng.dropEffect(sources: paths, dest: dest, mods: Self.mods(flags)), dest)
    }

    /// Drops `paths` at `row` (also used by the `drop:` debug step). The core
    /// decides; a move of a protected folder comes back as a confirmation and
    /// every move as a "Moved … Undo" toast (F-023).
    @discardableResult
    func performDrop(_ paths: [String], row: Int, flags: NSEvent.ModifierFlags) -> Bool {
        guard let dest = dropDestination(row: row) else { return false }
        do {
            let out = try model.engine.dropFiles(sources: paths, destDir: dest, mods: Self.mods(flags))
            print("DROP into \(dest): ops \(out.ops)\(out.shell.map { " shell \($0.kind)" } ?? "")")
            model.handle(out, in: model.keyWindowID)
            return true
        } catch {
            MoveSafety.refused(Self.sentence((error as? CoreError)?.message ?? error.localizedDescription), model: model)
            return false
        }
    }

    /// "the items can’t be dropped there" → "The items can’t be dropped there".
    static func sentence(_ s: String) -> String { s.prefix(1).uppercased() + s.dropFirst() }

    private static func paths(_ info: NSDraggingInfo) -> [String] {
        (info.draggingPasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL])?
            .map(\.path) ?? []
    }

    func tableView(_ tv: NSTableView, validateDrop info: NSDraggingInfo, proposedRow row: Int,
                   proposedDropOperation op: NSTableView.DropOperation) -> NSDragOperation {
        // F-023: sidebar rows are references; dropping one here never moves or copies it.
        if SidebarViewController.isSidebarDrag(info) {
            highlightedDrop = nil
            armSpring(-1)
            return []
        }
        let paths = Self.paths(info)
        var target = row
        if op == .above || row >= shownCount || self.row(row)?.kind != .folder {
            target = -1
            tv.setDropRow(-1, dropOperation: .on)
        }
        let (effect, dest) = dropEffect(paths, row: target, flags: NSEvent.modifierFlags)
        highlightedDrop = effect == .none ? nil : dest
        armSpring(effect == .none ? -1 : target)
        switch effect {
        case .copy: return .copy
        case .move: return .move
        case .none: return []
        }
    }

    func tableView(_ tv: NSTableView, acceptDrop info: NSDraggingInfo, row: Int,
                   dropOperation: NSTableView.DropOperation) -> Bool {
        springTimer?.invalidate(); springRow = -1
        if SidebarViewController.isSidebarDrag(info) { return false }
        let target = row >= 0 && self.row(row)?.kind == .folder ? row : -1
        // The drop goes exactly where the highlight was: if the rows changed
        // under the pointer (a spring-loaded folder opened, the folder
        // refreshed), the row index may now name another folder. Refuse.
        let highlighted = highlightedDrop
        highlightedDrop = nil
        guard let dest = dropDestination(row: target), dest == highlighted else {
            MoveSafety.refused("The folder under the pointer changed, so nothing was dropped", model: model)
            return false
        }
        return performDrop(Self.paths(info), row: target, flags: NSEvent.modifierFlags)
    }

    func tableView(_ tableView: NSTableView, draggingSession session: NSDraggingSession, endedAt p: NSPoint,
                   operation: NSDragOperation) {
        springTimer?.invalidate(); springRow = -1
    }

    /// Spring-loaded folders: hovering a folder row for 0.8 s opens it.
    private func armSpring(_ row: Int) {
        guard row != springRow else { return }
        springRow = row
        springTimer?.invalidate()
        guard row >= 0, let p = source?.pathAt(UInt32(row)) else { return }
        springTimer = Timer.scheduledTimer(withTimeInterval: 0.8, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.springRow = -1
                if self.source?.tab != nil {
                    self.model.perform(Invocation(id: "nav.open", arg: .path(p), tab: self.ownerTab), via: .mouse)
                } else {
                    self.delegate?.fileTable(self, clickedRow: row)
                }
            }
        }
    }

    // MARK: Context menu

    /// Finder's rule: the core picks the target, then builds the menu.
    func menuItems(row: Int) -> [MenuItem]? {
        guard let s = source, let owner = ownerTab else { return nil }
        let r: UInt32? = row >= 0 ? UInt32(row) : nil
        let target: MenuTarget
        switch s {
        case .tab: guard let t = try? model.engine.menuTarget(tab: owner, row: r) else { return nil }; target = t
        case .listing(let l): target = l.menuTarget(owner: owner, row: r)
        }
        return try? model.engine.contextMenu(target, context: MenuContext(pasteboardHasFiles: Pasteboard.hasFiles()))
    }

    private func contextMenu(_ e: NSEvent) -> NSMenu? {
        rename?.commitIfActive()
        let i = rowAt(e)
        guard let items = menuItems(row: i) else { return nil }
        #if DEBUG
        for line in ContextMenuBuilder.describe(items) { print("CTXMENU \(line)") }
        #endif
        return ContextMenuBuilder.menu(items, model: model)
    }

    // MARK: States

    private func updateState() {
        loadingTimer?.invalidate()
        loadingTimer = nil
        switch info.state {
        case .loading where info.count == 0:
            stateView.show(.none)
            // A spinner only when loading takes a while.
            loadingTimer = Timer.scheduledTimer(withTimeInterval: 0.15, repeats: false) { [weak self] _ in
                MainActor.assumeIsolated {
                    guard let self, self.info.state == .loading, self.info.count == 0 else { return }
                    self.stateView.show(.loading)
                }
            }
        case .error where info.count == 0:
            stateView.show(.error(info.error ?? "The folder can’t be read."))
        case .complete where info.count == 0:
            stateView.show(info.totalCount > 0 ? .emptyFiltered(Int(info.totalCount)) : .empty)
        default:
            stateView.show(.none)
        }
    }
}

/// The rubber-band rectangle: selects the rows it touches.
@MainActor
final class RubberBand {
    private let start: NSPoint
    private let base: [UInt32]
    private unowned let table: NSTableView
    private let layer = CAShapeLayer()

    init(start: NSPoint, base: [UInt32], table: NSTableView) {
        self.start = start; self.base = base; self.table = table
        table.wantsLayer = true
        layer.fillColor = NSColor.selectedContentBackgroundColor.withAlphaComponent(0.15).cgColor
        layer.strokeColor = NSColor.selectedContentBackgroundColor.withAlphaComponent(0.7).cgColor
        layer.lineWidth = 1
        layer.zPosition = 100
        table.layer?.addSublayer(layer)
    }

    /// The rows under the band, plus the ones selected before (⌘/⇧).
    func update(to p: NSPoint) -> [Int] {
        let r = NSRect(x: min(start.x, p.x), y: min(start.y, p.y), width: abs(p.x - start.x), height: abs(p.y - start.y))
        CATransaction.begin(); CATransaction.setDisableActions(true)
        layer.path = CGPath(rect: r, transform: nil)
        CATransaction.commit()
        let rows = table.rows(in: r)
        var out = Set(base.map(Int.init))
        if rows.length > 0 { out.formUnion(rows.location..<(rows.location + rows.length)) }
        return out.sorted()
    }

    func finish() { layer.removeFromSuperlayer() }
}

/// Time from opening a folder to its first drawn row (`bench:open`).
@MainActor
enum FirstRowsProbe {
    static var pending = false
    static var start = ContinuousClock.now
    static var result: Duration?
    static func arm() { pending = true; result = nil; start = .now }
    static func mark() { pending = false; result = .now - start }
}

// SidebarViewController.swift — the sidebar (F-019): 200 pt, system
// `.sidebar` material (from the split view's sidebar item), a source-list
// NSOutlineView with Favourites, Recent Folders, Locations and Tags, and a
// filter field at the bottom (QSpace).
//
// - Favourites come from the core's store: drag to reorder
//   (`moveFavourite`), drop folders from the list, Finder or another app to
//   pin them at the drop position (`addFavourite`), drag one out of the
//   sidebar (poof) or press ⌫ on it to unpin it (`sidebar.remove`), and
//   right-click › Remove from Favourites / Rename Favourite…. A favourite
//   whose folder has gone shows dimmed; clicking it offers Remove.
// - Recent Folders come from frecency (`engine.frecent`).
// - Locations are the core's volumes with free space; removable ones get an
//   eject button, which performs `drive.eject`; the core answers with
//   `ShellRequest.eject`, carried out here with NSWorkspace (errors shown).
// - Tags are Finder's tag names and colours (browsing by tag needs search, M3).
// - Right-click menus come from `engine.contextMenu(.sidebarItem…)`.
// - A click or arrow key goes to the folder in the focused tab; a middle-click
//   opens it in a new tab (`nav.open_new_tab`).
// The rows only mirror `AppModel`; nothing is stored here.

import AppKit

/// One sidebar row.
@MainActor
final class SidebarNode: NSObject {
    enum Kind: Equatable {
        case section(String)
        case favourite(Favourite)
        case recent(String)
        case volume(Volume)
        case tag(name: String, color: Int)
    }
    let id: String
    let kind: Kind
    var children: [SidebarNode] = []

    init(_ id: String, _ kind: Kind, children: [SidebarNode] = []) {
        self.id = id; self.kind = kind; self.children = children
    }

    var isSection: Bool { if case .section = kind { true } else { false } }

    var title: String {
        switch kind {
        case .section(let t): t
        case .favourite(let f): f.name
        case .recent(let p): p == "/" ? "/" : (p as NSString).lastPathComponent
        // The core names the boot volume "/" for now; show what Finder shows.
        case .volume(let v): v.name.isEmpty || v.name == "/" ? FileManager.default.displayName(atPath: v.mountPath) : v.name
        case .tag(let n, _): n
        }
    }

    /// The folder this row goes to.
    var path: String? {
        switch kind {
        case .favourite(let f): f.path
        case .recent(let p): p
        case .volume(let v): v.mountPath
        default: nil
        }
    }
}

@MainActor
final class SidebarViewController: NSViewController, NSOutlineViewDataSource, NSOutlineViewDelegate, NSSearchFieldDelegate {
    private unowned let model: AppModel
    private let windowModel: WindowModel
    let outline = SidebarOutlineView()
    private let scroll = NSScrollView()
    let filterField = NSSearchField()
    private var roots: [SidebarNode] = []
    private var observer: ObserverToken?
    private var collapsed = Set<String>()
    private var syncingSelection = false
    private var lastLocation: String?
    /// The drag type of a favourite being reordered or dragged out.
    static let favouriteType = NSPasteboard.PasteboardType("app.filestudio.sidebar-favourite")

    init(model: AppModel, window: WindowModel) {
        self.model = model
        self.windowModel = window
        super.init(nibName: nil, bundle: nil)
        SidebarServices.install(model)
        #if DEBUG
        SidebarDebugSteps.install()
        #endif
    }
    required init?(coder: NSCoder) { fatalError() }

    override func loadView() {
        let root = NSView()
        let col = NSTableColumn(identifier: .init("main"))
        col.resizingMask = .autoresizingMask
        outline.addTableColumn(col)
        outline.outlineTableColumn = col
        outline.headerView = nil
        outline.style = .sourceList
        outline.rowSizeStyle = .default
        outline.floatsGroupRows = false
        outline.indentationPerLevel = 0
        outline.autoresizesOutlineColumn = false
        outline.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        outline.dataSource = self
        outline.delegate = self
        outline.target = self
        outline.action = #selector(clicked)
        outline.registerForDraggedTypes([Self.favouriteType, .fileURL])
        outline.setDraggingSourceOperationMask([.move, .copy, .link], forLocal: true)
        outline.setDraggingSourceOperationMask([.copy, .link], forLocal: false)
        outline.setAccessibilityLabel("Sidebar")
        outline.menuProvider = { [weak self] row in self?.menu(row: row) }
        outline.onMiddleClick = { [weak self] row in self?.openInNewTab(row: row) }
        outline.onDelete = { [weak self] in self?.removeSelectedFavourite() ?? false }
        outline.isFavouriteDrag = { pb in pb.string(forType: SidebarViewController.favouriteType) != nil }
        scroll.documentView = outline
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        filterField.placeholderString = "Filter"
        filterField.controlSize = .small
        filterField.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        filterField.delegate = self
        filterField.sendsSearchStringImmediately = true
        filterField.setAccessibilityLabel("Filter the sidebar")
        for v in [scroll, filterField] {
            v.translatesAutoresizingMaskIntoConstraints = false
            root.addSubview(v)
        }
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: root.topAnchor),
            scroll.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            scroll.bottomAnchor.constraint(equalTo: filterField.topAnchor, constant: -8),
            filterField.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 10),
            filterField.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -10),
            filterField.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -10),
        ])
        view = root
        observer = model.observe { [weak self] e in
            switch e {
            case .favouritesChanged, .volumesChanged: self?.reload()
            default: break
            }
        }
        reload()
    }

    // MARK: Building rows

    private var focusedTab: TabModel? { windowModel.activeTabID.flatMap { model.tabModel($0) } }

    /// Rebuilds the rows from AppModel (and frecency), keeping collapsed sections.
    func reload() {
        _ = view
        let q = filterField.stringValue.trimmingCharacters(in: .whitespaces)
        func keep(_ n: SidebarNode) -> Bool { q.isEmpty || n.title.localizedStandardContains(q) }
        let favs = model.favourites.map { SidebarNode("fav:" + $0.path, .favourite($0)) }.filter(keep)
        let favPaths = Set(model.favourites.map(\.path))
        let recents = model.engine.frecent("", limit: 12).map(\.path)
            .filter { !favPaths.contains($0) && $0 != model.engine.home() }
            .prefix(6)
            .map { SidebarNode("rec:" + $0, .recent($0)) }.filter(keep)
        let vols = model.volumes.map { SidebarNode("vol:" + $0.mountPath, .volume($0)) }.filter(keep)
        let tags = Self.finderTags().map { SidebarNode("tag:" + $0.name, .tag(name: $0.name, color: $0.color)) }.filter(keep)
        var r: [SidebarNode] = []
        // Favourites always shows (it is the drop target for new ones).
        r.append(SidebarNode("sec:fav", .section("Favourites"), children: favs))
        if !recents.isEmpty { r.append(SidebarNode("sec:rec", .section("Recent Folders"), children: Array(recents))) }
        if !vols.isEmpty { r.append(SidebarNode("sec:vol", .section("Locations"), children: vols)) }
        if !tags.isEmpty { r.append(SidebarNode("sec:tag", .section("Tags"), children: tags)) }
        roots = r
        outline.reloadData()
        for s in roots where !collapsed.contains(s.id) || !q.isEmpty { outline.expandItem(s) }
        syncSelection()
    }

    /// The focused tab moved: refresh recents and the highlighted row.
    func locationChanged() {
        let loc = focusedTab?.location
        guard loc != lastLocation else { return }
        lastLocation = loc
        reload()
    }

    /// Finder's tag names and colours (index 1…7).
    private static func finderTags() -> [(name: String, color: Int)] {
        let labels = NSWorkspace.shared.fileLabels
        return labels.enumerated().dropFirst().map { ($0.element, $0.offset) }
    }

    // MARK: Selection follows the focused tab

    private func syncSelection() {
        guard let loc = focusedTab?.location else { return }
        var row = -1
        for i in 0..<outline.numberOfRows {
            if let n = outline.item(atRow: i) as? SidebarNode, n.path == loc { row = i; break }
        }
        syncingSelection = true
        if row >= 0 { outline.selectRowIndexes([row], byExtendingSelection: false) } else { outline.deselectAll(nil) }
        syncingSelection = false
    }

    func outlineViewSelectionDidChange(_ notification: Notification) {
        guard !syncingSelection, outline.selectedRow >= 0,
              let n = outline.item(atRow: outline.selectedRow) as? SidebarNode else { return }
        // Keyboard moves navigate as they go (Finder); clicks go through `clicked`.
        if NSApp.currentEvent?.type == .keyDown { go(n) }
    }

    @objc private func clicked() {
        guard outline.clickedRow >= 0, let n = outline.item(atRow: outline.clickedRow) as? SidebarNode else { return }
        if case .tag = n.kind {
            model.statusMessage = "Browsing by tag arrives with search (M3)."
            syncSelection()
            return
        }
        go(n)
    }

    private func go(_ n: SidebarNode) {
        if case .favourite(let f) = n.kind, f.missing {
            offerRemoveMissing(f)
            return
        }
        guard let path = n.path, let t = focusedTab else { return }
        do {
            model.handle(try t.api.go(path), in: windowModel.id)
        } catch {
            model.statusMessage = error.localizedDescription
            syncSelection()
        }
    }

    private func openInNewTab(row: Int) {
        guard let n = outline.item(atRow: row) as? SidebarNode, let p = n.path else { return }
        model.perform(Invocation(id: "nav.open_new_tab", arg: .path(p), tab: windowModel.activeTabID), via: .mouse,
                      window: windowModel.id)
    }

    // MARK: Removing favourites

    /// ⌫ (or ⌘⌫) on a selected favourite unpins it. The folder is untouched.
    /// Returns false when the selected row isn't a favourite (the key goes on).
    func removeSelectedFavourite() -> Bool {
        guard outline.selectedRow >= 0, let n = outline.item(atRow: outline.selectedRow) as? SidebarNode,
              case .favourite(let f) = n.kind else { return false }
        removeFavourite(f.path, via: .keyboard)
        return true
    }

    /// Unpins a favourite through the core (`sidebar.remove`), which says what happened.
    func removeFavourite(_ path: String, via: Via) {
        model.perform(Invocation(id: "sidebar.remove", arg: .path(path), tab: windowModel.activeTabID), via: via,
                      window: windowModel.id)
    }

    /// A favourite whose folder has gone: say so, and offer to remove it.
    private func offerRemoveMissing(_ f: Favourite) {
        syncSelection()
        let a = NSAlert()
        a.alertStyle = .informational
        a.messageText = "“\(f.name)” can’t be found."
        a.informativeText = "The folder may have been moved, renamed or deleted, or it’s on a drive that isn’t connected.\n\(f.path)"
        a.addButton(withTitle: "Remove from Favourites")
        a.addButton(withTitle: "Cancel")
        let done: (NSApplication.ModalResponse) -> Void = { [weak self] r in
            if r == .alertFirstButtonReturn { self?.removeFavourite(f.path, via: .mouse) }
        }
        if let w = view.window { a.beginSheetModal(for: w, completionHandler: done) } else { done(a.runModal()) }
    }

    // MARK: Context menus (F-022)

    private func menu(row: Int) -> NSMenu? {
        guard let n = outline.item(atRow: row) as? SidebarNode else { return nil }
        let target: MenuTarget
        switch n.kind {
        case .favourite(let f): target = .sidebarItem(path: f.path, kind: .favourite)
        case .volume(let v): target = .sidebarItem(path: v.mountPath, kind: .volume)
        case .recent(let p):
            guard let t = windowModel.activeTabID else { return nil }
            target = .folder(tab: t, path: p)
        default: return nil
        }
        guard let items = try? model.engine.contextMenu(target, context: MenuContext()) else { return nil }
        return ContextMenuBuilder.menu(items, model: model)
    }

    /// The core's menu for a row, as text (debug automation).
    func describeMenu(path: String) -> [String] {
        for i in 0..<outline.numberOfRows {
            if let n = outline.item(atRow: i) as? SidebarNode, n.path == path, let m = menu(row: i) {
                return m.items.map { $0.isSeparatorItem ? "---" : $0.title + ($0.isEnabled ? "" : " (disabled)") }
            }
        }
        return []
    }

    // MARK: Filter

    func controlTextDidChange(_ obj: Notification) { reload() }

    // MARK: Data source

    func outlineView(_ ov: NSOutlineView, numberOfChildrenOfItem item: Any?) -> Int {
        (item as? SidebarNode)?.children.count ?? roots.count
    }
    func outlineView(_ ov: NSOutlineView, child index: Int, ofItem item: Any?) -> Any {
        (item as? SidebarNode)?.children[index] ?? roots[index]
    }
    func outlineView(_ ov: NSOutlineView, isItemExpandable item: Any) -> Bool { (item as? SidebarNode)?.isSection ?? false }
    func outlineView(_ ov: NSOutlineView, isGroupItem item: Any) -> Bool { (item as? SidebarNode)?.isSection ?? false }
    func outlineView(_ ov: NSOutlineView, shouldSelectItem item: Any) -> Bool { !((item as? SidebarNode)?.isSection ?? true) }
    func outlineView(_ ov: NSOutlineView, heightOfRowByItem item: Any) -> CGFloat {
        (item as? SidebarNode)?.isSection == true ? 26 : 28
    }

    func outlineViewItemDidCollapse(_ n: Notification) {
        if let s = n.userInfo?["NSObject"] as? SidebarNode { collapsed.insert(s.id) }
    }
    func outlineViewItemDidExpand(_ n: Notification) {
        if let s = n.userInfo?["NSObject"] as? SidebarNode { collapsed.remove(s.id) }
    }

    func outlineView(_ ov: NSOutlineView, viewFor col: NSTableColumn?, item: Any) -> NSView? {
        guard let n = item as? SidebarNode else { return nil }
        if n.isSection {
            let cell = ov.makeView(withIdentifier: .init("header"), owner: nil) as? NSTableCellView ?? headerCell()
            cell.textField?.stringValue = n.title
            return cell
        }
        let cell = ov.makeView(withIdentifier: SidebarCell.id, owner: nil) as? SidebarCell ?? SidebarCell()
        cell.show(n, model: model)
        return cell
    }

    private func headerCell() -> NSTableCellView {
        let c = NSTableCellView()
        c.identifier = .init("header")
        let tf = NSTextField(labelWithString: "")
        tf.font = .systemFont(ofSize: 11, weight: .semibold)
        tf.textColor = .secondaryLabelColor
        tf.translatesAutoresizingMaskIntoConstraints = false
        c.addSubview(tf)
        c.textField = tf
        NSLayoutConstraint.activate([
            tf.leadingAnchor.constraint(equalTo: c.leadingAnchor, constant: 2),
            tf.centerYAnchor.constraint(equalTo: c.centerYAnchor, constant: 2),
        ])
        return c
    }

    // MARK: Drag and drop: reorder favourites, drop folders to add them

    func outlineView(_ ov: NSOutlineView, pasteboardWriterForItem item: Any) -> NSPasteboardWriting? {
        guard let n = item as? SidebarNode, let path = n.path else { return nil }
        let pb = NSPasteboardItem()
        if case .favourite = n.kind {
            // Only our own type: dropping a favourite on Finder or a list must
            // not copy or alias the folder. Dragging it out unpins it (poof).
            pb.setString(path, forType: Self.favouriteType)
            return pb
        }
        pb.setString(URL(fileURLWithPath: path).absoluteString, forType: .fileURL)
        return pb
    }

    /// A favourite dragged out of the sidebar and let go anywhere that didn't
    /// take it is unpinned, with the poof (Finder's sidebar).
    func outlineView(_ ov: NSOutlineView, draggingSession session: NSDraggingSession, endedAt screenPoint: NSPoint,
                     operation: NSDragOperation) {
        guard operation == [], let path = session.draggingPasteboard.string(forType: Self.favouriteType),
              !isOverSidebar(screenPoint) else { return }
        dragOut(path, at: screenPoint)
    }

    /// Is a screen point over the sidebar (where a let-go favourite stays)?
    func isOverSidebar(_ screenPoint: NSPoint) -> Bool {
        guard let w = view.window else { return false }
        let r = w.convertToScreen(view.convert(view.bounds, to: nil))
        return r.contains(screenPoint)
    }

    /// Unpins a dragged-out favourite and plays the poof where it was let go.
    func dragOut(_ path: String, at screenPoint: NSPoint) {
        Poof.show(at: screenPoint)
        removeFavourite(path, via: .mouse)
    }

    private var favSection: SidebarNode? { roots.first { $0.id == "sec:fav" } }

    func outlineView(_ ov: NSOutlineView, validateDrop info: NSDraggingInfo, proposedItem item: Any?,
                     proposedChildIndex index: Int) -> NSDragOperation {
        guard let fav = favSection, filterField.stringValue.isEmpty else { return [] }
        let pb = info.draggingPasteboard
        // Retarget a drop onto a favourite, or onto the section, to a gap.
        var at = index
        if let n = item as? SidebarNode, n !== fav {
            guard case .favourite = n.kind, let i = fav.children.firstIndex(of: n) else { return [] }
            at = i + 1
        } else if item == nil || (item as? SidebarNode) !== fav {
            return []
        }
        if at < 0 { at = fav.children.count }
        ov.setDropItem(fav, dropChildIndex: at)
        if pb.string(forType: Self.favouriteType) != nil { return .move }
        return folderURLs(pb).isEmpty ? [] : .link
    }

    func outlineView(_ ov: NSOutlineView, acceptDrop info: NSDraggingInfo, item: Any?, childIndex index: Int) -> Bool {
        acceptDrop(info.draggingPasteboard, at: index)
    }

    /// A drop on Favourites at `index` (a gap between favourites): a favourite
    /// moves there, folders are pinned there. Debug automation calls this with
    /// a pasteboard of its own.
    func acceptDrop(_ pb: NSPasteboard, at index: Int) -> Bool {
        do {
            if let path = pb.string(forType: Self.favouriteType) {
                guard let from = model.favourites.firstIndex(where: { $0.path == path }) else { return false }
                let to = index > from ? index - 1 : index
                if to != from { try model.engine.moveFavourite(path, to: UInt32(max(to, 0))) }
                return true
            }
            // The core decides what can be pinned (folders only, one per folder).
            let urls = fileURLs(pb)
            guard !urls.isEmpty else { return false }
            let before = model.engine.favourites().count
            var at = UInt32(max(index, 0))
            var refused: String?
            for url in urls {
                do {
                    try model.engine.addFavourite(url.path, index: at)
                    at += 1
                } catch { refused = error.localizedDescription }
            }
            let added = model.engine.favourites().count - before
            if added == 0 { model.statusMessage = refused ?? "Already in Favourites" }
            return added > 0
        } catch {
            model.statusMessage = error.localizedDescription
            return false
        }
    }

    /// File URLs on a pasteboard.
    private func fileURLs(_ pb: NSPasteboard) -> [URL] {
        pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
    }

    /// Folder URLs on a pasteboard: while dragging, only folders show the
    /// drop line (files can't be favourites; the core checks again on drop).
    private func folderURLs(_ pb: NSPasteboard) -> [URL] {
        fileURLs(pb).filter { (try? $0.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true }
    }

    // MARK: Debug

    /// Presses the eject button on a volume's row (debug automation).
    func pressEject(mountPath: String) -> Bool {
        for i in 0..<outline.numberOfRows {
            if let n = outline.item(atRow: i) as? SidebarNode, n.path == mountPath,
               let cell = outline.view(atColumn: 0, row: i, makeIfNecessary: true) as? SidebarCell {
                return cell.pressEject()
            }
        }
        return false
    }

    /// Selects a row by its folder (debug automation), as an arrow key would
    /// leave it, without navigating.
    func debugSelect(path: String) -> Bool {
        for i in 0..<outline.numberOfRows {
            if let n = outline.item(atRow: i) as? SidebarNode, n.path == path {
                syncingSelection = true
                outline.selectRowIndexes([i], byExtendingSelection: false)
                syncingSelection = false
                return true
            }
        }
        return false
    }

    /// The menu a right-click on a row would show (debug automation fires items from it).
    func debugMenu(path: String) -> NSMenu? {
        for i in 0..<outline.numberOfRows {
            if let n = outline.item(atRow: i) as? SidebarNode, n.path == path { return menu(row: i) }
        }
        return nil
    }

    /// Every visible row as text (debug automation: snapshots draw the material blank).
    var debugRows: [String] {
        (0..<outline.numberOfRows).compactMap { i in
            guard let n = outline.item(atRow: i) as? SidebarNode else { return nil }
            if n.isSection { return "# " + n.title }
            var s = (outline.selectedRow == i ? "▸ " : "  ") + n.title
            if case .favourite(let f) = n.kind {
                if f.missing { s += " [missing]" }
                if f.renamed { s += " (\(f.path))" }
            }
            if case .volume(let v) = n.kind {
                s += " — \(PaneText.bytes(v.freeBytes)) free" + (v.isRemovable ? " [eject]" : "")
            }
            if case .recent(let p) = n.kind { s += " (\(p))" }
            return s
        }
    }
}

/// A sidebar row: tinted symbol, name, free space for volumes, eject button.
@MainActor
final class SidebarCell: NSTableCellView {
    static let id = NSUserInterfaceItemIdentifier("item")
    private let icon = NSImageView()
    private let name = NSTextField(labelWithString: "")
    private let detail = NSTextField(labelWithString: "")
    private let eject = NSButton()
    private var ejectPath: String?
    private weak var model: AppModel?

    init() {
        super.init(frame: .zero)
        identifier = Self.id
        name.font = .systemFont(ofSize: 13)
        name.lineBreakMode = .byTruncatingTail
        name.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        name.setContentHuggingPriority(.defaultLow, for: .horizontal)
        detail.font = .monospacedDigitSystemFont(ofSize: 11, weight: .regular)
        detail.textColor = .secondaryLabelColor
        detail.setContentHuggingPriority(.required, for: .horizontal)
        detail.setContentCompressionResistancePriority(.defaultHigh, for: .horizontal)
        icon.symbolConfiguration = .init(pointSize: 14, weight: .regular)
        icon.contentTintColor = .controlAccentColor
        eject.bezelStyle = .accessoryBarAction
        eject.isBordered = false
        eject.image = NSImage(systemSymbolName: "eject.fill", accessibilityDescription: "Eject")
        eject.symbolConfiguration = .init(pointSize: 11, weight: .regular)
        eject.contentTintColor = .secondaryLabelColor
        eject.target = self
        eject.action = #selector(doEject)
        eject.toolTip = "Eject"
        let stack = NSStackView(views: [icon, name, detail, eject])
        stack.orientation = .horizontal
        stack.spacing = 6
        stack.alignment = .centerY
        stack.distribution = .fill
        for v in [icon, eject] as [NSView] { v.setContentHuggingPriority(.required, for: .horizontal) }
        stack.setCustomSpacing(8, after: icon)
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        textField = name
        imageView = icon
        NSLayoutConstraint.activate([
            icon.widthAnchor.constraint(equalToConstant: 18),
            eject.widthAnchor.constraint(equalToConstant: 18),
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 4),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -6),
            stack.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }
    required init?(coder: NSCoder) { fatalError() }

    func show(_ n: SidebarNode, model: AppModel) {
        self.model = model
        name.stringValue = n.title
        name.textColor = .labelColor
        detail.isHidden = true
        eject.isHidden = true
        ejectPath = nil
        icon.contentTintColor = .controlAccentColor
        toolTip = n.path
        switch n.kind {
        case .favourite(let f):
            icon.image = NSImage(systemSymbolName: f.missing ? "questionmark.folder"
                                 : Self.symbol(for: f.path, home: model.engine.home()), accessibilityDescription: nil)
            if f.missing {
                icon.contentTintColor = .tertiaryLabelColor
                name.textColor = .tertiaryLabelColor
                toolTip = "“\(f.name)” can’t be found (\(f.path)). Click or right-click to remove it."
            }
        case .recent:
            icon.image = NSImage(systemSymbolName: "clock", accessibilityDescription: nil)
        case .volume(let v):
            icon.image = NSImage(systemSymbolName: v.isNetwork ? "network" : v.isRemovable ? "externaldrive" : "internaldrive",
                                 accessibilityDescription: nil)
            if v.totalBytes > 0 {
                detail.stringValue = PaneText.bytes(v.freeBytes)
                detail.toolTip = "\(PaneText.bytes(v.freeBytes)) available of \(PaneText.bytes(v.totalBytes))"
                detail.isHidden = false
            }
            if v.isRemovable || v.isNetwork {
                eject.isHidden = false
                ejectPath = v.mountPath
                eject.setAccessibilityLabel("Eject \(n.title)")
            }
        case .tag(_, let c):
            icon.image = NSImage(systemSymbolName: "circle.fill", accessibilityDescription: nil)
            let colors = NSWorkspace.shared.fileLabelColors
            icon.contentTintColor = colors.indices.contains(c) ? colors[c] : .secondaryLabelColor
            toolTip = "Browsing by tag arrives with search (M3)"
        case .section:
            break
        }
        setAccessibilityLabel(detail.isHidden ? n.title : "\(n.title), \(detail.stringValue) available")
        if case .favourite(let f) = n.kind {
            setAccessibilityLabel(f.missing ? "\(n.title), missing" : n.title)
            setAccessibilityHelp(f.missing ? "The folder can’t be found. Press Delete to remove it from Favourites."
                                 : "Favourite. Press Delete to remove it from Favourites.")
        } else {
            setAccessibilityHelp(nil)
        }
    }

    /// Finder-like symbols for well-known folders.
    private static func symbol(for path: String, home: String) -> String {
        switch path {
        case "/Applications": "app.dashed"
        case home: "house"
        case home + "/Desktop": "menubar.dock.rectangle"
        case home + "/Documents": "doc"
        case home + "/Downloads": "arrow.down.circle"
        case home + "/Pictures": "photo"
        case home + "/Music": "music.note"
        case home + "/Movies": "film"
        default: path.contains("Mobile Documents") ? "icloud" : "folder"
        }
    }

    /// Presses the eject button as a click would (debug automation).
    func pressEject() -> Bool {
        guard !eject.isHidden else { return false }
        eject.performClick(nil)
        return true
    }

    @objc private func doEject() {
        guard let p = ejectPath, let model else { return }
        model.perform(Invocation(id: "drive.eject", arg: .path(p)), via: .mouse)
    }
}

/// The outline view: right-click menus and middle-clicks per row.
@MainActor
final class SidebarOutlineView: NSOutlineView {
    var menuProvider: ((Int) -> NSMenu?)?
    var onMiddleClick: ((Int) -> Void)?
    /// ⌫ / ⌘⌫ on the selected row; true when it removed a favourite.
    var onDelete: (() -> Bool)?
    /// Is this drag one of our favourites (dragged out, it unpins)?
    var isFavouriteDrag: ((NSPasteboard) -> Bool)?

    override func menu(for event: NSEvent) -> NSMenu? {
        let row = self.row(at: convert(event.locationInWindow, from: nil))
        return row >= 0 ? menuProvider?(row) : nil
    }

    /// Tab and ⇧Tab leave the sidebar for the next key view (address bar, list).
    override func keyDown(with event: NSEvent) {
        if event.keyCode == 48 {
            if event.modifierFlags.contains(.shift) { window?.selectPreviousKeyView(self) } else { window?.selectNextKeyView(self) }
            return
        }
        // ⌫ and ⌦, alone or with ⌘ (Finder's sidebar uses ⌘⌫ as well).
        let mods = event.modifierFlags.intersection([.shift, .control, .option, .command])
        if event.keyCode == 51 || event.keyCode == 117, mods == [] || mods == .command, onDelete?() == true { return }
        super.keyDown(with: event)
    }

    /// Outside the sidebar a dragged favourite shows the poof cursor, and a
    /// let-go doesn't slide back.
    override func draggingSession(_ session: NSDraggingSession, movedTo screenPoint: NSPoint) {
        super.draggingSession(session, movedTo: screenPoint)
        guard isFavouriteDrag?(session.draggingPasteboard) == true, let sv = enclosingScrollView?.superview,
              let w = window else { return }
        let outside = !w.convertToScreen(sv.convert(sv.bounds, to: nil)).contains(screenPoint)
        session.animatesToStartingPositionsOnCancelOrFail = !outside
        (outside ? NSCursor.disappearingItem : NSCursor.arrow).set()
    }

    override func otherMouseDown(with event: NSEvent) {
        let row = self.row(at: convert(event.locationInWindow, from: nil))
        if event.buttonNumber == 2, row >= 0 { onMiddleClick?(row) } else { super.otherMouseDown(with: event) }
    }
}

/// App-wide sidebar work: ejecting volumes with visible errors.
@MainActor
final class SidebarServices {
    private static var installed = false

    static func install(_ model: AppModel) {
        guard !installed else { return }
        installed = true
        model.registerShellHandler(for: "eject") { req, _ in
            guard case .eject(let path) = req else { return false }
            eject(path, model: model)
            return true
        }
        model.registerShellHandler(for: "askFavouriteName") { req, ctx in
            guard case .askFavouriteName(let path, let name) = req else { return false }
            askName(path: path, name: name, model: model, on: hostWindow(ctx.window))
            return true
        }
    }

    /// The field in the open Rename Favourite sheet (debug automation types into it).
    private(set) static var renameField: NSTextField?
    private static var renameAlert: NSAlert?

    /// Rename Favourite…: asks for a display name (empty goes back to the
    /// folder's own name), then `renameFavourite`. The folder isn't renamed.
    /// The main window to put a sheet on: the request's window, else the key one, else any.
    static func hostWindow(_ id: WindowID?) -> NSWindow? {
        let mains = NSApp.windows.filter { $0.windowController is CoreWindowIdentifying && $0.isVisible }
        if let id, let w = mains.first(where: { ($0.windowController as? CoreWindowIdentifying)?.coreWindowID == id }) {
            return w
        }
        return mains.first { $0.isKeyWindow } ?? mains.first
    }

    static func askName(path: String, name: String, model: AppModel, on host: NSWindow?) {
        let a = NSAlert()
        a.messageText = "Rename Favourite"
        a.informativeText = "The name shown in the sidebar. The folder itself keeps its name.\nLeave it empty to use the folder’s name."
        let field = NSTextField(string: name)
        field.frame = NSRect(x: 0, y: 0, width: 260, height: 24)
        field.placeholderString = (path as NSString).lastPathComponent
        field.setAccessibilityLabel("Favourite name")
        a.accessoryView = field
        a.addButton(withTitle: "Rename")
        a.addButton(withTitle: "Cancel")
        renameField = field
        renameAlert = a
        let done: (NSApplication.ModalResponse) -> Void = { r in
            renameField = nil
            renameAlert = nil
            guard r == .alertFirstButtonReturn else { return }
            do { try model.engine.renameFavourite(path, name: field.stringValue) } catch {
                model.statusMessage = error.localizedDescription
            }
        }
        if let w = host {
            a.beginSheetModal(for: w, completionHandler: done)
            a.window.initialFirstResponder = field
            a.window.makeFirstResponder(field)
        } else {
            done(a.runModal())
        }
    }

    /// Ends the open Rename Favourite sheet as if a button were pressed (debug automation).
    static func endRename(ok: Bool) {
        guard let a = renameAlert, let parent = a.window.sheetParent else { return }
        parent.endSheet(a.window, returnCode: ok ? .alertFirstButtonReturn : .alertSecondButtonReturn)
    }

    /// Unmounts and ejects off the main thread (it can take seconds), then
    /// reports a failure in an alert.
    static func eject(_ path: String, model: AppModel) {
        let name = model.volumes.first { $0.mountPath == path }?.name ?? (path as NSString).lastPathComponent
        model.statusMessage = "Ejecting \(name)…"
        DispatchQueue.global(qos: .userInitiated).async {
            var failure: String?
            do { try NSWorkspace.shared.unmountAndEjectDevice(at: URL(fileURLWithPath: path)) } catch {
                failure = error.localizedDescription
            }
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    if let failure {
                        model.statusMessage = "Couldn’t eject \(name)"
                        let a = NSAlert()
                        a.alertStyle = .warning
                        a.messageText = "“\(name)” couldn’t be ejected."
                        a.informativeText = failure
                        if let w = NSApp.keyWindow { a.beginSheetModal(for: w) } else { a.runModal() }
                    } else {
                        model.statusMessage = "Ejected \(name)"
                        model.engine.volumesChanged()
                    }
                }
            }
        }
    }
}

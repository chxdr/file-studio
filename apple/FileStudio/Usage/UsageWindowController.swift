// UsageWindowController.swift — the disk usage window (M3, QDirStat-style).
//
// Opened by `ShellRequest.showDiskUsage` (Go › Show Disk Usage, the folder
// and package context menus, a volume's sidebar menu). One window per
// scanned folder:
//
//   ┌ toolbar: ‹ zoom out · On Disk | Logical · rescan/stop ────────────────┐
//   │ tree list (size-sorted, expandable)          │ breakdown by type     │
//   ├──────────────────────────────────────────────┴───────────────────────┤
//   │ breadcrumb of the treemap's folder                                   │
//   │ treemap (Core Graphics)                                              │
//   │ legend: the top types' colours                                       │
//   └ status: hovered or selected item, or the scan's summary ─────────────┘
//
// Everything it shows comes formatted from the core (`UsageViewAPI`); it
// forwards clicks and keys as intents. Move to Trash goes through the ops
// queue (`usageTrash`) with the usual question, toast and undo; after any
// operation finishes, the trashed paths are re-checked so sizes update.

import AppKit

/// The usage window. Its ⌘ keys are taken by a local key monitor (see
/// `install`) before the main menu sees them, so ⌘⌫, ⌘↑ or ⌘W here never act
/// on a file window's selection or folder.
final class UsageWindow: NSWindow {
    var onCommandKey: ((NSEvent) -> Bool)?
}

@MainActor
final class UsageWindowController: NSWindowController, NSWindowDelegate, NSTableViewDataSource,
    NSTableViewDelegate, NSToolbarDelegate {
    // MARK: Opening

    private(set) static var open: [UsageWindowController] = []

    /// Registers the shell handlers (and debug steps). Call once at launch.
    static func install(_ model: AppModel) {
        model.registerShellHandler(for: "showDiskUsage") { req, _ in
            guard case .showDiskUsage(let path) = req else { return false }
            show(path: path, model: model)
            return true
        }
        // A toast for something done from this window shows in this window.
        for kind in ["moved", "toast"] {
            model.registerShellHandler(for: kind) { req, _ in
                guard let c = open.first(where: { $0.window?.isKeyWindow == true }) else { return false }
                switch req {
                case .moved(let text): c.banner(text, undo: true)
                case .toast(let text, _, let command, _): c.banner(text, undo: command == "edit.undo")
                default: return false
                }
                return true
            }
        }
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { e in
            guard e.modifierFlags.contains(.command),
                  let w = (e.window as? UsageWindow) ?? (NSApp.keyWindow as? UsageWindow),
                  w.attachedSheet == nil, w.onCommandKey?(e) == true else { return e }
            return nil
        }
        #if DEBUG
        UsageDebugSteps.install()
        #endif
    }

    nonisolated(unsafe) private static var keyMonitor: Any?

    /// Shows the window for `path`, reusing one already open on it.
    @discardableResult
    static func show(path: String, model: AppModel) -> UsageWindowController? {
        if let c = open.first(where: { $0.usage.rootPath() == path }) {
            c.showWindow(nil)
            c.window?.makeKeyAndOrderFront(nil)
            return c
        }
        guard let engine = model.engine as? UsageEngineAPI else { return nil }
        let holder = WeakBox()
        let usage: UsageViewAPI
        do {
            usage = try engine.usageScan(
                path: path,
                progress: { _ in AppModel.onMain { (holder.value as? UsageWindowController)?.progressTick() } },
                changed: { ch in AppModel.onMain { (holder.value as? UsageWindowController)?.changed(ch) } })
        } catch {
            model.statusMessage = (error as? CoreError)?.message ?? error.localizedDescription
            NSSound.beep()
            return nil
        }
        let c = UsageWindowController(model: model, usage: usage)
        holder.value = c
        open.append(c)
        c.showWindow(nil)
        c.window?.makeKeyAndOrderFront(nil)
        c.reloadAll()
        return c
    }

    private final class WeakBox: @unchecked Sendable { weak var value: AnyObject? }

    // MARK: State

    let model: AppModel
    let usage: UsageViewAPI
    let tree = UsageTableView()
    let types = UsageTableView()
    let treemap = UsageTreemapView()
    private let crumbs = NSStackView()
    private let legend = NSStackView()
    private let status = NSTextField(labelWithString: "")
    private let overlay = NSVisualEffectView()
    private let overlaySpinner = NSProgressIndicator()
    private let overlayTitle = NSTextField(labelWithString: "")
    private let overlayProgress = NSTextField(labelWithString: "")
    private let overlayCurrent = NSTextField(labelWithString: "")
    private let bannerView = NSVisualEffectView()
    private let bannerLabel = NSTextField(labelWithString: "")
    private let bannerUndo = NSButton(title: "Undo", target: nil, action: nil)
    private var bannerTimer: Timer?
    private let modeControl = NSSegmentedControl(labels: ["On Disk", "Logical"], trackingMode: .selectOne,
                                                 target: nil, action: nil)
    private let rescanButton = NSButton()
    private let zoomOutButton = NSButton()

    private var rowCache: [Int: UsageRow] = [:]
    private var typeRows: [UsageTypeRow] = []
    private var trashed = Set<String>()
    private var syncingSelection = false
    private var hoverID: UInt32?
    private var lastState: UsageState = .scanning
    private var eventToken: ObserverToken?

    init(model: AppModel, usage: UsageViewAPI) {
        self.model = model
        self.usage = usage
        let w = UsageWindow(contentRect: NSRect(x: 0, y: 0, width: 1120, height: 780),
                            styleMask: [.titled, .closable, .miniaturizable, .resizable],
                            backing: .buffered, defer: false)
        w.title = "Disk Usage"
        w.toolbarStyle = .unified
        w.minSize = NSSize(width: 720, height: 520)
        w.isReleasedWhenClosed = false
        w.setFrameAutosaveName("UsageWindow")
        if w.frame.origin == .zero { w.center() }
        super.init(window: w)
        w.delegate = self
        w.onCommandKey = { [weak self] e in self?.commandKey(e) ?? false }
        buildToolbar()
        buildContent()
        eventToken = model.observe { [weak self] e in self?.coreEvent(e) }
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    func windowWillClose(_ n: Notification) {
        usage.cancel()
        eventToken = nil
        bannerTimer?.invalidate()
        UsageWindowController.open.removeAll { $0 === self }
    }

    // MARK: Layout

    private func buildToolbar() {
        let tb = NSToolbar(identifier: "UsageToolbar")
        tb.delegate = self
        tb.displayMode = .iconOnly
        window?.toolbar = tb
        modeControl.target = self
        modeControl.action = #selector(modeChanged)
        modeControl.selectedSegment = 0
        modeControl.segmentStyle = .automatic
        modeControl.toolTip = "Sizes on disk, or the files’ own sizes as Finder shows them"
        for (b, sym, tip, act) in [
            (rescanButton, "arrow.clockwise", "Scan again", #selector(rescanOrStop)),
            (zoomOutButton, "chevron.backward", "Zoom out (⌘↑)", #selector(zoomOutAction)),
        ] {
            b.bezelStyle = .texturedRounded
            b.image = NSImage(systemSymbolName: sym, accessibilityDescription: tip)
            b.toolTip = tip
            b.target = self
            b.action = act
        }
    }

    func toolbarAllowedItemIdentifiers(_ t: NSToolbar) -> [NSToolbarItem.Identifier] {
        [.init("zoomOut"), .init("mode"), .flexibleSpace, .init("rescan")]
    }

    func toolbarDefaultItemIdentifiers(_ t: NSToolbar) -> [NSToolbarItem.Identifier] {
        toolbarAllowedItemIdentifiers(t)
    }

    func toolbar(_ t: NSToolbar, itemForItemIdentifier id: NSToolbarItem.Identifier,
                 willBeInsertedIntoToolbar flag: Bool) -> NSToolbarItem? {
        let item = NSToolbarItem(itemIdentifier: id)
        switch id.rawValue {
        case "zoomOut": item.view = zoomOutButton; item.label = "Zoom Out"
        case "mode": item.view = modeControl; item.label = "Size"
        case "rescan": item.view = rescanButton; item.label = "Rescan"
        default: return nil
        }
        return item
    }

    private func scroll(_ table: NSTableView) -> NSScrollView {
        let s = NSScrollView()
        s.documentView = table
        s.hasVerticalScroller = true
        s.autohidesScrollers = true
        s.drawsBackground = true
        return s
    }

    private func buildContent() {
        // Tree list.
        tree.addTableColumn(usageColumn("name", "Name", width: 330, min: 160))
        tree.addTableColumn(usageColumn("size", "Size", width: 86, min: 64, right: true))
        tree.addTableColumn(usageColumn("share", "% of Parent", width: 150, min: 90))
        tree.addTableColumn(usageColumn("items", "Items", width: 78, min: 50, right: true))
        tree.addTableColumn(usageColumn("modified", "Last Modified", width: 150, min: 90))
        tree.rowHeight = 22
        tree.style = .fullWidth
        tree.usesAlternatingRowBackgroundColors = false
        tree.allowsMultipleSelection = false
        tree.columnAutoresizingStyle = .firstColumnOnlyAutoresizingStyle
        tree.dataSource = self
        tree.delegate = self
        tree.target = self
        tree.doubleAction = #selector(treeDoubleClick)
        tree.onKey = { [weak self] e in self?.plainKey(e) ?? false }
        tree.onMenu = { [weak self] row, _ in
            guard let self, let r = self.row(row) else { return nil }
            self.select(r.id, from: .tree)
            return self.contextMenu(for: r.id)
        }
        tree.setAccessibilityLabel("Disk usage tree")

        // Breakdown by type.
        types.addTableColumn(usageColumn("type", "Type", width: 78, min: 60))
        types.addTableColumn(usageColumn("kind", "Kind", width: 82, min: 50))
        types.addTableColumn(usageColumn("tsize", "Size", width: 74, min: 60, right: true))
        types.addTableColumn(usageColumn("count", "Files", width: 56, min: 40, right: true))
        types.rowHeight = 22
        types.style = .fullWidth
        types.dataSource = self
        types.delegate = self
        types.columnAutoresizingStyle = .firstColumnOnlyAutoresizingStyle
        types.setAccessibilityLabel("Space by file type")

        let top = NSSplitView()
        top.isVertical = true
        top.dividerStyle = .thin
        let typesScroll = scroll(types)
        top.addArrangedSubview(scroll(tree))
        top.addArrangedSubview(typesScroll)
        typesScroll.widthAnchor.constraint(greaterThanOrEqualToConstant: 330).isActive = true
        top.autosaveName = "UsageTopSplit"
        top.setHoldingPriority(.defaultLow + 1, forSubviewAt: 0)
        top.setHoldingPriority(.defaultHigh, forSubviewAt: 1)

        // Treemap with its breadcrumb and legend.
        crumbs.orientation = .horizontal
        crumbs.spacing = 2
        crumbs.edgeInsets = NSEdgeInsets(top: 0, left: 8, bottom: 0, right: 8)
        legend.orientation = .horizontal
        legend.spacing = 12
        legend.edgeInsets = NSEdgeInsets(top: 0, left: 10, bottom: 0, right: 10)
        treemap.source = usage
        treemap.onSelect = { [weak self] id in self?.select(id, from: .treemap) }
        treemap.onZoom = { [weak self] id in self?.zoom(into: id) }
        treemap.onHover = { [weak self] id in self?.hoverID = id; self?.updateStatus() }
        treemap.onMenu = { [weak self] id, _ in self?.contextMenu(for: id) }
        treemap.onKey = { [weak self] e in self?.plainKey(e) ?? false }
        let mapBox = NSStackView(views: [crumbs, treemap, legend])
        mapBox.orientation = .vertical
        mapBox.spacing = 0
        mapBox.alignment = .leading
        for v in [crumbs, treemap, legend] {
            v.translatesAutoresizingMaskIntoConstraints = false
            v.widthAnchor.constraint(equalTo: mapBox.widthAnchor).isActive = true
        }
        crumbs.heightAnchor.constraint(equalToConstant: 28).isActive = true
        legend.heightAnchor.constraint(equalToConstant: 24).isActive = true
        treemap.setContentHuggingPriority(.defaultLow, for: .vertical)

        let main = NSSplitView()
        main.isVertical = false
        main.dividerStyle = .thin
        main.addArrangedSubview(top)
        main.addArrangedSubview(mapBox)
        main.autosaveName = "UsageMainSplit"

        // Status line.
        status.font = .systemFont(ofSize: 11)
        status.textColor = .secondaryLabelColor
        status.lineBreakMode = .byTruncatingMiddle
        let statusBar = NSView()
        statusBar.addSubview(status)
        status.translatesAutoresizingMaskIntoConstraints = false

        // The scanning panel, over everything until the first scan ends.
        overlay.material = .contentBackground
        overlay.blendingMode = .withinWindow
        overlaySpinner.style = .spinning
        overlaySpinner.controlSize = .regular
        overlayTitle.font = .systemFont(ofSize: 15, weight: .semibold)
        overlayProgress.font = .monospacedDigitSystemFont(ofSize: 13, weight: .regular)
        overlayCurrent.font = .systemFont(ofSize: 11)
        overlayCurrent.textColor = .secondaryLabelColor
        overlayCurrent.lineBreakMode = .byTruncatingMiddle
        overlayCurrent.maximumNumberOfLines = 1
        let stop = NSButton(title: "Stop", target: self, action: #selector(stopScan))
        stop.bezelStyle = .push
        let col = NSStackView(views: [overlaySpinner, overlayTitle, overlayProgress, overlayCurrent, stop])
        col.orientation = .vertical
        col.spacing = 8
        col.alignment = .centerX
        col.translatesAutoresizingMaskIntoConstraints = false
        overlay.addSubview(col)
        overlayCurrent.widthAnchor.constraint(lessThanOrEqualToConstant: 520).isActive = true

        // The in-window toast (Move to Trash with Undo).
        bannerView.material = .hudWindow
        bannerView.blendingMode = .withinWindow
        bannerView.state = .active
        bannerView.wantsLayer = true
        bannerView.layer?.cornerRadius = 10
        bannerView.isHidden = true
        bannerLabel.font = .systemFont(ofSize: 12, weight: .medium)
        bannerLabel.lineBreakMode = .byTruncatingMiddle
        bannerUndo.bezelStyle = .push
        bannerUndo.controlSize = .small
        bannerUndo.target = self
        bannerUndo.action = #selector(undo)
        let bannerRow = NSStackView(views: [bannerLabel, bannerUndo])
        bannerRow.spacing = 10
        bannerRow.translatesAutoresizingMaskIntoConstraints = false
        bannerView.addSubview(bannerRow)

        let root = NSView()
        for v in [main, statusBar, overlay, bannerView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            root.addSubview(v)
        }
        NSLayoutConstraint.activate([
            main.topAnchor.constraint(equalTo: root.topAnchor),
            main.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            main.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            main.bottomAnchor.constraint(equalTo: statusBar.topAnchor),
            statusBar.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            statusBar.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            statusBar.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            statusBar.heightAnchor.constraint(equalToConstant: 24),
            status.leadingAnchor.constraint(equalTo: statusBar.leadingAnchor, constant: 10),
            status.trailingAnchor.constraint(equalTo: statusBar.trailingAnchor, constant: -10),
            status.centerYAnchor.constraint(equalTo: statusBar.centerYAnchor),
            overlay.topAnchor.constraint(equalTo: root.topAnchor),
            overlay.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            overlay.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            overlay.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            col.centerXAnchor.constraint(equalTo: overlay.centerXAnchor),
            col.centerYAnchor.constraint(equalTo: overlay.centerYAnchor),
            bannerView.centerXAnchor.constraint(equalTo: root.centerXAnchor),
            bannerView.bottomAnchor.constraint(equalTo: statusBar.topAnchor, constant: -14),
            bannerRow.topAnchor.constraint(equalTo: bannerView.topAnchor, constant: 8),
            bannerRow.bottomAnchor.constraint(equalTo: bannerView.bottomAnchor, constant: -8),
            bannerRow.leadingAnchor.constraint(equalTo: bannerView.leadingAnchor, constant: 14),
            bannerRow.trailingAnchor.constraint(equalTo: bannerView.trailingAnchor, constant: -10),
            bannerView.widthAnchor.constraint(lessThanOrEqualToConstant: 620),
        ])
        window?.contentView = root
        // Give the treemap most of the height on first open.
        DispatchQueue.main.async {
            main.setPosition(330, ofDividerAt: 0)
            top.setPosition(max(400, top.bounds.width - 360), ofDividerAt: 0)
        }
    }

    // MARK: Refresh

    /// Re-reads everything from the core (after a scan, a rescan, a trash).
    func reloadAll() {
        rowCache.removeAll()
        let s = usage.summary()
        lastState = s.state
        window?.subtitle = s.state == .scanning && usage.rowCount() == 0
            ? s.title : "\(s.title) — \(s.sizeText) · \(s.itemsText)"
        window?.title = "Disk Usage"
        modeControl.selectedSegment = s.mode == .onDisk ? 0 : 1
        let scanning = s.state == .scanning
        rescanButton.image = NSImage(systemSymbolName: scanning ? "stop.fill" : "arrow.clockwise",
                                     accessibilityDescription: scanning ? "Stop" : "Scan again")
        rescanButton.toolTip = scanning ? "Stop scanning" : "Scan again"
        overlay.isHidden = !(scanning && usage.rowCount() == 0)
        if overlay.isHidden { overlaySpinner.stopAnimation(nil) } else { overlaySpinner.startAnimation(nil) }
        overlayTitle.stringValue = "Scanning “\(s.title)”…"
        tree.reloadData()
        typeRows = usage.types()
        types.reloadData()
        treemap.reload()
        rebuildCrumbs()
        rebuildLegend()
        syncSelectionFromCore(scroll: false)
        progressTick()
        updateStatus()
    }

    fileprivate func progressTick() {
        let s = usage.summary()
        overlayProgress.stringValue = s.progressText
        overlayCurrent.stringValue = s.current
        if s.state == .scanning, usage.rowCount() > 0 {
            status.stringValue = "Scanning… \(s.progressText)"
        }
    }

    fileprivate func changed(_ c: UsageChange) {
        reloadAll()
        if c == .finished, window?.firstResponder === window || window?.firstResponder == nil {
            window?.makeFirstResponder(tree)
        }
    }

    private func coreEvent(_ e: CoreEvent) {
        switch e {
        case .opFinished, .undoChanged:
            // Something may have trashed or put back what this window shows.
            if !trashed.isEmpty { usage.syncPaths(Array(trashed)) }
        default: break
        }
    }

    private func rebuildCrumbs() {
        crumbs.arrangedSubviews.forEach { $0.removeFromSuperview() }
        let path = usage.zoomPath()
        for (i, c) in path.enumerated() {
            if i > 0 {
                let chev = NSImageView(image: NSImage(systemSymbolName: "chevron.right",
                                                      accessibilityDescription: nil) ?? NSImage())
                chev.contentTintColor = .tertiaryLabelColor
                chev.symbolConfiguration = .init(pointSize: 9, weight: .semibold)
                crumbs.addArrangedSubview(chev)
            }
            let b = NSButton(title: c.name, target: self, action: #selector(crumbClicked(_:)))
            b.bezelStyle = .accessoryBarAction
            b.controlSize = .small
            b.font = .systemFont(ofSize: 12, weight: i == path.count - 1 ? .semibold : .regular)
            b.tag = Int(c.id)
            crumbs.addArrangedSubview(b)
        }
        zoomOutButton.isEnabled = path.count > 1
    }

    private func rebuildLegend() {
        legend.arrangedSubviews.forEach { $0.removeFromSuperview() }
        var entries = usage.legend().map { (name: $0.name, slot: UInt8($0.id)) }
        if !entries.isEmpty { entries.append((name: "Other", slot: 0)) }
        defer {
            // Keep the entries together on the left.
            let spacer = NSView()
            spacer.setContentHuggingPriority(.init(1), for: .horizontal)
            legend.addArrangedSubview(spacer)
        }
        for e in entries {
            let sw = NSImageView(image: UsagePalette.swatch(e.slot))
            let l = NSTextField(labelWithString: e.name)
            l.font = .systemFont(ofSize: 11)
            l.textColor = .secondaryLabelColor
            let pair = NSStackView(views: [sw, l])
            pair.spacing = 4
            legend.addArrangedSubview(pair)
        }
    }

    private func updateStatus() {
        let id = hoverID ?? usage.selected()
        if let id, let i = usage.info(id) {
            var parts = ["\(i.name) — \(i.sizeText)"]
            if !i.shareText.isEmpty { parts.append(i.shareText) }
            parts.append(i.typeText)
            if !i.itemsText.isEmpty { parts.append(i.itemsText) }
            if !i.note.isEmpty { parts.append(i.note) }
            parts.append(i.path)
            status.stringValue = parts.joined(separator: " · ")
            treemap.toolTip = hoverID == nil ? nil : "\(i.path)\n\(i.sizeText)"
        } else {
            status.stringValue = usage.summary().detail
        }
    }

    // MARK: Selection

    private enum Origin { case tree, treemap, core }

    /// Selects a node in the core, then mirrors it in the tree and treemap.
    private func select(_ id: UInt32, from: Origin) {
        usage.select(id)
        rowCache.removeAll()
        tree.reloadData()
        syncSelectionFromCore(scroll: from != .tree)
        updateStatus()
    }

    private func syncSelectionFromCore(scroll: Bool) {
        let sel = usage.selected()
        treemap.selectedID = sel
        syncingSelection = true
        defer { syncingSelection = false }
        if let sel, let row = usage.rowOf(sel) {
            tree.selectRowIndexes([Int(row)], byExtendingSelection: false)
            if scroll { tree.scrollRowToVisible(Int(row)) }
        } else {
            tree.deselectAll(nil)
        }
    }

    private func move(_ mv: UsageMove) {
        usage.moveSelection(mv)
        rowCache.removeAll()
        tree.reloadData()
        syncSelectionFromCore(scroll: true)
        updateStatus()
    }

    private var selectedPaths: [String] { usage.selected().map { usage.paths([$0]) } ?? [] }

    // MARK: Keys

    /// Plain keys in the tree or treemap. The core decides where they go.
    private func plainKey(_ e: NSEvent) -> Bool {
        let mods = e.modifierFlags.intersection([.command, .option, .control, .shift])
        guard mods.isEmpty || mods == [.shift] else { return false }
        let page = UInt32(max(1, Int(tree.visibleRect.height / tree.rowHeight) - 1))
        switch e.keyCode {
        case 126: move(.up)
        case 125: move(.down)
        case 123: move(.left)
        case 124: move(.right)
        case 115: move(.home)
        case 119: move(.end)
        case 116: move(.pageUp(page))
        case 121: move(.pageDown(page))
        case 49: quickLook()
        case 36, 76: if let s = usage.selected() { zoom(into: s) }
        case 53, 51: zoomOutAction()
        default: return false
        }
        return true
    }

    /// Command keys anywhere in this window (before the main menu).
    private func commandKey(_ e: NSEvent) -> Bool {
        guard e.type == .keyDown else { return false }
        let mods = e.modifierFlags.intersection([.command, .option, .control, .shift])
        let key = e.charactersIgnoringModifiers?.lowercased() ?? ""
        switch (mods, key, e.keyCode) {
        case ([.command], _, 51): trashSelected()
        case ([.command], "i", _): getInfo()
        case ([.command], "r", _): revealInPane()
        case ([.command], "y", _): quickLook()
        case ([.command], _, 126): zoomOutAction()
        case ([.command], _, 125): if let s = usage.selected() { zoom(into: s) }
        case ([.command], "w", _): window?.performClose(nil)
        case ([.command], "z", _): undo()
        case ([.command, .shift], "r", _): rescanSelected()
        default: return false
        }
        return true
    }

    // MARK: Actions

    @objc private func modeChanged() {
        usage.setMode(modeControl.selectedSegment == 0 ? .onDisk : .logical)
        reloadAll()
    }

    @objc private func rescanOrStop() {
        if usage.summary().state == .scanning { usage.cancel() } else { usage.rescan() }
    }

    @objc private func stopScan() { usage.cancel() }

    @objc private func zoomOutAction() {
        if usage.zoomOut() { afterZoom() }
    }

    @objc private func crumbClicked(_ b: NSButton) {
        usage.setZoom(UInt32(b.tag))
        afterZoom()
    }

    @objc private func treeDoubleClick() {
        let r = tree.clickedRow
        guard r >= 0, let row = row(r) else { return }
        if row.expandable { usage.setExpanded(row.id, !row.expanded); select(row.id, from: .tree) }
        zoom(into: row.id)
    }

    private func zoom(into id: UInt32) {
        usage.setZoom(id)
        select(id, from: .core)
        afterZoom()
    }

    private func afterZoom() {
        treemap.reload()
        typeRows = usage.types()
        types.reloadData()
        rebuildCrumbs()
        treemap.selectedID = usage.selected()
    }

    private func quickLook() {
        let p = selectedPaths
        if !p.isEmpty { QuickLookController.shared.toggle(p) }
    }

    private func getInfo() {
        let p = selectedPaths
        if !p.isEmpty { PlatformActions.getInfo(p) }
    }

    /// Shows the item in the focused file window's pane, selected.
    private func revealInPane() {
        guard let p = selectedPaths.first, let t = model.focusedTab else { return }
        do { model.handle(try t.api.go(p), in: model.keyWindowID) } catch {
            model.statusMessage = (error as? CoreError)?.message ?? error.localizedDescription
        }
        let w = NSApp.windows.first { ($0.windowController as? CoreWindowIdentifying)?.coreWindowID == model.keyWindowID }
        w?.makeKeyAndOrderFront(nil)
    }

    private func rescanSelected() {
        guard let s = usage.selected() else { return }
        usage.refresh(s)
    }

    private func trashSelected() {
        guard let s = usage.selected(), usage.info(s)?.isRoot == false else { NSSound.beep(); return }
        let paths = usage.paths([s])
        guard !paths.isEmpty, let engine = model.engine as? UsageEngineAPI else { return }
        trashed.formUnion(paths)
        do { model.handle(try engine.usageTrash(paths: paths), in: model.keyWindowID) } catch {
            model.statusMessage = (error as? CoreError)?.message ?? error.localizedDescription
            NSSound.beep()
        }
    }

    @objc private func undo() {
        bannerView.isHidden = true
        model.perform(Invocation(id: "edit.undo"), via: .mouse)
    }

    /// A short message at the bottom of this window, with Undo when it applies.
    func banner(_ text: String, undo: Bool) {
        bannerLabel.stringValue = text
        bannerUndo.isHidden = !undo
        bannerView.isHidden = false
        bannerTimer?.invalidate()
        bannerTimer = Timer.scheduledTimer(withTimeInterval: 6, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.bannerView.isHidden = true }
        }
    }

    private func contextMenu(for id: UInt32) -> NSMenu {
        let m = NSMenu()
        let info = usage.info(id)
        func add(_ title: String, _ sel: Selector, _ key: String = "", _ mods: NSEvent.ModifierFlags = .command) {
            let i = NSMenuItem(title: title, action: sel, keyEquivalent: key)
            i.keyEquivalentModifierMask = mods
            i.target = self
            m.addItem(i)
        }
        add("Reveal in Pane", #selector(menuReveal), "r")
        add("Quick Look", #selector(menuQuickLook), "y")
        add("Get Info", #selector(menuGetInfo), "i")
        m.addItem(.separator())
        if info?.isFolder == true { add("Zoom In", #selector(menuZoomIn), "\u{F701}") }
        if usage.zoomPath().count > 1 { add("Zoom Out", #selector(zoomOutAction), "\u{F700}") }
        if info?.isFolder == true { add("Rescan Folder", #selector(menuRescan), "r", [.command, .shift]) }
        if info?.isRoot == false {
            m.addItem(.separator())
            add("Move to Trash", #selector(menuTrash), "\u{8}")
        }
        return m
    }

    @objc private func menuReveal() { revealInPane() }
    @objc private func menuQuickLook() { quickLook() }
    @objc private func menuGetInfo() { getInfo() }
    @objc private func menuZoomIn() { if let s = usage.selected() { zoom(into: s) } }
    @objc private func menuRescan() { rescanSelected() }
    @objc private func menuTrash() { trashSelected() }

    // MARK: Tables

    private func row(_ i: Int) -> UsageRow? {
        if let r = rowCache[i] { return r }
        let start = max(0, i - 16)
        for (k, r) in usage.rows(start: UInt32(start), count: 64).enumerated() { rowCache[start + k] = r }
        return rowCache[i]
    }

    func numberOfRows(in t: NSTableView) -> Int {
        t === tree ? Int(usage.rowCount()) : typeRows.count
    }

    func tableView(_ t: NSTableView, viewFor col: NSTableColumn?, row i: Int) -> NSView? {
        guard let col else { return nil }
        if t === types { return typeCell(col, i) }
        guard let r = row(i) else { return nil }
        switch col.identifier.rawValue {
        case "name":
            let id = NSUserInterfaceItemIdentifier("usage.name")
            let c = (t.makeView(withIdentifier: id, owner: nil) as? UsageNameCell) ?? {
                let c = UsageNameCell(frame: .zero)
                c.identifier = id
                return c
            }()
            c.show(r, path: r.kind == .package ? usage.paths([r.id]).first : nil)
            c.onToggle = { [weak self] in
                guard let self else { return }
                self.usage.setExpanded(r.id, !r.expanded)
                self.rowCache.removeAll()
                self.tree.reloadData()
                self.syncSelectionFromCore(scroll: false)
            }
            return c
        case "share":
            let id = NSUserInterfaceItemIdentifier("usage.share")
            let c = (t.makeView(withIdentifier: id, owner: nil) as? UsageBarView) ?? {
                let c = UsageBarView()
                c.identifier = id
                return c
            }()
            c.fraction = r.fraction
            c.text = r.percentText
            return c
        case "size":
            let c = usageTextCell(t, id: "usage.size", right: true, secondary: false)
            c.textField?.stringValue = r.sizeText
            return c
        case "items":
            let c = usageTextCell(t, id: "usage.items", right: true, secondary: true)
            c.textField?.stringValue = r.itemsText
            return c
        default:
            let c = usageTextCell(t, id: "usage.modified", right: false, secondary: true)
            c.textField?.stringValue = r.modifiedText
            return c
        }
    }

    private func typeCell(_ col: NSTableColumn, _ i: Int) -> NSView? {
        guard i < typeRows.count else { return nil }
        let r = typeRows[i]
        switch col.identifier.rawValue {
        case "type":
            let id = NSUserInterfaceItemIdentifier("usage.type")
            let c = (types.makeView(withIdentifier: id, owner: nil) as? UsageTypeCell) ?? {
                let c = UsageTypeCell(frame: .zero)
                c.identifier = id
                return c
            }()
            c.swatch.image = UsagePalette.swatch(r.slot)
            c.label.stringValue = r.name
            c.toolTip = "\(r.name): \(r.sizeText), \(r.countText) files"
            return c
        case "kind":
            let c = usageTextCell(types, id: "usage.kind", right: false, secondary: true)
            c.textField?.stringValue = r.kindText
            return c
        case "tsize":
            let c = usageTextCell(types, id: "usage.tsize", right: true, secondary: false)
            c.textField?.stringValue = r.sizeText
            return c
        default:
            let c = usageTextCell(types, id: "usage.count", right: true, secondary: true)
            c.textField?.stringValue = r.countText
            return c
        }
    }

    func tableViewSelectionDidChange(_ n: Notification) {
        guard !syncingSelection, (n.object as? NSTableView) === tree else { return }
        let i = tree.selectedRow
        guard i >= 0, let r = row(i) else { return }
        usage.select(r.id)
        treemap.selectedID = r.id
        updateStatus()
    }

    func tableView(_ t: NSTableView, rowViewForRow row: Int) -> NSTableRowView? { nil }

    // MARK: Debug

    #if DEBUG
    /// For debug steps: the window's state as plain lines.
    func debugLines() -> [String] {
        let s = usage.summary()
        var out = ["USAGE summary \(s.title) | \(s.sizeText) | \(s.itemsText) | \(s.state) | \(s.detail)"]
        for r in usage.rows(start: 0, count: 12) {
            out.append("USAGE row \(String(repeating: "  ", count: Int(r.depth)))\(r.name) | \(r.sizeText) | \(r.percentText) | \(r.itemsText) | \(r.selected ? "selected" : "")")
        }
        for t in usage.types().prefix(6) {
            out.append("USAGE type \(t.name) | \(t.kindText) | \(t.sizeText) | \(t.countText) | slot \(t.slot)")
        }
        out.append("USAGE treemap tiles=\(treemap.tileCount) selected=\(usage.selected().map(String.init) ?? "-") zoom=\(usage.zoomPath().map(\.name).joined(separator: "/"))")
        out.append("USAGE status \(status.stringValue)")
        return out
    }

    func debugSelect(path: String) {
        if let id = usage.find(path) { select(id, from: .core) }
    }

    func debugZoom(path: String) {
        if let id = usage.find(path) { zoom(into: id) }
    }

    func debugTrash() { trashSelected() }

    func debugTreemapPoint(of path: String) -> CGPoint? {
        guard let id = usage.find(path), let r = treemap.rect(of: id) else { return nil }
        return treemap.convert(CGPoint(x: r.midX, y: r.midY), to: nil)
    }
    #endif
}

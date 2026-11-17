// TileGrid.swift — a scrolling collection of tiles: the icon view (a
// vertical grid with names) and the gallery's filmstrip (one horizontal row).
//
// Everything it shows and does comes from the core, as in FileTable:
// - layout: `gridLayout` (items per row, cell size) for the icon view; the
//   column count goes back with `setGridColumns` so ↑/↓ move a row;
// - rows: `rows_packed` blocks through RowCache, dropped on every change;
// - selection: clicks, ⌘/⇧-clicks and the rubber band send `SelectionChange`s;
// - keys: never handled here; KeyRouter sends them to the core (`.list`);
// - thumbnails: `thumbnailRequests` says which, in which order;
// - menus: `menuTarget` + `contextMenu`; opening: `nav.open`; drops:
//   `dropEffect` / `dropFiles` (the core decides move or copy), with
//   spring-loaded folders after 0.8 s; rename: `beginRename` → RenameHost.
// Only the tiles on screen exist; they are reused as the view scrolls.

import AppKit

/// The tiles' document view: reports `.list` focus for its tab and passes
/// mouse and drag input to its controller.
@MainActor
final class TileDocView: NSView, CoreFocusReporting {
    var coreFocus: Focus { .list }
    var coreTab: TabID?
    weak var grid: TileGrid?
    var takesFocus = true

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { takesFocus }
    override var wantsUpdateLayer: Bool { true }
    override func updateLayer() { layer?.backgroundColor = NSColor.textBackgroundColor.cgColor }

    override func mouseDown(with event: NSEvent) {
        if takesFocus { window?.makeFirstResponder(self) }
        grid?.mouseDown(event)
    }
    override func mouseDragged(with event: NSEvent) { grid?.mouseDragged(event) }
    override func mouseUp(with event: NSEvent) { grid?.mouseUp(event) }
    override func rightMouseDown(with event: NSEvent) {
        if takesFocus { window?.makeFirstResponder(self) }
        super.rightMouseDown(with: event)
    }
    override func menu(for event: NSEvent) -> NSMenu? { grid?.contextMenu(event) }
    override func becomeFirstResponder() -> Bool {
        DispatchQueue.main.async { [weak self] in self?.grid?.refreshActive() }
        return true
    }
    override func resignFirstResponder() -> Bool {
        DispatchQueue.main.async { [weak self] in self?.grid?.refreshActive() }
        return true
    }
    // The core handles ⌘C/⌘V/⌘A through KeyRouter; these cover the menu items.
    @objc func copy(_ sender: Any?) { AppModel.current?.perform(Invocation(id: "edit.copy", tab: coreTab), via: .menu) }
    @objc func paste(_ sender: Any?) { AppModel.current?.perform(Invocation(id: "edit.paste", tab: coreTab), via: .menu) }
    override func selectAll(_ sender: Any?) {
        AppModel.current?.perform(Invocation(id: "select.all", tab: coreTab), via: .menu)
    }

    // Drops.
    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation { grid?.validateDrop(sender) ?? [] }
    override func draggingUpdated(_ sender: NSDraggingInfo) -> NSDragOperation { grid?.validateDrop(sender) ?? [] }
    override func draggingExited(_ sender: NSDraggingInfo?) { grid?.dragExited() }
    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool { grid?.acceptDrop(sender) ?? false }
    override func concludeDragOperation(_ sender: NSDraggingInfo?) { grid?.dragExited() }

    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .list }
}

/// The filmstrip's scroll view: a mouse wheel's vertical turn scrolls it
/// sideways (the user scrolls with a wheel, not a trackpad).
final class StripScrollView: NSScrollView {
    override func scrollWheel(with e: NSEvent) {
        guard abs(e.scrollingDeltaY) > abs(e.scrollingDeltaX) else { super.scrollWheel(with: e); return }
        let k: CGFloat = e.hasPreciseScrollingDeltas ? 1 : 12
        let clip = contentView
        let maxX = max((documentView?.frame.width ?? 0) - clip.bounds.width, 0)
        let x = min(max(clip.bounds.minX - e.scrollingDeltaY * k, 0), maxX)
        clip.scroll(to: NSPoint(x: x, y: clip.bounds.minY))
        reflectScrolledClipView(clip)
    }
}

@MainActor
final class TileGrid: NSObject, NSDraggingSource {
    enum Axis { case vertical, horizontal }

    unowned let model: AppModel
    let axis: Axis
    let doc = TileDocView()
    let scrollView: NSScrollView
    /// What hosts lay out: the scroll view with the state messages over it.
    let view = NSView()
    let stateView = ListStateView()
    private(set) var source: RowSource?
    private(set) var ownerTab: TabID?
    let cache = RowCache()
    private(set) var info = ListingInfo(version: .max, dir: "", state: .loading, error: nil, count: 0, totalCount: 0)
    private var count: Int { Int(info.count) }
    private var observer: ObserverToken?
    var rename: RenameController?
    /// The item under a click or tap changed the cursor (the gallery follows it).
    var onCursorChange: (() -> Void)?
    private let thumbs = ThumbnailLoader()

    // Geometry.
    private(set) var layout = GridLayout(columns: 1, cellWidth: 100, cellHeight: 100, imageSize: 64, labelHeight: 32,
                                         inset: 12)
    /// The filmstrip's thumbnail size (points).
    var stripSize: CGFloat = 72
    private var tiles: [Int: GridTile] = [:]
    private var pool: [GridTile] = []
    private var lastOrigin: CGFloat = 0
    private var forward = true
    private var thumbsScheduled = false

    // Mouse.
    private var downEvent: NSEvent?
    private var downIndex = -1
    private var deferredOnly = false
    private var band: (start: NSPoint, base: Set<Int>, layer: CAShapeLayer)?

    // Drops.
    private struct DropTarget: Equatable { let dest: String; let version: UInt64; let index: Int }
    private var highlightedDrop: DropTarget?
    private var springTimer: Timer?
    private var springIndex = -1
    private var loadingTimer: Timer?

    init(model: AppModel, axis: Axis) {
        self.model = model
        self.axis = axis
        scrollView = axis == .horizontal ? StripScrollView() : NSScrollView()
        super.init()
        doc.grid = self
        doc.wantsLayer = true
        doc.registerForDraggedTypes([.fileURL])
        doc.setAccessibilityLabel(axis == .vertical ? "Files" : "Filmstrip")
        scrollView.documentView = doc
        scrollView.hasVerticalScroller = axis == .vertical
        scrollView.hasHorizontalScroller = axis == .horizontal
        scrollView.autohidesScrollers = true
        scrollView.drawsBackground = true
        scrollView.backgroundColor = .textBackgroundColor
        scrollView.contentView.postsBoundsChangedNotifications = true
        scrollView.contentView.postsFrameChangedNotifications = true
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
        let nc = NotificationCenter.default
        nc.addObserver(self, selector: #selector(boundsChanged), name: NSView.boundsDidChangeNotification,
                       object: scrollView.contentView)
        nc.addObserver(self, selector: #selector(frameChanged), name: NSView.frameDidChangeNotification,
                       object: scrollView.contentView)
        for n in [NSWindow.didBecomeKeyNotification, NSWindow.didResignKeyNotification] {
            nc.addObserver(self, selector: #selector(windowKeyChanged(_:)), name: n, object: nil)
        }
        observer = model.observe { [weak self] e in self?.event(e) }
        thumbs.onImage = { [weak self] key, img, _ in self?.thumbArrived(key, img) }
    }

    private var grid: GridEngineAPI? { model.engine as? GridEngineAPI }

    // MARK: Source

    /// Shows a tab. `owner` is the tab commands act through.
    func show(_ s: RowSource, owner: TabID) {
        let sameDir = source.map { $0.id == s.id && $0.dir == s.dir } ?? false
        source = s
        ownerTab = owner
        doc.coreTab = owner
        doc.takesFocus = s.tab != nil
        if !sameDir { rename?.cancel(); thumbs.cancelAll() }
        reload(restoreScroll: !sameDir)
    }

    /// Stops (the view is going away or hidden).
    func detach() {
        rename?.cancel()
        thumbs.cancelAll()
        springTimer?.invalidate()
        loadingTimer?.invalidate()
        source = nil
    }

    func row(_ i: Int) -> Row? {
        guard let s = source, i >= 0, i < count else { return nil }
        if cache.version != info.version { cache.invalidate(version: info.version) }
        return cache.row(i, from: s)
    }

    func path(_ r: Row) -> String { (info.dir as NSString).appendingPathComponent(r.name) }

    // MARK: Events

    private func event(_ e: CoreEvent) {
        guard let s = source else { return }
        switch e {
        case .listingChanged(let id, _, _) where id == s.id:
            let n = s.listingInfo()
            if n.dir != info.dir { rename?.cancel(); thumbs.cancelAll(); reload(restoreScroll: true); return }
            // Keep the top item in place unless at the top.
            let anchor = contentOffset > 0.5 ? topAnchor() : nil
            info = n
            cache.invalidate(version: info.version)
            relayout()
            if let a = anchor, let i = s.indexOf(name: a.name) { scroll(toIndex: Int(i), offset: a.offset) }
            refreshTiles(all: true)
            updateState()
            rename?.reposition()
        case .selectionChanged(let id, _, let cursor) where id == s.id:
            info = s.listingInfo()
            cache.invalidate(version: info.version)
            if Int(info.count) != tilesCountLaidOut { relayout() }
            refreshTiles(all: true)
            if let c = cursor, band == nil { reveal(Int(c)) }
            onCursorChange?()
        case .settingsChanged:
            relayout()
            refreshTiles(all: true)
        default: break
        }
    }

    private var tilesCountLaidOut = -1

    private func reload(restoreScroll: Bool) {
        guard let s = source else { return }
        info = s.listingInfo()
        cache.invalidate(version: info.version)
        relayout()
        if restoreScroll {
            let cursor = s.selection().cursor.map(Int.init)
            if axis == .vertical, let t = s.tab, let a = t.scrollAnchor(), let i = t.indexOf(name: a.name) {
                scroll(toIndex: Int(i), offset: CGFloat(a.offset))
                // Coming from the gallery (or the list) the cursor stays in sight.
                if let c = cursor, !visibleIndices.contains(c) { reveal(c) }
            } else if let c = cursor {
                reveal(Int(c), centred: axis == .horizontal)
            } else {
                scrollToStart()
            }
        }
        refreshTiles(all: true)
        updateState()
        refreshActive()
    }

    // MARK: Geometry

    /// Recomputes the layout (the core's) and the document size.
    func relayout() {
        let clip = scrollView.contentView.bounds
        tilesCountLaidOut = count
        switch axis {
        case .vertical:
            let size = GridSettings.iconSize(model)
            let l = grid?.gridLayout(width: Double(clip.width), iconSize: size) ?? layout
            if l != layout || doc.frame.width != clip.width {
                layout = l
                if let t = ownerTab { grid?.setGridColumns(tab: t, columns: l.columns) }
            }
            let rows = (count + layout.columns - 1) / layout.columns
            let h = max(CGFloat(layout.inset * 2) + CGFloat(rows) * CGFloat(layout.cellHeight), clip.height)
            doc.setFrameSize(NSSize(width: clip.width, height: h))
        case .horizontal:
            let s = Double(stripSize)
            layout = GridLayout(columns: max(count, 1), cellWidth: s + 10, cellHeight: s + 12, imageSize: s,
                                labelHeight: 0, inset: 8)
            let w = max(CGFloat(layout.inset * 2) + CGFloat(count) * CGFloat(layout.cellWidth), clip.width)
            doc.setFrameSize(NSSize(width: w, height: clip.height))
        }
        for (i, t) in tiles { t.frame = frame(of: i) }
        rename?.reposition()
    }

    /// The cell of item `i`, in document coordinates.
    func frame(of i: Int) -> NSRect {
        let l = layout
        switch axis {
        case .vertical:
            let c = i % l.columns, r = i / l.columns
            return NSRect(x: l.inset + Double(c) * l.cellWidth, y: l.inset + Double(r) * l.cellHeight,
                          width: l.cellWidth, height: l.cellHeight)
        case .horizontal:
            let h = scrollView.contentView.bounds.height
            return NSRect(x: l.inset + Double(i) * l.cellWidth, y: max((h - l.cellHeight) / 2, 0),
                          width: l.cellWidth, height: l.cellHeight)
        }
    }

    /// The item at a point in document coordinates, or -1.
    func index(at p: NSPoint) -> Int {
        let l = layout
        let i: Int
        switch axis {
        case .vertical:
            let x = p.x - l.inset, y = p.y - l.inset
            guard x >= 0, y >= 0 else { return -1 }
            let c = Int(x / l.cellWidth), r = Int(y / l.cellHeight)
            guard c < l.columns else { return -1 }
            i = r * l.columns + c
        case .horizontal:
            let x = p.x - l.inset
            guard x >= 0 else { return -1 }
            i = Int(x / l.cellWidth)
        }
        guard i >= 0, i < count else { return -1 }
        // Only the picture and the name count as the item (Finder): empty cell space starts a band.
        let f = frame(of: i)
        let t = tiles[i]
        let hot = t.map { $0.pictureRect.union($0.labelRect).offsetBy(dx: f.minX, dy: f.minY) } ?? f
        return hot.insetBy(dx: -4, dy: -4).contains(p) ? i : -1
    }

    /// Items whose cells meet a rectangle (document coordinates).
    private func indices(in r: NSRect) -> Range<Int> {
        let l = layout
        guard count > 0 else { return 0..<0 }
        switch axis {
        case .vertical:
            let r0 = max(Int(floor((r.minY - l.inset) / l.cellHeight)), 0)
            let r1 = max(Int(floor((r.maxY - l.inset) / l.cellHeight)), 0)
            return min(r0 * l.columns, count)..<min((r1 + 1) * l.columns, count)
        case .horizontal:
            let a = max(Int(floor((r.minX - l.inset) / l.cellWidth)), 0)
            let b = max(Int(floor((r.maxX - l.inset) / l.cellWidth)), 0)
            return min(a, count)..<min(b + 1, count)
        }
    }

    private var visibleRect: NSRect { scrollView.contentView.bounds }
    private var contentOffset: CGFloat { axis == .vertical ? visibleRect.minY : visibleRect.minX }

    /// Items fully or partly on screen.
    var visibleIndices: Range<Int> { indices(in: visibleRect) }

    // MARK: Tiles

    @objc private func boundsChanged() {
        let o = contentOffset
        if o != lastOrigin { forward = o > lastOrigin; lastOrigin = o }
        refreshTiles(all: false)
        rename?.reposition()
    }

    private var lastWidth: CGFloat = -1
    @objc private func frameChanged() {
        let w = axis == .vertical ? visibleRect.width : visibleRect.height
        guard w != lastWidth else { return }
        lastWidth = w
        let anchor = topAnchor()
        relayout()
        if let a = anchor, let i = source?.indexOf(name: a.name) { scroll(toIndex: Int(i), offset: a.offset) }
        refreshTiles(all: true)
    }

    private var isActiveNow = false

    /// Places tiles for the items on screen (plus a row each side), reusing
    /// the ones that scrolled off. `all` reconfigures every tile (new data).
    func refreshTiles(all: Bool) {
        guard source != nil else { return }
        let vis = visibleIndices
        let pad = axis == .vertical ? layout.columns : 3
        let want = max(vis.lowerBound - pad, 0)..<min(vis.upperBound + pad, count)
        if let s = source, !want.isEmpty { cache.prefetch(want, from: s) }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        for (i, t) in tiles where !want.contains(i) {
            t.isHidden = true
            t.index = -1
            pool.append(t)
            tiles[i] = nil
        }
        let active = isActive
        isActiveNow = active
        for i in want {
            if let t = tiles[i] {
                if all { configure(t, i, active: active) }
                continue
            }
            let t = pool.popLast() ?? {
                let t = GridTile(strip: axis == .horizontal)
                doc.addSubview(t)
                return t
            }()
            t.isHidden = false
            t.frame = frame(of: i)
            tiles[i] = t
            configure(t, i, active: active)
        }
        CATransaction.commit()
        reportVisible(vis)
        scheduleThumbnails()
    }

    private var pixels: Int {
        let scale = Double(doc.window?.backingScaleFactor ?? 2)
        return grid?.thumbPixels(points: layout.imageSize, scale: scale) ?? 256
    }

    private func thumbKey(_ r: Row) -> String {
        ThumbRequest.key(path: path(r), modifiedNs: r.modifiedNs, size: r.size, pixels: pixels)
    }

    private func configure(_ t: GridTile, _ i: Int, active: Bool) {
        guard let r = row(i) else { return }
        t.index = i
        t.frame = frame(of: i)
        let icon = IconCache.shared.icon(key: r.iconKey, path: r.kind == .package ? path(r) : nil)
        let file = r.kind != .folder && r.kind != .package && !r.isDataless
        let key = file ? thumbKey(r) : nil
        t.thumbKey = key
        let img = key.flatMap { ThumbnailLoader.image($0)?.cg }
        t.configure(row: r, image: img, icon: icon, isThumbnail: img != nil, imageSize: CGFloat(layout.imageSize),
                    labelHeight: CGFloat(layout.labelHeight), active: active, marks: GridMarks.of(r))
    }

    private func thumbArrived(_ key: String, _ img: GridImage) {
        for t in tiles.values where t.thumbKey == key {
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            t.setImage(img.cg)
            CATransaction.commit()
        }
    }

    /// Asks the core which thumbnails to make (once per run-loop turn).
    private func scheduleThumbnails() {
        guard !thumbsScheduled else { return }
        thumbsScheduled = true
        DispatchQueue.main.async { [weak self] in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.thumbsScheduled = false
                self.requestThumbnails()
            }
        }
    }

    private func requestThumbnails() {
        guard let t = ownerTab, let grid, let s = source, doc.window != nil else { return }
        let vis = visibleIndices
        let order = grid.thumbnailRequests(tab: t, start: vis.lowerBound, count: vis.count, forward: forward)
        if let lo = order.min(), let hi = order.max() { cache.prefetch(lo..<(hi + 1), from: s) }
        let px = pixels
        let reqs: [ThumbRequest] = order.compactMap { i in
            guard let r = row(i) else { return nil }
            return ThumbRequest(key: ThumbRequest.key(path: path(r), modifiedNs: r.modifiedNs, size: r.size, pixels: px),
                                path: path(r), pixels: px)
        }
        thumbs.want(reqs)
    }

    private var lastReported = -1..<(-1)
    private func reportVisible(_ vis: Range<Int>) {
        guard let s = source, vis != lastReported else { return }
        lastReported = vis
        s.setVisibleRange(start: UInt32(vis.lowerBound), count: UInt32(vis.count))
        if axis == .vertical, let t = s.tab, let a = topAnchor() {
            t.setScrollAnchor(ScrollAnchor(name: a.name, offset: Double(a.offset)))
        }
    }

    /// The first visible item's name and its offset from the top.
    private func topAnchor() -> (name: String, offset: CGFloat)? {
        let vis = visibleIndices
        guard !vis.isEmpty, let r = row(vis.lowerBound) else { return nil }
        let f = frame(of: vis.lowerBound)
        return (r.name, axis == .vertical ? f.minY - visibleRect.minY : f.minX - visibleRect.minX)
    }

    private func scroll(toIndex i: Int, offset: CGFloat) {
        let clip = scrollView.contentView
        let f = frame(of: i)
        if axis == .vertical {
            let maxY = max(doc.frame.height - clip.bounds.height, 0)
            clip.scroll(to: NSPoint(x: 0, y: min(max(f.minY - offset, 0), maxY)))
        } else {
            let maxX = max(doc.frame.width - clip.bounds.width, 0)
            clip.scroll(to: NSPoint(x: min(max(f.minX - offset, 0), maxX), y: 0))
        }
        scrollView.reflectScrolledClipView(clip)
    }

    func scrollToStart() {
        scrollView.contentView.scroll(to: .zero)
        scrollView.reflectScrolledClipView(scrollView.contentView)
    }

    /// Scrolls item `i` into view (the filmstrip keeps it towards the middle).
    func reveal(_ i: Int, centred: Bool = false) {
        guard i >= 0, i < count else { return }
        let f = frame(of: i)
        if centred || axis == .horizontal {
            let vis = visibleRect
            let margin = axis == .horizontal ? f.width * 2 : 0
            if centred || f.minX - margin < vis.minX || f.maxX + margin > vis.maxX {
                scroll(toIndex: i, offset: (vis.width - f.width) / 2)
            }
        } else {
            doc.scrollToVisible(f.insetBy(dx: 0, dy: -4))
        }
    }

    // MARK: Focus

    /// The pane is active: key window and this view (or its rename field) has focus.
    var isActive: Bool {
        guard let w = doc.window, w.isKeyWindow || FileTable.assumeKeyWindow else { return false }
        var r = w.firstResponder
        if let tv = r as? NSTextView, tv.isFieldEditor, let d = tv.delegate as? NSResponder { r = d }
        return r === doc || (r as? NSView)?.isDescendant(of: doc) == true
    }

    func refreshActive() {
        let a = isActive
        guard a != isActiveNow else { return }
        isActiveNow = a
        for t in tiles.values { t.setActive(a) }
    }

    @objc private func windowKeyChanged(_ n: Notification) {
        guard (n.object as? NSWindow) === doc.window else { return }
        refreshActive()
    }

    // MARK: Mouse

    private func point(_ e: NSEvent) -> NSPoint { doc.convert(e.locationInWindow, from: nil) }

    func mouseDown(_ e: NSEvent) {
        guard let s = source else { return }
        let i = index(at: point(e))
        let f = e.modifierFlags
        downEvent = e
        downIndex = i
        deferredOnly = false
        rename?.commitIfActive()
        if e.clickCount == 2, i >= 0 {
            model.perform(Invocation(id: "nav.open", tab: ownerTab), via: .mouse)
            return
        }
        if i < 0 {
            let keep = f.contains(.command) || f.contains(.shift)
            if !keep { s.setSelection(.none) }
            let l = CAShapeLayer()
            l.fillColor = NSColor.selectedContentBackgroundColor.withAlphaComponent(0.15).cgColor
            l.strokeColor = NSColor.selectedContentBackgroundColor.withAlphaComponent(0.7).cgColor
            l.lineWidth = 1
            l.zPosition = 100
            doc.layer?.addSublayer(l)
            band = (point(e), keep ? Set(s.selection().rows.map(Int.init)) : [], l)
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

    func mouseDragged(_ e: NSEvent) {
        guard let s = source, let down = downEvent else { return }
        if let b = band {
            doc.autoscroll(with: e)
            let p = point(e)
            let rect = NSRect(x: min(b.start.x, p.x), y: min(b.start.y, p.y), width: abs(p.x - b.start.x),
                              height: abs(p.y - b.start.y))
            CATransaction.begin(); CATransaction.setDisableActions(true)
            b.layer.path = CGPath(rect: rect, transform: nil)
            CATransaction.commit()
            var out = b.base
            for i in indices(in: rect) where frame(of: i).insetBy(dx: 8, dy: 4).intersects(rect) { out.insert(i) }
            s.setSelection(.rows(out.sorted().map { UInt32($0) }))
            return
        }
        let d = hypot(e.locationInWindow.x - down.locationInWindow.x, e.locationInWindow.y - down.locationInWindow.y)
        guard downIndex >= 0, d > 4 else { return }
        downEvent = nil
        deferredOnly = false
        var paths = s.selectedPaths()
        if paths.isEmpty || !(row(downIndex)?.isSelected ?? false), let r = row(downIndex) { paths = [path(r)] }
        startDrag(paths, event: down)
    }

    func mouseUp(_ e: NSEvent) {
        if let b = band { b.layer.removeFromSuperlayer(); band = nil }
        if deferredOnly, downIndex >= 0, let s = source { s.setSelection(.only(UInt32(downIndex))) }
        deferredOnly = false
        downEvent = nil
    }

    // MARK: Drag out

    func startDrag(_ paths: [String], event: NSEvent) {
        let start = doc.convert(event.locationInWindow, from: nil)
        let items: [NSDraggingItem] = paths.prefix(64).enumerated().map { k, p in
            let item = NSDraggingItem(pasteboardWriter: URL(fileURLWithPath: p) as NSURL)
            var img: NSImage = NSWorkspace.shared.icon(forFile: p)
            if let t = tiles.values.first(where: { $0.thumbKey != nil && row($0.index).map(path) == p }),
               let key = t.thumbKey, let g = ThumbnailLoader.image(key) {
                img = NSImage(cgImage: g.cg, size: .zero)
            }
            let side: CGFloat = 64
            let a = img.size.width / max(img.size.height, 1)
            let size = a >= 1 ? NSSize(width: side, height: side / a) : NSSize(width: side * a, height: side)
            item.setDraggingFrame(NSRect(x: start.x - size.width / 2 + CGFloat(k * 4), y: start.y - size.height / 2 + CGFloat(k * 4),
                                         width: size.width, height: size.height), contents: img)
            return item
        }
        guard !items.isEmpty else { return }
        doc.beginDraggingSession(with: items, event: event, source: self)
    }

    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        context == .withinApplication ? [.copy, .move, .generic] : [.copy, .move, .link, .generic]
    }

    func draggingSession(_ session: NSDraggingSession, endedAt screenPoint: NSPoint, operation: NSDragOperation) {
        springTimer?.invalidate(); springIndex = -1
    }

    // MARK: Drop in

    private static func paths(_ info: NSDraggingInfo) -> [String] {
        (info.draggingPasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL])?
            .map(\.path) ?? []
    }

    /// The folder a drop on item `i` goes into: a folder, else this view's folder.
    private func dropDestination(_ i: Int) -> String? {
        guard let s = source else { return nil }
        if i >= 0, let r = row(i), r.kind == .folder { return path(r) }
        return s.dir
    }

    func validateDrop(_ info: NSDraggingInfo) -> NSDragOperation {
        // F-023: sidebar rows are references; dropping one here never moves or copies it.
        if SidebarViewController.isSidebarDrag(info) { highlightedDrop = nil; armSpring(-1); return [] }
        let paths = Self.paths(info)
        var i = index(at: doc.convert(info.draggingLocation, from: nil))
        if i >= 0, row(i)?.kind != .folder { i = -1 }
        guard let dest = dropDestination(i), let eng = model.engine as? ListEngineAPI else { return [] }
        let effect = eng.dropEffect(sources: paths, dest: dest, mods: Mods(NSEvent.modifierFlags))
        highlightedDrop = effect == .none ? nil : DropTarget(dest: dest, version: self.info.version, index: i)
        armSpring(effect == .none ? -1 : i)
        switch effect {
        case .copy: return .copy
        case .move: return .move
        case .none: return []
        }
    }

    func acceptDrop(_ info: NSDraggingInfo) -> Bool {
        springTimer?.invalidate(); springIndex = -1
        if SidebarViewController.isSidebarDrag(info) { return false }
        // The drop goes exactly where the highlight was (safety audit S8).
        let h = highlightedDrop
        highlightedDrop = nil
        let stillThere: (DropTarget) -> Bool = { h in
            h.index < 0 ? self.source?.dir == h.dest : self.row(h.index).map(self.path) == h.dest
        }
        guard let h, h.version == self.info.version || stillThere(h) else {
            MoveSafety.refused("The folder under the pointer changed, so nothing was dropped", model: model)
            return false
        }
        do {
            let out = try model.engine.dropFiles(sources: Self.paths(info), destDir: h.dest, mods: Mods(NSEvent.modifierFlags))
            model.handle(out, in: model.keyWindowID)
            return true
        } catch {
            MoveSafety.refused(FileTable.sentence((error as? CoreError)?.message ?? error.localizedDescription), model: model)
            return false
        }
    }

    func dragExited() {
        springTimer?.invalidate(); springTimer = nil; springIndex = -1
    }

    /// Spring-loaded folders: hovering a folder for 0.8 s opens it.
    private func armSpring(_ i: Int) {
        guard i != springIndex else { return }
        springIndex = i
        springTimer?.invalidate()
        guard i >= 0, let r = row(i) else { return }
        let p = path(r)
        springTimer = Timer.scheduledTimer(withTimeInterval: 0.8, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.springIndex = -1
                self.model.perform(Invocation(id: "nav.open", arg: .path(p), tab: self.ownerTab), via: .mouse)
            }
        }
    }

    // MARK: Context menu

    /// Finder's rule: the core picks the target, then builds the menu.
    func menuItems(index i: Int) -> [MenuItem]? {
        guard let owner = ownerTab else { return nil }
        let r: UInt32? = i >= 0 ? UInt32(i) : nil
        guard let target = try? model.engine.menuTarget(tab: owner, row: r) else { return nil }
        return try? model.engine.contextMenu(target, context: MenuContext(pasteboardHasFiles: Pasteboard.hasFiles()))
    }

    func contextMenu(_ e: NSEvent) -> NSMenu? {
        rename?.commitIfActive()
        guard let items = menuItems(index: index(at: point(e))) else { return nil }
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

    // MARK: Debug

    var debugInfo: [String: Any] {
        ["axis": axis == .vertical ? "grid" : "strip", "count": count, "columns": layout.columns,
         "image": layout.imageSize, "tiles": tiles.count, "pool": pool.count,
         "visible": "\(visibleIndices.lowerBound)..<\(visibleIndices.upperBound)",
         "thumbsShown": tiles.values.filter(\.isThumbnail).count, "made": thumbs.made,
         "cancelled": thumbs.cancelled, "pending": thumbs.pending]
    }
}

// MARK: Rename

extension TileGrid: RenameHost {
    var renameSource: RowSource? { source }
    var renameOwnerTab: TabID? { ownerTab }
    var renameContainer: NSView { doc }
    var renameFocusView: NSView { doc }
    func renameReveal(_ i: Int) { reveal(i) }
    func renameFieldFrame(_ i: Int) -> NSRect? {
        let f = frame(of: i)
        let h: CGFloat = 20
        let w = max(f.width - 4, 120)
        let label = tiles[i]?.labelRect.offsetBy(dx: f.minX, dy: f.minY) ?? f
        return NSRect(x: f.midX - w / 2, y: label.minY - 1, width: w, height: h)
    }
    func renameIsFolder(_ i: Int) -> Bool { row(i)?.kind == .folder }
}

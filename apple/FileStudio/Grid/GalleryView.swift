// GalleryView.swift — the gallery (M3): a big picture of the cursor item,
// a line of photo details under it, and a filmstrip (a horizontal TileGrid)
// below. Made for culling: ←/→ (the core's keys) step through the items;
// the big picture shows the filmstrip's thumbnail at once, QuickLook's quick
// picture next and the proper one last, and the core says which pictures to
// make ahead of the cursor (`galleryRequests`) so holding → through a folder
// of raws keeps up. Details come from the preview pane's summaries
// (fstudio-preview: EXIF for JPEG, HEIC, TIFF raws, CR3 and RAF), read off
// the main thread. iCloud placeholders are never read (F-016): they show
// their icon and say so.

import AppKit

/// The big picture: draws one image, fitted, and passes mouse input on.
@MainActor
final class GalleryPictureView: NSView {
    let imageLayer = CALayer()
    let cloudNote = NSTextField(labelWithString: "")
    var onMouseDown: ((NSEvent) -> Void)?
    var onMouseDragged: ((NSEvent) -> Void)?
    var menuProvider: ((NSEvent) -> NSMenu?)?
    weak var dropTarget: TileDocView?
    private(set) var aspect: CGFloat?

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.addSublayer(imageLayer)
        imageLayer.contentsGravity = .resizeAspect
        imageLayer.minificationFilter = .trilinear
        cloudNote.textColor = .secondaryLabelColor
        cloudNote.alignment = .center
        cloudNote.isHidden = true
        cloudNote.translatesAutoresizingMaskIntoConstraints = false
        addSubview(cloudNote)
        NSLayoutConstraint.activate([
            cloudNote.centerXAnchor.constraint(equalTo: centerXAnchor),
            cloudNote.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -16),
        ])
        registerForDraggedTypes([.fileURL])
    }
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }
    override var wantsUpdateLayer: Bool { true }
    override func updateLayer() { layer?.backgroundColor = NSColor.textBackgroundColor.cgColor }

    /// Shows a picture (or an icon, which is drawn smaller).
    func show(_ contents: Any?, aspect: CGFloat?, isIcon: Bool) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        imageLayer.contents = contents
        self.aspect = aspect
        iconMode = isIcon
        layoutPicture()
        CATransaction.commit()
    }

    private var iconMode = false

    override func layout() {
        super.layout()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        layoutPicture()
        CATransaction.commit()
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        imageLayer.contentsScale = window?.backingScaleFactor ?? 2
    }

    private func layoutPicture() {
        let b = bounds.insetBy(dx: 16, dy: 12)
        if iconMode {
            let s = min(min(b.width, b.height), 256)
            imageLayer.frame = NSRect(x: b.midX - s / 2, y: b.midY - s / 2, width: s, height: s)
        } else {
            imageLayer.frame = b
        }
    }

    override func mouseDown(with e: NSEvent) { onMouseDown?(e) }
    override func mouseDragged(with e: NSEvent) { onMouseDragged?(e) }
    override func menu(for event: NSEvent) -> NSMenu? { menuProvider?(event) }
    // Drops on the picture go into the folder, as on the filmstrip's empty space.
    override func draggingEntered(_ s: NSDraggingInfo) -> NSDragOperation { dropTarget?.grid?.validateDrop(s) ?? [] }
    override func draggingUpdated(_ s: NSDraggingInfo) -> NSDragOperation { dropTarget?.grid?.validateDrop(s) ?? [] }
    override func draggingExited(_ s: NSDraggingInfo?) { dropTarget?.grid?.dragExited() }
    override func performDragOperation(_ s: NSDraggingInfo) -> Bool { dropTarget?.grid?.acceptDrop(s) ?? false }
}

@MainActor
final class GalleryView: NSView {
    private unowned let model: AppModel
    let strip: TileGrid
    let picture = GalleryPictureView()
    private let details = NSTextField(labelWithString: "")
    private let name = NSTextField(labelWithString: "")
    private let marks = MarksStrip()
    private var stripHeight: NSLayoutConstraint!
    private var observer: ObserverToken?
    private let bigLoader = ThumbnailLoader(maxInflight: 2)
    private var tab: TabModel?
    private var shownIndex: Int?
    private var shownKey: String?
    private var lastCursor: Int?
    private var forward = true
    private var downEvent: NSEvent?
    /// When the shown item was asked for, and how long its proper picture took (debug log).
    private var shownAt: CFTimeInterval?
    private var latencies: [Double] = []
    // Details.
    private lazy var previewer: PreviewerAPI = model.engine.makePreviewer()
    private var detailTask: Task<Void, Never>?
    private var detailCancel: PreviewCancel?

    init(model: AppModel) {
        self.model = model
        strip = TileGrid(model: model, axis: .horizontal)
        super.init(frame: .zero)
        wantsLayer = true
        strip.rename = RenameController(model: model, host: strip)
        strip.onCursorChange = { [weak self] in self?.follow() }
        name.font = .systemFont(ofSize: 13, weight: .semibold)
        name.lineBreakMode = .byTruncatingMiddle
        name.alignment = .center
        details.font = .systemFont(ofSize: 11)
        details.textColor = .secondaryLabelColor
        details.lineBreakMode = .byTruncatingTail
        details.alignment = .center
        let sep = NSBox()
        sep.boxType = .separator
        for v in [picture, name, details, marks, sep, strip.view] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            addSubview(v)
        }
        picture.dropTarget = strip.doc
        picture.onMouseDown = { [weak self] e in self?.pictureMouseDown(e) }
        picture.onMouseDragged = { [weak self] e in self?.pictureMouseDragged(e) }
        picture.menuProvider = { [weak self] _ in self?.pictureMenu() }
        stripHeight = strip.view.heightAnchor.constraint(equalToConstant: 100)
        NSLayoutConstraint.activate([
            picture.topAnchor.constraint(equalTo: topAnchor),
            picture.leadingAnchor.constraint(equalTo: leadingAnchor),
            picture.trailingAnchor.constraint(equalTo: trailingAnchor),
            name.topAnchor.constraint(equalTo: picture.bottomAnchor, constant: 2),
            name.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 16),
            name.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -16),
            marks.topAnchor.constraint(equalTo: name.bottomAnchor, constant: 2),
            marks.centerXAnchor.constraint(equalTo: centerXAnchor),
            details.topAnchor.constraint(equalTo: marks.bottomAnchor, constant: 2),
            details.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 16),
            details.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -16),
            sep.topAnchor.constraint(equalTo: details.bottomAnchor, constant: 8),
            sep.leadingAnchor.constraint(equalTo: leadingAnchor),
            sep.trailingAnchor.constraint(equalTo: trailingAnchor),
            strip.view.topAnchor.constraint(equalTo: sep.bottomAnchor),
            strip.view.leadingAnchor.constraint(equalTo: leadingAnchor),
            strip.view.trailingAnchor.constraint(equalTo: trailingAnchor),
            strip.view.bottomAnchor.constraint(equalTo: bottomAnchor),
            stripHeight,
        ])
        bigLoader.onImage = { [weak self] key, img, final in
            guard let self, key == self.shownKey else { return }
            if final, let t0 = self.shownAt { self.latencies.append(CACurrentMediaTime() - t0); self.shownAt = nil }
            self.picture.show(img.cg, aspect: CGFloat(img.cg.width) / CGFloat(max(img.cg.height, 1)), isIcon: false)
        }
        observer = model.observe { [weak self] e in
            guard let self else { return }
            switch e {
            case .settingsChanged: self.applySettings(); self.follow(force: true)
            case .listingChanged(let id, _, _) where id == self.tab?.id: self.follow(force: true)
            default: break
            }
        }
        applySettings()
    }
    required init?(coder: NSCoder) { fatalError() }

    override var wantsUpdateLayer: Bool { true }
    override func updateLayer() { layer?.backgroundColor = NSColor.textBackgroundColor.cgColor }

    /// The view that takes keyboard focus (the filmstrip).
    var focusView: NSView { strip.doc }

    func show(_ t: TabModel) {
        if tab?.id != t.id { lastCursor = nil }
        tab = t
        strip.show(.tab(t.api), owner: t.id)
        follow(force: true)
    }

    func detach() {
        strip.detach()
        bigLoader.cancelAll()
        detailTask?.cancel()
        detailCancel?.cancel()
        tab = nil
    }

    private func applySettings() {
        let s = CGFloat(GridSettings.stripSize(model))
        strip.stripSize = s
        stripHeight.constant = s + 12 + 16 + 12
        details.isHidden = !GridSettings.galleryInfo(model)
        strip.relayout()
        strip.refreshTiles(all: true)
    }

    override func layout() {
        super.layout()
        // A new size may need a bigger picture.
        if let i = shownIndex, bigPixels != lastPixels { lastPixels = bigPixels; showBig(i, force: true) }
    }

    private var lastPixels = 0
    private var bigPixels: Int {
        let side = max(picture.bounds.width, picture.bounds.height, 200)
        return (model.engine as? GridEngineAPI)?.galleryPixels(points: Double(side),
                                                              scale: Double(window?.backingScaleFactor ?? 2)) ?? 2048
    }

    // MARK: Following the cursor

    /// Shows the cursor item (called on every selection change).
    private func follow(force: Bool = false) {
        guard let t = tab else { return }
        let c = t.api.selection().cursor.map(Int.init) ?? (strip.row(0) != nil ? 0 : nil)
        if let c, let l = lastCursor, c != l { forward = c > l }
        lastCursor = c
        guard let c else {
            shownIndex = nil; shownKey = nil
            picture.show(nil, aspect: nil, isIcon: true)
            name.stringValue = ""; details.stringValue = ""; marks.marks = nil
            return
        }
        showBig(c, force: force)
    }

    private func showBig(_ i: Int, force: Bool) {
        guard let r = strip.row(i) else { return }
        let path = strip.path(r)
        let px = bigPixels
        lastPixels = px
        let file = r.kind != .folder && r.kind != .package && !r.isDataless
        let key = file ? ThumbRequest.key(path: path, modifiedNs: r.modifiedNs, size: r.size, pixels: px) : nil
        if !force, key == shownKey, shownIndex == i { return }
        shownIndex = i
        shownKey = key
        shownAt = key.flatMap { ThumbnailLoader.image($0) } == nil ? CACurrentMediaTime() : nil
        if shownAt == nil, key != nil { latencies.append(0) }
        name.stringValue = r.shownName
        marks.marks = GridMarks.of(r)
        picture.cloudNote.isHidden = !r.isDataless
        picture.cloudNote.stringValue = r.isDataless ? "In iCloud: not downloaded, so there’s no preview." : ""
        if let key {
            if let g = ThumbnailLoader.image(key) ?? ThumbnailLoader.quickImage(key) ?? smallThumb(r, path) {
                picture.show(g.cg, aspect: CGFloat(g.cg.width) / CGFloat(max(g.cg.height, 1)), isIcon: false)
            } else {
                picture.show(IconCache.shared.icon(key: r.iconKey), aspect: nil, isIcon: true)
            }
        } else {
            picture.show(IconCache.shared.icon(key: r.iconKey, path: r.kind == .package ? path : nil), aspect: nil,
                         isIcon: true)
        }
        requestBig(px)
        loadDetails(path: path, row: r)
    }

    /// The filmstrip's thumbnail, if it has one: shown at once while the big one is made.
    private func smallThumb(_ r: Row, _ path: String) -> GridImage? {
        guard let g = model.engine as? GridEngineAPI else { return nil }
        let scale = Double(window?.backingScaleFactor ?? 2)
        for points in [Double(strip.stripSize), 128, 256] {
            let px = g.thumbPixels(points: points, scale: scale)
            if let img = ThumbnailLoader.image(ThumbRequest.key(path: path, modifiedNs: r.modifiedNs, size: r.size, pixels: px)) {
                return img
            }
        }
        return nil
    }

    /// The core's order: the cursor, two ahead, one behind.
    private func requestBig(_ px: Int) {
        guard let t = tab, let g = model.engine as? GridEngineAPI else { return }
        let reqs: [ThumbRequest] = g.galleryRequests(tab: t.id, forward: forward).compactMap { i in
            guard let r = strip.row(i) else { return nil }
            let p = strip.path(r)
            return ThumbRequest(key: ThumbRequest.key(path: p, modifiedNs: r.modifiedNs, size: r.size, pixels: px),
                                path: p, pixels: px, quickFirst: true)
        }
        bigLoader.want(reqs)
    }

    /// Photo details from the core's summary (EXIF), off the main thread,
    /// after a short pause so holding → doesn't queue disk reads.
    private func loadDetails(path: String, row r: Row) {
        detailTask?.cancel()
        detailCancel?.cancel()
        let fallback = [r.kindText, r.kind == .folder ? nil : r.sizeText, r.modifiedText].compactMap { $0 }
            .filter { !$0.isEmpty }.joined(separator: "  ·  ")
        details.stringValue = fallback
        guard GridSettings.galleryInfo(model), !r.isDataless, r.kind != .folder else { return }
        if let c = previewer.cached(path) { details.stringValue = Self.describe(c, fallback: fallback); return }
        let token = PreviewCancel()
        detailCancel = token
        let previewer = self.previewer
        detailTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(80))
            if Task.isCancelled { return }
            let info: PreviewInfo? = await withTaskCancellationHandler {
                await Task.detached(priority: .userInitiated) {
                    guard var i = previewer.summarize(path, cancel: token) else { return nil }
                    ImageSize.fill(&i)
                    return i
                }.value
            } onCancel: { token.cancel() }
            guard !Task.isCancelled, let self, let info, self.shownIndex.flatMap({ self.strip.row($0) })
                .map({ self.strip.path($0) }) == path else { return }
            self.details.stringValue = Self.describe(info, fallback: fallback)
        }
    }

    /// "6000 × 4000  ·  ILCE-7M4  ·  FE 24-70mm F2.8 GM II  ·  35 mm  ·  ƒ/2.8  ·  1/250 s  ·  ISO 400"
    static func describe(_ i: PreviewInfo, fallback: String) -> String {
        guard let p = i.photo else {
            if let v = i.video {
                return [v.resolution, v.duration, v.codec, v.frameRate].compactMap { $0 }.joined(separator: "  ·  ")
            }
            return fallback
        }
        let parts = [p.dimensions, p.camera, p.lens, p.focalLength, p.aperture, p.shutter, p.iso,
                     p.exposureComp, p.dateTaken]
        let s = parts.compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: "  ·  ")
        return s.isEmpty ? fallback : s
    }

    // MARK: Mouse on the big picture

    private func pictureMouseDown(_ e: NSEvent) {
        window?.makeFirstResponder(strip.doc)
        downEvent = e
        if e.clickCount == 2, shownIndex != nil {
            model.perform(Invocation(id: "nav.open", tab: tab?.id), via: .mouse)
        }
    }

    private func pictureMouseDragged(_ e: NSEvent) {
        guard let down = downEvent, let t = tab else { return }
        let d = hypot(e.locationInWindow.x - down.locationInWindow.x, e.locationInWindow.y - down.locationInWindow.y)
        guard d > 4 else { return }
        downEvent = nil
        let paths = t.api.selectedPaths()
        guard !paths.isEmpty else { return }
        strip.startDrag(paths, event: down)
    }

    private func pictureMenu() -> NSMenu? {
        guard let i = shownIndex, let items = strip.menuItems(index: i) else { return nil }
        return ContextMenuBuilder.menu(items, model: model)
    }

    // MARK: Debug

    /// "n=12 ready 7 p50 0 ms max 180 ms": how soon the proper big picture was there.
    static func latencyText(_ l: [Double]) -> String {
        let s = l.sorted()
        guard !s.isEmpty else { return "none" }
        let ready = s.filter { $0 == 0 }.count
        return "n=\(s.count) cached=\(ready) p50 \(Int(s[s.count / 2] * 1000)) ms max \(Int(s.last! * 1000)) ms"
    }

    var debugInfo: [String: Any] {
        ["shown": shownIndex ?? -1, "name": name.stringValue, "details": details.stringValue,
         "big": shownKey.map { ThumbnailLoader.image($0) != nil } ?? false, "bigMade": bigLoader.made, "bigImageIO": bigLoader.fromImageIO,
         "bigMs": Self.latencyText(latencies),
         "strip": strip.debugInfo]
    }
}

/// The culling marks under the gallery's name: stars, a label dot, pick/reject.
@MainActor
final class MarksStrip: NSView {
    var marks: GridMarks? {
        didSet {
            guard marks != oldValue else { return }
            invalidateIntrinsicContentSize()
            needsDisplay = true
        }
    }
    override var intrinsicContentSize: NSSize { NSSize(width: 160, height: marks == nil ? 0 : 16) }
    override var isFlipped: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        guard let m = marks else { return }
        var x: CGFloat = 0
        var parts: [(String, NSColor)] = []
        if m.reject { parts.append(("Rejected", .systemRed)) }
        if m.pick { parts.append(("Pick", .systemGreen)) }
        let stars = String(repeating: "★", count: m.rating) + String(repeating: "☆", count: 5 - min(m.rating, 5))
        parts.append((stars, .systemYellow))
        let font = NSFont.systemFont(ofSize: 11, weight: .semibold)
        var total: CGFloat = 0
        let sizes = parts.map { ($0.0 as NSString).size(withAttributes: [.font: font]).width }
        total = sizes.reduce(0, +) + CGFloat(parts.count - 1) * 10 + (m.label != nil ? 16 : 0)
        x = (bounds.width - total) / 2
        if let l = m.label {
            GridStyle.labelColor(l).setFill()
            NSBezierPath(ovalIn: NSRect(x: x, y: 3, width: 10, height: 10)).fill()
            x += 16
        }
        for (k, p) in parts.enumerated() {
            (p.0 as NSString).draw(at: NSPoint(x: x, y: 1), withAttributes: [.font: font, .foregroundColor: p.1])
            x += sizes[k] + 10
        }
    }
}

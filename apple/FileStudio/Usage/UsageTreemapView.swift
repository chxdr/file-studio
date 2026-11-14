// UsageTreemapView.swift — the squarified treemap, drawn with Core Graphics.
//
// The core lays the tiles out (`UsageViewAPI.layout`, cached there) and says
// which tile is under the mouse (`hit`). This view only paints: every tile is
// filled into one offscreen bitmap at the screen's scale, redrawn when the
// layout, the appearance or the backing scale changes, and blitted on each
// `draw`. Hover and selection outlines are drawn on top, so moving the mouse
// never repaints 60,000 rectangles. No SwiftUI view per rectangle.

import AppKit

/// Colours for treemap slots: 12 type colours, then "other", small folders,
/// folder frames and iCloud placeholders. Muted, in the spirit of ForkLift.
enum UsagePalette {
    static let typeColors: [NSColor] = [
        .systemBlue, .systemOrange, .systemGreen, .systemPink, .systemPurple, .systemTeal,
        .systemYellow, .systemRed, .systemIndigo, .systemMint, .systemBrown, .systemCyan,
    ]

    /// The fill for a slot, resolved for the current appearance.
    static func color(_ slot: UInt8, dark: Bool) -> NSColor {
        switch slot {
        case 1...12:
            let base = typeColors[Int(slot) - 1].usingColorSpace(.deviceRGB) ?? .systemBlue
            // Softer than the raw system colours: blend toward the background.
            return base.blended(withFraction: dark ? 0.28 : 0.18, of: dark ? .black : .white) ?? base
        case 250: return NSColor.systemCyan.withAlphaComponent(dark ? 0.35 : 0.30)
        case 251: return dark ? NSColor(white: 0.30, alpha: 1) : NSColor(white: 0.78, alpha: 1)
        case 255: return dark ? NSColor(white: 0.12, alpha: 1) : NSColor(white: 0.62, alpha: 1)
        default: return dark ? NSColor(white: 0.42, alpha: 1) : NSColor(white: 0.86, alpha: 1)
        }
    }

    /// A small swatch image for menus and tables.
    static func swatch(_ slot: UInt8, size: CGFloat = 10) -> NSImage {
        NSImage(size: NSSize(width: size, height: size), flipped: false) { r in
            let dark = NSApp.effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            color(slot, dark: dark).setFill()
            NSBezierPath(roundedRect: r.insetBy(dx: 0.5, dy: 0.5), xRadius: 2.5, yRadius: 2.5).fill()
            NSColor.black.withAlphaComponent(0.18).setStroke()
            NSBezierPath(roundedRect: r.insetBy(dx: 0.5, dy: 0.5), xRadius: 2.5, yRadius: 2.5).stroke()
            return true
        }
    }
}

@MainActor
final class UsageTreemapView: NSView {
    var source: UsageViewAPI?
    /// The selected node (drawn outlined).
    var selectedID: UInt32? { didSet { if oldValue != selectedID { needsDisplay = true } } }
    var onSelect: ((UInt32) -> Void)?
    var onZoom: ((UInt32) -> Void)?
    var onHover: ((UInt32?) -> Void)?
    var onMenu: ((UInt32, NSEvent) -> NSMenu?)?
    var onKey: ((NSEvent) -> Bool)?

    private var tiles: [UsageTile] = []
    private var rects: [UInt32: CGRect] = [:]
    private var image: CGImage?
    private var imageSize: CGSize = .zero
    private var hoverID: UInt32?
    private var tracking: NSTrackingArea?
    /// Set while the user drags the window edge: the cached image stretches,
    /// and the layout is asked for once the resize ends.
    private var resizing = false

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }
    override var isOpaque: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        setAccessibilityRole(.image)
        setAccessibilityLabel("Treemap")
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    /// Asks the core for tiles again (zoom, mode or tree changed).
    func reload() {
        tiles = []
        image = nil
        needsDisplay = true
    }

    /// The rectangle a node occupies, if drawn.
    func rect(of id: UInt32) -> CGRect? { rects[id] }

    /// How many tiles the last layout had (debug).
    var tileCount: Int { tiles.count }

    private var isDark: Bool { effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        image = nil
        needsDisplay = true
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        image = nil
        needsDisplay = true
    }

    override func viewWillStartLiveResize() {
        super.viewWillStartLiveResize()
        resizing = true
    }

    override func viewDidEndLiveResize() {
        super.viewDidEndLiveResize()
        resizing = false
        reload()
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        if !resizing { reload() }
    }

    private func ensureLayout() {
        guard let source, bounds.width >= 1, bounds.height >= 1 else { return }
        if tiles.isEmpty || imageSize != bounds.size {
            tiles = source.layout(width: Double(bounds.width), height: Double(bounds.height))
            rects = Dictionary(tiles.map { ($0.id, $0.rect) }, uniquingKeysWith: { a, _ in a })
            image = nil
        }
        if image == nil { image = render() }
    }

    /// Paints every tile into a bitmap at the backing scale.
    private func render() -> CGImage? {
        let scale = window?.backingScaleFactor ?? 2
        let w = Int(bounds.width * scale), h = Int(bounds.height * scale)
        guard w > 0, h > 0,
              let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0,
                                  space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue) else { return nil }
        // Top-left origin in points, like the core's layout.
        ctx.translateBy(x: 0, y: CGFloat(h))
        ctx.scaleBy(x: scale, y: -scale)
        let dark = isDark
        var fills: [UInt8: CGColor] = [:]
        func fill(_ s: UInt8) -> CGColor {
            if let c = fills[s] { return c }
            let c = UsagePalette.color(s, dark: dark).cgColor
            fills[s] = c
            return c
        }
        let light = CGColor(gray: 1, alpha: dark ? 0.10 : 0.35)
        let shade = CGColor(gray: 0, alpha: dark ? 0.35 : 0.16)
        ctx.setFillColor(fill(255))
        ctx.fill(CGRect(origin: .zero, size: bounds.size))
        for t in tiles {
            let r = t.rect
            if !t.leaf {
                ctx.setFillColor(fill(255))
                ctx.fill(r)
                continue
            }
            // A hairline gap between neighbours, then a soft bevel (light top
            // and left, shade bottom and right) so tiles read as cushions.
            let inner = r.width > 2 && r.height > 2 ? r.insetBy(dx: 0.25, dy: 0.25) : r
            ctx.setFillColor(fill(t.color))
            ctx.fill(inner)
            if inner.width >= 4, inner.height >= 4 {
                ctx.setFillColor(light)
                ctx.fill(CGRect(x: inner.minX, y: inner.minY, width: inner.width, height: 1))
                ctx.fill(CGRect(x: inner.minX, y: inner.minY, width: 1, height: inner.height))
                ctx.setFillColor(shade)
                ctx.fill(CGRect(x: inner.minX, y: inner.maxY - 1, width: inner.width, height: 1))
                ctx.fill(CGRect(x: inner.maxX - 1, y: inner.minY, width: 1, height: inner.height))
            }
        }
        imageSize = bounds.size
        return ctx.makeImage()
    }

    override func draw(_ dirty: NSRect) {
        ensureLayout()
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        if let image {
            ctx.saveGState()
            // The bitmap's rows run top-down; flip it into this flipped view.
            ctx.translateBy(x: 0, y: bounds.height)
            ctx.scaleBy(x: 1, y: -1)
            ctx.interpolationQuality = resizing ? .low : .none
            ctx.draw(image, in: CGRect(origin: .zero, size: bounds.size))
            ctx.restoreGState()
        } else {
            NSColor.controlBackgroundColor.setFill()
            bounds.fill()
        }
        if let s = selectedID, let r = rects[s] {
            let p = NSBezierPath(rect: r.insetBy(dx: 1, dy: 1))
            p.lineWidth = 2
            NSColor.controlAccentColor.setStroke()
            p.stroke()
            let q = NSBezierPath(rect: r.insetBy(dx: 2.5, dy: 2.5))
            q.lineWidth = 1
            NSColor.white.withAlphaComponent(0.8).setStroke()
            q.stroke()
        }
        if let h = hoverID, h != selectedID, let r = rects[h] {
            let p = NSBezierPath(rect: r.insetBy(dx: 0.5, dy: 0.5))
            p.lineWidth = 1
            NSColor.labelColor.withAlphaComponent(0.7).setStroke()
            p.stroke()
        }
    }

    // MARK: Mouse

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let t = NSTrackingArea(rect: .zero, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow,
                                                      .inVisibleRect], owner: self)
        addTrackingArea(t)
        tracking = t
    }

    private func id(at e: NSEvent) -> UInt32? {
        let p = convert(e.locationInWindow, from: nil)
        return source?.hit(x: Double(p.x), y: Double(p.y))
    }

    override func mouseMoved(with e: NSEvent) {
        let h = id(at: e)
        guard h != hoverID else { return }
        hoverID = h
        needsDisplay = true
        onHover?(h)
    }

    override func mouseExited(with e: NSEvent) {
        hoverID = nil
        needsDisplay = true
        onHover?(nil)
    }

    override func mouseDown(with e: NSEvent) {
        window?.makeFirstResponder(self)
        guard let h = id(at: e) else { return }
        if e.clickCount >= 2 { onZoom?(h) } else { onSelect?(h) }
    }

    override func menu(for e: NSEvent) -> NSMenu? {
        guard let h = id(at: e) else { return nil }
        onSelect?(h)
        return onMenu?(h, e)
    }

    override func keyDown(with e: NSEvent) {
        if onKey?(e) != true { super.keyDown(with: e) }
    }

    // MARK: Focus ring

    override var focusRingMaskBounds: NSRect { bounds }
    override func drawFocusRingMask() { bounds.fill() }
}

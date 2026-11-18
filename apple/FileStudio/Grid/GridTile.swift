// GridTile.swift — one drawn tile of the icon view or the gallery's
// filmstrip: the picture (a QuickLook thumbnail, or the icon until one
// arrives), the name under it, the selection and Import Studio's culling
// marks. Layers only (no draw(_:), no subviews, no Auto Layout), so
// reusing a tile while scrolling is a few property sets.

import AppKit

/// Shared fonts and attributes.
@MainActor
enum GridStyle {
    static let nameFont = NSFont.systemFont(ofSize: 12)
    static let para: NSParagraphStyle = {
        let p = NSMutableParagraphStyle()
        p.alignment = .center
        p.lineBreakMode = .byTruncatingMiddle
        return p
    }()
    /// Import Studio's label colours (1 red … 5 purple).
    nonisolated static func labelColor(_ l: Int) -> NSColor {
        switch l {
        case 1: .systemRed
        case 2: .systemYellow
        case 3: .systemGreen
        case 4: .systemBlue
        default: .systemPurple
        }
    }
}

/// Stars, a label dot and pick/reject, drawn over the picture's corners.
final class MarksLayer: CALayer {
    var marks: GridMarks? { didSet { if marks != oldValue { setNeedsDisplay() } } }

    override init() {
        super.init()
        needsDisplayOnBoundsChange = true
    }
    override init(layer: Any) { super.init(layer: layer) }
    required init?(coder: NSCoder) { fatalError() }

    override func draw(in ctx: CGContext) {
        guard let m = marks else { return }
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx, flipped: true)
        defer { NSGraphicsContext.restoreGraphicsState() }
        let b = bounds
        // Stars: a dark pill along the bottom edge.
        if m.rating > 0 {
            let stars = String(repeating: "★", count: min(m.rating, 5))
            let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 9, weight: .bold),
                                                        .foregroundColor: NSColor.systemYellow]
            let size = (stars as NSString).size(withAttributes: attrs)
            let pill = NSRect(x: b.midX - size.width / 2 - 4, y: b.maxY - size.height - 5, width: size.width + 8,
                              height: size.height + 2)
            NSColor.black.withAlphaComponent(0.55).setFill()
            NSBezierPath(roundedRect: pill, xRadius: pill.height / 2, yRadius: pill.height / 2).fill()
            (stars as NSString).draw(at: NSPoint(x: pill.minX + 4, y: pill.minY + 1), withAttributes: attrs)
        }
        // Colour label: a dot in the top-right corner.
        if let l = m.label {
            let d: CGFloat = 10
            let r = NSRect(x: b.maxX - d - 4, y: 4, width: d, height: d)
            GridStyle.labelColor(l).setFill()
            NSBezierPath(ovalIn: r).fill()
            NSColor.white.withAlphaComponent(0.9).setStroke()
            let p = NSBezierPath(ovalIn: r.insetBy(dx: 0.5, dy: 0.5))
            p.lineWidth = 1
            p.stroke()
        }
        // Reject: a red ✕ badge top-left (the picture is also dimmed). Pick: a green flag.
        if m.reject || m.pick {
            let r = NSRect(x: 4, y: 4, width: 14, height: 14)
            (m.reject ? NSColor.systemRed : NSColor.systemGreen).setFill()
            NSBezierPath(roundedRect: r, xRadius: 3, yRadius: 3).fill()
            let name = m.reject ? "xmark" : "flag.fill"
            if let img = NSImage(systemSymbolName: name, accessibilityDescription: nil)?
                .withSymbolConfiguration(.init(pointSize: 8, weight: .heavy)) {
                let tinted = NSImage(size: img.size, flipped: false) { rect in
                    img.draw(in: rect)
                    NSColor.white.set()
                    rect.fill(using: .sourceAtop)
                    return true
                }
                tinted.draw(in: NSRect(x: r.midX - img.size.width / 2, y: r.midY - img.size.height / 2,
                                       width: img.size.width, height: img.size.height))
            }
        }
    }
}

/// One tile. `strip` tiles (the filmstrip) have no name and show the
/// selection as a ring.
@MainActor
final class GridTile: NSView {
    let strip: Bool
    /// The item index shown, and the thumbnail key it wants.
    var index = -1
    var thumbKey: String?
    private let selBg = CALayer()
    private let imageLayer = CALayer()
    private let labelBg = CALayer()
    private let label = CATextLayer()
    private let marksLayer = MarksLayer()
    private let cloud = CALayer()
    private var imageSize: CGFloat = 64
    private var labelHeight: CGFloat = 32
    private var pictureAspect: CGFloat?
    private var name = ""
    private var selected = false
    private var active = false
    private var dimmed = false
    private var marks: GridMarks?
    private(set) var isThumbnail = false
    /// For VoiceOver.
    var summary = ""

    init(strip: Bool) {
        self.strip = strip
        super.init(frame: .zero)
        wantsLayer = true
        layerContentsRedrawPolicy = .never
        for l in [selBg, imageLayer, labelBg, label, marksLayer, cloud] as [CALayer] { layer?.addSublayer(l) }
        selBg.cornerRadius = strip ? 5 : 6
        imageLayer.contentsGravity = .resizeAspect
        imageLayer.minificationFilter = .trilinear
        labelBg.cornerRadius = 4
        label.isWrapped = true
        label.truncationMode = .middle
        label.alignmentMode = .center
        label.font = GridStyle.nameFont
        label.fontSize = GridStyle.nameFont.pointSize
        cloud.contents = CellStyle.cloud
        cloud.contentsGravity = .resizeAspect
        label.isHidden = strip
        labelBg.isHidden = strip
    }
    required init?(coder: NSCoder) { fatalError() }

    override nonisolated var isFlipped: Bool { true }
    override var wantsUpdateLayer: Bool { true }
    // Mouse input goes to the grid (TileGrid handles clicks and drags).
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    /// The square the picture is fitted into, in tile coordinates.
    var imageSquare: NSRect {
        let s = imageSize
        return NSRect(x: (bounds.width - s) / 2, y: strip ? (bounds.height - s) / 2 : 6, width: s, height: s)
    }

    /// Where the picture is drawn (fitted to its aspect), in tile coordinates.
    var pictureRect: NSRect {
        let sq = imageSquare
        guard let a = pictureAspect, a > 0 else { return sq }
        let w = a >= 1 ? sq.width : sq.width * a
        let h = a >= 1 ? sq.height / a : sq.height
        // Pictures sit on the square's bottom edge (as in Finder), centred across.
        let y = strip ? sq.midY - h / 2 : sq.maxY - h
        return NSRect(x: sq.midX - w / 2, y: y, width: w, height: h).integral
    }

    /// The name's rectangle (for the rename field), in tile coordinates.
    var labelRect: NSRect {
        NSRect(x: 4, y: imageSquare.maxY + 6, width: bounds.width - 8, height: labelHeight)
    }

    /// Sets what the tile shows.
    func configure(row r: Row, image: CGImage?, icon: NSImage, isThumbnail: Bool, imageSize: CGFloat,
                   labelHeight: CGFloat, active: Bool, marks: GridMarks?) {
        self.imageSize = imageSize
        self.labelHeight = labelHeight
        name = r.shownName
        selected = r.isSelected
        self.active = active
        dimmed = r.isHidden || r.isCutPending
        self.marks = marks
        self.isThumbnail = isThumbnail && image != nil
        if let image {
            imageLayer.contents = image
            pictureAspect = CGFloat(image.width) / CGFloat(max(image.height, 1))
        } else {
            imageLayer.contents = icon
            pictureAspect = nil
        }
        cloud.isHidden = !r.isDataless
        summary = [r.shownName, r.isDataless ? "in iCloud" : nil, r.marksText.isEmpty ? nil : r.marksText, r.kindText,
                   r.isSelected ? "selected" : nil]
            .compactMap { $0 }.joined(separator: ", ")
        layoutLayers()
    }

    /// A thumbnail arrived for this tile.
    func setImage(_ image: CGImage) {
        imageLayer.contents = image
        pictureAspect = CGFloat(image.width) / CGFloat(max(image.height, 1))
        isThumbnail = true
        layoutLayers()
    }

    /// The pane became active or inactive (the selection colour changes).
    func setActive(_ a: Bool) {
        guard a != active else { return }
        active = a
        layoutLayers()
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        layoutLayers()
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        let s = window?.backingScaleFactor ?? 2
        for l in [imageLayer, label, marksLayer, cloud] as [CALayer] { l.contentsScale = s }
        marksLayer.setNeedsDisplay()
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        layoutLayers()
    }

    private func layoutLayers() {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        defer { CATransaction.commit() }
        let pic = pictureRect
        imageLayer.frame = pic
        imageLayer.opacity = dimmed || marks?.reject == true ? 0.4 : 1
        // A thin edge around photos, so white pictures don't melt into the background.
        imageLayer.borderWidth = isThumbnail ? 0.5 : 0
        var accent = NSColor.selectedContentBackgroundColor
        var unemph = NSColor.unemphasizedSelectedContentBackgroundColor
        var edge = NSColor.separatorColor
        var text = NSColor.labelColor
        effectiveAppearance.performAsCurrentDrawingAppearance {
            accent = NSColor.selectedContentBackgroundColor.usingColorSpace(.deviceRGB) ?? accent
            unemph = NSColor.unemphasizedSelectedContentBackgroundColor.usingColorSpace(.deviceRGB) ?? unemph
            edge = NSColor.separatorColor.usingColorSpace(.deviceRGB) ?? edge
            text = NSColor.labelColor.usingColorSpace(.deviceRGB) ?? text
        }
        imageLayer.borderColor = edge.cgColor
        if strip {
            selBg.frame = pic.insetBy(dx: -3, dy: -3)
            selBg.backgroundColor = nil
            selBg.borderWidth = selected ? 3 : 0
            selBg.borderColor = (active ? accent : unemph).cgColor
        } else {
            selBg.frame = pic.insetBy(dx: -4, dy: -4)
            selBg.borderWidth = 0
            selBg.backgroundColor = selected ? unemph.cgColor : nil
            let lr = labelRect
            let onAccent = selected && active
            let str = NSAttributedString(string: name, attributes: [
                .font: GridStyle.nameFont, .paragraphStyle: GridStyle.para,
                .foregroundColor: (onAccent ? NSColor.white : text).withAlphaComponent(dimmed ? 0.5 : 1),
            ])
            label.string = str
            // The pill hugs the text (one or two lines).
            let fit = str.boundingRect(with: NSSize(width: lr.width - 6, height: lr.height),
                                       options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
            let h = min(ceil(fit.height), lr.height)
            let w = min(ceil(fit.width) + 8, lr.width)
            label.frame = NSRect(x: lr.minX, y: lr.minY + 1, width: lr.width, height: h + 1)
            labelBg.frame = NSRect(x: lr.midX - w / 2, y: lr.minY, width: w, height: h + 2)
            labelBg.backgroundColor = selected ? (active ? accent : unemph).cgColor : nil
        }
        marksLayer.frame = pic
        marksLayer.marks = marks
        marksLayer.isHidden = marks == nil
        let c: CGFloat = 14
        cloud.frame = NSRect(x: pic.maxX - c - 2, y: pic.maxY - c - 2, width: c, height: c)
    }

    // MARK: Accessibility

    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .cell }
    override func accessibilityLabel() -> String? { summary }
    override func isAccessibilitySelected() -> Bool { selected }
}

// FileCells.swift — the drawn parts of a file table: one row view per row
// (the selection background) and one cell per column. Cells draw their text
// and icon directly (no NSTextField, no Auto Layout), so configuring a
// reused cell is a few property sets and one `setNeedsDisplay` (F-019:
// 16 pt icons, 13 pt names, 12 pt monospaced-digit secondary columns).

import AppKit

/// The column a cell shows (list view columns, or the column view's single column).
enum CellKind: Equatable {
    case name
    /// Size, dates, kind, extension. Right-aligned for sizes.
    case text(rightAligned: Bool)
    /// The column view's row: icon, name, a chevron for folders.
    case miller
}

/// Fonts and attributes, made once.
@MainActor
enum CellStyle {
    static let nameFont = NSFont.systemFont(ofSize: 13)
    static let secondaryFont = NSFont.monospacedDigitSystemFont(ofSize: 12, weight: .regular)
    static let middle: NSParagraphStyle = {
        let p = NSMutableParagraphStyle(); p.lineBreakMode = .byTruncatingMiddle; return p
    }()
    static let tail: NSParagraphStyle = {
        let p = NSMutableParagraphStyle(); p.lineBreakMode = .byTruncatingTail; return p
    }()
    static let right: NSParagraphStyle = {
        let p = NSMutableParagraphStyle(); p.lineBreakMode = .byTruncatingHead; p.alignment = .right; return p
    }()
    static let cloud: NSImage? = NSImage(systemSymbolName: "icloud", accessibilityDescription: "In iCloud")?
        .withSymbolConfiguration(.init(pointSize: 11, weight: .regular))
    static let chevron: NSImage? = NSImage(systemSymbolName: "chevron.right", accessibilityDescription: nil)?
        .withSymbolConfiguration(.init(pointSize: 10, weight: .semibold))
    static let aliasArrow: NSImage? = NSImage(systemSymbolName: "arrow.up.right", accessibilityDescription: "Symbolic link")?
        .withSymbolConfiguration(.init(pointSize: 6.5, weight: .black))

    /// Main-thread time spent configuring cells (for `bench:scroll`).
    static var configureTime: Duration = .zero
    static var configureCalls = 0
    /// Main-thread time spent drawing cells (for `bench:scroll`).
    static var drawTime: Duration = .zero
    static var drawCalls = 0
}

/// One cell. Drawn, not composed of subviews.
@MainActor
final class FileCellView: NSView {
    var kind: CellKind = .name { didSet { if kind != oldValue { needsDisplay = true } } }
    private(set) var text = ""
    /// M3 search: the folder the item is in, drawn after the name in
    /// secondary text (truncated from the front).
    private(set) var detail = ""
    private var icon: NSImage?
    private var isDataless = false
    private var isSymlink = false
    private var isDimmed = false
    private var showsChevron = false
    /// Selected, and the pane is active: draw light text on the accent colour.
    private var onAccent = false

    override var isFlipped: Bool { true }
    override var isOpaque: Bool { false }
    override var wantsUpdateLayer: Bool { false }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layerContentsRedrawPolicy = .onSetNeedsDisplay
        canDrawSubviewsIntoLayer = true
    }
    required init?(coder: NSCoder) { fatalError() }

    /// Sets what the cell shows for a row.
    func configure(text: String, icon: NSImage? = nil, row: Row?, onAccent: Bool, chevron: Bool = false,
                   detail: String = "") {
        let dimmed = row.map { $0.isHidden || $0.isCutPending } ?? false
        let dataless = row?.isDataless ?? false
        let symlink = row?.isSymlink ?? false
        if text == self.text, icon === self.icon, dimmed == isDimmed, dataless == isDataless, symlink == isSymlink,
           onAccent == self.onAccent, chevron == showsChevron, detail == self.detail { return }
        self.text = text; self.icon = icon; isDimmed = dimmed; isDataless = dataless; isSymlink = symlink
        self.detail = detail
        self.onAccent = onAccent; showsChevron = chevron
        needsDisplay = true
    }

    /// Where the name text sits (for the rename field).
    var nameTextRect: NSRect {
        let h = ceil(CellStyle.nameFont.ascender - CellStyle.nameFont.descender + CellStyle.nameFont.leading) + 4
        return NSRect(x: 21, y: (bounds.height - h) / 2, width: max(bounds.width - 23, 20), height: h)
    }

    override func draw(_ dirtyRect: NSRect) {
        if RowCache.measuring {
            let c = ContinuousClock()
            let t0 = c.now
            drawContent()
            CellStyle.drawTime += c.now - t0
            CellStyle.drawCalls += 1
        } else {
            drawContent()
        }
    }

    private func drawContent() {
        let b = bounds
        let alpha: CGFloat = isDimmed ? 0.5 : 1
        switch kind {
        case .name, .miller:
            var x: CGFloat = 2
            if let icon {
                let r = NSRect(x: x, y: floor((b.height - 16) / 2), width: 16, height: 16)
                icon.draw(in: r, from: .zero, operation: .sourceOver, fraction: alpha, respectFlipped: true, hints: nil)
                if isSymlink { drawAliasBadge(in: r) }
                x += 21
            }
            var right = b.maxX - 2
            if showsChevron, let c = CellStyle.chevron {
                let s = c.size
                let r = NSRect(x: right - s.width - 4, y: floor((b.height - s.height) / 2), width: s.width, height: s.height)
                tinted(c, onAccent ? .alternateSelectedControlTextColor : .secondaryLabelColor).draw(in: r)
                right = r.minX - 4
            }
            if isDataless, let c = CellStyle.cloud {
                let s = c.size
                let r = NSRect(x: right - s.width - 2, y: floor((b.height - s.height) / 2), width: s.width, height: s.height)
                tinted(c, onAccent ? .alternateSelectedControlTextColor : .secondaryLabelColor).draw(in: r)
                right = r.minX - 4
            }
            let color: NSColor = onAccent ? .alternateSelectedControlTextColor : .labelColor
            if !detail.isEmpty {
                // M3 search: "name   ~/Documents/Work", the folder truncated from its front.
                let avail = max(right - x, 0)
                let nameW = min(ceil((text as NSString).size(withAttributes: [.font: CellStyle.nameFont]).width) + 4,
                                avail * 0.62)
                drawText(NSRect(x: x, y: 0, width: nameW, height: b.height), font: CellStyle.nameFont,
                         color: color.withAlphaComponent(alpha * color.alphaComponent), para: CellStyle.middle)
                let dc: NSColor = onAccent ? .alternateSelectedControlTextColor : .secondaryLabelColor
                drawText(NSRect(x: x + nameW + 8, y: 0, width: max(avail - nameW - 8, 0), height: b.height),
                         font: CellStyle.secondaryFont, color: dc.withAlphaComponent(alpha * dc.alphaComponent),
                         para: Self.headTruncating, string: detail)
                return
            }
            drawText(NSRect(x: x, y: 0, width: max(right - x, 0), height: b.height), font: CellStyle.nameFont,
                     color: color.withAlphaComponent(alpha * color.alphaComponent), para: CellStyle.middle)
        case .text(let rightAligned):
            let color: NSColor = onAccent ? .alternateSelectedControlTextColor : .secondaryLabelColor
            drawText(b.insetBy(dx: 3, dy: 0), font: CellStyle.secondaryFont,
                     color: color.withAlphaComponent(alpha * color.alphaComponent),
                     para: rightAligned ? CellStyle.right : CellStyle.tail)
        }
    }

    private static let headTruncating: NSParagraphStyle = {
        let p = NSMutableParagraphStyle()
        p.lineBreakMode = .byTruncatingHead
        return p
    }()

    private func drawText(_ r: NSRect, font: NSFont, color: NSColor, para: NSParagraphStyle, string: String? = nil) {
        let text = string ?? self.text
        guard r.width > 4, !text.isEmpty else { return }
        let lineH = ceil(font.ascender - font.descender)
        let rect = NSRect(x: r.minX, y: floor((r.height - lineH) / 2) - 0.5, width: r.width, height: lineH + 1)
        (text as NSString).draw(with: rect, options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine],
                                attributes: [.font: font, .foregroundColor: color, .paragraphStyle: para])
    }

    /// Finder's alias arrow in the icon's bottom-left corner.
    private func drawAliasBadge(in icon: NSRect) {
        let r = NSRect(x: icon.minX - 1, y: icon.maxY - 8, width: 8, height: 8)
        NSColor.white.setFill()
        NSBezierPath(roundedRect: r, xRadius: 1.5, yRadius: 1.5).fill()
        NSColor.black.withAlphaComponent(0.35).setStroke()
        let p = NSBezierPath(roundedRect: r.insetBy(dx: 0.25, dy: 0.25), xRadius: 1.5, yRadius: 1.5)
        p.lineWidth = 0.5
        p.stroke()
        if let a = CellStyle.aliasArrow { tinted(a, .black).draw(in: r.insetBy(dx: 1, dy: 1)) }
    }

    private static var tintCache: [String: NSImage] = [:]

    /// The image filled with a colour (cached; dynamic colours resolve at draw time).
    private func tinted(_ img: NSImage, _ color: NSColor) -> NSImage {
        let key = "\(ObjectIdentifier(img).hashValue)-\(color.description)"
        if let c = Self.tintCache[key] { return c }
        let out = NSImage(size: img.size, flipped: false) { rect in
            img.draw(in: rect)
            color.set()
            rect.fill(using: .sourceAtop)
            return true
        }
        Self.tintCache[key] = out
        return out
    }

    // Mouse input goes to the table itself (FileTable handles clicks and drags).
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    // MARK: Accessibility

    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .staticText }
    override func accessibilityValue() -> Any? {
        let t = detail.isEmpty ? text : "\(text), in \(detail)"
        return isDataless ? t + ", in iCloud" : t
    }
}

/// One row: draws the selection from the core (never from NSTableView's own
/// selection, which stays empty).
@MainActor
final class FileRowView: NSTableRowView {
    var coreSelected = false { didSet { if coreSelected != oldValue { needsDisplay = true } } }
    var isCursorRow = false { didSet { if isCursorRow != oldValue { needsDisplay = true } } }
    /// The pane is active (key window, list focused): accent colour.
    var paneActive = false { didSet { if paneActive != oldValue { needsDisplay = true } } }
    /// For VoiceOver: "name, kind, size, date".
    var summary = ""

    override func drawBackground(in dirtyRect: NSRect) {
        // F-019: an opaque content background, no zebra stripes.
        NSColor.textBackgroundColor.setFill()
        dirtyRect.fill()
        if coreSelected {
            (paneActive ? NSColor.selectedContentBackgroundColor : NSColor.unemphasizedSelectedContentBackgroundColor).setFill()
            bounds.fill()
        } else if isCursorRow && paneActive {
            NSColor.selectedContentBackgroundColor.withAlphaComponent(0.6).setStroke()
            let p = NSBezierPath(rect: bounds.insetBy(dx: 1, dy: 1))
            p.lineWidth = 1
            p.stroke()
        }
    }
    override func drawSelection(in dirtyRect: NSRect) {}
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func accessibilityLabel() -> String? { summary }
    override func isAccessibilitySelected() -> Bool { coreSelected }
}

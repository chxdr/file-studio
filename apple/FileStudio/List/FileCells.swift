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
    // MARK: XMP culling additions
    /// Five stars, filled up to the rating; a reject mark before them.
    case rating
    /// A colour-label dot.
    case label
    /// A pick flag or a reject mark.
    case flag
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
    // MARK: XMP culling additions
    static let rejectMark: NSImage? = NSImage(systemSymbolName: "xmark.circle.fill", accessibilityDescription: "Rejected")?
        .withSymbolConfiguration(.init(pointSize: 10, weight: .semibold))
    static let pickFlag: NSImage? = NSImage(systemSymbolName: "flag.fill", accessibilityDescription: "Picked")?
        .withSymbolConfiguration(.init(pointSize: 10, weight: .semibold))
    static let starFont = NSFont.systemFont(ofSize: 10)
    /// The colour a label is drawn in.
    static func color(_ l: CullLabel) -> NSColor {
        switch l {
        case .red: .systemRed
        case .yellow: .systemYellow
        case .green: .systemGreen
        case .blue: .systemBlue
        case .purple: .systemPurple
        }
    }
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
    /// Culling marks from the row's XMP sidecar (drawn, never decided here).
    private var marks: CullMarks?
    private var marksText = ""
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
        let marks = row?.marks
        if text == self.text, icon === self.icon, dimmed == isDimmed, dataless == isDataless, symlink == isSymlink,
           onAccent == self.onAccent, chevron == showsChevron, detail == self.detail, marks == self.marks { return }
        self.text = text; self.icon = icon; isDimmed = dimmed; isDataless = dataless; isSymlink = symlink
        self.marks = marks; marksText = row?.marksText ?? ""
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
            if kind == .name, let m = marks {
                right = drawCompactMarks(m, right: right, height: b.height)
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
        case .rating:
            guard let m = marks else { return }
            var x: CGFloat = 4
            if m.flag == .reject, let img = CellStyle.rejectMark {
                x = drawSymbol(img, x: x, height: b.height, color: rejectColor) + 4
            }
            let filled = String(repeating: "★", count: Int(min(m.rating, 5)))
            let empty = String(repeating: "☆", count: 5 - Int(min(m.rating, 5)))
            let a = NSMutableAttributedString(string: filled, attributes: [
                .font: CellStyle.starFont, .foregroundColor: starColor(dim: m.flag == .reject)])
            a.append(NSAttributedString(string: empty, attributes: [
                .font: CellStyle.starFont,
                .foregroundColor: (onAccent ? NSColor.alternateSelectedControlTextColor : .tertiaryLabelColor)
                    .withAlphaComponent(0.6)]))
            let h = ceil(a.size().height)
            a.draw(at: NSPoint(x: x, y: floor((b.height - h) / 2)))
        case .label:
            guard let l = marks?.label else { return }
            drawDot(CellStyle.color(l), centre: NSPoint(x: 10, y: b.height / 2))
        case .flag:
            guard let m = marks else { return }
            switch m.flag {
            case .pick: if let img = CellStyle.pickFlag { _ = drawSymbol(img, x: 6, height: b.height, color: pickColor) }
            case .reject: if let img = CellStyle.rejectMark { _ = drawSymbol(img, x: 6, height: b.height, color: rejectColor) }
            case .none: break
            }
        }
    }

    // MARK: XMP culling additions (drawing only)

    private var rejectColor: NSColor { onAccent ? .alternateSelectedControlTextColor : .systemRed }
    private var pickColor: NSColor { onAccent ? .alternateSelectedControlTextColor : .systemGreen }
    private func starColor(dim: Bool) -> NSColor {
        let c: NSColor = onAccent ? .alternateSelectedControlTextColor : .systemOrange
        return dim ? c.withAlphaComponent(0.45) : c
    }

    /// Draws a symbol at `x`, centred vertically; returns its right edge.
    private func drawSymbol(_ img: NSImage, x: CGFloat, height: CGFloat, color: NSColor) -> CGFloat {
        let s = img.size
        let r = NSRect(x: x, y: floor((height - s.height) / 2), width: s.width, height: s.height)
        tinted(img, color).draw(in: r)
        return r.maxX
    }

    private func drawDot(_ color: NSColor, centre: NSPoint) {
        let r = NSRect(x: centre.x - 4, y: centre.y - 4, width: 8, height: 8)
        color.setFill()
        NSBezierPath(ovalIn: r).fill()
        if onAccent {
            NSColor.alternateSelectedControlTextColor.setStroke()
            let p = NSBezierPath(ovalIn: r.insetBy(dx: 0.5, dy: 0.5)); p.lineWidth = 1; p.stroke()
        }
    }

    /// The name cell's compact marks, right-aligned before `right`: filled
    /// stars, a label dot and a reject mark. Returns the new right edge.
    private func drawCompactMarks(_ m: CullMarks, right: CGFloat, height: CGFloat) -> CGFloat {
        var right = right
        if m.flag == .reject, let img = CellStyle.rejectMark {
            let w = img.size.width
            _ = drawSymbol(img, x: right - w - 2, height: height, color: rejectColor)
            right -= w + 6
        } else if m.flag == .pick, let img = CellStyle.pickFlag {
            let w = img.size.width
            _ = drawSymbol(img, x: right - w - 2, height: height, color: pickColor)
            right -= w + 6
        }
        if let l = m.label {
            drawDot(CellStyle.color(l), centre: NSPoint(x: right - 6, y: height / 2))
            right -= 14
        }
        if m.rating > 0 {
            let a = NSAttributedString(string: String(repeating: "★", count: Int(min(m.rating, 5))), attributes: [
                .font: CellStyle.starFont, .foregroundColor: starColor(dim: m.flag == .reject)])
            let s = a.size()
            a.draw(at: NSPoint(x: right - ceil(s.width) - 2, y: floor((height - ceil(s.height)) / 2)))
            right -= ceil(s.width) + 6
        }
        return right
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
        switch kind {
        case .rating, .label, .flag: return marksText
        default: break
        }
        var t = detail.isEmpty ? text : "\(text), in \(detail)"
        if !marksText.isEmpty, kind == .name { t += ", " + marksText }
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

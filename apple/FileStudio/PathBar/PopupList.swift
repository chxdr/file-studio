// PopupList.swift — the floating list used by the address bar: the
// completion list under the bar and the chevron / "…" folder menus.
// A borderless child panel with the `.popover` material (HANDS-ON §10)
// holding one view that draws all rows (cheap to refresh per keystroke).
// Rows are data given by the owner; this file only draws and reports
// hover and clicks.

import AppKit

/// One row of a popup list.
struct PopupRow: Equatable {
    enum Kind: Equatable {
        /// A folder (completion or menu entry).
        case item
        /// An inline error line (not selectable).
        case error
        /// An action ("Create Folder", "Go to Nearest Folder").
        case action
        /// A disabled note ("No Subfolders").
        case note
    }
    var kind: Kind = .item
    var icon: NSImage?
    var title: String
    /// Bold ranges of `title` (completion matches), as NSRanges.
    var bold: [NSRange] = []
    /// Secondary text after the title (the full breadcrumb, ForkLift style).
    var detail: String?
    /// A trailing indicator symbol and its text ("clock" + "2 h").
    var badgeSymbol: String?
    var badgeText: String?
    /// Shows a › (has a submenu).
    var hasSubmenu = false
    /// What the row stands for (a path).
    var payload = ""
    /// Read by VoiceOver.
    var accessibilityText: String { [title, detail, badgeText].compactMap { $0 }.joined(separator: ", ") }

    var isSelectable: Bool { kind == .item || kind == .action }
}

/// Draws the rows; reports hover and clicks.
@MainActor
final class PopupListView: NSView {
    var rows: [PopupRow] = [] { didSet { rowsChanged() } }
    /// The highlighted row (nil: none).
    var highlight: Int? { didSet { if highlight != oldValue { needsDisplay = true; highlightChanged() } } }
    var rowHeight: CGFloat = PathBarStyle.suggestionRowHeight
    /// Grey preview text (Tab/↓ previews in the field) — rows don't change.
    var onHover: ((Int) -> Void)?
    var onActivate: ((Int) -> Void)?
    /// Clicked the › area of a row (open its submenu).
    var onOpenSubmenu: ((Int) -> Void)?
    /// Called after a draw, for the keystroke-to-list timing.
    var onDidDraw: (() -> Void)?
    private var tracking: NSTrackingArea?
    private var axRows: [PopupRowElement] = []

    override nonisolated var isFlipped: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    var contentHeight: CGFloat { CGFloat(rows.count) * rowHeight + 8 }

    private func rowsChanged() {
        if let h = highlight, h >= rows.count || !rows[h].isSelectable { highlight = nil }
        setFrameSize(NSSize(width: frame.width, height: contentHeight))
        axRows = []
        needsDisplay = true
    }

    func rect(ofRow i: Int) -> NSRect {
        NSRect(x: 0, y: 4 + CGFloat(i) * rowHeight, width: bounds.width, height: rowHeight)
    }

    func row(at p: NSPoint) -> Int? {
        let i = Int(floor((p.y - 4) / rowHeight))
        return rows.indices.contains(i) ? i : nil
    }

    /// Scrolls the highlighted row into view.
    func revealHighlight() {
        if let h = highlight { scrollToVisible(rect(ofRow: h)) }
    }

    // MARK: Drawing

    override func draw(_ dirtyRect: NSRect) {
        for (i, r) in rows.enumerated() {
            let rr = rect(ofRow: i)
            guard rr.intersects(dirtyRect) else { continue }
            drawRow(r, in: rr, highlighted: i == highlight)
        }
        onDidDraw?()
    }

    private func drawRow(_ r: PopupRow, in rr: NSRect, highlighted: Bool) {
        let hi = highlighted && r.isSelectable
        if hi {
            NSColor.selectedContentBackgroundColor.setFill()
            NSBezierPath(roundedRect: rr.insetBy(dx: 5, dy: 1), xRadius: 5, yRadius: 5).fill()
        }
        let fg: NSColor = hi ? .alternateSelectedControlTextColor
            : r.kind == .error ? .systemRed : r.kind == .note ? .tertiaryLabelColor : .labelColor
        let fg2: NSColor = hi ? .alternateSelectedControlTextColor.withAlphaComponent(0.8) : .secondaryLabelColor
        var x = rr.minX + 12
        let midY = rr.midY
        if let icon = r.icon {
            icon.draw(in: NSRect(x: x, y: midY - 8, width: 16, height: 16), from: .zero, operation: .sourceOver,
                      fraction: r.kind == .note ? 0.5 : 1, respectFlipped: true, hints: nil)
            x += 16 + 6
        } else if r.kind == .error {
            if let s = PathBarStyle.symbol("exclamationmark.triangle.fill", size: 12, color: .systemRed) {
                s.draw(in: NSRect(x: x + 1, y: midY - s.size.height / 2, width: s.size.width, height: s.size.height),
                       from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
            }
            x += 16 + 6
        }
        // Right side: badge, then the submenu chevron.
        var right = rr.maxX - 12
        if r.hasSubmenu {
            let c = PathBarStyle.chevron(pointSize: 10, color: hi ? .alternateSelectedControlTextColor : .secondaryLabelColor)
            c.draw(in: NSRect(x: right - c.size.width, y: midY - c.size.height / 2, width: c.size.width, height: c.size.height),
                   from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
            right -= c.size.width + 8
        }
        if r.badgeSymbol != nil || r.badgeText != nil {
            let small = NSFont.systemFont(ofSize: 11)
            if let t = r.badgeText {
                let a = NSAttributedString(string: t, attributes: [.font: small, .foregroundColor: fg2])
                let w = ceil(a.size().width)
                a.draw(at: NSPoint(x: right - w, y: midY - a.size().height / 2))
                right -= w + 3
            }
            if let name = r.badgeSymbol, let s = PathBarStyle.symbol(name, size: 10, color: fg2) {
                s.draw(in: NSRect(x: right - s.size.width, y: midY - s.size.height / 2, width: s.size.width,
                                  height: s.size.height), from: .zero, operation: .sourceOver, fraction: 1,
                       respectFlipped: true, hints: nil)
                right -= s.size.width
            }
            right -= 10
        }
        // Title (bold matches), then the detail in secondary text, truncated.
        let para = NSMutableParagraphStyle()
        para.lineBreakMode = .byTruncatingTail
        let title = NSMutableAttributedString(string: r.title, attributes: [
            .font: r.kind == .action ? NSFont.systemFont(ofSize: 13, weight: .medium) : PathBarStyle.font,
            .foregroundColor: fg, .paragraphStyle: para,
        ])
        for b in r.bold where b.location != NSNotFound && NSMaxRange(b) <= title.length {
            title.addAttribute(.font, value: NSFont.systemFont(ofSize: 13, weight: .bold), range: b)
        }
        let avail = max(0, right - x)
        let tw = min(ceil(title.size().width), r.detail == nil ? avail : max(avail * 0.6, min(avail, 120)))
        let th = title.size().height
        title.draw(with: NSRect(x: x, y: midY - th / 2, width: tw, height: th),
                   options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
        if let d = r.detail, avail - tw > 30 {
            let dpara = NSMutableParagraphStyle()
            dpara.lineBreakMode = .byTruncatingHead
            let det = NSAttributedString(string: d, attributes: [
                .font: NSFont.systemFont(ofSize: 11), .foregroundColor: fg2, .paragraphStyle: dpara,
            ])
            let dh = det.size().height
            det.draw(with: NSRect(x: x + tw + 8, y: midY - dh / 2 + 0.5, width: avail - tw - 8, height: dh),
                     options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
        }
    }

    // MARK: Mouse

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let t = tracking { removeTrackingArea(t) }
        let t = NSTrackingArea(rect: bounds, options: [.mouseMoved, .mouseEnteredAndExited, .activeAlways, .inVisibleRect],
                               owner: self, userInfo: nil)
        addTrackingArea(t)
        tracking = t
    }

    override func mouseMoved(with event: NSEvent) {
        if let i = row(at: convert(event.locationInWindow, from: nil)), rows[i].isSelectable { onHover?(i) }
    }

    override func mouseDown(with event: NSEvent) {}

    override func mouseUp(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        guard let i = row(at: p), rows[i].isSelectable else { return }
        if rows[i].hasSubmenu, p.x > bounds.maxX - 30 { onOpenSubmenu?(i); return }
        onActivate?(i)
    }

    // MARK: Accessibility

    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .list }
    override func accessibilityChildren() -> [Any]? {
        if axRows.count != rows.count {
            axRows = rows.indices.map { PopupRowElement(list: self, index: $0) }
        }
        return axRows
    }
    override func accessibilitySelectedChildren() -> [Any]? {
        guard let h = highlight, let kids = accessibilityChildren(), kids.indices.contains(h) else { return [] }
        return [kids[h]]
    }

    private func highlightChanged() {
        NSAccessibility.post(element: self, notification: .selectedChildrenChanged)
    }

    fileprivate func activate(_ i: Int) { onActivate?(i) }
}

/// One row for VoiceOver (called on the main thread).
final class PopupRowElement: NSAccessibilityElement {
    private weak var list: PopupListView?
    private let index: Int
    @MainActor
    init(list: PopupListView, index: Int) {
        self.list = list
        self.index = index
        super.init()
        setAccessibilityParent(list)
        setAccessibilityRole(.row)
    }
    override func accessibilityLabel() -> String? {
        let l = list, i = index
        return MainActor.assumeIsolated {
            guard let l, l.rows.indices.contains(i) else { return nil }
            return "\(l.rows[i].accessibilityText), \(i + 1) of \(l.rows.count)"
        }
    }
    override func accessibilityFrame() -> NSRect {
        let l = list, i = index
        return MainActor.assumeIsolated {
            guard let l, let w = l.window else { return .zero }
            return w.convertToScreen(l.convert(l.rect(ofRow: i), to: nil))
        }
    }
    override func isAccessibilitySelected() -> Bool {
        let l = list, i = index
        return MainActor.assumeIsolated { l?.highlight == i }
    }
    override func accessibilityPerformPress() -> Bool {
        let l = list, i = index
        MainActor.assumeIsolated { l?.activate(i) }
        return true
    }
}

/// The floating panel: `.popover` material, rounded, with a scrolling list.
@MainActor
final class PopupPanel: NSPanel {
    let list = PopupListView()
    let scroll = NSScrollView()
    private let canKey: Bool
    /// Called on keyDown when this panel is key (folder menus).
    var onKey: ((NSEvent) -> Bool)?
    var onResignKey: (() -> Void)?

    init(canBecomeKey: Bool, rowHeight: CGFloat) {
        canKey = canBecomeKey
        super.init(contentRect: NSRect(x: 0, y: 0, width: 300, height: 100),
                   styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
        isFloatingPanel = true
        level = .popUpMenu
        hasShadow = true
        isOpaque = false
        backgroundColor = .clear
        hidesOnDeactivate = true
        animationBehavior = .none
        isReleasedWhenClosed = false
        list.rowHeight = rowHeight

        let fx = NSVisualEffectView()
        fx.material = .popover
        fx.state = .active
        fx.blendingMode = .behindWindow
        fx.wantsLayer = true
        fx.layer?.cornerRadius = 8
        fx.layer?.masksToBounds = true
        fx.layer?.borderWidth = 0.5
        fx.layer?.borderColor = NSColor.separatorColor.cgColor
        contentView = fx

        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.scrollerStyle = .overlay
        scroll.documentView = list
        scroll.frame = fx.bounds
        scroll.autoresizingMask = [.width, .height]
        fx.addSubview(scroll)
    }

    override var canBecomeKey: Bool { canKey }
    override var canBecomeMain: Bool { false }

    override func keyDown(with event: NSEvent) {
        if onKey?(event) == true { return }
        super.keyDown(with: event)
    }

    override func cancelOperation(_ sender: Any?) { _ = onKey?(NSEvent.keyEvent(
        with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: windowNumber, context: nil,
        characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}", isARepeat: false, keyCode: 53)!) }

    override func resignKey() {
        super.resignKey()
        onResignKey?()
    }

    /// Sizes the panel for its rows (at most `maxRows` visible) and places its
    /// top-left at `topLeft` (screen coordinates).
    func place(topLeft: NSPoint, width: CGFloat, maxRows: Int = PathBarStyle.maxVisibleRows) {
        let visibleRows = min(list.rows.count, maxRows)
        let h = CGFloat(visibleRows) * list.rowHeight + 8
        list.setFrameSize(NSSize(width: width, height: list.contentHeight))
        var frame = NSRect(x: topLeft.x, y: topLeft.y - h, width: width, height: h)
        if let screen = NSScreen.screens.first(where: { $0.frame.contains(topLeft) }) ?? NSScreen.main {
            let vis = screen.visibleFrame
            if frame.maxX > vis.maxX { frame.origin.x = max(vis.minX, vis.maxX - width) }
            if frame.minY < vis.minY { frame.size.height = max(list.rowHeight + 8, frame.maxY - vis.minY); frame.origin.y = vis.minY }
        }
        setFrame(frame, display: true)
    }

    /// Fades (and, without Reduce Motion, slides 4 pt) in.
    func present(over parent: NSWindow, makeKey: Bool) {
        let target = frame
        if parent.childWindows?.contains(self) != true { parent.addChildWindow(self, ordered: .above) }
        alphaValue = 0
        if !PathBarStyle.reduceMotion { setFrame(target.offsetBy(dx: 0, dy: 4), display: false) }
        if makeKey { makeKeyAndOrderFront(nil) } else { orderFront(nil) }
        NSAnimationContext.runAnimationGroup { ctx in
            ctx.duration = PathBarStyle.fade
            // A slight spring: ease out with overshoot.
            ctx.timingFunction = PathBarStyle.reduceMotion ? CAMediaTimingFunction(name: .easeOut)
                : CAMediaTimingFunction(controlPoints: 0.2, 1.3, 0.4, 1)
            animator().alphaValue = 1
            if !PathBarStyle.reduceMotion { animator().setFrame(target, display: true) }
        }
    }

    func dismiss() {
        parent?.removeChildWindow(self)
        orderOut(nil)
    }
}

// CrumbStripView.swift — the address bar at REST (HANDS-ON §8): one icon +
// name per crumb, a chevron after each (its subfolders), and, when the path
// doesn't fit, the first crumb + "…" + the last crumbs. Draws everything
// itself (one view, no subviews) and reports clicks, menus and drops to its
// owner. The crumbs come from the core; this decides only where to draw them.

import AppKit

@MainActor
protocol CrumbStripDelegate: AnyObject {
    func crumbStrip(go crumb: Crumb)
    func crumbStripEnterEdit()
    func crumbStrip(contextMenuFor crumb: Crumb) -> NSMenu?
    /// The chevron after crumb `index` (its subfolders).
    func crumbStrip(openSubfoldersOf index: Int, anchor: NSRect)
    func crumbStrip(openOverflow hidden: [Crumb], anchor: NSRect)
    func crumbStrip(dropAction sources: [String], onto dir: String) -> DropAction
    func crumbStrip(drop sources: [String], onto dir: String) -> Bool
    func crumbStrip(springLoad crumb: Crumb)
    func crumbStrip(hovering: Bool)
}

/// What fits: which crumbs are drawn and which hide behind "…". Pure, for tests.
struct CrumbLayoutPlan: Equatable {
    var visible: [Int]
    /// Indices behind the "…" button (after `visible[0]`).
    var hidden: [Int]
    /// Width taken by each visible crumb's text (shrunk when nothing else fits).
    var textWidths: [CGFloat]

    /// `textWidths` are the names' natural widths, `fixed` each crumb's
    /// width without its text (icon, padding, chevron). Keeps every crumb
    /// when they fit and there are at most `maxVisible`; otherwise the first
    /// crumb, "…", and as many trailing crumbs as fit (at least the last two,
    /// at most `maxVisible - 1`). If even that doesn't fit, names shrink, the
    /// earlier ones first, the current folder last.
    static func make(textWidths w: [CGFloat], fixed: CGFloat, overflowWidth: CGFloat, available: CGFloat,
                     maxVisible: Int) -> CrumbLayoutPlan {
        let n = w.count
        guard n > 0 else { return CrumbLayoutPlan(visible: [], hidden: [], textWidths: []) }
        func width(_ idx: [Int], overflow: Bool) -> CGFloat {
            idx.reduce(0) { $0 + w[$1] + fixed } + (overflow ? overflowWidth : 0)
        }
        let all = Array(0..<n)
        if n <= max(maxVisible, 2), width(all, overflow: false) <= available {
            return CrumbLayoutPlan(visible: all, hidden: [], textWidths: w)
        }
        var plan: CrumbLayoutPlan?
        if n > 3 {
            let most = min(max(maxVisible - 1, 2), n - 2)
            for tail in stride(from: most, through: 2, by: -1) {
                let vis = [0] + Array((n - tail)..<n)
                if width(vis, overflow: true) <= available {
                    return CrumbLayoutPlan(visible: vis, hidden: Array(1..<(n - tail)), textWidths: vis.map { w[$0] })
                }
            }
            let vis = [0] + Array((n - 2)..<n)
            plan = CrumbLayoutPlan(visible: vis, hidden: Array(1..<(n - 2)), textWidths: vis.map { w[$0] })
        } else {
            plan = CrumbLayoutPlan(visible: all, hidden: [], textWidths: w)
        }
        var p = shrink(plan!, w: w, fixed: fixed, overflowWidth: overflowWidth, available: available)
        // Very narrow (a dual-pane pane): if the current folder's name would be cut
        // below ~60 pt, drop the parent too (first + "…" + current).
        if n > 2, let cur = p.textWidths.last, cur < min(60, w[n - 1]) {
            let one = CrumbLayoutPlan(visible: [0, n - 1], hidden: Array(1..<(n - 1)), textWidths: [w[0], w[n - 1]])
            let q = shrink(one, w: w, fixed: fixed, overflowWidth: overflowWidth, available: available)
            if (q.textWidths.last ?? 0) > cur { p = q }
        }
        return p
    }

    /// Shrinks names until the plan fits: earlier crumbs first, the current one last.
    private static func shrink(_ plan: CrumbLayoutPlan, w: [CGFloat], fixed: CGFloat, overflowWidth: CGFloat,
                               available: CGFloat) -> CrumbLayoutPlan {
        var p = plan
        var excess = p.textWidths.reduce(0, +) + CGFloat(p.visible.count) * fixed
            + (p.hidden.isEmpty ? 0 : overflowWidth) - available
        let order = Array(p.visible.indices.dropLast()) + [p.visible.count - 1]
        for k in order where excess > 0 {
            let minW: CGFloat = k == p.visible.count - 1 ? 24 : 0
            let cut = min(excess, max(0, p.textWidths[k] - minW))
            p.textWidths[k] -= cut
            excess -= cut
        }
        // A sliver of a name reads as noise: show just the icon.
        for k in p.visible.indices.dropLast() where p.textWidths[k] < 20 { p.textWidths[k] = 0 }
        return p
    }
}

@MainActor
final class CrumbStripView: NSView {
    enum Part: Equatable { case crumb(Int), chevron(Int), overflow, separator }

    private struct Slot {
        var part: Part
        var rect: NSRect
        var textWidth: CGFloat = 0
    }

    weak var delegate: CrumbStripDelegate?
    private(set) var crumbs: [Crumb] = []
    var maxVisible = 6 { didSet { if maxVisible != oldValue { needsLayout = true } } }
    private var slots: [Slot] = []
    private(set) var hiddenCrumbs: [Crumb] = []
    private var hovered: Part?
    private var pressed: Part?
    private var dropTarget: Part?
    /// The folder the highlighted crumb stood for (F-023: the drop goes
    /// exactly there, or nowhere).
    private var dropTargetPath: String?
    /// The chevron whose menu is open (drawn pressed).
    var openChevron: Part? { didSet { needsDisplay = true } }
    private var tracking: NSTrackingArea?
    private var springTimer: Timer?
    private var dragSources: (seq: Int, paths: [String])?
    private var axElements: [CrumbElement] = []

    override init(frame: NSRect) {
        super.init(frame: frame)
        registerForDraggedTypes([.fileURL])
        setAccessibilityElement(true)
        setAccessibilityRole(.list)
        setAccessibilityLabel("Path")
    }
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    func setCrumbs(_ c: [Crumb]) {
        guard c != crumbs else { return }
        crumbs = c
        hovered = nil; pressed = nil; dropTarget = nil
        axElements = []
        needsLayout = true
        needsDisplay = true
        NSAccessibility.post(element: self, notification: .layoutChanged)
    }

    /// The last crumb's right edge (empty space starts here).
    var contentMaxX: CGFloat { slots.last?.rect.maxX ?? 0 }

    // MARK: Layout

    private func textWidth(_ c: Crumb) -> CGFloat {
        ceil((c.name as NSString).size(withAttributes: [.font: c.isCurrent ? PathBarStyle.currentFont : PathBarStyle.font]).width)
    }

    private static let overflowText = "…"
    private var fixedCrumbWidth: CGFloat {
        PathBarStyle.crumbPad * 2 + PathBarStyle.iconSize + PathBarStyle.iconGap + PathBarStyle.chevronWidth
    }

    override func layout() {
        super.layout()
        let s = PathBarStyle.self
        let ow = ceil((Self.overflowText as NSString).size(withAttributes: [.font: s.font]).width) + s.crumbPad * 2
            + s.chevronWidth
        let plan = CrumbLayoutPlan.make(textWidths: crumbs.map(textWidth), fixed: fixedCrumbWidth, overflowWidth: ow,
                                        available: bounds.width - 4, maxVisible: maxVisible)
        hiddenCrumbs = plan.hidden.map { crumbs[$0] }
        slots = []
        var x: CGFloat = 0
        let h = bounds.height
        for (k, i) in plan.visible.enumerated() {
            let tw = plan.textWidths[k]
            // Icon only when the name has shrunk to nothing.
            let gap = tw > 0 ? s.iconGap : 0
            let w = s.crumbPad * 2 + s.iconSize + gap + tw
            slots.append(Slot(part: .crumb(i), rect: NSRect(x: x, y: 0, width: w, height: h), textWidth: tw))
            x += w
            slots.append(Slot(part: .chevron(i), rect: NSRect(x: x, y: 0, width: s.chevronWidth, height: h)))
            x += s.chevronWidth
            if k == 0, !plan.hidden.isEmpty {
                let w = ow - s.chevronWidth
                slots.append(Slot(part: .overflow, rect: NSRect(x: x, y: 0, width: w, height: h)))
                x += w
                slots.append(Slot(part: .separator, rect: NSRect(x: x, y: 0, width: s.chevronWidth, height: h)))
                x += s.chevronWidth
            }
        }
        axElements = []
        needsDisplay = true
    }

    func rect(of part: Part) -> NSRect? { slots.first { $0.part == part }?.rect }

    func part(at p: NSPoint) -> Part? {
        slots.first { $0.rect.contains(p) && $0.part != .separator }?.part
    }

    // MARK: Drawing

    override func draw(_ dirtyRect: NSRect) {
        let s = PathBarStyle.self
        for slot in slots {
            let r = slot.rect
            let isDrop = dropTarget == slot.part
            if isDrop {
                NSColor.controlAccentColor.withAlphaComponent(0.25).setFill()
                let p = NSBezierPath(roundedRect: r.insetBy(dx: 0, dy: 1), xRadius: 5, yRadius: 5)
                p.fill()
                NSColor.controlAccentColor.setStroke()
                p.lineWidth = 1.5
                p.stroke()
            } else if slot.part == pressed || slot.part == openChevron {
                s.crumbPressed.setFill()
                NSBezierPath(roundedRect: r.insetBy(dx: 0, dy: 1), xRadius: 5, yRadius: 5).fill()
            } else if slot.part == hovered {
                s.crumbHover.setFill()
                NSBezierPath(roundedRect: r.insetBy(dx: 0, dy: 1), xRadius: 5, yRadius: 5).fill()
            }
            switch slot.part {
            case .crumb(let i):
                let c = crumbs[i]
                let icon = CrumbIcons.icon(c)
                let ir = NSRect(x: r.minX + s.crumbPad, y: r.midY - s.iconSize / 2, width: s.iconSize, height: s.iconSize)
                icon.draw(in: ir, from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
                if slot.textWidth > 0 {
                    let para = NSMutableParagraphStyle()
                    para.lineBreakMode = .byTruncatingTail
                    let a = NSAttributedString(string: c.name, attributes: [
                        .font: c.isCurrent ? s.currentFont : s.font,
                        .foregroundColor: isDrop ? NSColor.labelColor : (c.isCurrent ? NSColor.labelColor : NSColor.secondaryLabelColor),
                        .paragraphStyle: para,
                    ])
                    let th = ceil(a.size().height)
                    a.draw(with: NSRect(x: ir.maxX + s.iconGap, y: r.midY - th / 2, width: slot.textWidth, height: th),
                           options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
                }
            case .chevron, .separator:
                let active = slot.part == hovered || slot.part == openChevron || isDrop
                let c = s.chevron(color: active ? .labelColor : .tertiaryLabelColor)
                c.draw(in: NSRect(x: r.midX - c.size.width / 2, y: r.midY - c.size.height / 2, width: c.size.width,
                                  height: c.size.height), from: .zero, operation: .sourceOver, fraction: 1,
                       respectFlipped: true, hints: nil)
            case .overflow:
                let a = NSAttributedString(string: Self.overflowText, attributes: [
                    .font: s.font, .foregroundColor: NSColor.secondaryLabelColor,
                ])
                let sz = a.size()
                a.draw(at: NSPoint(x: r.midX - sz.width / 2, y: r.midY - sz.height / 2))
            }
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

    private func setHovered(_ p: Part?) {
        guard p != hovered else { return }
        hovered = p
        needsDisplay = true
    }

    override func mouseEntered(with event: NSEvent) { delegate?.crumbStrip(hovering: true) }
    override func mouseMoved(with event: NSEvent) { setHovered(part(at: convert(event.locationInWindow, from: nil))) }
    override func mouseExited(with event: NSEvent) {
        setHovered(nil)
        delegate?.crumbStrip(hovering: false)
    }

    /// Simulates hover (debug automation snapshots).
    func debugHover(_ p: Part?) { setHovered(p) }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        guard let part = part(at: p) else {
            delegate?.crumbStripEnterEdit()
            return
        }
        switch part {
        case .crumb(let i) where event.modifierFlags.contains(.option):
            showContextMenu(for: i, event: event)
        case .crumb:
            pressed = part
            needsDisplay = true
        case .chevron(let i):
            delegate?.crumbStrip(openSubfoldersOf: i, anchor: screenRect(of: part))
        case .overflow:
            delegate?.crumbStrip(openOverflow: hiddenCrumbs, anchor: screenRect(of: part))
        case .separator:
            break
        }
    }

    override func mouseUp(with event: NSEvent) {
        defer { pressed = nil; needsDisplay = true }
        guard let pr = pressed, case .crumb(let i) = pr,
              part(at: convert(event.locationInWindow, from: nil)) == pr, crumbs.indices.contains(i) else { return }
        delegate?.crumbStrip(go: crumbs[i])
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        let p = convert(event.locationInWindow, from: nil)
        switch part(at: p) {
        case .crumb(let i), .chevron(let i): return crumbs.indices.contains(i) ? delegate?.crumbStrip(contextMenuFor: crumbs[i]) : nil
        default: return nil
        }
    }

    private func showContextMenu(for i: Int, event: NSEvent) {
        guard let m = delegate?.crumbStrip(contextMenuFor: crumbs[i]) else { return }
        NSMenu.popUpContextMenu(m, with: event, for: self)
    }

    func screenRect(of part: Part) -> NSRect {
        guard let r = rect(of: part), let w = window else { return .zero }
        return w.convertToScreen(convert(r, to: nil))
    }

    /// The crumb index a part acts on (a chevron drops into its crumb).
    private func crumbIndex(_ p: Part?) -> Int? {
        switch p {
        case .crumb(let i), .chevron(let i): crumbs.indices.contains(i) ? i : nil
        default: nil
        }
    }

    // MARK: Drop (the core decides move or copy)

    private func sources(_ info: NSDraggingInfo) -> [String] {
        if let s = dragSources, s.seq == info.draggingSequenceNumber { return s.paths }
        let urls = info.draggingPasteboard.readObjects(forClasses: [NSURL.self],
                                                       options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
        let paths = urls.map(\.path)
        dragSources = (info.draggingSequenceNumber, paths)
        return paths
    }

    private func dragOperation(_ info: NSDraggingInfo) -> NSDragOperation {
        // F-023: sidebar rows are references; dropping one here never moves or copies it.
        if SidebarViewController.isSidebarDrag(info) { setDropTarget(nil); return [] }
        let p = convert(info.draggingLocation, from: nil)
        let target = part(at: p)
        guard let i = crumbIndex(target) else { setDropTarget(nil); return [] }
        let action = delegate?.crumbStrip(dropAction: sources(info), onto: crumbs[i].path) ?? .none
        setDropTarget(action == .none ? nil : target)
        dropTargetPath = action == .none ? nil : crumbs[i].path
        switch action {
        case .none: return []
        case .copy: return .copy
        case .move: return .move
        }
    }

    private func setDropTarget(_ p: Part?) {
        guard p != dropTarget else { return }
        dropTarget = p
        needsDisplay = true
        springTimer?.invalidate()
        springTimer = nil
        guard let i = crumbIndex(p), !crumbs[i].isCurrent else { return }
        let c = crumbs[i]
        springTimer = Timer.scheduledTimer(withTimeInterval: PathBarStyle.springLoadDelay, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self, self.dropTarget == p else { return }
                self.flashSpringLoad()
                self.delegate?.crumbStrip(springLoad: c)
                self.dropTarget = nil
            }
        }
    }

    /// A short blink before a spring-loaded folder opens (none under Reduce Motion).
    private func flashSpringLoad() {
        guard !PathBarStyle.reduceMotion else { return }
        layer?.add({
            let a = CABasicAnimation(keyPath: "opacity")
            a.fromValue = 0.4; a.toValue = 1; a.duration = 0.12
            return a
        }(), forKey: "spring")
    }

    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation { dragOperation(sender) }
    override func draggingUpdated(_ sender: NSDraggingInfo) -> NSDragOperation { dragOperation(sender) }
    override func draggingExited(_ sender: NSDraggingInfo?) { setDropTarget(nil) }
    override func draggingEnded(_ sender: NSDraggingInfo) { setDropTarget(nil); dragSources = nil }
    override func prepareForDragOperation(_ sender: NSDraggingInfo) -> Bool { dropTarget != nil }
    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        guard !SidebarViewController.isSidebarDrag(sender), let i = crumbIndex(dropTarget),
              crumbs[i].path == dropTargetPath else { setDropTarget(nil); return false }
        let ok = delegate?.crumbStrip(drop: sources(sender), onto: crumbs[i].path) ?? false
        setDropTarget(nil)
        return ok
    }

    // MARK: Accessibility: a list of buttons ("Pictures, folder, 3 of 5")

    override func accessibilityChildren() -> [Any]? {
        if axElements.count != slots.filter({ $0.part != .separator }).count {
            axElements = slots.filter { $0.part != .separator }.map { CrumbElement(strip: self, part: $0.part) }
        }
        return axElements
    }

    fileprivate func axLabel(_ p: Part) -> String {
        switch p {
        case .crumb(let i):
            let c = crumbs[i]
            let kind = switch c.kind {
            case .volumeRoot: "volume"
            case .home: "home folder"
            case .package: "package"
            case .folder: "folder"
            }
            return "\(c.name), \(kind)\(c.isCurrent ? ", current" : ""), \(i + 1) of \(crumbs.count)"
        case .chevron(let i): return "Subfolders of \(crumbs[i].name)"
        case .overflow: return "\(hiddenCrumbs.count) hidden \(hiddenCrumbs.count == 1 ? "folder" : "folders")"
        case .separator: return ""
        }
    }

    fileprivate func axPress(_ p: Part) {
        switch p {
        case .crumb(let i): delegate?.crumbStrip(go: crumbs[i])
        case .chevron(let i): delegate?.crumbStrip(openSubfoldersOf: i, anchor: screenRect(of: p))
        case .overflow: delegate?.crumbStrip(openOverflow: hiddenCrumbs, anchor: screenRect(of: p))
        case .separator: break
        }
    }

    fileprivate func axShowMenu(_ p: Part) {
        guard case .crumb(let i) = p, let m = delegate?.crumbStrip(contextMenuFor: crumbs[i]),
              let r = rect(of: p) else { return }
        m.popUp(positioning: nil, at: NSPoint(x: r.minX, y: r.maxY), in: self)
    }
}

/// One crumb, chevron or "…" for VoiceOver (called on the main thread).
final class CrumbElement: NSAccessibilityElement {
    private weak var strip: CrumbStripView?
    private let part: CrumbStripView.Part
    @MainActor
    init(strip: CrumbStripView, part: CrumbStripView.Part) {
        self.strip = strip
        self.part = part
        super.init()
        setAccessibilityParent(strip)
        setAccessibilityRole(.button)
    }
    override func accessibilityLabel() -> String? {
        let s = strip, p = part
        return MainActor.assumeIsolated { s?.axLabel(p) }
    }
    override func accessibilityFrame() -> NSRect {
        let s = strip, p = part
        return MainActor.assumeIsolated { s?.screenRect(of: p) ?? .zero }
    }
    override func accessibilityPerformPress() -> Bool {
        let s = strip, p = part
        MainActor.assumeIsolated { s?.axPress(p) }
        return true
    }
    override func accessibilityPerformShowMenu() -> Bool {
        let s = strip, p = part
        MainActor.assumeIsolated { s?.axShowMenu(p) }
        return true
    }
}

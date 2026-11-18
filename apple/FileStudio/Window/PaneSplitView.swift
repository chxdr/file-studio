// PaneSplitView.swift — lays out one or two panes (F-019): side by side
// (`.horizontal`) or stacked (`.vertical`), split at the core's ratio, with a
// 1 pt hairline divider and an 8 pt invisible drag target. Dragging is shown
// live and reported to the core when the mouse goes up; a double-click asks
// the core to equalise (`pane.equalize`). The core owns the layout; this view
// keeps only the in-flight drag.

import AppKit

@MainActor
final class PaneSplitView: NSView {
    /// Called when a drag ends with the new ratio (0…1, first pane's share).
    var onRatioChanged: ((Double) -> Void)?
    /// Called on a double-click on the divider.
    var onEqualize: (() -> Void)?

    private(set) var panes: [NSView] = []
    private let divider = DividerView()
    private var orientation: Orientation = .horizontal
    private var ratio: Double = 0.5
    /// The ratio while the user drags (nil otherwise).
    private var dragRatio: Double?
    /// Smallest pane, in points along the split.
    private let minPane: CGFloat = 220
    static let hit: CGFloat = 8

    override init(frame: NSRect) {
        super.init(frame: frame)
        divider.owner = self
        addSubview(divider)
        setAccessibilityElement(false)
    }
    required init?(coder: NSCoder) { fatalError() }

    override nonisolated var isFlipped: Bool { true }

    /// Replaces the panes (one or two views). New views fade in (0.14 s).
    func setPanes(_ views: [NSView]) {
        let old = Set(panes.map(ObjectIdentifier.init))
        for v in panes where !views.contains(where: { $0 === v }) { v.removeFromSuperview() }
        panes = views
        for v in views where v.superview !== self {
            v.translatesAutoresizingMaskIntoConstraints = true
            addSubview(v, positioned: .below, relativeTo: divider)
            if !old.isEmpty && !old.contains(ObjectIdentifier(v)) {
                v.alphaValue = 0
                NSAnimationContext.runAnimationGroup { c in
                    c.duration = 0.14
                    v.animator().alphaValue = 1
                }
            }
        }
        divider.isHidden = views.count < 2
        needsLayout = true
    }

    /// Applies the core's layout (ignored while a drag is in flight).
    func apply(orientation o: Orientation, ratio r: Double) {
        guard dragRatio == nil else { return }
        if o != orientation || abs(r - ratio) > 0.0001 {
            orientation = o
            ratio = r
            divider.vertical = o == .horizontal
            needsLayout = true
            window?.invalidateCursorRects(for: divider)
        }
    }

    private var length: CGFloat { orientation == .horizontal ? bounds.width : bounds.height }

    /// The split position in points for a ratio, kept inside the minimums.
    private func position(for r: Double) -> CGFloat {
        let len = length - 1
        let lo = min(minPane, len / 2), hi = max(len - minPane, len / 2)
        return (CGFloat(r) * len).clamped(lo, hi).rounded()
    }

    override func layout() {
        super.layout()
        guard let first = panes.first else { return }
        guard panes.count == 2 else {
            first.frame = bounds
            return
        }
        let p = position(for: dragRatio ?? ratio)
        let half = Self.hit / 2
        if orientation == .horizontal {
            panes[0].frame = NSRect(x: 0, y: 0, width: p, height: bounds.height)
            panes[1].frame = NSRect(x: p + 1, y: 0, width: bounds.width - p - 1, height: bounds.height)
            divider.frame = NSRect(x: p - half + 0.5, y: 0, width: Self.hit, height: bounds.height)
        } else {
            panes[0].frame = NSRect(x: 0, y: 0, width: bounds.width, height: p)
            panes[1].frame = NSRect(x: 0, y: p + 1, width: bounds.width, height: bounds.height - p - 1)
            divider.frame = NSRect(x: 0, y: p - half + 0.5, width: bounds.width, height: Self.hit)
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        guard panes.count == 2 else { return }
        NSColor.separatorColor.setFill()
        let p = position(for: dragRatio ?? ratio)
        if orientation == .horizontal {
            NSRect(x: p, y: 0, width: 1, height: bounds.height).fill()
        } else {
            NSRect(x: 0, y: p, width: bounds.width, height: 1).fill()
        }
    }

    // MARK: Divider dragging

    fileprivate func dragged(to point: NSPoint) {
        let p = convert(point, from: nil)
        let len = length - 1
        guard len > 0 else { return }
        let at = orientation == .horizontal ? p.x : p.y
        dragRatio = Double(at / len).clamped(0.05, 0.95)
        needsLayout = true
        needsDisplay = true
    }

    fileprivate func dragEnded() {
        guard let r = dragRatio else { return }
        // Store what is actually shown (the minimums may have clamped it).
        let shown = Double(position(for: r) / max(length - 1, 1))
        dragRatio = nil
        ratio = shown
        needsLayout = true
        needsDisplay = true
        onRatioChanged?(shown)
    }

    fileprivate func equalize() { onEqualize?() }

    /// The divider's position and orientation, for debug logs.
    var debugDescriptionLine: String {
        "panes=\(panes.count) orientation=\(orientation) ratio=\(String(format: "%.3f", ratio)) at=\(Int(position(for: ratio)))"
    }
}

/// The invisible 8 pt drag target over the hairline.
@MainActor
private final class DividerView: NSView {
    weak var owner: PaneSplitView?
    var vertical = true

    override init(frame: NSRect) {
        super.init(frame: frame)
        setAccessibilityElement(true)
        setAccessibilityRole(.splitter)
        setAccessibilityLabel("Pane divider")
        setAccessibilityHelp("Drag to resize the panes; double-click to make them equal.")
    }
    required init?(coder: NSCoder) { fatalError() }

    override func resetCursorRects() {
        addCursorRect(bounds, cursor: vertical ? .resizeLeftRight : .resizeUpDown)
    }

    override func mouseDown(with event: NSEvent) {
        if event.clickCount == 2 { owner?.equalize(); return }
        guard let w = window else { return }
        while let e = w.nextEvent(matching: [.leftMouseDragged, .leftMouseUp]) {
            if e.type == .leftMouseUp { break }
            owner?.dragged(to: e.locationInWindow)
        }
        owner?.dragEnded()
    }
}

extension Comparable {
    /// The value kept within `lo…hi`.
    func clamped(_ lo: Self, _ hi: Self) -> Self { min(max(self, lo), hi) }
}

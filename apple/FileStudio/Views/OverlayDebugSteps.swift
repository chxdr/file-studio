// OverlayDebugSteps.swift — DEBUG-only script steps for the held-modifier
// overlay (registered with DebugAutomation from `OverlayController.install`):
//
//   wheel:<x>,<y>,<dy>[,line|pixel[,<title>]]   one scroll-wheel event at a point (main window content
//                                     points from the top-left), sent through the window like
//                                     a real one; `line` (the default) is a click-wheel mouse,
//                                     `pixel` a smooth-scrolling mouse or trackpad; <title> picks
//                                     another window by title (Settings)
//   winsize:<w>,<h>                   resize the main window (points), to make the list overflow
//   heldlog                           HELD lines: rows, columns, scroll offset/content/visible,
//                                     the panel frame, and what a wheel at its centre would hit

#if DEBUG
import AppKit

@MainActor
enum OverlayDebugSteps {
    private static var installed = false

    static func install() {
        guard !installed else { return }
        installed = true
        DebugAutomation.register("wheel") { arg, _ in wheel(arg) }
        DebugAutomation.register("heldlog") { _, _ in log() }
        DebugAutomation.register("winsize") { arg, _ in
            let n = arg.split(separator: ",").compactMap { Double($0) }
            guard n.count == 2, let w = window else { return }
            var f = w.frame
            f.origin.y += f.height - n[1]
            f.size = NSSize(width: n[0], height: n[1])
            w.setFrame(f, display: true)
            print("WINSIZE \(NSStringFromRect(w.frame))")
        }
    }

    private static var window: NSWindow? {
        NSApp.windows.first { $0.windowController is CoreWindowIdentifying && $0.isVisible }
    }

    private static func wheel(_ arg: String) {
        let parts = arg.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
        guard parts.count >= 3, let x = Double(parts[0]), let y = Double(parts[1]), let dy = Int32(parts[2]),
              let w = parts.count > 4 ? NSApp.windows.first(where: { $0.isVisible && ($0.title.contains(parts[4])
                  || String(describing: type(of: $0)).contains(parts[4]) || $0.identifier?.rawValue.contains(parts[4]) == true) }) : window,
              let content = w.contentView else { print("WHEEL bad \(arg) windows: \(NSApp.windows.filter(\.isVisible).map { "\($0.title)|\(type(of: $0))" })"); return }
        let lines = parts.count < 4 || parts[3] != "pixel"
        let inWindow = content.convert(NSPoint(x: x, y: content.isFlipped ? y : content.bounds.height - y), to: nil)
        let onScreen = w.convertPoint(toScreen: inWindow)
        let primary = NSScreen.screens.first?.frame.height ?? 0
        guard let cg = CGEvent(scrollWheelEvent2Source: nil, units: lines ? .line : .pixel, wheelCount: 1,
                               wheel1: dy, wheel2: 0, wheel3: 0) else { return }
        cg.location = CGPoint(x: onScreen.x, y: primary - onScreen.y)
        if !lines {
            // LinearMouse's smooth mode: continuous pixel deltas.
            cg.setIntegerValueField(.scrollWheelEventIsContinuous, value: 1)
            cg.setIntegerValueField(.scrollWheelEventPointDeltaAxis1, value: Int64(dy))
            cg.setDoubleValueField(.scrollWheelEventFixedPtDeltaAxis1, value: Double(dy))
        }
        guard let e = NSEvent(cgEvent: cg) else { return }
        // A synthetic event has no window, so route it as NSWindow routes a real
        // wheel event: to the view the window's frame view hit-tests at the pointer
        // (which passes it up the responder chain if it doesn't scroll).
        let hit = content.superview?.hitTest(inWindow)
        print("WHEEL \(arg) precise=\(e.hasPreciseScrollingDeltas) dy=\(e.scrollingDeltaY) -> \(hit.map { String(describing: type(of: $0)) } ?? "nil")")
        hit?.scrollWheel(with: e)
    }

    private static func log() {
        let s = OverlayController.shared.state
        guard let held = s.held else { print("HELD none"); return }
        let m = s.heldScroll
        let cats = held.rows.reduce(into: [String]()) { if $0.last != $1.category { $0.append($1.category) } }
        print("HELD \(held.title) rows=\(held.rows.count) categories=\(cats.count)")
        print(String(format: "HELD scroll offset=%.0f content=%.0f visible=%.0f overflows=%@", m.offset, m.content, m.visible,
                     m.metrics.overflows ? "yes" : "no"))
        print("HELD panel \(NSStringFromRect(m.panelFrame))")
        if let first = held.rows.first, let last = held.rows.last {
            print("HELD first=\(first.keys) \(first.label) last=\(last.keys) \(last.label)")
        }
    }
}
#endif

// GridDebugSteps.swift — DEBUG-only automation steps for the icon view and
// the gallery (registered with DebugAutomation.extraSteps):
//
//   gridlog                 a GRID line: mode, columns, tiles alive and pooled, the
//                           visible range, thumbnails shown/made/cancelled/pending,
//                           and for the gallery the item shown and its details
//   gridwait[:<s>]          waits (default 2 s) for thumbnails, then logs
//   gridmarks:<fake|off>    made-up culling marks from the names (until the ratings
//                           branch lands), to check the drawing
//   gridbench:<s>:<wheel|smooth>[:<speed>]   scrolls the icon view (or the filmstrip)
//                           with synthetic wheel events from a display link, as
//                           bench:scroll does for the list; prints frame times,
//                           dropped frames and main-thread time per frame (BENCH lines)
//   gridsize:<points>       sets the thumbnail size through the slider's call

#if DEBUG
import AppKit
import QuartzCore

@MainActor
enum GridDebugSteps {
    private static var installed = false

    /// The focused pane's file area.
    private static func active(_ m: AppModel) -> ListPlaceholderView? {
        let ctrls = NSApp.windows.compactMap { $0.windowController as? MainWindowController }
        let panes = ctrls.flatMap(\.allPanes)
        return panes.first { $0.tabID == m.focusedTab?.id }?.list ?? panes.first?.list
    }

    private static func grid(_ m: AppModel) -> TileGrid? {
        guard let l = active(m) else { return nil }
        return l.icons?.superview != nil ? l.icons?.grid : l.gallery?.superview != nil ? l.gallery?.strip : nil
    }

    static func install(model: AppModel) {
        guard !installed else { return }
        installed = true
        DebugAutomation.extraSteps["gridlog"] = { _ in log(model) }
        DebugAutomation.extraSteps["gridwait"] = { arg in
            try? await Task.sleep(for: .seconds(Double(arg) ?? 2))
            log(model)
        }
        DebugAutomation.extraSteps["gridmarks"] = { arg in
            GridMarks.debugFake = arg == "fake"
            if let g = grid(model) { g.refreshTiles(all: true) }
        }
        DebugAutomation.extraSteps["gridsize"] = { arg in
            (model.engine as? GridEngineAPI)?.setIconSize(Int(arg) ?? 128)
        }
        DebugAutomation.extraSteps["gridbench"] = { arg in await bench(model, arg) }
    }

    private static func log(_ m: AppModel) {
        guard let l = active(m) else { print("GRID none"); return }
        let mode = m.focusedTab?.api.view().mode
        if let g = l.gallery, g.superview != nil {
            print("GRID gallery \(g.debugInfo)")
        } else if let i = l.icons, i.superview != nil {
            print("GRID icons size=\(GridSettings.iconSize(m)) \(i.grid.debugInfo)")
        } else {
            print("GRID mode=\(mode.map { "\($0)" } ?? "?") (no grid shown)")
        }
    }

    private static func bench(_ m: AppModel, _ arg: String) async {
        let parts = arg.split(separator: ":").map(String.init)
        let seconds = parts.first.flatMap(Double.init) ?? 3
        let wheel = parts.count > 1 && parts[1] == "wheel"
        let speed = parts.count > 2 ? Int32(parts[2]) ?? 12 : 12
        guard let g = grid(m), let w = g.scrollView.window else { print("BENCH no grid"); return }
        g.scrollToStart()
        try? await Task.sleep(for: .milliseconds(300))
        let d = GridScrollDriver(scrollView: g.scrollView, window: w, wheel: wheel, speed: speed)
        d.start()
        try? await Task.sleep(for: .seconds(seconds))
        d.stop()
        let fps = Double(w.screen?.maximumFramesPerSecond ?? 60)
        let period = 1000 / fps
        var dts = d.intervals.sorted()
        if dts.isEmpty { dts = [0] }
        func pct(_ a: [Double], _ p: Double) -> Double { a.isEmpty ? 0 : a[min(a.count - 1, Int(Double(a.count - 1) * p))] }
        let dropped = d.intervals.reduce(0) { $0 + max(0, Int(($1 / period).rounded()) - 1) }
        let expected = Int(seconds * fps)
        let busy = d.busy.sorted()
        print(String(format: "BENCH grid %@ (%d) %.0fs at %.0f Hz: %d frames (expected ~%d), frame time p50 %.2f ms, p95 %.2f ms, "
                     + "p99 %.2f ms, max %.2f ms, dropped %d (%.1f%%); main-thread work per frame p50 %.2f ms, p99 %.2f ms, max %.2f ms",
                     wheel ? "wheel" : "smooth", Int(speed), seconds, fps, d.intervals.count, expected, pct(dts, 0.5),
                     pct(dts, 0.95), pct(dts, 0.99), dts.last!, dropped, 100 * Double(dropped) / Double(max(expected, 1)),
                     pct(busy, 0.5), pct(busy, 0.99), busy.last ?? 0))
        print("BENCH grid after: \(g.debugInfo)")
    }
}

/// Synthetic wheel events from a display link (the same path as a real mouse).
@MainActor
final class GridScrollDriver: NSObject {
    private unowned let scrollView: NSScrollView
    private unowned let window: NSWindow
    private let wheel: Bool
    private let speed: Int32
    private var link: CADisplayLink?
    private var last: CFTimeInterval = 0
    private var tick = 0
    private(set) var intervals: [Double] = []
    private(set) var busy: [Double] = []

    init(scrollView: NSScrollView, window: NSWindow, wheel: Bool, speed: Int32) {
        self.scrollView = scrollView; self.window = window; self.wheel = wheel; self.speed = speed
    }

    func start() {
        let l = window.displayLink(target: self, selector: #selector(frame(_:)))
        l.add(to: .main, forMode: .common)
        link = l
    }

    func stop() { link?.invalidate(); link = nil }

    @objc private func frame(_ l: CADisplayLink) {
        if last > 0 { intervals.append((l.timestamp - last) * 1000) }
        last = l.timestamp
        tick += 1
        let t0 = CACurrentMediaTime()
        if wheel {
            if tick % max(Int((1 / max(l.duration, 1.0 / 240)) / 15), 1) == 0 { post(lines: -max(speed / 4, 3)) }
        } else {
            post(pixels: -speed)
        }
        scrollView.displayIfNeeded()
        busy.append((CACurrentMediaTime() - t0) * 1000)
    }

    private func post(lines: Int32 = 0, pixels: Int32 = 0) {
        let units: CGScrollEventUnit = lines != 0 ? .line : .pixel
        guard let cg = CGEvent(scrollWheelEvent2Source: nil, units: units, wheelCount: 1,
                               wheel1: lines != 0 ? lines : pixels, wheel2: 0, wheel3: 0) else { return }
        if lines == 0 {
            cg.setIntegerValueField(.scrollWheelEventIsContinuous, value: 1)
            cg.setIntegerValueField(.scrollWheelEventPointDeltaAxis1, value: Int64(pixels))
            cg.setDoubleValueField(.scrollWheelEventFixedPtDeltaAxis1, value: Double(pixels))
        }
        if let e = NSEvent(cgEvent: cg) { scrollView.scrollWheel(with: e) }
    }
}
#endif

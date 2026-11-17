// ListDebug.swift — DEBUG-only automation steps for the list and column views
// (registered with DebugAutomation.extraSteps):
//
//   bench:scroll:<seconds>:<wheel|smooth>[:<speed>]   scroll the focused list with a display link (speed: px/frame or lines/notch ×4);
//                         prints frame times (p50/p95/p99), dropped frames at the display's
//                         rate, and main-thread time in row fetching, cell configuration
//                         and cell drawing (BENCH lines)
//   bench:open:<path>     time from going to a folder to its first drawn row
//   drop:<src>|<src>…#<row or name>#<alt|cmd|->   drops files on a row (-1: the folder) as a
//                         drag would, through the core's drop rule (DROP lines)
//   dropeffect:<src>|…#<row>#<mods>       prints what that drop would do
//   density:<compact|regular|comfortable> sets the row density setting
//   appearance:<light|dark>               the app's appearance
//   columns:<id,id,…>     sets the list columns (name,size,modified,created,kind,ext)
//   sortcol:<id>          clicks a column header
//   headermenu            prints the column chooser (HEADERMENU lines)
//   ctx:<row>             prints the core's context menu for a row (-1: empty space)
//   deactivate / activate takes keyboard focus away from / back to the list
//   listlog               prints the list's state (LIST lines)
//   select:<name>|<name>  selects rows by name through the core
//   state:<loading|empty|privacy|none>  forces a state overlay (for snapshots)
//   resize:<w>,<h>        resizes the main window's content

#if DEBUG
import AppKit
import QuartzCore
import QuickLookUI

@MainActor
enum ListDebug {
    private static var installed = false
    private static var lists: [WeakList] = []
    private final class WeakList { weak var v: ListPlaceholderView?; init(_ v: ListPlaceholderView) { self.v = v } }

    static func register(_ l: ListPlaceholderView) {
        lists.removeAll { $0.v == nil }
        if !lists.contains(where: { $0.v === l }) { lists.append(WeakList(l)) }
    }

    /// The focused pane's list.
    private static func active(_ m: AppModel) -> ListPlaceholderView? {
        let all = lists.compactMap(\.v)
        return all.first { $0.tab?.id == m.focusedTab?.id } ?? all.first
    }

    static func install(model: AppModel) {
        guard !installed else { return }
        installed = true
        FileTable.assumeKeyWindow = UserDefaults.standard.string(forKey: "FSDebugScript") != nil
        let steps: [String: @MainActor (String) async -> Void] = [
            "bench": { arg in
                if arg.hasPrefix("scroll") { await benchScroll(model, arg) }
                if arg.hasPrefix("open:") { await benchOpen(model, String(arg.dropFirst(5))) }
            },
            "drop": { arg in drop(model, arg, perform: true) },
            "dropeffect": { arg in drop(model, arg, perform: false) },
            "density": { arg in
                let v = ["compact": "Compact", "regular": "Regular", "comfortable": "Comfortable"][arg] ?? "Regular"
                do { try model.engine.setSetting("view.density", json: "\"\(v)\"") } catch { print("DENSITY error \(error)") }
            },
            "appearance": { arg in
                NSApp.appearance = NSAppearance(named: arg == "dark" ? .darkAqua : .aqua)
            },
            "columns": { arg in
                guard let t = model.focusedTab else { return }
                var v = t.api.view()
                v.columns = arg.split(separator: ",").compactMap { ColumnID(rawValue: String($0)) }
                    .map { ColumnSpec(id: $0, width: $0.defaultWidth) }
                t.api.setView(v)
            },
            "sortcol": { arg in
                guard let t = model.focusedTab, let id = ColumnID(rawValue: arg),
                      let eng = model.engine as? ListEngineAPI else { return }
                _ = try? eng.sortByColumn(tab: t.id, column: id)
            },
            "headermenu": { _ in
                for l in active(model)?.headerMenuDescription() ?? [] { print("HEADERMENU \(l)") }
            },
            "ctx": { arg in
                guard let l = active(model) else { return }
                let items = l.activeFileTable.menuItems(row: Int(arg) ?? -1) ?? []
                for line in ContextMenuBuilder.describe(items) { print("CTXMENU \(line)") }
            },
            "deactivate": { _ in
                guard let l = active(model) else { return }
                l.scrollView.window?.makeFirstResponder(nil)
                l.activeFileTable.refreshActive()
            },
            "activate": { _ in
                guard let l = active(model) else { return }
                l.scrollView.window?.makeFirstResponder(l.table)
            },
            "listlog": { _ in listLog(model) },
            "viewtree": { @MainActor arg in
                guard let v = active(model)?.scrollView else { return }
                @MainActor func dump(_ v: NSView, _ d: Int) {
                    guard d <= (Int(arg) ?? 6) else { return }
                    let l = v.layer
                    print("TREE \(String(repeating: "  ", count: d))\(type(of: v)) frame=\(v.frame) hidden=\(v.isHidden) "
                          + "alpha=\(v.alphaValue) layer=\(l.map { "\($0.frame) h=\($0.isHidden) o=\($0.opacity) c=\($0.contents != nil) m=\($0.mask != nil)" } ?? "nil")")
                    if v is ColumnStrip || v is NSClipView {
                        for sl in l?.sublayers ?? [] {
                            print("TREE \(String(repeating: "  ", count: d))  sublayer \(type(of: sl)) delegate=\(sl.delegate.map { String(describing: type(of: $0)) } ?? "nil") frame=\(sl.frame) h=\(sl.isHidden) z=\(sl.zPosition)")
                        }
                    }
                    for s in v.subviews { dump(s, d + 1) }
                }
                dump(v, 0)
            },
            "snapview": { arg in
                // The pane's file area drawn through AppKit's drawing path (cacheDisplay).
                guard let v = active(model)?.scrollView,
                      let rep = v.bitmapImageRepForCachingDisplay(in: v.bounds) else { return }
                v.cacheDisplay(in: v.bounds, to: rep)
                let dir = UserDefaults.standard.string(forKey: "FSDebugSnapshotDir") ?? NSTemporaryDirectory()
                let url = URL(fileURLWithPath: dir).appendingPathComponent("\(arg).png")
                try? rep.representation(using: .png, properties: [:])?.write(to: url)
                print("SNAP \(url.path)")
            },
            "select": { arg in
                guard let l = active(model) else { return }
                l.activeFileTable.source?.setSelection(.names(arg.split(separator: "|").map(String.init)))
            },
            "state": { arg in
                // Forces a state overlay for a snapshot (loading is usually too quick to catch).
                guard let l = active(model) else { return }
                let st: ListStateView.State = switch arg {
                case "loading": .loading
                case "empty": .empty
                case "privacy": .error("/Users/x/Library/Mail: Operation not permitted (os error 1)")
                default: .none
                }
                l.activeFileTable.stateView.show(st)
            },
            "resize": { arg in
                let n = arg.split(separator: ",").compactMap { Double($0) }
                guard n.count == 2, let w = active(model)?.scrollView.window else { return }
                w.setContentSize(NSSize(width: n[0], height: n[1]))
            },
        ]
        for (k, v) in steps { DebugAutomation.extraSteps[k] = v }
    }

    // MARK: Logs

    private static func listLog(_ m: AppModel) {
        guard let l = active(m), let t = l.tab else { print("LIST none"); return }
        let ft = l.activeFileTable
        let vis = ft.table.rows(in: ft.contentRect)
        let v = t.api.view()
        print("LIST mode=\(v.mode) columns=\(v.columns.map { "\($0.id.rawValue):\(Int($0.width))" }) "
              + "sort=\(v.sort) asc=\(v.ascending) rows=\(ft.table.numberOfRows) visible=\(vis.location)+\(vis.length) "
              + "active=\(ft.isActive) state=\(ft.stateView.isHidden ? "rows" : "\(ft.stateView.state)") "
              + "rowHeight=\(ft.table.rowHeight) renaming=\(ft.rename?.isActive ?? false)")
        let sv = ft.scrollView
        print("LIST geometry scroll=\(sv.convert(sv.bounds, to: nil)) insets=\(sv.contentInsets.top) "
              + "header=\(ft.table.headerView.map { $0.convert($0.bounds, to: nil) } ?? .zero) "
              + "row0=\(ft.table.convert(ft.table.rect(ofRow: 0), to: nil)) clip=\(sv.contentView.convert(sv.contentView.bounds, to: nil))")
        print("LIST tableColumns=\(ft.table.tableColumns.map { "\($0.identifier.rawValue):\(Int($0.width))" }) "
              + "nsSelection=\(ft.table.selectedRowIndexes.count)")
        for i in vis.location..<min(vis.location + min(vis.length, 12), ft.table.numberOfRows) {
            if let r = ft.row(i) {
                print("LIST row \(i) \(r.isSelected ? "[x]" : "[ ]")\(r.isCursor ? ">" : " ") \(r.name) | \(r.sizeText) | "
                      + "\(r.modifiedText) | \(r.createdText) | \(r.kindText) | \(r.extText)"
                      + "\(r.isDataless ? " (cloud)" : "")\(r.isSymlink ? " (link)" : "")\(r.isHidden ? " (hidden)" : "")"
                      + (r.marksText.isEmpty ? "" : " | marks: \(r.marksText)\(r.hasHiddenSidecar ? " (sidecar hidden)" : "")"))
            }
        }
        if let c = l.columns, v.mode == .columns {
            print("LIST columnDirs=\(c.columnDirs.map { ($0 as NSString).lastPathComponent })")
            for g in c.columnGeometry { print("LIST column \(g)") }
        }
        if let rv = ft.table.rowView(atRow: max(vis.location, 0), makeIfNecessary: false) {
            print("LIST a11y row: \(rv.accessibilityLabel() ?? "") selected=\(rv.isAccessibilitySelected())")
        }
        let ql = QLPreviewPanel.sharedPreviewPanelExists() && (QLPreviewPanel.shared()?.isVisible ?? false)
        print("LIST quickLookVisible=\(ql) status=\(m.statusMessage ?? "")")
    }

    // MARK: Drops

    private static func drop(_ m: AppModel, _ arg: String, perform: Bool) {
        guard let l = active(m) else { return }
        let parts = arg.split(separator: "#", omittingEmptySubsequences: false).map(String.init)
        let srcs = parts[0].split(separator: "|").map { NSString(string: String($0)).expandingTildeInPath }
        let ft0 = l.activeFileTable
        let row = parts.count > 1 ? Int(parts[1]) ?? ft0.source.flatMap { $0.indexOf(name: parts[1]) }.map(Int.init) ?? -1 : -1
        var flags: NSEvent.ModifierFlags = []
        if parts.count > 2 { if parts[2] == "alt" { flags.insert(.option) }; if parts[2] == "cmd" { flags.insert(.command) } }
        let ft = l.activeFileTable
        let (effect, dest) = ft.dropEffect(srcs, row: row, flags: flags)
        print("DROP effect=\(effect) dest=\(dest ?? "-") sources=\(srcs.map { ($0 as NSString).lastPathComponent })")
        if perform { ft.performDrop(srcs, row: row, flags: flags) }
    }

    // MARK: Time to first rows

    private static func benchOpen(_ m: AppModel, _ path: String) async {
        guard let t = m.focusedTab else { return }
        let p = NSString(string: path).expandingTildeInPath
        FirstRowsProbe.arm()
        let clock = ContinuousClock()
        let t0 = clock.now
        do { m.handle(try t.api.go(p), in: m.keyWindowID) } catch { print("BENCH open error \(error)"); return }
        var complete: Duration?
        while clock.now - t0 < .seconds(10) {
            if complete == nil, t.listing.dir == p, t.listing.state == .complete { complete = clock.now - t0 }
            if FirstRowsProbe.result != nil, complete != nil { break }
            try? await Task.sleep(for: .milliseconds(1))
        }
        func fmt(_ d: Duration?) -> String { d.map { String(format: "%.1f ms", ListDebug.ms($0)) } ?? "n/a" }
        print("BENCH open \(p): first rows drawn after \(fmt(FirstRowsProbe.result)), listing complete after "
              + "\(fmt(complete)) (\(t.listing.count) rows)")
    }

    static func ms(_ d: Duration) -> Double {
        Double(d.components.seconds) * 1000 + Double(d.components.attoseconds) / 1e15
    }

    // MARK: Scroll benchmark

    private static func benchScroll(_ m: AppModel, _ arg: String) async {
        let parts = arg.split(separator: ":").map(String.init)
        let seconds = parts.count > 1 ? Double(parts[1]) ?? 3 : 3
        let mode = parts.count > 2 ? parts[2] : "smooth"
        let speed = parts.count > 3 ? Int32(parts[3]) ?? 12 : 12
        guard let l = active(m), let w = l.scrollView.window else { return }
        let ft = l.activeFileTable
        ft.scrollToTop()
        try? await Task.sleep(for: .milliseconds(300))
        let driver = ScrollDriver(table: ft, window: w, wheel: mode == "wheel", speed: speed)
        RowCache.fetchTime = .zero; RowCache.fetchCalls = 0
        CellStyle.configureTime = .zero; CellStyle.configureCalls = 0
        CellStyle.drawTime = .zero; CellStyle.drawCalls = 0
        RowCache.measuring = true
        driver.start()
        try? await Task.sleep(for: .seconds(seconds))
        driver.stop()
        RowCache.measuring = false
        let fps = Double(w.screen?.maximumFramesPerSecond ?? 60)
        let period = 1000 / fps
        var dts = driver.intervals.sorted()
        if dts.isEmpty { dts = [0] }
        func pct(_ p: Double) -> Double { dts[min(dts.count - 1, Int(Double(dts.count - 1) * p))] }
        let dropped = driver.intervals.reduce(0) { $0 + max(0, Int(($1 / period).rounded()) - 1) }
        let expected = Int(seconds * fps)
        let rowsMoved = Int(ft.table.rows(in: ft.contentRect).location)
        print(String(format: "BENCH scroll %@ (%d) %.0fs on %d rows at %.0f Hz: %d frames (expected ~%d), frame time p50 %.2f ms, "
                     + "p95 %.2f ms, p99 %.2f ms, max %.2f ms, dropped %d (%.1f%%), scrolled to row %d",
                     mode, Int(speed), seconds, ft.table.numberOfRows, fps, driver.intervals.count, expected, pct(0.5), pct(0.95),
                     pct(0.99), dts.last!, dropped, 100 * Double(dropped) / Double(max(expected, 1)), rowsMoved))
        print(String(format: "BENCH   main thread: row fetching %.2f ms (%d blocks of %d), cell configuration %.2f ms (%d cells), "
                     + "cell drawing %.2f ms (%d cells), per frame %.3f ms",
                     ms(RowCache.fetchTime), RowCache.fetchCalls, Int(RowCache.block), ms(CellStyle.configureTime),
                     CellStyle.configureCalls, ms(CellStyle.drawTime), CellStyle.drawCalls,
                     (ms(RowCache.fetchTime) + ms(CellStyle.configureTime) + ms(CellStyle.drawTime))
                        / Double(max(driver.intervals.count, 1))))
        print(String(format: "BENCH   main-thread work per frame (event + layout + draw): p50 %.2f ms, p99 %.2f ms",
                     driver.busyPercentile(0.5), driver.busyPercentile(0.99)))
    }
}

/// Drives scrolling from a display link with synthetic scroll-wheel events
/// (the same path as a real mouse): notched wheel steps, or LinearMouse-style
/// small continuous deltas every frame.
@MainActor
final class ScrollDriver: NSObject {
    private unowned let ft: FileTable
    private unowned let window: NSWindow
    private let wheel: Bool
    private var link: CADisplayLink?
    private var last: CFTimeInterval = 0
    private var tick = 0
    private(set) var intervals: [Double] = []
    private var busy: [Double] = []

    private let speed: Int32

    /// `speed`: pixels per frame (smooth) or lines per notch (wheel).
    init(table: FileTable, window: NSWindow, wheel: Bool, speed: Int32) {
        self.ft = table; self.window = window; self.wheel = wheel; self.speed = speed
    }

    func start() {
        let l = window.displayLink(target: self, selector: #selector(frame(_:)))
        l.add(to: .main, forMode: .common)
        link = l
    }

    func stop() {
        link?.invalidate()
        link = nil
        post(phaseEnd: true)
    }

    func busyPercentile(_ p: Double) -> Double {
        let s = busy.sorted()
        return s.isEmpty ? 0 : s[min(s.count - 1, Int(Double(s.count - 1) * p))]
    }

    @objc private func frame(_ l: CADisplayLink) {
        if last > 0 { intervals.append((l.timestamp - last) * 1000) }
        last = l.timestamp
        tick += 1
        let t0 = CACurrentMediaTime()
        if wheel {
            // A notched wheel: ~15 notches a second, 3 lines each.
            if tick % max(Int((1 / max(l.duration, 1.0 / 240)) / 15), 1) == 0 { post(lines: -max(speed / 4, 3)) }
        } else {
            // LinearMouse smooth: a small pixel delta every frame, continuous phase.
            post(pixels: -speed)
        }
        // Lay out and draw now, so the measured time is this frame's main-thread work.
        ft.scrollView.displayIfNeeded()
        busy.append((CACurrentMediaTime() - t0) * 1000)
    }


    private func post(lines: Int32 = 0, pixels: Int32 = 0, phaseEnd: Bool = false) {
        let units: CGScrollEventUnit = lines != 0 ? .line : .pixel
        guard let cg = CGEvent(scrollWheelEvent2Source: nil, units: units, wheelCount: 1,
                               wheel1: lines != 0 ? lines : pixels, wheel2: 0, wheel3: 0) else { return }
        let r = ft.scrollView.convert(ft.scrollView.bounds, to: nil)
        let p = window.convertPoint(toScreen: NSPoint(x: r.midX, y: r.midY))
        cg.location = CGPoint(x: p.x, y: (NSScreen.screens.first?.frame.height ?? 0) - p.y)
        if lines == 0 {
            // LinearMouse's smooth mode: continuous pixel deltas, no gesture phases.
            cg.setIntegerValueField(.scrollWheelEventIsContinuous, value: 1)
            cg.setIntegerValueField(.scrollWheelEventPointDeltaAxis1, value: Int64(pixels))
            cg.setDoubleValueField(.scrollWheelEventFixedPtDeltaAxis1, value: Double(pixels))
            _ = phaseEnd
        }
        // Straight to the scroll view (the window may not be frontmost in a script run).
        if let e = NSEvent(cgEvent: cg) { ft.scrollView.scrollWheel(with: e) }
    }
}

#endif

// WindowDebugSteps.swift — DEBUG-only script steps for the window layer
// (registered with DebugAutomation):
//
//   winlog                    WINDOW (layout, panes, tab bars, toolbar) and SIDEBAR lines
//   sidebarmenu:<path>        the core's context menu for a sidebar row (SIDEBARMENU lines)
//   layout:<horizontal|vertical>[,<ratio>]   setLayout on the key window
//   ratio:<0…1>               as if the divider were dragged there
//   newtab:<path>             opens a tab in the active pane
//   movetab:<from>,<to>[,other]   moves the active pane's tab <from> to index <to>
//                             (in the other pane with `other`), as a tab drag would
//   tabbar:<always|auto>      the "always show the tab bar" preference
//   setup:<page>              shows first-run setup at a page (0-based); setup:close closes it
//   button:<n>,<x>,<y>        presses mouse button n (2 middle, 3 back, 4 forward) at a window
//                             point, through the window's mouse handler (synthetic button
//                             events never reach local event monitors)
//   focus:<sidebar|filter|list|sidebarfilter>   makes that view the first responder
//   eject:<mount path>        presses the eject button on that volume's sidebar row
//   reducemotion              reports the system's Reduce Motion setting
//   sidebarsnap:<name>        draws the sidebar alone to <SnapshotDir>/<name>.png over the
//                             window background (window snaps draw the sidebar material blank)

#if DEBUG
import AppKit

@MainActor
enum WindowDebugSteps {
    private static var installed = false

    static func install() {
        guard !installed else { return }
        installed = true
        DebugAutomation.register("winlog") { _, m in log(m) }
        DebugAutomation.register("sidebarmenu") { path, _ in
            for line in controller?.sidebar.describeMenu(path: path) ?? [] { print("SIDEBARMENU \(line)") }
        }
        DebugAutomation.register("layout") { arg, m in
            guard let c = controller else { return }
            let parts = arg.split(separator: ",")
            let o: Orientation = parts.first == "vertical" ? .vertical : .horizontal
            let r = parts.count > 1 ? Double(parts[1]) ?? 0.5 : c.windowModel.state.splitRatio
            do { try m.engine.setLayout(window: c.coreWindowID, orientation: o, splitRatio: r) } catch { print("DEBUG \(error)") }
        }
        DebugAutomation.register("ratio") { arg, _ in controller?.debugSetRatio(Double(arg) ?? 0.5) }
        DebugAutomation.register("newtab") { arg, m in
            guard let c = controller, let p = c.activePane else { return }
            do { _ = try m.engine.openTab(inPane: p.paneID, path: arg.isEmpty ? nil : arg, index: nil) } catch { print("DEBUG \(error)") }
        }
        DebugAutomation.register("movetab") { arg, m in
            guard let c = controller else { return }
            let parts = arg.split(separator: ",").map(String.init)
            let st = c.windowModel.state
            guard parts.count >= 2, let from = Int(parts[0]), let to = Int(parts[1]),
                  st.panes.indices.contains(st.activePane) else { return }
            let src = st.panes[st.activePane]
            guard src.tabs.indices.contains(from) else { return }
            let dest = parts.count > 2 && parts[2] == "other" && st.panes.count == 2 ? st.panes[1 - st.activePane] : src
            do { try m.engine.moveTab(src.tabs[from].id, toPane: dest.id, index: UInt32(to)) } catch { print("DEBUG \(error)") }
        }
        DebugAutomation.register("tabbar") { arg, _ in WindowPreferences.tabBarAlways = arg == "always" }
        DebugAutomation.register("setup") { arg, m in
            if arg == "close" { SetupController.finish(model: m); return }
            SetupController.present(model: m, page: Int(arg) ?? 0)
        }
        DebugAutomation.register("sidebarsnap") { name, _ in sidebarSnap(name) }
        DebugAutomation.register("button") { arg, _ in pressButton(arg) }
        DebugAutomation.register("focus") { arg, _ in
            guard let c = controller, let w = c.window else { return }
            let v: NSView? = switch arg {
            case "sidebar": c.sidebar.outline
            case "sidebarfilter": c.sidebar.filterField
            case "list": c.activePane?.focusView
            case "filter": w.toolbar?.items.compactMap { ($0 as? NSSearchToolbarItem)?.searchField }.first
            default: nil
            }
            if let v { w.makeFirstResponder(v) }
        }
        DebugAutomation.register("eject") { arg, _ in
            print("EJECT pressed=\(controller?.sidebar.pressEject(mountPath: arg) ?? false)")
        }
        DebugAutomation.register("reducemotion") { _, _ in print("REDUCEMOTION \(Motion.reduce)") }
    }

    /// A real mouse-button event (via CGEvent) sent through NSApp, like hardware input.
    private static func pressButton(_ arg: String) {
        let n = arg.split(separator: ",").compactMap { Double($0) }
        guard n.count == 3, let w = controller?.window, let content = w.contentView,
              let screen = NSScreen.screens.first else { return }
        let inWindow = NSPoint(x: n[1], y: content.bounds.height - n[2])
        let onScreen = w.convertPoint(toScreen: inWindow)
        let cgPoint = CGPoint(x: onScreen.x, y: screen.frame.height - onScreen.y)
        if n[0] == 2 {
            // AppKit can't build a synthetic event with a button number, so this
            // calls the row's middle-click action for the row under the point.
            if let c = controller, let hit = w.contentView?.superview?.hitTest(inWindow) as? SidebarOutlineView {
                let row = hit.row(at: hit.convert(inWindow, from: nil))
                print("BUTTON 2 sidebar row=\(row)")
                if row >= 0 { hit.onMiddleClick?(row) }
                _ = c
            } else {
                print("BUTTON 2 not over the sidebar")
            }
            return
        }
        let button = CGMouseButton(rawValue: UInt32(n[0]))!
        for type in [CGEventType.otherMouseDown, .otherMouseUp] {
            guard let cg = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: cgPoint, mouseButton: button)
            else { continue }
            cg.setIntegerValueField(.mouseEventButtonNumber, value: Int64(n[0]))
            cg.setIntegerValueField(.mouseEventWindowUnderMousePointer, value: Int64(w.windowNumber))
            cg.setIntegerValueField(.mouseEventWindowUnderMousePointerThatCanHandleThisEvent, value: Int64(w.windowNumber))
            if let e = NSEvent(cgEvent: cg) {
                print("BUTTON \(e.buttonNumber) \(e.type == .otherMouseDown ? "down" : "up") window=\(e.window === w)")
                controller?.debugMouse(e)
            }
        }
    }

    private static var controller: MainWindowController? {
        (NSApp.keyWindow?.windowController as? MainWindowController)
            ?? NSApp.windows.compactMap { $0.windowController as? MainWindowController }.first
    }

    /// Draws the sidebar's own layer tree over the window background.
    private static func sidebarSnap(_ name: String) {
        guard let c = controller else { return }
        let v = c.sidebar.view
        v.layoutSubtreeIfNeeded()
        v.displayIfNeeded()
        let dir = URL(fileURLWithPath: UserDefaults.standard.string(forKey: "FSDebugSnapshotDir") ?? NSTemporaryDirectory())
        let scale = v.window?.backingScaleFactor ?? 2
        let size = v.bounds.size
        guard let layer = v.layer, let ctx = CGContext(
            data: nil, width: Int(size.width * scale), height: Int(size.height * scale), bitsPerComponent: 8,
            bytesPerRow: 0, space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue)
        else { return }
        var bg = NSColor.windowBackgroundColor.cgColor
        v.effectiveAppearance.performAsCurrentDrawingAppearance { bg = NSColor.windowBackgroundColor.cgColor }
        ctx.setFillColor(bg)
        ctx.fill(CGRect(x: 0, y: 0, width: size.width * scale, height: size.height * scale))
        ctx.scaleBy(x: scale, y: scale)
        if !layer.isGeometryFlipped && v.isFlipped { ctx.translateBy(x: 0, y: size.height); ctx.scaleBy(x: 1, y: -1) }
        func walk(_ l: CALayer, _ d: Int) {
            let owner = l.delegate.map { String(describing: type(of: $0)) } ?? "-"
            if d < 9 { print("LAYER \(String(repeating: " ", count: d))\(type(of: l)) \(owner) hidden=\(l.isHidden) bg=\(l.backgroundColor != nil) filters=\(l.filters?.count ?? 0) \(NSStringFromRect(l.frame))") }
            for c in l.sublayers ?? [] { walk(c, d + 1) }
        }
        if ProcessInfo.processInfo.environment["FS_LAYERS"] != nil { walk(layer, 0) }
        // Backdrops and the rows' vibrant background layers render opaque off-screen.
        func all(_ l: CALayer) -> [CALayer] { [l] + (l.sublayers ?? []).flatMap(all) }
        let skip = all(layer).filter { l in
            String(describing: type(of: l)).contains("Backdrop") || (l.delegate == nil && l.backgroundColor != nil && type(of: l) == CALayer.self)
        }
        let was = skip.map(\.isHidden)
        skip.forEach { $0.isHidden = true }
        layer.render(in: ctx)
        zip(skip, was).forEach { $0.isHidden = $1 }
        guard let img = ctx.makeImage(),
              let png = NSBitmapImageRep(cgImage: img).representation(using: .png, properties: [:]) else { return }
        let url = dir.appendingPathComponent("\(name).png")
        try? png.write(to: url)
        print("SNAP \(url.path)")
    }

    private static func json(_ o: Any) -> String {
        (try? JSONSerialization.data(withJSONObject: o, options: [.sortedKeys]))
            .flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
    }

    private static func log(_ m: AppModel) {
        for c in NSApp.windows.compactMap({ $0.windowController as? MainWindowController }) {
            print("WINDOW " + json(c.debugInfo))
            for r in c.sidebar.debugRows { print("SIDEBAR \(r)") }
            let o = c.sidebar.outline
            print("SIDEBARVIEW outline=\(NSStringFromRect(o.frame)) clip=\(NSStringFromRect(o.enclosingScrollView?.contentView.frame ?? .zero)) col=\(o.tableColumns.first?.width ?? -1) rows=\(o.numberOfRows)")
            let fr = c.window?.firstResponder
            print("FOCUS \(fr.map { String(describing: type(of: $0)) } ?? "nil") activePane=\(c.windowModel.state.activePane)")
            var chain: [String] = []
            var k: NSView? = c.sidebar.outline
            for _ in 0..<8 { guard let cur = k else { break }; chain.append(String(describing: type(of: cur))); k = cur.nextValidKeyView; if k === c.sidebar.outline { break } }
            print("KEYLOOP \(chain.joined(separator: " → "))")
        }
        if let s = SetupController.state {
            print("SETUP page=\(s.page.rawValue) index=\(s.index) pages=\(s.pages.map(\.rawValue)) fda=\(s.hasFullDiskAccess)")
        }
        print("SETTINGS profile=\(m.settings.keyboard.profile) dualDefault=\(m.settings.general.dualPaneByDefault)")
    }
}
#endif

// DebugAutomation.swift — DEBUG-only self-test driver (port of Import Studio's).
// Replays synthetic key and mouse events through NSApp.sendEvent (the same
// path as real input: KeyRouter → core, then menus and the focused view) and
// renders the app's own windows to PNG. No Accessibility or Screen Recording
// permission is needed.
//
//   "File Studio" -FSDebugScript "wait:1;key:cmd+l;type:~/Documents;key:return;wait:0.5;snap:docs;log;quit" \
//                 -FSDebugSnapshotDir /tmp/shots [-FSMockCore] [-FSDataDir /tmp/fs-data]
//
// Steps (separated by `;`):
//   wait:<s>              pause
//   key:[shift+|cmd+|alt+|ctrl+]<name>   one key (names: a–z, 0–9, return, tab, space, delete,
//                         esc, up/down/left/right, home/end, pageup/pagedown, f1–f12, / . , [ ] = - \)
//   type:<text>           type text: inserted into the focused text field, else sent as key presses
//   click:<x>,<y>         left click (main window content points from the top-left)
//   dclick:<x>,<y>        double click
//   rclick:<x>,<y>        logs the context menu a right-click there would show (MENU lines)
//   action:<selector:>    sends an action up the main window's responder chain (`undo:`), as
//                         a menu would when the app is active (scripted runs often aren't)
//   hover:<x>,<y>         mouseEntered + mouseMoved to the views under the point that track the mouse
//   scroll:<dx>,<dy>      scroll wheel over the window's centre (pixels)
//   drag:<x>,<y>,<dx>,<dy>
//   go:<path>             the focused tab goes to a path (as typed in the address bar)
//   snap:<name>           <SnapshotDir>/<name>.png for each visible window (SNAP lines)
//   log                   JSON lines: windows, tabs, the focused listing and selection (STATE lines)
//   menus                 the main menu's items with key equivalents (MAINMENU lines)
//   bench:rows            times TabAPI.rows(start:, 60) from Swift (BENCH line)
//   quit

#if DEBUG
import AppKit

@MainActor
enum DebugAutomation {
    static func runIfRequested(_ model: AppModel) {
        guard let script = UserDefaults.standard.string(forKey: "FSDebugScript") else { return }
        let dir = URL(fileURLWithPath: UserDefaults.standard.string(forKey: "FSDebugSnapshotDir")
                      ?? NSTemporaryDirectory())
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        Task { @MainActor in
            try? await Task.sleep(for: .seconds(1))
            NSApp.activate()
            mainWindow?.makeKeyAndOrderFront(nil)
            for raw in script.split(separator: ";") {
                let step = raw.trimmingCharacters(in: .whitespaces)
                let parts = step.split(separator: ":", maxSplits: 1).map(String.init)
                let op = parts.first ?? "", arg = parts.count > 1 ? parts[1] : ""
                print("STEP \(step)")
                switch op {
                case "wait": try? await Task.sleep(for: .seconds(Double(arg) ?? 1))
                case "key": sendKey(arg)
                case "type": typeText(arg)
                case "click": click(arg, count: 1)
                case "dclick": click(arg, count: 2)
                case "rclick": rightClick(arg)
                case "hover": hover(arg)
                case "action":
                    let ok = mainWindow?.firstResponder?.tryToPerform(Selector(arg), with: nil) ?? false
                    print("ACTION \(arg) \(ok ? "handled" : "not handled")")
                case "drag": drag(arg)
                case "scroll": scroll(arg)
                case "go":
                    if let t = model.focusedTab {
                        do { model.handle(try t.api.go(arg), in: model.keyWindowID) } catch { print("GO error \(error)") }
                    }
                case "log": log(model)
                case "menus": dumpMenus()
                case "bench": if arg == "rows" { benchRows(model) }
                case "snap": snap(arg, dir: dir)
                case "quit": NSApp.terminate(nil)
                default: print("DEBUG unknown step \(step)")
                }
                fflush(stdout)
                try? await Task.sleep(for: .milliseconds(150))
            }
        }
    }

    private static var mainWindow: NSWindow? {
        NSApp.keyWindow.flatMap { $0.windowController is CoreWindowIdentifying ? $0 : nil }
            ?? NSApp.windows.first { $0.windowController is CoreWindowIdentifying && $0.isVisible }
    }

    // MARK: State

    private static func json(_ o: [String: Any]) -> String {
        (try? JSONSerialization.data(withJSONObject: o, options: [.sortedKeys]))
            .flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
    }

    private static func log(_ m: AppModel) {
        print("STATE " + json(["type": "app", "engine": m.engine.implementationName, "windows": m.windows.count,
                               "tabs": m.tabs.count, "ops": m.ops.count, "status": m.statusMessage ?? "",
                               "keyWindow": m.keyWindowID.map { Int($0) } ?? -1, "appActive": NSApp.isActive,
                               "nsKeyWindow": NSApp.keyWindow.map { String(describing: type(of: $0)) } ?? "nil",
                               "firstResponder": mainWindow?.firstResponder.map { String(describing: type(of: $0)) } ?? "nil"]))
        for w in m.workspace.windows {
            print("STATE " + json(["type": "window", "id": Int(w.id), "panes": w.panes.count, "activePane": w.activePane,
                                   "tabs": w.panes.map { $0.tabs.map { $0.location } }]))
        }
        if let t = m.focusedTab {
            let l = t.listing
            let sel = t.api.selection()
            print("STATE " + json(["type": "tab", "id": Int(t.id), "location": t.location, "canGoBack": t.canGoBack]))
            print("STATE " + json(["type": "listing", "version": Int(l.version), "dir": l.dir, "count": Int(l.count),
                                   "total": Int(l.totalCount), "state": "\(l.state)", "error": l.error ?? "",
                                   "first": t.api.rows(start: 0, count: 5).map(\.name)]))
            print("STATE " + json(["type": "selection", "rows": sel.rows.prefix(50).map { Int($0) },
                                   "names": Array(sel.names.prefix(50)), "cursor": sel.cursor.map { Int($0) } ?? -1]))
        }
    }

    private static func dumpMenus() {
        for top in NSApp.mainMenu?.items ?? [] {
            for it in top.submenu?.items ?? [] where !it.isSeparatorItem {
                print("MAINMENU \(top.title) › \(it.title) key=\(it.keyEquivalent.debugDescription) "
                      + "mods=\(it.keyEquivalentModifierMask.rawValue)")
            }
        }
    }

    private static func dump(_ menu: NSMenu, depth: Int = 0) {
        for it in menu.items {
            let pad = String(repeating: "  ", count: depth)
            if it.isSeparatorItem { print("MENU \(pad)---"); continue }
            let key = it.keyEquivalent.isEmpty ? "" : "  [\(it.keyEquivalentModifierMask.rawValue)+\(it.keyEquivalent.debugDescription)]"
            print("MENU \(pad)\(it.state == .on ? "✓ " : "")\(it.title)\(key)\(it.isEnabled ? "" : "  (disabled)")")
            if let sub = it.submenu { dump(sub, depth: depth + 1) }
        }
    }

    /// The Swift-side cost of one `rows(start:count: 60)` call, FFI included.
    private static func benchRows(_ m: AppModel) {
        guard let t = m.focusedTab else { return }
        let n = max(Int(t.listing.count) - 60, 1)
        _ = t.api.rows(start: 0, count: 60)
        let iterations = 2000
        let clock = ContinuousClock()
        func us(_ d: Duration) -> Double {
            (Double(d.components.seconds) * 1e6 + Double(d.components.attoseconds) / 1e12) / Double(iterations)
        }
        let d = clock.measure {
            for k in 0..<iterations { _ = t.api.rows(start: UInt32((k * 37) % n), count: 60) }
        }
        print(String(format: "BENCH rows(start, 60) from Swift: %.1f µs per call (%d rows in folder, %d calls)",
                     us(d), Int(t.listing.count), iterations))
        if let rt = t.api as? RustTab {
            let raw = clock.measure {
                for k in 0..<iterations { _ = rt.ffi.rows(start: UInt32((k * 37) % n), count: 60) }
            }
            print(String(format: "BENCH   generated record lifting instead (ffi.rows): %.1f µs", us(raw)))
        }
    }

    // MARK: Keys

    private static let codes: [String: UInt16] = [
        "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11, "q": 12,
        "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21, "6": 22, "5": 23,
        "9": 25, "7": 26, "8": 28, "0": 29, "]": 30, "o": 31, "u": 32, "[": 33, "i": 34, "p": 35, "l": 37,
        "j": 38, "k": 40, "n": 45, "m": 46, "=": 24, "-": 27, "/": 44, ".": 47, ",": 43, "\\": 42, ";": 41,
        "'": 39, "`": 50,
        "return": 36, "tab": 48, "space": 49, "delete": 51, "esc": 53, "home": 115, "pageup": 116,
        "end": 119, "pagedown": 121, "left": 123, "right": 124, "down": 125, "up": 126,
        "f1": 122, "f2": 120, "f3": 99, "f4": 118, "f5": 96, "f6": 97, "f7": 98, "f8": 100, "f9": 101,
        "f10": 109, "f11": 103, "f12": 111,
    ]
    private static let specialChars: [String: Int] = [
        "left": NSLeftArrowFunctionKey, "right": NSRightArrowFunctionKey, "up": NSUpArrowFunctionKey,
        "down": NSDownArrowFunctionKey, "home": NSHomeFunctionKey, "end": NSEndFunctionKey,
        "pageup": NSPageUpFunctionKey, "pagedown": NSPageDownFunctionKey,
        "f1": NSF1FunctionKey, "f2": NSF2FunctionKey, "f3": NSF3FunctionKey, "f4": NSF4FunctionKey,
        "f5": NSF5FunctionKey, "f6": NSF6FunctionKey, "f7": NSF7FunctionKey, "f8": NSF8FunctionKey,
        "f9": NSF9FunctionKey, "f10": NSF10FunctionKey, "f11": NSF11FunctionKey, "f12": NSF12FunctionKey,
    ]

    private static func sendKey(_ spec: String) {
        var parts = spec.lowercased().split(separator: "+", omittingEmptySubsequences: false).map(String.init)
        // "cmd++" style is not supported; "+" itself isn't a key here.
        let name = parts.removeLast()
        var flags: NSEvent.ModifierFlags = []
        for p in parts {
            switch p {
            case "shift": flags.insert(.shift)
            case "cmd": flags.insert(.command)
            case "alt": flags.insert(.option)
            case "ctrl": flags.insert(.control)
            default: break
            }
        }
        guard let code = codes[name] else { print("DEBUG no key \(spec)"); return }
        let chars: String
        if let s = specialChars[name] {
            chars = String(Character(UnicodeScalar(UInt32(s))!))
            flags.insert(.function)
            if ["left", "right", "up", "down"].contains(name) { flags.insert(.numericPad) }
        } else {
            chars = switch name {
            case "return": "\r"
            case "tab": "\t"
            case "space": " "
            case "delete": "\u{7f}"
            case "esc": "\u{1b}"
            default: flags.contains(.shift) ? name.uppercased() : name
            }
        }
        post(chars: chars, code: code, flags: flags)
    }

    private static func post(chars: String, code: UInt16, flags: NSEvent.ModifierFlags) {
        guard let w = NSApp.keyWindow ?? mainWindow else { return }
        for type in [NSEvent.EventType.keyDown, .keyUp] {
            if let e = NSEvent.keyEvent(with: type, location: .zero, modifierFlags: flags,
                                        timestamp: ProcessInfo.processInfo.systemUptime,
                                        windowNumber: w.windowNumber, context: nil, characters: chars,
                                        charactersIgnoringModifiers: chars, isARepeat: false, keyCode: code) {
                // Through the event queue, like real input: `NSApp.currentEvent`
                // is then this event while it's handled (menus and timing rely on it).
                NSApp.postEvent(e, atStart: false)
            }
        }
    }

    /// Text into the focused field, or key presses (type-to-select) elsewhere.
    private static func typeText(_ text: String) {
        if let tv = (NSApp.keyWindow ?? mainWindow)?.firstResponder as? NSTextView {
            tv.insertText(text, replacementRange: tv.selectedRange())
            // A real keystroke or paste arrives in an event, and the undo group it
            // opens closes when that event is done; give scripted text an event too.
            if let e = NSEvent.otherEvent(with: .applicationDefined, location: .zero, modifierFlags: [],
                                          timestamp: ProcessInfo.processInfo.systemUptime,
                                          windowNumber: 0, context: nil, subtype: 0, data1: 0, data2: 0) {
                NSApp.postEvent(e, atStart: false)
            }
            return
        }
        for ch in text {
            let s = String(ch)
            post(chars: s, code: codes[s.lowercased()] ?? 0, flags: ch.isUppercase ? .shift : [])
        }
    }

    // MARK: Mouse

    private static func point(_ arg: String, in w: NSWindow) -> NSPoint? {
        let n = arg.split(separator: ",").compactMap { Double($0.trimmingCharacters(in: .whitespaces)) }
        guard n.count >= 2, let content = w.contentView else { return nil }
        return NSPoint(x: n[0], y: content.bounds.height - n[1])
    }

    private static func mouse(_ type: NSEvent.EventType, _ p: NSPoint, _ w: NSWindow, count: Int) -> NSEvent? {
        NSEvent.mouseEvent(with: type, location: p, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                           windowNumber: w.windowNumber, context: nil, eventNumber: 0, clickCount: count, pressure: 1)
    }

    private static func hitView(_ p: NSPoint, _ w: NSWindow) -> NSView? {
        w.contentView?.superview?.hitTest(p) ?? w.contentView?.hitTest(p)
    }

    private static func click(_ arg: String, count: Int) {
        guard let w = mainWindow, let p = point(arg, in: w) else { return }
        print("HIT \(p) -> \(hitView(p, w).map { String(describing: type(of: $0)) } ?? "nil")")
        for c in 1...count {
            if let e = mouse(.leftMouseDown, p, w, count: c) { w.sendEvent(e) }
            if let e = mouse(.leftMouseUp, p, w, count: c) { w.sendEvent(e) }
        }
    }

    /// Asks the view under the point for its context menu and prints it
    /// (popping it up would block in the menu's tracking loop).
    private static func rightClick(_ arg: String) {
        guard let w = mainWindow, let p = point(arg, in: w), let e = mouse(.rightMouseDown, p, w, count: 1) else { return }
        var v = hitView(p, w)
        while let cur = v {
            if let m = cur.menu(for: e) {
                print("MENU for \(String(describing: type(of: cur))) at \(arg)")
                dump(m)
                return
            }
            v = cur.superview
        }
        print("MENU none at \(arg)")
    }

    /// Tracking areas only see real mouse movement; tell the views under the
    /// point directly (hover looks in snapshots).
    private static func hover(_ arg: String) {
        guard let w = mainWindow, let p = point(arg, in: w), let e = mouse(.mouseMoved, p, w, count: 0) else { return }
        var v = hitView(p, w)
        while let cur = v {
            // Only the app's own views: AppKit's expect real enter/exit events.
            let name = String(describing: type(of: cur))
            if !cur.trackingAreas.isEmpty, !name.hasPrefix("NS"), !name.hasPrefix("_") {
                cur.mouseEntered(with: e)
                cur.mouseMoved(with: e)
            }
            v = cur.superview
        }
        print("HOVER \(arg) -> \(hitView(p, w).map { String(describing: type(of: $0)) } ?? "nil")")
    }

    private static func drag(_ arg: String) {
        let n = arg.split(separator: ",").compactMap { Double($0) }
        guard n.count == 4, let w = mainWindow, let p = point("\(n[0]),\(n[1])", in: w) else { return }
        if let e = mouse(.leftMouseDown, p, w, count: 1) { w.sendEvent(e) }
        for k in 1...10 {
            let q = NSPoint(x: p.x + n[2] * Double(k) / 10, y: p.y - n[3] * Double(k) / 10)
            if let e = mouse(.leftMouseDragged, q, w, count: 1) { w.sendEvent(e) }
        }
        if let e = mouse(.leftMouseUp, NSPoint(x: p.x + n[2], y: p.y - n[3]), w, count: 1) { w.sendEvent(e) }
    }

    private static func scroll(_ arg: String) {
        let n = arg.split(separator: ",").compactMap { Int32($0) }
        guard n.count == 2, let w = mainWindow,
              let cg = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 2, wheel1: n[1], wheel2: n[0], wheel3: 0)
        else { return }
        let content = w.contentView!.bounds
        cg.location = CGPoint(x: w.frame.minX + content.midX,
                              y: (NSScreen.screens.first?.frame.height ?? 0) - (w.frame.minY + content.midY))
        if let e = NSEvent(cgEvent: cg) { w.sendEvent(e) }
    }

    // MARK: Snapshots

    /// Draws a child window's content at its place over the parent (with a
    /// popover-like background, since materials don't render off-screen).
    private static func renderChild(_ child: NSWindow, of w: NSWindow, into ctx: CGContext, rootSize: NSSize, flipped: Bool) {
        guard let cv = child.contentView, let layer = cv.layer else { return }
        let origin = NSPoint(x: child.frame.minX - w.frame.minX, y: child.frame.minY - w.frame.minY)
        ctx.saveGState()
        // The parent was drawn flipped when its view is flipped; undo that for the child.
        if flipped { ctx.translateBy(x: 0, y: rootSize.height); ctx.scaleBy(x: 1, y: -1) }
        ctx.translateBy(x: origin.x, y: origin.y)
        let r = CGRect(origin: .zero, size: child.frame.size)
        var bg = NSColor.windowBackgroundColor.cgColor
        child.effectiveAppearance.performAsCurrentDrawingAppearance { bg = NSColor.windowBackgroundColor.cgColor }
        ctx.setShadow(offset: CGSize(width: 0, height: -3), blur: 12, color: NSColor.black.withAlphaComponent(0.35).cgColor)
        ctx.setFillColor(bg)
        ctx.addPath(CGPath(roundedRect: r, cornerWidth: 8, cornerHeight: 8, transform: nil))
        ctx.fillPath()
        ctx.setShadow(offset: .zero, blur: 0)
        if cv.isFlipped && !layer.isGeometryFlipped { ctx.translateBy(x: 0, y: r.height); ctx.scaleBy(x: 1, y: -1) }
        let backdrops = allLayers(layer).filter {
            let n = String(describing: type(of: $0))
            return n.contains("Backdrop") || n.contains("VisualEffect")
        }
        let was = backdrops.map(\.isHidden)
        backdrops.forEach { $0.isHidden = true }
        layer.render(in: ctx)
        zip(backdrops, was).forEach { $0.isHidden = $1 }
        ctx.restoreGState()
    }

    private static func allLayers(_ l: CALayer) -> [CALayer] {
        [l] + (l.sublayers ?? []).flatMap { allLayers($0) }
    }

    /// Renders each visible window of this app (only) to PNG through its own
    /// layer tree (SwiftUI content included; system materials draw flat).
    private static func snap(_ name: String, dir: URL) {
        // Child windows (popovers, completion lists) are drawn into their parent's image.
        let tops = NSApp.windows.filter { $0.isVisible && $0.frame.width > 200 && $0.parent == nil }
        for (i, w) in tops.enumerated() {
            guard let view = w.contentView?.superview ?? w.contentView else { continue }
            view.layoutSubtreeIfNeeded()
            view.displayIfNeeded()
            let scale = w.backingScaleFactor
            let size = view.bounds.size
            guard let ctx = CGContext(data: nil, width: Int(size.width * scale), height: Int(size.height * scale),
                                      bitsPerComponent: 8, bytesPerRow: 0,
                                      space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                      bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue
                                        | CGBitmapInfo.byteOrder32Little.rawValue) else { continue }
            var bg = NSColor.windowBackgroundColor.cgColor
            w.effectiveAppearance.performAsCurrentDrawingAppearance { bg = NSColor.underPageBackgroundColor.cgColor }
            ctx.setFillColor(bg)
            ctx.fill(CGRect(x: 0, y: 0, width: size.width * scale, height: size.height * scale))
            ctx.scaleBy(x: scale, y: scale)
            if let layer = view.layer {
                if !layer.isGeometryFlipped && view.isFlipped {
                    ctx.translateBy(x: 0, y: size.height); ctx.scaleBy(x: 1, y: -1)
                }
                // Material backdrops render as opaque white off-screen; hide them
                // so the window background shows through.
                let backdrops = allLayers(layer).filter {
                    let n = String(describing: type(of: $0))
                    return n.contains("Backdrop") || n.contains("VisualEffect")
                }
                let was = backdrops.map(\.isHidden)
                backdrops.forEach { $0.isHidden = true }
                layer.render(in: ctx)
                zip(backdrops, was).forEach { $0.isHidden = $1 }
            }
            for child in (w.childWindows ?? []) where child.isVisible {
                renderChild(child, of: w, into: ctx, rootSize: size, flipped: view.isFlipped && !(view.layer?.isGeometryFlipped ?? false))
            }
            guard let img = ctx.makeImage() else { continue }
            let url = dir.appendingPathComponent("\(name)\(i == 0 ? "" : "-w\(i)").png")
            try? NSBitmapImageRep(cgImage: img).representation(using: .png, properties: [:])?.write(to: url)
            print("SNAP \(url.path)")
        }
    }
}
#endif

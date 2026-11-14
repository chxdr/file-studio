// UsageDebugSteps.swift — debug-automation steps for the disk usage window
// (DEBUG only; see App/DebugAutomation.swift for the script format).
//
//   usage:<path>            open the usage window for a folder (as Show Disk Usage does)
//   usagewait               wait until the front usage window's scan has finished (≤ 120 s)
//   usagelog                USAGE lines: summary, first rows, top types, treemap, status
//   usagesel:<path>         select an item (tree and treemap follow)
//   usagezoom:<path>        zoom the treemap into a folder
//   usageclick:<path>       click the treemap at the centre of an item's tile
//   usagedclick:<path>      double-click it (zooms in)
//   usagetrash              Move to Trash on the selection (asks first if that setting is on)
//   usagemode:disk|logical  switch the size shown
//   usageclose              close the front usage window
//   usagekey:[cmd+]<up|down|left|right|delete|space|return|i|z>   a key sent to the usage window

#if DEBUG
import AppKit

@MainActor
enum UsageDebugSteps {
    private static var installed = false

    static func install() {
        guard !installed else { return }
        installed = true
        let front = { UsageWindowController.open.last }
        DebugAutomation.register("usage") { arg, m in
            m.dispatch(.showDiskUsage(path: (arg as NSString).expandingTildeInPath), ShellContext(window: nil, tab: nil))
        }
        DebugAutomation.extraSteps["usagewait"] = { _ in
            let start = Date()
            while let c = front(), c.usage.summary().state == .scanning, Date().timeIntervalSince(start) < 120 {
                try? await Task.sleep(for: .milliseconds(100))
            }
            if let c = front() {
                print("USAGE waited \(String(format: "%.2f", Date().timeIntervalSince(start))) s: \(c.usage.summary().detail)")
            }
        }
        DebugAutomation.register("usagelog") { _, _ in
            guard let c = front() else { return print("USAGE no window") }
            c.debugLines().forEach { print($0) }
        }
        DebugAutomation.register("usagesel") { arg, _ in front()?.debugSelect(path: (arg as NSString).expandingTildeInPath) }
        DebugAutomation.register("usagezoom") { arg, _ in front()?.debugZoom(path: (arg as NSString).expandingTildeInPath) }
        DebugAutomation.register("usagetrash") { _, _ in front()?.debugTrash() }
        DebugAutomation.register("usagemode") { arg, _ in
            guard let c = front() else { return }
            c.usage.setMode(arg == "logical" ? .logical : .onDisk)
            c.reloadAll()
        }
        DebugAutomation.register("usagekey") { arg, _ in
            guard let c = front(), let w = c.window else { return }
            var parts = arg.split(separator: "+").map(String.init)
            let name = parts.removeLast()
            var flags: NSEvent.ModifierFlags = parts.contains("cmd") ? [.command] : []
            let table: [String: (UInt16, Int)] = [
                "up": (126, 0xF700), "down": (125, 0xF701), "left": (123, 0xF702), "right": (124, 0xF703),
                "delete": (51, 0x7F), "space": (49, 0x20), "return": (36, 0x0D), "i": (34, 0x69), "z": (6, 0x7A),
            ]
            guard let (code, ch) = table[name] else { return print("USAGE no key \(arg)") }
            if ch >= 0xF700 { flags.insert([.function, .numericPad]) }
            let chars = String(Character(UnicodeScalar(UInt32(ch))!))
            w.makeKeyAndOrderFront(nil)
            if let e = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags, timestamp: 0,
                                        windowNumber: w.windowNumber, context: nil, characters: chars,
                                        charactersIgnoringModifiers: chars, isARepeat: false, keyCode: code) {
                NSApp.sendEvent(e)
            }
            print("USAGE key \(arg) key-window-is-usage=\(NSApp.keyWindow === w)")
        }
        DebugAutomation.register("usageclose") { _, _ in front()?.window?.performClose(nil) }
        for (name, count) in [("usageclick", 1), ("usagedclick", 2)] {
            DebugAutomation.register(name) { arg, _ in
                guard let c = front(), let w = c.window,
                      let p = c.debugTreemapPoint(of: (arg as NSString).expandingTildeInPath) else {
                    return print("USAGE no tile for \(arg)")
                }
                for (type, n) in [(NSEvent.EventType.leftMouseDown, count), (.leftMouseUp, count)] {
                    if let e = NSEvent.mouseEvent(with: type, location: p, modifierFlags: [], timestamp: 0,
                                                  windowNumber: w.windowNumber, context: nil, eventNumber: 0,
                                                  clickCount: n, pressure: 1) {
                        w.sendEvent(e)
                    }
                }
                print("USAGE clicked \(arg) x\(count)")
            }
        }
    }
}
#endif

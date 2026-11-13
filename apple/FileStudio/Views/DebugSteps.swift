// DebugSteps.swift — extra debug-automation steps for the ops UI, toasts and
// Settings (DEBUG only; `DebugAutomation` hands unknown steps here).
//
//   ops:open | ops:close              the queue popover
//   op:copy:<src>:<destDir>           queue a real copy (Rust core) or fake one (mock)
//   op:pause | op:resume | op:cancel  the first running / paused op
//   op:clear                          clear finished ops
//   perform:<command>[:mouse|menu]    run a command like a click would (shows hints)
//   hold:<cmd|shift|alt|ctrl|fn>[+…]  show the held-modifier overlay now (no delay)
//   release                           hide it
//   toast:<text>                      show a toast
//   settings:<tab>                    open Settings (general, appearance, views, addressBar, operations, shortcuts, menus, advanced)
//   shortcuts:<commands|conflicts|customised|behaviour>   not needed yet; the tab keeps its sub-tab
//   record:<command>                  arm the shortcut recorder for a command; record:section:<commands|conflicts|customised|keyboard> picks the sub-tab
//   appearance:<light|dark>           switch the app's appearance
//   undo | redo                       run the Edit-menu actions
//   set:<dotted.key>=<json>           change one core setting
//   snapring:<name>                   the toolbar ring in each state (light and dark), plus the live one
//   alert:<ok|cancel>                 press a button of the sheet on the front window
//   snapv:<name>                      like snap:, but draws each window's views (shows lists, popovers and sheets better)
//   conflicts                         log the pending conflicts and ops (OPS lines)
//   opsum                             log each op's queue line (OPSUM <title> | <summary>)

#if DEBUG
import AppKit
import SwiftUI

@MainActor
enum UIDebugSteps {
    static func run(_ op: String, _ arg: String, _ model: AppModel) -> Bool {
        let ops = OpsModel.of(model)
        switch op {
        case "opsum":
            for o in ops.ops { print("OPSUM \(o.title) | \(o.summary)") }
        case "ops":
            ops.popoverShown = arg != "close"
        case "op":
            let parts = arg.split(separator: ":", maxSplits: 2).map(String.init)
            switch parts.first ?? "" {
            case "copy" where parts.count == 3:
                do { _ = try model.engine.enqueue(.copy(sources: [parts[1]], destDir: parts[2])) } catch { print("OP error \(error)") }
            case "pause": if let o = ops.active.first(where: { $0.state == .running }) { ops.pause(o) }
            case "resume": if let o = ops.active.first(where: { $0.state == .paused }) { ops.resume(o) }
            case "cancel": if let o = ops.active.first { ops.cancel(o) }
            case "clear": ops.clearFinished()
            default: return false
            }
        case "perform":
            let parts = arg.split(separator: ":").map(String.init)
            let via: Via = parts.count > 1 && parts[1] == "menu" ? .menu : .mouse
            model.perform(Invocation(id: parts[0]), via: via)
        case "hold":
            var f: NSEvent.ModifierFlags = []
            for p in arg.split(separator: "+") {
                switch p {
                case "cmd": f.insert(.command)
                case "shift": f.insert(.shift)
                case "alt": f.insert(.option)
                case "ctrl": f.insert(.control)
                case "fn": f.insert(.function)
                default: break
                }
            }
            OverlayController.shared.modifiersChanged(f, in: NSApp.windows.first { $0.windowController is CoreWindowIdentifying && $0.isVisible })
        case "release": OverlayController.shared.modifiersChanged([], in: nil)
        case "toast": ToastCenter.of(model).show(arg, style: arg.lowercased().contains("can") ? .warning : .info, seconds: 30)
        case "settings":
            SettingsWindow.show(model: model, section: SettingsTab(rawValue: arg))
        case "record":
            NotificationCenter.default.post(name: .fsDebugRecord, object: arg)
        case "appearance":
            NSApp.appearance = NSAppearance(named: arg == "dark" ? .darkAqua : .aqua)
        case "set":
            let kv = arg.split(separator: "=", maxSplits: 1).map(String.init)
            if kv.count == 2 { do { try model.engine.setSetting(kv[0], json: kv[1]) } catch { print("SET error \(error)") } }
        case "get":
            print("SETTING \(arg) = \((try? model.engine.setting(arg)) ?? "?")")
        case "alert":
            // Press the default (arg "ok") or the last button ("cancel") of the sheet on the front window.
            if let w = NSApp.windows.first(where: { $0.attachedSheet != nil }), let s = w.attachedSheet {
                let buttons = s.contentView?.subviewsRecursive.compactMap { $0 as? NSButton } ?? []
                (arg == "cancel" ? buttons.last : buttons.first { $0.keyEquivalent == "\r" } ?? buttons.first)?.performClick(nil)
            } else { print("ALERT none") }
        case "snapv": snapViews(arg)
        case "snapring": snapRing(arg, model)
        case "undo": _ = model.perform(Invocation(id: "edit.undo"), via: .menu)
        case "redo": _ = model.perform(Invocation(id: "edit.redo"), via: .menu)
        case "conflicts":
            print("OPS pending=\(ops.pending.count) active=\(ops.active.map { "\($0.id):\($0.state):\(Int($0.fraction * 100))%" }) "
                  + "finished=\(ops.finished.map { "\($0.id):\($0.state)" }) interrupted=\(ops.interrupted.count) "
                  + "status=\(ops.statusText ?? "-") undo=\(model.undoState.undo ?? "-")")
        default: return false
        }
        return true
    }

    /// The ring in every state, on light and dark backgrounds, and the live toolbar item.
    private static func snapRing(_ name: String, _ model: AppModel) {
        let dir = URL(fileURLWithPath: UserDefaults.standard.string(forKey: "FSDebugSnapshotDir") ?? NSTemporaryDirectory())
        func row(_ scheme: ColorScheme) -> some View {
            HStack(spacing: 22) {
                ForEach(Array([
                    ("Idle", OpsRing(fraction: 0, running: false, allPaused: false, conflicts: 0, failures: 0, interrupted: 0, hasFinished: false)),
                    ("Done", OpsRing(fraction: 1, running: false, allPaused: false, conflicts: 0, failures: 0, interrupted: 0, hasFinished: true)),
                    ("12 %", OpsRing(fraction: 0.12, running: true, allPaused: false, conflicts: 0, failures: 0, interrupted: 0, hasFinished: false)),
                    ("42 %", OpsRing(fraction: 0.42, running: true, allPaused: false, conflicts: 0, failures: 0, interrupted: 0, hasFinished: false)),
                    ("Paused", OpsRing(fraction: 0.6, running: true, allPaused: true, conflicts: 0, failures: 0, interrupted: 0, hasFinished: false)),
                    ("Conflicts", OpsRing(fraction: 0.3, running: true, allPaused: false, conflicts: 3, failures: 0, interrupted: 0, hasFinished: false)),
                    ("Failed", OpsRing(fraction: 1, running: false, allPaused: false, conflicts: 0, failures: 1, interrupted: 0, hasFinished: true)),
                    ("Interrupted", OpsRing(fraction: 0, running: false, allPaused: false, conflicts: 0, failures: 0, interrupted: 1, hasFinished: false)),
                ].enumerated()), id: \.offset) { _, item in
                    VStack(spacing: 6) {
                        item.1
                        Text(item.0).font(.caption2).foregroundStyle(.secondary)
                    }
                }
                VStack(spacing: 6) {
                    OpsPlaceholderView(model: model)
                    Text("Live").font(.caption2).foregroundStyle(.secondary)
                }
            }
            .padding(20)
            .background(scheme == .dark ? Color(white: 0.16) : Color(white: 0.96))
            .environment(\.colorScheme, scheme)
        }
        let r = ImageRenderer(content: VStack(spacing: 0) { row(.light); row(.dark) })
        r.scale = 2
        if let img = r.nsImage, let tiff = img.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
           let png = rep.representation(using: .png, properties: [:]) {
            let url = dir.appendingPathComponent("\(name).png")
            try? png.write(to: url)
            print("SNAP \(url.path)")
        }
    }

    /// Renders each visible window with `cacheDisplay` (AppKit's own drawing),
    /// which captures table-backed SwiftUI lists that the layer snapshot misses.
    private static func snapViews(_ name: String) {
        let dir = URL(fileURLWithPath: UserDefaults.standard.string(forKey: "FSDebugSnapshotDir") ?? NSTemporaryDirectory())
        for (i, w) in NSApp.windows.filter({ $0.isVisible && $0.frame.width > 200 }).enumerated() {
            guard let view = w.contentView?.superview ?? w.contentView else { continue }
            view.layoutSubtreeIfNeeded()
            guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { continue }
            view.cacheDisplay(in: view.bounds, to: rep)
            let url = dir.appendingPathComponent("\(name)\(i == 0 ? "" : "-w\(i)").png")
            try? rep.representation(using: .png, properties: [:])?.write(to: url)
            print("SNAP \(url.path)")
        }
    }
}

private extension NSView {
    var subviewsRecursive: [NSView] { subviews + subviews.flatMap(\.subviewsRecursive) }
}
#endif

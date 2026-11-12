// SafetyDebugSteps.swift — DEBUG-only script steps that prove the safety
// audit's fixes (docs/SAFETY-AUDIT.md) in the real app. Run them against a
// scratch home only (`HOME=<scratch> scripts/run-debug.sh "…"`).
//
//   confirm:<yes|no>          answers the open Trash / Delete Immediately
//                             question as if its button were pressed (S1)
//   ls:<path>                 prints the folder's names on disk (LS line)
//   sel                       prints the focused tab's selection (SEL line)
//   pbfiles:<path>|<path>…    puts file URLs on the general pasteboard, as
//                             copying them in Finder does (S7)
//   holdkey:<key>:<n>         sends <key> once, then <n> auto-repeats of it
//                             (isARepeat), as holding the key does (S6); keys:
//                             cmd+delete, cmd+d, down, return, f6

#if DEBUG
import AppKit

@MainActor
enum SafetyDebugSteps {
    private static var installed = false

    static func install() {
        guard !installed else { return }
        installed = true
        DebugAutomation.register("confirm") { arg, _ in
            ConfirmSheets.answer(yes: arg == "yes")
        }
        DebugAutomation.register("sel") { _, m in
            let names = m.focusedTab?.api.selection().names ?? []
            print("SEL \(names.count): \(names.sorted().prefix(8).joined(separator: ", "))")
        }
        DebugAutomation.register("holdkey") { arg, _ in
            let parts = arg.split(separator: ":").map(String.init)
            guard let spec = parts.first, let n = parts.count > 1 ? Int(parts[1]) : 3 else { return }
            holdKey(spec, repeats: n)
        }
        DebugAutomation.register("pbfiles") { arg, _ in
            let pb = NSPasteboard.general
            pb.clearContents()
            pb.writeObjects(arg.split(separator: "|").map { URL(fileURLWithPath: String($0)) as NSURL })
            print("PASTEBOARD \(pb.changeCount) \(arg)")
        }
        DebugAutomation.register("ls") { arg, _ in
            let path = (arg as NSString).expandingTildeInPath
            let names = ((try? FileManager.default.contentsOfDirectory(atPath: path)) ?? []).sorted()
            print("LS \(path): \(names.joined(separator: ", "))")
        }
    }

    /// Key-downs as a held key produces them: one press, then repeats.
    private static func holdKey(_ spec: String, repeats: Int) {
        let table: [String: (String, UInt16, NSEvent.ModifierFlags)] = [
            "cmd+delete": ("\u{7f}", 51, .command),
            "cmd+d": ("d", 2, .command),
            "down": (String(Character(UnicodeScalar(UInt32(NSDownArrowFunctionKey))!)), 125, [.function, .numericPad]),
            "return": ("\r", 36, []),
            "f6": (String(Character(UnicodeScalar(UInt32(NSF6FunctionKey))!)), 97, .function),
        ]
        guard let (chars, code, flags) = table[spec], let w = NSApp.keyWindow ?? NSApp.mainWindow else {
            print("HOLDKEY unknown \(spec)")
            return
        }
        for k in 0...repeats {
            if let e = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags,
                                        timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: w.windowNumber,
                                        context: nil, characters: chars, charactersIgnoringModifiers: chars,
                                        isARepeat: k > 0, keyCode: code) {
                NSApp.postEvent(e, atStart: false)
            }
        }
        if let up = NSEvent.keyEvent(with: .keyUp, location: .zero, modifierFlags: flags,
                                     timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: w.windowNumber,
                                     context: nil, characters: chars, charactersIgnoringModifiers: chars,
                                     isARepeat: false, keyCode: code) {
            NSApp.postEvent(up, atStart: false)
        }
    }
}
#endif

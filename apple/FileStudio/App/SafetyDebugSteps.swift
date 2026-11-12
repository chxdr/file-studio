// SafetyDebugSteps.swift — DEBUG-only script steps that prove the safety
// audit's fixes (docs/SAFETY-AUDIT.md) in the real app. Run them against a
// scratch home only (`HOME=<scratch> scripts/run-debug.sh "…"`).
//
//   confirm:<yes|no>          answers the open Trash / Delete Immediately
//                             question as if its button were pressed (S1)
//   ls:<path>                 prints the folder's names on disk (LS line)
//   sel                       prints the focused tab's selection (SEL line)

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
        DebugAutomation.register("ls") { arg, _ in
            let path = (arg as NSString).expandingTildeInPath
            let names = ((try? FileManager.default.contentsOfDirectory(atPath: path)) ?? []).sorted()
            print("LS \(path): \(names.joined(separator: ", "))")
        }
    }
}
#endif

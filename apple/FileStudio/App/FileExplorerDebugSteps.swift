// FileExplorerDebugSteps.swift — debug-automation steps for File Studio as
// the file explorer (DEBUG only):
//
//   sysopen:<path>[|<path>…]     a folder open from another app (application(_:open:))
//   sysreveal:<path>[|<path>…]   a real reveal Apple Event (misc/mvis), sent to this app
//   viewer[:make|:finder]        the default-viewer state (VIEWER line); make this app
//                                the viewer, or give it back to Finder
//   panelprobe:open|save         opens an Open or Save panel here and prints what
//                                Accessibility sees and the core's verdict (PANEL lines)
//   panelclose                   closes that panel
//   hotkey                       the panel-jump hotkey's state (HOTKEY line)
//   paneljumpself:<dir>         send this app's own panel (panelprobe:) to <dir>
//   paneljump:<bundle id>        run panel.jump_here against that app's Open/Save panel

#if DEBUG
import AppKit
import Carbon.HIToolbox

@MainActor
enum FileExplorerDebugSteps {
    private static var openPanel: NSSavePanel?

    static func register() {
        DebugAutomation.register("sysopen") { arg, _ in
            let urls = arg.split(separator: "|").map { URL(fileURLWithPath: String($0)) }
            NSApp.delegate?.application?(NSApp, open: urls)
        }
        DebugAutomation.register("sysreveal") { arg, _ in
            let ev = NSAppleEventDescriptor(eventClass: AEEventClass(kAEMiscStandards),
                                            eventID: AEEventID(kAEMakeObjectsVisible),
                                            targetDescriptor: .currentProcess(),
                                            returnID: AEReturnID(kAutoGenerateReturnID),
                                            transactionID: AETransactionID(kAnyTransactionID))
            let list = NSAppleEventDescriptor.list()
            for (i, p) in arg.split(separator: "|").enumerated() {
                list.insert(NSAppleEventDescriptor(fileURL: URL(fileURLWithPath: String(p))), at: i + 1)
            }
            ev.setParam(list, forKeyword: AEKeyword(keyDirectObject))
            do { _ = try ev.sendEvent(options: [.noReply], timeout: 2) } catch { print("SYSREVEAL failed \(error)") }
        }
        DebugAutomation.register("viewer") { arg, _ in
            let done: @MainActor (Error?) -> Void = { e in print("VIEWER \(arg) done error=\(e.map { "\($0)" } ?? "-")") }
            if arg == "make" { return DefaultFileViewer.makeDefault(done) }
            if arg == "finder" { return DefaultFileViewer.restoreFinder(done) }
            let s = DefaultFileViewer.status()
            print("VIEWER state=\(s.state) viewer=\(DefaultFileViewer.fileViewer() ?? "-") folders=\(DefaultFileViewer.folderHandler() ?? "-") "
                  + "own=\(DefaultFileViewer.ownID) headline=\(s.headline)")
        }
        DebugAutomation.register("panelprobe") { arg, _ in
            let p: NSSavePanel = arg == "save" ? NSSavePanel() : NSOpenPanel()
            openPanel = p
            p.directoryURL = URL(fileURLWithPath: NSHomeDirectory())
            p.begin { _ in }
            let t = Timer(timeInterval: 0.8, repeats: false) { _ in
                MainActor.assumeIsolated {
                    let app = AXUIElementCreateApplication(getpid())
                    print("PANEL trusted=\(AXIsProcessTrusted())")
                    for w in AX.elements(app, kAXWindowsAttribute) {
                        let pr = PanelFinder.probe(w)
                        print("PANEL window role=\(pr.role) subrole=\(pr.subrole) id=\(pr.identifier) "
                              + "buttons=\(pr.buttons) roles=\(pr.innerRoles) -> \(String(describing: classifyFilePanel(probe: pr)))")
                    }
                    if let f = PanelFinder.find(in: app) { print("PANEL found \(f.kind) frame=\(f.frame)") }
                }
            }
            RunLoop.main.add(t, forMode: .common)
        }
        DebugAutomation.register("panelclose") { _, _ in openPanel?.cancel(nil); openPanel = nil }
        DebugAutomation.register("paneljumpself") { arg, _ in
            // Drives this app's own panel (opened by panelprobe:) the way
            // another app's is driven: real ⌘⇧G, AX value, Return. Only while
            // this app is in front, since the keys go to the frontmost app.
            guard NSApp.isActive else { return print("PANELJUMP skipped: not in front") }
            FilePanelJump.shared.jump(to: arg, in: .current)
            let t = Timer(timeInterval: 2.5, repeats: false) { _ in
                MainActor.assumeIsolated { print("PANELJUMP panel now at \(openPanel?.directoryURL?.path ?? "-")") }
            }
            RunLoop.main.add(t, forMode: .common)
        }
        DebugAutomation.register("paneljump") { arg, _ in FilePanelJump.shared.runCommand(targeting: arg) }
        DebugAutomation.register("hotkey") { _, m in
            let keys = m.engine.commandKeys("panel.jump_here")
            print("HOTKEY on=\(FilePanelJump.shared.isOn) keys=\(keys) problem=\(FilePanelJump.shared.hotKeyProblem ?? "-") "
                  + "chord=\(keys.first.flatMap { globalHotkeyChord(keys: $0) }.map { "\($0)" } ?? "-")")
        }
    }
}
#endif

// main.swift — an AppKit app (the menu bar and windows are ours, not SwiftUI's
// scene machinery); SwiftUI is used inside windows. Top-level code runs on the
// main actor; the run loop is started the conventional way.

import AppKit

let app = FileStudioApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()

/// Logs every Objective-C exception AppKit catches and swallows while it
/// handles events, with the stack, so one never passes silently. (Swallowed
/// exceptions leave Swift code half-run; the debug scripts print these as
/// `EXCEPTION` lines.)
final class FileStudioApplication: NSApplication {
    override func reportException(_ e: NSException) {
        FileHandle.standardError.write(Data(
            "EXCEPTION \(e.name.rawValue): \(e.reason ?? "")\n\(e.callStackSymbols.joined(separator: "\n"))\n".utf8))
        print("EXCEPTION \(e.name.rawValue): \(e.reason ?? "")")
        fflush(stdout)
        super.reportException(e)
    }
}

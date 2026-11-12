// Pasteboard.swift — the general pasteboard for paths and file URLs.

import AppKit

@MainActor
enum Pasteboard {
    /// Copy Path and friends: plain text.
    static func writeText(_ s: String) {
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(s, forType: .string)
    }

    /// Copy (files): file URLs other apps can paste, plus the paths as text.
    static func writeFiles(_ paths: [String]) {
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.writeObjects(paths.map { URL(fileURLWithPath: $0) as NSURL })
        pb.setString(paths.joined(separator: "\n"), forType: .string)
    }

    /// File paths on the pasteboard (from Finder or another app), if any.
    static func readFiles() -> [String] {
        let urls = NSPasteboard.general.readObjects(forClasses: [NSURL.self],
                                                    options: [.urlReadingFileURLsOnly: true]) as? [URL]
        return urls?.map(\.path) ?? []
    }

    /// The `changeCount` last reported to the core.
    private static var synced: Int?

    /// Tells the core what is on the system pasteboard when it changed since
    /// the last report (S7). Called before every key and command, so Paste
    /// always pastes what the Edit menu promises.
    static func sync(_ engine: EngineAPI) {
        let n = NSPasteboard.general.changeCount
        guard n != synced else { return }
        synced = n
        engine.syncPasteboard(changeCount: n, files: readFiles())
    }

    /// Copy or Cut in File Studio: writes the files and tells the core the
    /// pasteboard's new `changeCount`, which ties its clipboard to it.
    static func writeFiles(_ paths: [String], engine: EngineAPI) {
        writeFiles(paths)
        let n = NSPasteboard.general.changeCount
        synced = n
        engine.pasteboardWritten(changeCount: n)
    }

    /// The pasteboard holds file URLs (greys Paste in context menus).
    static func hasFiles() -> Bool {
        NSPasteboard.general.canReadObject(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true])
    }
}

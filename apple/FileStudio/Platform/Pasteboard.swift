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

    /// The pasteboard holds file URLs (greys Paste in context menus).
    static func hasFiles() -> Bool {
        NSPasteboard.general.canReadObject(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true])
    }
}

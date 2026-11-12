// IconCache.swift — icons by the core's icon key (F-005: icons never cross the FFI).
//
// Keys: `folder`, `file`, `file.<ext>`, `package.<ext>`, `symlink`, `other`.
// Type icons are cached per key. Packages (apps) have their own icon, so they
// are cached per path. Never reads file content (dataless-safe, F-016).

import AppKit
import UniformTypeIdentifiers

@MainActor
final class IconCache {
    static let shared = IconCache()
    private var byKey: [String: NSImage] = [:]
    private var byPath: [String: NSImage] = [:]

    /// The icon for a row. `path` is used only for packages.
    func icon(key: String, path: String? = nil) -> NSImage {
        if key.hasPrefix("package."), let path {
            if let i = byPath[path] { return i }
            let i = FileManager.default.fileExists(atPath: path) ? NSWorkspace.shared.icon(forFile: path)
                : typeIcon(key)
            byPath[path] = i
            return i
        }
        if let i = byKey[key] { return i }
        let i = typeIcon(key)
        byKey[key] = i
        return i
    }

    private func typeIcon(_ key: String) -> NSImage {
        let t: UTType
        switch key {
        case "folder": t = .folder
        case "symlink": t = .symbolicLink
        case "file", "other": t = .data
        default:
            let ext = key.split(separator: ".", maxSplits: 1).last.map(String.init) ?? ""
            t = key.hasPrefix("package.") ? (UTType(filenameExtension: ext) ?? .package)
                : (UTType(filenameExtension: ext) ?? .data)
        }
        return NSWorkspace.shared.icon(for: t)
    }
}

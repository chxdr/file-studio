// PathBarStyle.swift — the address bar's look (HANDS-ON §10, F-019) and icons.
// Drawing constants only; no behaviour.

import AppKit

@MainActor
enum PathBarStyle {
    /// Crumb and field text: 13 pt; the current folder in medium weight.
    static let font = NSFont.systemFont(ofSize: 13)
    static let currentFont = NSFont.systemFont(ofSize: 13, weight: .medium)
    static let iconSize: CGFloat = 16
    /// Space between a crumb's icon and its name.
    static let iconGap: CGFloat = 4
    /// Horizontal padding inside a crumb's hover highlight.
    static let crumbPad: CGFloat = 4
    /// Width of the chevron button after each crumb.
    static let chevronWidth: CGFloat = 14
    /// Inset of the crumbs and field from the capsule's left edge.
    static let leadingInset: CGFloat = 6
    /// The REST ⇄ EDIT cross-fade.
    static let fade: TimeInterval = 0.14
    /// Hover on a drop target this long to open it (spring-loading).
    static let springLoadDelay: TimeInterval = 0.8
    /// Suggestion rows.
    static let suggestionRowHeight: CGFloat = 24
    static let menuRowHeight: CGFloat = 22
    static let maxVisibleRows = 10

    static var reduceMotion: Bool { NSWorkspace.shared.accessibilityDisplayShouldReduceMotion }

    /// The capsule outline at rest-hover (QSpace) and while editing.
    static var hoverStroke: NSColor { .separatorColor }
    static var hoverFill: NSColor { .quaternaryLabelColor.withAlphaComponent(0.06) }
    static var editFill: NSColor { .textBackgroundColor }
    static var errorStroke: NSColor { .systemRed }
    /// A crumb or chevron under the mouse.
    static var crumbHover: NSColor { .quaternaryLabelColor }
    static var crumbPressed: NSColor { .tertiaryLabelColor }

    /// A chevron glyph for separators and menu rows.
    static func chevron(pointSize: CGFloat = 9, color: NSColor = .tertiaryLabelColor) -> NSImage {
        let cfg = NSImage.SymbolConfiguration(pointSize: pointSize, weight: .semibold)
            .applying(.init(paletteColors: [color]))
        return NSImage(systemSymbolName: "chevron.right", accessibilityDescription: nil)?
            .withSymbolConfiguration(cfg) ?? NSImage()
    }

    static func symbol(_ name: String, size: CGFloat = 11, weight: NSFont.Weight = .regular,
                       color: NSColor = .secondaryLabelColor) -> NSImage? {
        let cfg = NSImage.SymbolConfiguration(pointSize: size, weight: weight).applying(.init(paletteColors: [color]))
        return NSImage(systemSymbolName: name, accessibilityDescription: nil)?.withSymbolConfiguration(cfg)
    }
}

/// Icons for crumbs, menu rows and suggestions. Folders and packages come
/// from the shared `IconCache`; volume roots and Home use the system's own
/// icon for that path (Cocoa-only, cached here). Never reads file content.
@MainActor
enum CrumbIcons {
    private static var byPath: [String: NSImage] = [:]

    static func icon(_ c: Crumb) -> NSImage {
        switch c.kind {
        case .volumeRoot, .home:
            if let i = byPath[c.path] { return i }
            let i: NSImage = FileManager.default.fileExists(atPath: c.path)
                ? NSWorkspace.shared.icon(forFile: c.path)
                : (c.kind == .volumeRoot ? NSWorkspace.shared.icon(for: .volume) : IconCache.shared.icon(key: "folder"))
            byPath[c.path] = i
            return i
        case .package:
            let ext = (c.name as NSString).pathExtension
            return IconCache.shared.icon(key: "package.\(ext.isEmpty ? "app" : ext)", path: c.path)
        case .folder:
            return IconCache.shared.icon(key: "folder")
        }
    }

    /// The icon for a folder path that isn't a crumb (suggestion rows).
    static func folder(path: String) -> NSImage {
        let ext = (path as NSString).pathExtension.lowercased()
        if ["app", "bundle", "framework", "plugin", "kext", "photoslibrary"].contains(ext) {
            return IconCache.shared.icon(key: "package.\(ext)", path: path)
        }
        return IconCache.shared.icon(key: "folder")
    }

    /// A 16 pt copy, for menus (cached images are shared; never resize them).
    static func small(_ i: NSImage) -> NSImage {
        let c = i.copy() as? NSImage ?? i
        c.size = NSSize(width: 16, height: 16)
        return c
    }
}

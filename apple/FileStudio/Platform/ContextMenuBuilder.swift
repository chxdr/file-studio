// ContextMenuBuilder.swift — draws the core's context menu (F-022) as an
// NSMenu. The contents, order, titles, shortcuts, enabled and checked states
// all come from the core; choosing an item performs its invocation. The
// system submenus (Open With, Share, Quick Actions, Services) are filled here
// from the OS.

import AppKit

@MainActor
enum ContextMenuBuilder {
    static func menu(_ items: [MenuItem], model: AppModel) -> NSMenu {
        let m = NSMenu()
        m.autoenablesItems = false
        for i in items { m.addItem(item(i, model: model)) }
        return m
    }

    /// Plain-text dump for logs and debug scripts.
    static func describe(_ items: [MenuItem], depth: Int = 0) -> [String] {
        let pad = String(repeating: "  ", count: depth)
        return items.flatMap { i -> [String] in
            switch i {
            case .separator: ["\(pad)---"]
            case .command(let c):
                ["\(pad)\(c.checked ? "✓ " : "")\(c.title)\(c.shortcut.map { "  [\($0)]" } ?? "")\(c.enabled ? "" : "  (disabled)")  → \(c.invocation.id)"]
            case .submenu(_, let t, let sub): ["\(pad)\(t) ›"] + describe(sub, depth: depth + 1)
            case .system(let k, let t, _): ["\(pad)\(t) › (system: \(k))"]
            }
        }
    }

    private static func item(_ i: MenuItem, model: AppModel) -> NSMenuItem {
        switch i {
        case .separator:
            return .separator()
        case .command(let c):
            let it = ActionItem(title: c.title) { model.perform(c.invocation, via: .mouse) }
            it.isEnabled = c.enabled
            it.state = c.checked ? .on : .off
            if let s = c.shortcut, let (k, mods) = MainMenu.keyEquivalent(s) {
                it.keyEquivalent = k
                it.keyEquivalentModifierMask = mods
            }
            if let color = c.tagColor { it.image = tagDot(color) }
            return it
        case .submenu(_, let title, let sub):
            let it = NSMenuItem(title: title, action: nil, keyEquivalent: "")
            it.submenu = menu(sub, model: model)
            return it
        case .system(.share, let title, let paths):
            let it = NSSharingServicePicker(items: paths.map { URL(fileURLWithPath: $0) as NSURL }).standardShareMenuItem
            it.title = title
            return it
        case .system(let kind, let title, let paths):
            let it = NSMenuItem(title: title, action: nil, keyEquivalent: "")
            it.submenu = systemMenu(kind, paths: paths, model: model)
            return it
        }
    }

    private static func systemMenu(_ kind: SystemMenu, paths: [String], model: AppModel) -> NSMenu {
        let m = NSMenu()
        let urls = paths.map { URL(fileURLWithPath: $0) }
        switch kind {
        case .openWith:
            if let first = urls.first {
                let ws = NSWorkspace.shared
                let def = ws.urlForApplication(toOpen: first)
                let apps = ws.urlsForApplications(toOpen: first)
                for app in ([def].compactMap { $0 } + apps.filter { $0 != def }).prefix(20) {
                    let name = FileManager.default.displayName(atPath: app.path)
                    let it = ActionItem(title: app == def ? "\(name) (default)" : name) {
                        PlatformActions.open(paths, with: app)
                    }
                    it.image = ws.icon(forFile: app.path)
                    it.image?.size = NSSize(width: 16, height: 16)
                    m.addItem(it)
                    if app == def { m.addItem(.separator()) }
                }
            }
            m.addItem(.separator())
            m.addItem(ActionItem(title: "Other…") {
                model.perform(Invocation(id: "file.open_with"), via: .mouse)
            })
        case .share, .quickActions, .services:
            // The system fills Services for the selection through the
            // responder chain (NSServicesMenuRequestor); until a view adopts
            // it, point at the app's Services menu.
            let it = NSMenuItem(title: "Services Preferences…", action: nil, keyEquivalent: "")
            it.isEnabled = false
            m.addItem(it)
        }
        return m
    }

    private static func tagDot(_ index: UInt8) -> NSImage {
        let colors: [UInt8: NSColor] = [1: .systemGray, 2: .systemGreen, 3: .systemPurple, 4: .systemBlue,
                                        5: .systemYellow, 6: .systemRed, 7: .systemOrange]
        let c = colors[index] ?? .clear
        return NSImage(size: NSSize(width: 12, height: 12), flipped: false) { r in
            c.setFill()
            NSBezierPath(ovalIn: r.insetBy(dx: 1, dy: 1)).fill()
            return true
        }
    }
}

/// A menu item that runs a closure.
@MainActor
final class ActionItem: NSMenuItem {
    private let run: () -> Void
    init(title: String, run: @escaping () -> Void) {
        self.run = run
        super.init(title: title, action: #selector(fire), keyEquivalent: "")
        target = self
    }
    required init(coder: NSCoder) { fatalError() }
    @objc private func fire() { run() }
}

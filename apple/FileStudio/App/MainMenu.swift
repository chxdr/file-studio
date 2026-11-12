// MainMenu.swift — the menu bar, built from the core's command registry (F-006).
//
// A minimal first pass: one menu per group of categories, every command that
// `showsInMenu`, titled and keyed as the registry says. Choosing an item runs
// it through the core (`Via.menu`). Key presses never reach these items
// first: KeyRouter sends every key to the core, and an item fired by a key
// the core declined does nothing. Only ⌘ shortcuts are set as key
// equivalents (a bare key would steal typing from text fields); the rest
// show in the title's tooltip until the shortcut-overlay work lands.

import AppKit

@MainActor
enum MainMenu {
    /// Top-level menus and the registry categories they hold, in order.
    private static let layout: [(title: String, categories: [String])] = [
        ("File", ["File", "Paths & Open In", "Quick Look & Info"]),
        ("Edit", ["Edit", "Selection"]),
        ("View", ["View", "Sort", "Panes"]),
        ("Go", ["Navigation", "Workspaces"]),
        ("Window", ["Tabs & Windows"]),
    ]

    static func build(model: AppModel, target: AnyObject) -> NSMenu {
        let main = NSMenu()
        let commands = model.engine.commands().filter { $0.showsInMenu && $0.handling != .later }

        // App menu.
        let app = NSMenu(title: "File Studio")
        app.addItem(withTitle: "About File Studio", action: #selector(NSApplication.orderFrontStandardAboutPanel(_:)),
                    keyEquivalent: "")
        app.addItem(.separator())
        if let s = commands.first(where: { $0.id == "app.settings" }) { app.addItem(item(s, target: target)) }
        if let s = commands.first(where: { $0.id == "app.keyboard_shortcuts" }) { app.addItem(item(s, target: target)) }
        app.addItem(.separator())
        let services = NSMenu(title: "Services")
        let servicesItem = app.addItem(withTitle: "Services", action: nil, keyEquivalent: "")
        servicesItem.submenu = services
        NSApp.servicesMenu = services
        app.addItem(.separator())
        app.addItem(withTitle: "Hide File Studio", action: #selector(NSApplication.hide(_:)), keyEquivalent: "h")
        let others = app.addItem(withTitle: "Hide Others", action: #selector(NSApplication.hideOtherApplications(_:)),
                                 keyEquivalent: "h")
        others.keyEquivalentModifierMask = [.command, .option]
        app.addItem(withTitle: "Show All", action: #selector(NSApplication.unhideAllApplications(_:)), keyEquivalent: "")
        app.addItem(.separator())
        app.addItem(withTitle: "Quit File Studio", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        main.addItem(withTitle: "File Studio", action: nil, keyEquivalent: "").submenu = app

        for (title, cats) in layout {
            let m = NSMenu(title: title)
            if title == "Edit" { addTextEditing(to: m) }
            for (i, cat) in cats.enumerated() {
                let items = commands.filter { $0.category == cat && !$0.id.hasPrefix("app.") }
                if items.isEmpty { continue }
                if i > 0 || title == "Edit", m.numberOfItems > 0 { m.addItem(.separator()) }
                for c in items { m.addItem(item(c, target: target)) }
            }
            main.addItem(withTitle: title, action: nil, keyEquivalent: "").submenu = m
            if title == "Window" { NSApp.windowsMenu = m }
        }
        return main
    }

    /// ⌘X for text fields (the core's Cut has no ⌘X binding). ⌘C, ⌘V, ⌘A,
    /// ⌘Z and ⇧⌘Z reach text fields through `AppDelegate.runCommand`, which
    /// turns a declined key into the standard text action.
    private static func addTextEditing(to m: NSMenu) {
        m.addItem(withTitle: "Cut Text", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
    }

    /// Standard text actions for commands whose keys a text field also uses.
    static let textActions: [String: Selector] = [
        "edit.copy": #selector(NSText.copy(_:)),
        "edit.paste": #selector(NSText.paste(_:)),
        "select.all": #selector(NSText.selectAll(_:)),
        "edit.undo": Selector(("undo:")),
        "edit.redo": Selector(("redo:")),
    ]

    private static func item(_ c: CommandInfo, target: AnyObject) -> NSMenuItem {
        let it = NSMenuItem(title: c.title, action: #selector(AppDelegate.runCommand(_:)), keyEquivalent: "")
        it.target = target
        it.representedObject = c.id
        it.toolTip = c.description + (c.shortcut.map { " (\($0))" } ?? "")
        if let s = c.shortcut, let (key, mods) = keyEquivalent(s), mods.contains(.command) {
            it.keyEquivalent = key
            it.keyEquivalentModifierMask = mods
        }
        return it
    }

    /// Key-cap text (`⇧⌘N`, `⌘↓`, `⌥⌘⌫`) → an NSMenuItem key equivalent.
    static func keyEquivalent(_ text: String) -> (String, NSEvent.ModifierFlags)? {
        guard !text.contains(" ") else { return nil }   // sequences (`g g`) can't be menu keys
        var mods: NSEvent.ModifierFlags = []
        var rest = Substring(text)
        while let c = rest.first, "⌃⌥⇧⌘".contains(c) {
            switch c {
            case "⌃": mods.insert(.control)
            case "⌥": mods.insert(.option)
            case "⇧": mods.insert(.shift)
            default: mods.insert(.command)
            }
            rest = rest.dropFirst()
        }
        let k = String(rest)
        let special: [String: Int] = [
            "↑": NSUpArrowFunctionKey, "↓": NSDownArrowFunctionKey, "←": NSLeftArrowFunctionKey,
            "→": NSRightArrowFunctionKey, "↖": NSHomeFunctionKey, "↘": NSEndFunctionKey,
            "⇞": NSPageUpFunctionKey, "⇟": NSPageDownFunctionKey,
        ]
        if let s = special[k], let u = UnicodeScalar(UInt32(s)) { return (String(Character(u)), mods) }
        switch k {
        case "⌫": return ("\u{8}", mods)
        case "⌦": return ("\u{7f}", mods)
        case "↩": return ("\r", mods)
        case "⎋": return ("\u{1b}", mods)
        case "⇥": return ("\t", mods)
        case "Space": return (" ", mods)
        default: break
        }
        if k.hasPrefix("F"), let n = Int(k.dropFirst()), (1...20).contains(n),
           let u = UnicodeScalar(UInt32(NSF1FunctionKey + n - 1)) {
            return (String(Character(u)), mods)
        }
        guard k.count == 1 else { return nil }
        return (k.lowercased(), mods)
    }
}
